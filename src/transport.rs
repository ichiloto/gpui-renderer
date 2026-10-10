use crate::assets::AssetRoot;
#[cfg(test)]
use crate::canvas_protocol::Canvas;
use crate::diagnostics::Diagnostics;
#[cfg(test)]
use crate::protocol::Sprite;
use crate::protocol::{self, Capability, Hello, Incoming, Message, Version};
use crate::retained_prepared::PreparedScene;
use crate::retained_protocol::Viewport;
use crate::retained_state::{RetainedSession, SceneSource};
#[cfg(test)]
use crate::state::PreparedFrame;
use std::io::{BufRead, Read};
use std::sync::Arc;

pub struct RetainedDisplay {
    pub generation: u64,
    pub frame: u64,
    pub reset: bool,
    pub scene: Arc<PreparedScene>,
    pub viewport: Option<Viewport>,
    pub observation: Option<crate::diagnostics::FrameTrace>,
}

pub enum Update {
    Hello(Version, Hello),
    #[cfg(test)]
    Frame(Box<PreparedFrame>),
    RetainedFrame(Box<RetainedDisplay>),
    RetainedAck {
        generation: u64,
        frame: u64,
    },
    RetainedRejected {
        generation: u64,
        expected_generation: u64,
        message: String,
    },
    Error(String),
    Fatal(String),
    Shutdown,
    Eof,
}

impl Update {
    pub fn observation(&self) -> Option<crate::diagnostics::FrameTrace> {
        match self {
            #[cfg(test)]
            Self::Frame(frame) => frame.observation,
            Self::RetainedFrame(frame) => frame.observation,
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Session {
    initialized: Option<(Version, Hello, AssetRoot)>,
    retained: RetainedSession,
    prepared: Option<Arc<PreparedScene>>,
}

impl Session {
    fn reject_malformed(&mut self, line: &[u8], message: String) -> Update {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            return Update::Error(message);
        };
        if value.get("protocol").and_then(serde_json::Value::as_u64) != Some(2)
            || value.get("type").and_then(serde_json::Value::as_str) != Some("frame")
            || value.get("generation").is_none()
            || !matches!(self.initialized, Some((Version::V2, _, _)))
        {
            return Update::Error(message);
        }
        self.retained.require_reset();
        Update::RetainedRejected {
            generation: value
                .get("generation")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            expected_generation: self.retained.expected_generation(),
            message,
        }
    }

    pub fn prepare(&mut self, incoming: Incoming) -> Result<Update, String> {
        if let Some((version, _, _)) = &self.initialized
            && *version != incoming.version
        {
            return Err(format!(
                "message protocol {} differs from session protocol {}",
                incoming.version.number(),
                version.number()
            ));
        }
        match incoming.message {
            Message::Hello(hello) => {
                if self.initialized.is_some() {
                    return Err("hello has already initialized this session".into());
                }
                let assets = AssetRoot::new(&hello.asset_root)?;
                self.initialized = Some((incoming.version, hello.clone(), assets));
                Ok(Update::Hello(incoming.version, hello))
            }
            #[cfg(test)]
            Message::Frame(frame) => {
                let (version, hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                validate_capabilities(&frame.sprites, &hello.get_enabled_capabilities(*version))?;
                Ok(Update::Frame(Box::new(PreparedFrame::prepare(
                    frame, hello.grid, assets,
                )?)))
            }
            #[cfg(test)]
            Message::FrameV2(frame) => {
                let (version, hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                let enabled = hello.get_enabled_capabilities(*version);
                validate_capabilities(&frame.sprites, &enabled)?;
                if frame.viewport.is_some() && !enabled.contains(&Capability::FrameViewport) {
                    return Err("viewport requires negotiated frame_viewport capability".into());
                }
                if let Some(canvas) = &frame.canvas {
                    CanvasFeatures::get_from_canvas(canvas).validate(&enabled)?;
                }
                if frame.tile_batches.is_some() && !enabled.contains(&Capability::TileBatches) {
                    return Err("tileBatches requires negotiated tile_batches capability".into());
                }
                Ok(Update::Frame(Box::new(PreparedFrame::prepare_v2(
                    frame, hello.grid, assets,
                )?)))
            }
            Message::RetainedFrame(frame) => {
                let (version, hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                let generation = frame.generation;
                let number = frame.frame;
                let mut candidate = self.retained.clone();
                let accepted = match candidate.apply(frame, hello.grid) {
                    Ok(accepted) => accepted,
                    Err(message) => {
                        self.retained.require_reset();
                        return Ok(Update::RetainedRejected {
                            generation,
                            expected_generation: self.retained.expected_generation(),
                            message,
                        });
                    }
                };
                let enabled = hello.get_enabled_capabilities(*version);
                if let Err(message) = validate_retained_canvas_capability(&accepted.scene, &enabled)
                    .and_then(|()| validate_retained_sprite_capability(&accepted.scene, &enabled))
                {
                    self.retained.require_reset();
                    return Ok(Update::RetainedRejected {
                        generation,
                        expected_generation: self.retained.expected_generation(),
                        message,
                    });
                }
                if !accepted.visible {
                    self.retained = candidate;
                    return Ok(Update::RetainedAck {
                        generation,
                        frame: accepted.frame,
                    });
                }
                let prepared = if let Some(previous) = &self.prepared
                    && Arc::ptr_eq(&previous.source, &accepted.scene)
                {
                    previous.clone()
                } else {
                    match PreparedScene::prepare(
                        accepted.scene,
                        number,
                        hello.grid,
                        assets,
                        self.prepared.as_deref(),
                    ) {
                        Ok(prepared) => Arc::new(prepared),
                        Err(message) => {
                            self.retained.require_reset();
                            return Ok(Update::RetainedRejected {
                                generation,
                                expected_generation: self.retained.expected_generation(),
                                message,
                            });
                        }
                    }
                };
                self.retained = candidate;
                self.prepared = Some(prepared.clone());
                Ok(Update::RetainedFrame(Box::new(RetainedDisplay {
                    generation,
                    frame: accepted.frame,
                    reset: accepted.reset,
                    scene: prepared,
                    viewport: accepted.viewport,
                    observation: None,
                })))
            }
            Message::Shutdown => Ok(Update::Shutdown),
        }
    }
}

fn validate_retained_canvas_capability(
    scene: &SceneSource,
    enabled: &[Capability],
) -> Result<(), String> {
    if scene.canvas.as_ref().is_some_and(|root| root.is_overlay())
        && (!enabled.contains(&Capability::CanvasOverlay)
            || !enabled.contains(&Capability::GraphicalCanvas))
    {
        return Err(
            "overlay canvas requires negotiated canvas_overlay and graphical_canvas capabilities"
                .into(),
        );
    }
    if scene.canvas.is_some() {
        CanvasFeatures::get_from_scene(scene).validate(enabled)?;
    }
    Ok(())
}

fn validate_retained_sprite_capability(
    scene: &SceneSource,
    enabled: &[Capability],
) -> Result<(), String> {
    if !scene.sprite_pivots.is_empty() && !enabled.contains(&Capability::SpritePivot) {
        return Err("sprite pivot requires negotiated sprite_pivot capability".into());
    }
    if !scene.sprite_quarter_turns.is_empty() && !enabled.contains(&Capability::SpriteQuarterTurns)
    {
        return Err(
            "sprite quarterTurns requires negotiated sprite_quarter_turns capability".into(),
        );
    }
    Ok(())
}

#[derive(Default)]
struct CanvasFeatures {
    composites: bool,
    glyph_effects: bool,
    clip_opacity: bool,
    image_tone: bool,
    image_flip: bool,
    source_rect: bool,
}

impl CanvasFeatures {
    #[cfg(test)]
    fn get_from_canvas(canvas: &Canvas) -> Self {
        Self {
            composites: canvas.composites.is_some(),
            glyph_effects: canvas
                .text_layers
                .iter()
                .any(|text| text.glyph_effects.is_some()),
            clip_opacity: canvas.images.iter().any(|image| image.clip_rect.is_some())
                || canvas
                    .text_layers
                    .iter()
                    .any(|text| text.clip_rect.is_some() || text.opacity.is_some()),
            image_tone: canvas.images.iter().any(|image| image.brightness.is_some()),
            image_flip: canvas
                .images
                .iter()
                .any(|image| image.flip_x.is_some() || image.flip_y.is_some()),
            source_rect: canvas
                .images
                .iter()
                .any(|image| image.source_rect.is_some()),
        }
    }

    fn get_from_scene(scene: &SceneSource) -> Self {
        Self {
            composites: !scene.canvas_composites.is_empty(),
            glyph_effects: scene
                .canvas_text
                .values()
                .any(|text| text.item.glyph_effects.is_some()),
            clip_opacity: scene
                .canvas_images
                .values()
                .any(|image| image.item.clip_rect.is_some())
                || scene
                    .canvas_text
                    .values()
                    .any(|text| text.item.clip_rect.is_some() || text.item.opacity.is_some()),
            image_tone: scene
                .canvas_images
                .values()
                .any(|image| image.item.brightness.is_some()),
            image_flip: scene
                .canvas_images
                .values()
                .any(|image| image.item.flip_x.is_some() || image.item.flip_y.is_some()),
            source_rect: scene
                .canvas_images
                .values()
                .any(|image| image.item.source_rect.is_some()),
        }
    }

    fn validate(&self, enabled: &[Capability]) -> Result<(), String> {
        if self.composites && !enabled.contains(&Capability::CanvasCompositing) {
            return Err("composites requires negotiated canvas_compositing capability".into());
        }
        if !enabled.contains(&Capability::GraphicalCanvas) {
            return Err("canvas requires negotiated graphical_canvas capability".into());
        }
        if self.glyph_effects && !enabled.contains(&Capability::CanvasGlyphEffects) {
            return Err("glyphEffects requires negotiated canvas_glyph_effects capability".into());
        }
        if self.clip_opacity && !enabled.contains(&Capability::CanvasClipOpacity) {
            return Err(
                "canvas clipRect/text opacity requires negotiated canvas_clip_opacity capability"
                    .into(),
            );
        }
        if self.image_tone && !enabled.contains(&Capability::CanvasImageTone) {
            return Err(
                "canvas brightness requires negotiated canvas_image_tone and graphical_canvas capabilities"
                    .into(),
            );
        }
        if self.image_flip && !enabled.contains(&Capability::CanvasImageFlip) {
            return Err("canvas mirroring requires negotiated canvas_image_flip capability".into());
        }
        if self.source_rect && !enabled.contains(&Capability::SpriteSourceRect) {
            return Err(
                "canvas sourceRect requires negotiated sprite_source_rect capability".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
fn validate_capabilities(sprites: &[Sprite], enabled: &[Capability]) -> Result<(), String> {
    if sprites.iter().any(|sprite| sprite.source_rect.is_some())
        && !enabled.contains(&Capability::SpriteSourceRect)
    {
        return Err("sourceRect requires negotiated sprite_source_rect capability".into());
    }
    Ok(())
}

pub fn start_reader(diagnostics: Diagnostics) -> async_channel::Receiver<Update> {
    // Backpressure pauses only the reader. No polling loop or presentation timer.
    let (tx, rx) = async_channel::bounded(2);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        read_protocol_observed(stdin.lock(), &diagnostics, |update| {
            submit_update(&tx, update, &diagnostics)
        });
    });
    rx
}

fn submit_update(
    tx: &async_channel::Sender<Update>,
    update: Update,
    diagnostics: &Diagnostics,
) -> bool {
    let observation = update.observation();
    diagnostics.frame_queue_stage(observation, "submit_begin", || (tx.len(), tx.capacity()));
    let sent = tx.send_blocking(update).is_ok();
    diagnostics.frame_queue_stage(
        observation,
        if sent { "submit_end" } else { "submit_failed" },
        || (tx.len(), tx.capacity()),
    );
    sent
}

#[cfg(test)]
fn read_protocol(reader: impl BufRead, send: impl FnMut(Update) -> bool) {
    read_protocol_observed(reader, &Diagnostics::default(), send);
}

fn read_protocol_observed(
    mut reader: impl BufRead,
    diagnostics: &Diagnostics,
    mut send: impl FnMut(Update) -> bool,
) {
    let mut session = Session::default();
    loop {
        let mut bytes = Vec::new();
        let count = match reader
            .by_ref()
            .take(protocol::MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)
        {
            Ok(count) => count,
            Err(error) => {
                send(Update::Fatal(format!("stdin read failed: {error}")));
                break;
            }
        };
        if count == 0 {
            send(Update::Eof);
            break;
        }
        if count > protocol::MAX_LINE_BYTES {
            if bytes.last() != Some(&b'\n')
                && let Err(error) = reader.skip_until(b'\n')
            {
                send(Update::Fatal(format!("stdin read failed: {error}")));
                break;
            }
            if !send(Update::Error("protocol line exceeds 4 MiB".into())) {
                break;
            }
            continue;
        }
        let received_at = diagnostics.now();
        let update = protocol::parse(&bytes)
            .and_then(|incoming| {
                let observation = match &incoming.message {
                    #[cfg(test)]
                    Message::Frame(frame) => diagnostics.frame_received(
                        incoming.version.number(),
                        frame.frame,
                        received_at,
                    ),
                    #[cfg(test)]
                    Message::FrameV2(frame) => diagnostics.frame_received(
                        incoming.version.number(),
                        frame.frame,
                        received_at,
                    ),
                    Message::RetainedFrame(frame) => diagnostics.frame_received(
                        incoming.version.number(),
                        frame.frame,
                        received_at,
                    ),
                    _ => None,
                };
                let mut update = session.prepare(incoming)?;
                #[cfg(test)]
                if let Update::Frame(frame) = &mut update {
                    frame.observation = observation;
                    diagnostics.frame_stage(observation, "accepted");
                    diagnostics.frame_resources(observation, frame.resources.clone());
                }
                if let Update::RetainedFrame(frame) = &mut update {
                    frame.observation = observation;
                    diagnostics.frame_stage(observation, "accepted");
                }
                Ok(update)
            })
            .unwrap_or_else(|message| session.reject_malformed(&bytes, message));
        let shutdown = matches!(update, Update::Shutdown);
        if !send(update) || shutdown {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retained_overlay_requires_both_negotiated_capabilities() {
        let mut scene = SceneSource::default();
        scene
            .apply(
                serde_json::from_value(json!({"op":"put","kind":"canvas","id":"canvas",
                "value":{"width":40,"height":40,"mode":"overlay"}}))
                .unwrap(),
                protocol::Grid {
                    columns: 4,
                    rows: 2,
                    cell_width: 10,
                    cell_height: 20,
                },
            )
            .unwrap();
        for enabled in [
            vec![],
            vec![Capability::GraphicalCanvas],
            vec![Capability::CanvasOverlay],
        ] {
            assert!(validate_retained_canvas_capability(&scene, &enabled).is_err());
        }
        assert!(
            validate_retained_canvas_capability(
                &scene,
                &[Capability::CanvasOverlay, Capability::GraphicalCanvas]
            )
            .is_ok()
        );
    }

    #[test]
    fn retained_image_tone_is_gated_by_field_presence_and_updates() {
        let grid = protocol::Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        };
        let mut scene = SceneSource::default();
        scene
            .apply(
                serde_json::from_value(json!({"op":"put","kind":"canvas","id":"canvas",
                "value":{"width":40,"height":40}}))
                .unwrap(),
                grid,
            )
            .unwrap();
        let image = |brightness: Option<f64>| {
            let mut value = json!({
                "id":"portrait","order":0,"layer":1,"asset":"portrait.png",
                "destination":{"x":0,"y":0,"width":10,"height":10}
            });
            if let Some(brightness) = brightness {
                value["brightness"] = json!(brightness);
            }
            serde_json::from_value(json!({
                "op":"put","kind":"canvas_image","id":"portrait","value":value
            }))
            .unwrap()
        };
        scene.apply(image(None), grid).unwrap();
        assert!(
            validate_retained_canvas_capability(&scene, &[Capability::GraphicalCanvas]).is_ok()
        );

        // Explicit 1.0 still requires the negotiated feature; omission does not.
        scene.apply(image(Some(1.0)), grid).unwrap();
        assert_eq!(
            validate_retained_canvas_capability(&scene, &[]).unwrap_err(),
            "canvas requires negotiated graphical_canvas capability"
        );
        assert_eq!(
            validate_retained_canvas_capability(&scene, &[Capability::GraphicalCanvas])
                .unwrap_err(),
            "canvas brightness requires negotiated canvas_image_tone and graphical_canvas capabilities"
        );
        assert_eq!(
            validate_retained_canvas_capability(&scene, &[Capability::CanvasImageTone])
                .unwrap_err(),
            "canvas requires negotiated graphical_canvas capability"
        );
        let enabled = [Capability::GraphicalCanvas, Capability::CanvasImageTone];
        assert!(validate_retained_canvas_capability(&scene, &enabled).is_ok());
        scene.apply(image(Some(0.6)), grid).unwrap();
        assert_eq!(scene.canvas_images["portrait"].item.get_brightness(), 0.6);
        assert!(validate_retained_canvas_capability(&scene, &enabled).is_ok());
        scene.apply(image(None), grid).unwrap();
        assert!(
            validate_retained_canvas_capability(&scene, &[Capability::GraphicalCanvas]).is_ok()
        );
    }

    #[test]
    fn retained_image_flips_require_negotiation_and_clear_on_replacement() {
        let grid = protocol::Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        };
        let mut scene = SceneSource::default();
        scene
            .apply(
                serde_json::from_value(json!({"op":"put","kind":"canvas","id":"canvas",
            "value":{"width":40,"height":40}}))
                .unwrap(),
                grid,
            )
            .unwrap();
        let image = |flip: Option<bool>| {
            let mut value = json!({"id":"stroke","order":0,"layer":1,"asset":"stroke.png",
                "destination":{"x":0,"y":0,"width":10,"height":10}});
            if let Some(flip) = flip {
                value["flipX"] = json!(flip);
            }
            serde_json::from_value(
                json!({"op":"put","kind":"canvas_image","id":"stroke","value":value}),
            )
            .unwrap()
        };
        let graphical = [Capability::GraphicalCanvas];
        scene.apply(image(None), grid).unwrap();
        assert!(validate_retained_canvas_capability(&scene, &graphical).is_ok());
        for flip in [false, true] {
            scene.apply(image(Some(flip)), grid).unwrap();
            assert_eq!(
                validate_retained_canvas_capability(&scene, &graphical).unwrap_err(),
                "canvas mirroring requires negotiated canvas_image_flip capability"
            );
            assert!(
                validate_retained_canvas_capability(
                    &scene,
                    &[Capability::GraphicalCanvas, Capability::CanvasImageFlip]
                )
                .is_ok()
            );
        }
        scene.apply(image(None), grid).unwrap();
        assert!(validate_retained_canvas_capability(&scene, &graphical).is_ok());
        assert_eq!(scene.canvas_images["stroke"].item.flip_x, None);
    }

    #[test]
    fn retained_canvas_crops_and_clips_require_the_same_capabilities_as_complete_frames() {
        let grid = protocol::Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        };
        let mut scene = SceneSource::default();
        scene
            .apply(
                serde_json::from_value(json!({"op":"put","kind":"canvas","id":"canvas",
                    "value":{"width":40,"height":40}}))
                .unwrap(),
                grid,
            )
            .unwrap();
        assert_eq!(
            validate_retained_canvas_capability(&scene, &[]).unwrap_err(),
            "canvas requires negotiated graphical_canvas capability"
        );
        let image = |clip: bool| {
            let mut value = json!({
                "id":"portrait","order":0,"layer":1,"asset":"portrait.png",
                "destination":{"x":0,"y":0,"width":10,"height":10},
                "sourceRect":{"x":0,"y":0,"width":2,"height":2}
            });
            if clip {
                value["clipRect"] = json!({"x":0,"y":0,"width":10,"height":10});
            }
            serde_json::from_value(json!({
                "op":"put","kind":"canvas_image","id":"portrait","value":value
            }))
            .unwrap()
        };
        scene.apply(image(false), grid).unwrap();
        let graphical = [Capability::GraphicalCanvas];
        assert_eq!(
            validate_retained_canvas_capability(&scene, &graphical).unwrap_err(),
            "canvas sourceRect requires negotiated sprite_source_rect capability"
        );
        let cropped = [Capability::GraphicalCanvas, Capability::SpriteSourceRect];
        assert!(validate_retained_canvas_capability(&scene, &cropped).is_ok());
        scene.apply(image(true), grid).unwrap();
        assert_eq!(
            validate_retained_canvas_capability(&scene, &cropped).unwrap_err(),
            "canvas clipRect/text opacity requires negotiated canvas_clip_opacity capability"
        );
        assert!(
            validate_retained_canvas_capability(
                &scene,
                &[
                    Capability::GraphicalCanvas,
                    Capability::SpriteSourceRect,
                    Capability::CanvasClipOpacity,
                ]
            )
            .is_ok()
        );
    }

    #[test]
    fn retained_sprite_quarter_turns_requires_capability_even_for_explicit_zero() {
        let grid = protocol::Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        };
        let mut scene = SceneSource::default();
        let sprite = |turns: Option<u8>| {
            let mut value = json!({"id":"arrow","order":0,"layer":1,
                "asset":"arrow.png","x":0,"y":0,"width":10,"height":10,
                "anchor":"bottom_center"});
            if let Some(turns) = turns {
                value["quarterTurns"] = json!(turns);
            }
            serde_json::from_value(json!({"op":"put","kind":"sprite","id":"arrow",
                "value":value}))
            .unwrap()
        };
        scene.apply(sprite(None), grid).unwrap();
        assert!(validate_retained_sprite_capability(&scene, &[]).is_ok());
        scene.apply(sprite(Some(0)), grid).unwrap();
        assert_eq!(
            validate_retained_sprite_capability(&scene, &[]).unwrap_err(),
            "sprite quarterTurns requires negotiated sprite_quarter_turns capability"
        );
        let enabled = [Capability::SpriteQuarterTurns];
        assert!(validate_retained_sprite_capability(&scene, &enabled).is_ok());
        scene.apply(sprite(Some(3)), grid).unwrap();
        assert!(validate_retained_sprite_capability(&scene, &enabled).is_ok());
        scene.apply(sprite(None), grid).unwrap();
        assert!(validate_retained_sprite_capability(&scene, &[]).is_ok());
    }

    #[test]
    fn retained_sprite_pivot_requires_capability_even_for_explicit_bottom_center() {
        let grid = protocol::Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        };
        let mut scene = SceneSource::default();
        for pivot in [
            None,
            Some(json!({"x":0.5,"y":1})),
            Some(json!({"x":0.25,"y":0.625})),
            None,
        ] {
            let mut value = json!({"id":"effect","order":0,"layer":1,
                "asset":"effect.png","x":0,"y":0,"width":10,"height":10,"anchor":"bottom_center"});
            if let Some(pivot) = &pivot {
                value["pivot"] = pivot.clone();
            }
            scene
                .apply(
                    serde_json::from_value(
                        json!({"op":"put","kind":"sprite","id":"effect","value":value}),
                    )
                    .unwrap(),
                    grid,
                )
                .unwrap();
            assert_eq!(
                validate_retained_sprite_capability(&scene, &[]).is_ok(),
                pivot.is_none()
            );
            assert!(
                validate_retained_sprite_capability(&scene, &[Capability::SpritePivot]).is_ok()
            );
        }
    }

    #[test]
    #[ignore = "requires ICHILOTO_RETAINED_REPLAY NDJSON"]
    fn replay_engine_retained_packets_without_a_native_window() {
        let input =
            std::fs::read_to_string(std::env::var("ICHILOTO_RETAINED_REPLAY").unwrap()).unwrap();
        let baseline = std::env::var("ICHILOTO_RETAINED_BASELINE")
            .ok()
            .map(|path| {
                std::fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .filter_map(|line| {
                        let parsed = protocol::parse(line.as_bytes()).unwrap();
                        match parsed.message {
                            Message::FrameV2(frame) => Some((frame.frame, frame)),
                            _ => None,
                        }
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            });
        let mut session = Session::default();
        let mut grid = None;
        let mut staged = 0;
        let mut presented = 0;
        let mut reports = Vec::new();
        for (index, line) in input.lines().filter(|line| !line.is_empty()).enumerate() {
            let started = std::time::Instant::now();
            let parsed = protocol::parse(line.as_bytes())
                .unwrap_or_else(|error| panic!("line {}: {error}", index + 1));
            if let Message::Hello(hello) = &parsed.message {
                grid = Some(hello.grid);
            }
            let parse_ms = started.elapsed().as_secs_f64() * 1000.0;
            let prepared = session
                .prepare(parsed)
                .unwrap_or_else(|error| panic!("line {}: {error}", index + 1));
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            match prepared {
                Update::Hello(..) => {}
                Update::RetainedAck { .. } => staged += 1,
                Update::RetainedFrame(frame) => {
                    presented += 1;
                    if std::env::var_os("ICHILOTO_RETAINED_SCREEN_PARITY").is_some() {
                        let baseline = baseline.as_ref().expect("screen parity requires baseline");
                        let skipped = std::env::var("ICHILOTO_RETAINED_BASELINE_SKIP_AT")
                            .ok()
                            .and_then(|value| value.parse::<u64>().ok());
                        let old_number = frame.frame
                            + u64::from(skipped.is_some_and(|number| frame.frame >= number));
                        let old = &baseline[&old_number];
                        let current = frame.scene.source.materialize_screen(frame.frame).unwrap();
                        assert_eq!(
                            current.text_layers, old.text_layers,
                            "text frame {}",
                            frame.frame
                        );
                        assert_eq!(
                            current.sprites, old.sprites,
                            "sprites frame {}",
                            frame.frame
                        );
                        let mut canvas = current.canvas;
                        if let Some(canvas) = &mut canvas
                            && canvas.composites.as_ref().is_some_and(Vec::is_empty)
                        {
                            canvas.composites = None;
                        }
                        assert_eq!(canvas, old.canvas, "canvas frame {}", frame.frame);
                        if old
                            .canvas
                            .as_ref()
                            .and_then(|canvas| canvas.composites.as_ref())
                            .is_some_and(|composites| !composites.is_empty())
                        {
                            let assets = &session.initialized.as_ref().unwrap().2;
                            let original =
                                PreparedFrame::prepare_v2(old.clone(), grid.unwrap(), assets)
                                    .unwrap();
                            let original = original.canvas.unwrap();
                            let retained = frame.scene.screen.canvas.as_ref().unwrap();
                            assert_eq!(retained.composites.len(), original.composites.len());
                            for (index, (retained, original)) in retained
                                .composites
                                .iter()
                                .zip(&original.composites)
                                .enumerate()
                            {
                                assert_eq!(retained.as_bytes(0), original.as_bytes(0));
                                if let Ok(directory) = std::env::var("ICHILOTO_RETAINED_PNG_DIR") {
                                    std::fs::create_dir_all(&directory).unwrap();
                                    let label = std::env::var("ICHILOTO_RETAINED_LABEL").unwrap();
                                    for (kind, image) in
                                        [("reference", original), ("retained", retained)]
                                    {
                                        let size = image.size(0);
                                        let width = size.width.0 as u32;
                                        let height = size.height.0 as u32;
                                        let pixels = image.as_bytes(0).unwrap();
                                        let rgba =
                                            image::RgbaImage::from_fn(width, height, |x, y| {
                                                let at = ((y * width + x) * 4) as usize;
                                                image::Rgba([
                                                    pixels[at + 2],
                                                    pixels[at + 1],
                                                    pixels[at],
                                                    pixels[at + 3],
                                                ])
                                            });
                                        rgba.save(format!(
                                            "{directory}/{label}-frame-{old_number:03}-composite-{index}-{kind}.png"
                                        ))
                                        .unwrap();
                                    }
                                }
                            }
                        }
                    }
                    reports.push(json!({"line":index+1,"frame":frame.frame,
                        "generation":frame.generation,"parseMs":parse_ms,"totalMs":total_ms,
                        "worlds":frame.scene.worlds.len(),
                        "sourceBytes":frame.scene.source.estimated_bytes()}));
                }
                Update::RetainedRejected { message, .. } => {
                    panic!("line {} rejected: {message}", index + 1)
                }
                _ => panic!("unexpected update at line {}", index + 1),
            }
        }
        if let Ok(report) = std::env::var("ICHILOTO_RETAINED_REPORT") {
            std::fs::write(report, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
        }
        assert!(presented > 0);
        eprintln!("retained replay: {staged} staged, {presented} presented");
    }

    #[test]
    fn retained_wire_stages_world_reuses_prepared_scene_and_rejects_bad_delta() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let mut session = Session::default();
        let hello = json!({"protocol":2,"type":"hello","title":"Retained",
            "assetRoot":root,"grid":{"columns":4,"rows":2,"cellWidth":10,"cellHeight":20}});
        assert!(matches!(
            session.prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap()),
            Ok(Update::Hello(..))
        ));
        let cell = json!({"glyph":"..","foreground":null,"background":null,
            "ownerLayerId":"map:terrain"});
        let stage = json!({"protocol":2,"type":"frame","frame":1,"baseGeneration":0,
        "generation":1,"reset":true,"present":false,"operations":[
            {"op":"put","kind":"world","id":"map","value":{"columns":4,"rows":2,"cellWidth":5,
                "cellHeight":10,"layers":[{"id":"map:terrain","layer":-100,"kind":"gameplay"}]}},
            {"op":"worldRows","id":"map","rows":[
                {"row":0,"cells":vec![cell.clone();4]},
                {"row":1,"cells":vec![cell;4]}]}
        ]});
        let value = session
            .prepare(protocol::parse(&serde_json::to_vec(&stage).unwrap()).unwrap())
            .unwrap();
        assert!(matches!(value, Update::RetainedAck { generation: 1, .. }));
        let viewport = json!({"scale":1,"origin":{"x":0,"y":0},
            "worldOrigin":{"column":0,"row":0},
            "clipRect":{"x":0,"y":0,"width":40,"height":40},"worldId":"map"});
        let commit = json!({"protocol":2,"type":"frame","frame":1,"baseGeneration":1,
            "generation":2,"present":true,"operations":[],"viewport":viewport});
        let Update::RetainedFrame(first) = session
            .prepare(protocol::parse(&serde_json::to_vec(&commit).unwrap()).unwrap())
            .unwrap()
        else {
            panic!("expected retained commit")
        };
        assert_eq!(first.scene.worlds["map"].layers[0].id, "map:terrain");
        assert_eq!(
            first.scene.worlds["map"].source.get_row(0).unwrap().cells[0].glyph,
            ".."
        );
        let scroll = json!({"protocol":2,"type":"frame","frame":2,"baseGeneration":2,
            "generation":3,"present":true,"operations":[],"viewport":viewport});
        let Update::RetainedFrame(second) = session
            .prepare(protocol::parse(&serde_json::to_vec(&scroll).unwrap()).unwrap())
            .unwrap()
        else {
            panic!("expected camera frame")
        };
        assert!(Arc::ptr_eq(&first.scene, &second.scene));
        let bad = json!({"protocol":2,"type":"frame","frame":3,"baseGeneration":2,
            "generation":4,"present":true,"operations":[]});
        let Update::RetainedRejected {
            expected_generation,
            ..
        } = session
            .prepare(protocol::parse(&serde_json::to_vec(&bad).unwrap()).unwrap())
            .unwrap()
        else {
            panic!("expected rejection")
        };
        assert_eq!(expected_generation, 3);
        let malformed = json!({"protocol":2,"type":"frame","frame":4,
            "baseGeneration":3,"generation":5,"present":true,"operations":[],
            "unexpected":true});
        let bytes = serde_json::to_vec(&malformed).unwrap();
        let error = protocol::parse(&bytes).unwrap_err();
        assert!(matches!(
            session.reject_malformed(&bytes, error),
            Update::RetainedRejected {
                generation: 5,
                expected_generation: 3,
                ..
            }
        ));
    }

    #[test]
    fn retained_wire_tiles_survive_missing_sheets_and_animate_by_camera() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let mut session = Session::default();
        let hello = json!({"protocol":2,"type":"hello","title":"Tiles",
            "assetRoot":root,"grid":{"columns":4,"rows":2,"cellWidth":10,"cellHeight":20}});
        assert!(matches!(
            session.prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap()),
            Ok(Update::Hello(..))
        ));
        let piece = |sheet: u32, x: u32| json!([{"sheet":sheet,"x":x,"y":0,"width":16,"height":16,"left":0,"top":0}]);
        let cell = json!({"glyph":"..","foreground":null,"background":null,
            "ownerLayerId":"map:terrain"});
        let viewport = |frame: u32| {
            json!({"scale":1,"origin":{"x":0,"y":0},"clipRect":{"x":0,"y":0,"width":40,"height":40},
                "worldId":"map","tileFrame":frame})
        };
        let put = json!({"protocol":2,"type":"frame","frame":1,"baseGeneration":0,
        "generation":1,"reset":true,"present":true,"operations":[
            {"op":"put","kind":"world","id":"map","value":{"columns":3,"rows":1,"cellWidth":5,
                "cellHeight":10,"layers":[
                    {"id":"map:ground","layer":-100,"kind":"tiles"},
                    {"id":"map:terrain","layer":-99,"kind":"gameplay"}],
                "tileset":{"tileSize":16,"sheets":["test-sprite.png","missing-tiles.png"],
                    "tiles":[{"frames":[piece(0,0),piece(0,16)]},{"frames":[piece(1,0)]}]}}},
            {"op":"worldRows","id":"map","rows":[{"row":0,"cells":[cell.clone(),cell.clone(),cell]}]},
            {"op":"worldTiles","id":"map","layerId":"map:ground","rows":[
                {"row":0,"cells":[{"column":0,"tile":0},{"column":2,"tile":1}]}]}
        ],"viewport":viewport(0)});
        let Update::RetainedFrame(first) = session
            .prepare(protocol::parse(&serde_json::to_vec(&put).unwrap()).unwrap())
            .unwrap()
        else {
            panic!("a missing sheet must not reject the scene")
        };
        let world = &first.scene.worlds["map"];
        // A tile hides only its own cell's glyph; the tile whose sheet is
        // missing leaves its cell to the glyph.
        assert!(world.has_covering_tile(0, 0, "map:terrain"));
        assert!(!world.has_covering_tile(1, 0, "map:terrain"));
        assert!(!world.has_covering_tile(2, 0, "map:terrain"));
        let still = world.get_tile_image(0, 0).unwrap().id;
        // Animation is a camera-only frame that reuses the prepared scene.
        let animate = json!({"protocol":2,"type":"frame","frame":2,"baseGeneration":1,
            "generation":2,"present":true,"operations":[],"viewport":viewport(3)});
        let Update::RetainedFrame(second) = session
            .prepare(protocol::parse(&serde_json::to_vec(&animate).unwrap()).unwrap())
            .unwrap()
        else {
            panic!("expected camera frame")
        };
        assert!(Arc::ptr_eq(&first.scene, &second.scene));
        let frame = second.viewport.as_ref().unwrap().tile_frame;
        assert_eq!(frame, 3);
        assert_ne!(world.get_tile_image(0, frame).unwrap().id, still);
        let bad = json!({"protocol":2,"type":"frame","frame":3,"baseGeneration":2,
            "generation":3,"present":true,"operations":[
                {"op":"worldTiles","id":"map","layerId":"map:terrain","rows":[
                    {"row":0,"cells":[{"column":0,"tile":0}]}]}]});
        assert!(matches!(
            session
                .prepare(protocol::parse(&serde_json::to_vec(&bad).unwrap()).unwrap())
                .unwrap(),
            Update::RetainedRejected { .. }
        ));
    }

    #[test]
    fn retained_canvas_composite_matches_historical_cpu_pixels() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("compositing/frame.json")).unwrap())
                .unwrap();
        let Message::FrameV2(reference) = protocol::parse(&serde_json::to_vec(&raw).unwrap())
            .unwrap()
            .message
        else {
            panic!("reference frame")
        };
        let grid = protocol::Grid {
            columns: 64,
            rows: 48,
            cell_width: 1,
            cell_height: 1,
        };
        let assets = AssetRoot::new(&root).unwrap();
        let reference_canvas = PreparedFrame::prepare_v2(reference.clone(), grid, &assets)
            .unwrap()
            .canvas
            .unwrap();
        let reference_pixels = reference_canvas.composites[0].as_bytes(0).unwrap();
        let mut session = Session::default();
        let hello = json!({"protocol":2,"type":"hello","title":"Retained canvas",
            "assetRoot":root,"grid":{"columns":64,"rows":48,"cellWidth":1,"cellHeight":1}});
        session
            .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
            .unwrap();
        let canvas = &raw["canvas"];
        let mut operations = vec![json!({"op":"put","kind":"canvas","id":"canvas",
            "value":{"id":"canvas","width":canvas["width"],"height":canvas["height"]}})];
        for (order, composite) in canvas["composites"].as_array().unwrap().iter().enumerate() {
            let mut value = composite.clone();
            value["order"] = json!(order);
            operations.push(json!({"op":"put","kind":"canvas_composite",
                "id":composite["id"],"value":value}));
        }
        let update = json!({"protocol":2,"type":"frame","frame":1,
            "baseGeneration":0,"generation":1,"reset":true,"present":true,
            "operations":operations,"viewport":null});
        let Update::RetainedFrame(retained) = session
            .prepare(protocol::parse(&serde_json::to_vec(&update).unwrap()).unwrap())
            .unwrap()
        else {
            panic!("retained composite frame")
        };
        assert_eq!(
            retained.scene.source.materialize_canvas().unwrap(),
            reference.canvas
        );
        let retained_pixels = retained.scene.screen.canvas.as_ref().unwrap().composites[0]
            .as_bytes(0)
            .unwrap();
        assert_eq!(retained_pixels, reference_pixels);
    }

    #[test]
    fn retained_reset_releases_old_world_after_rejected_update() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let mut session = Session::default();
        let hello = json!({"protocol":2,"type":"hello","title":"Retained lifecycle",
            "assetRoot":root,"grid":{"columns":1,"rows":1,"cellWidth":10,"cellHeight":20}});
        session
            .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
            .unwrap();
        let first = json!({"protocol":2,"type":"frame","frame":1,
            "baseGeneration":0,"generation":1,"reset":true,"present":true,
            "operations":[
                {"op":"put","kind":"world","id":"map","value":{
                    "columns":1,"rows":1,"cellWidth":5,"cellHeight":10,"layers":[{"id":"map:terrain","layer":-100,"kind":"gameplay"}]}},
                {"op":"worldRows","id":"map","rows":[{"row":0,"cells":[{
                    "glyph":"..","foreground":null,"background":null,
                    "ownerLayerId":"map:terrain"}]}]}
            ],"viewport":{"worldId":"map","scale":1,
                "origin":{"x":0,"y":0},"clipRect":{"x":0,"y":0,"width":10,"height":20}}});
        let visible = session
            .prepare(protocol::parse(&serde_json::to_vec(&first).unwrap()).unwrap())
            .unwrap();
        let old_scene = Arc::downgrade(session.prepared.as_ref().unwrap());
        let old_world = Arc::downgrade(&session.prepared.as_ref().unwrap().source.worlds["map"]);
        drop(visible);

        let invalid = json!({"protocol":2,"type":"frame","frame":2,
            "baseGeneration":0,"generation":2,"operations":[]});
        assert!(matches!(
            session
                .prepare(protocol::parse(&serde_json::to_vec(&invalid).unwrap()).unwrap())
                .unwrap(),
            Update::RetainedRejected { .. }
        ));
        assert!(old_scene.upgrade().is_some());
        assert!(old_world.upgrade().is_some());

        let reset = json!({"protocol":2,"type":"frame","frame":3,
            "baseGeneration":1,"generation":3,"reset":true,"present":true,
            "operations":[],"viewport":null});
        let replacement = session
            .prepare(protocol::parse(&serde_json::to_vec(&reset).unwrap()).unwrap())
            .unwrap();
        assert!(matches!(replacement, Update::RetainedFrame(_)));
        assert!(old_scene.upgrade().is_none());
        assert!(old_world.upgrade().is_none());
    }

    #[test]
    fn cross_language_tile_fixtures_validate_exact_payloads_and_keep_last_display_on_error() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let dir = root.join("tile-batches");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        for case in manifest["cases"].as_array().unwrap() {
            let name = case["file"].as_str().unwrap();
            let stage = case["stage"].as_str().unwrap();
            let mut hello: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("hello.json")).unwrap()).unwrap();
            hello["assetRoot"] = serde_json::json!(root);
            if let Some(caps) = case.get("capabilities") {
                hello["requiredCapabilities"] = caps.clone();
            }
            let mut session = Session::default();
            let Update::Hello(_, config) = session
                .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
                .unwrap()
            else {
                panic!()
            };
            let assets = AssetRoot::new(&root).unwrap();
            let mut state = crate::state::RendererState {
                hello: config.clone(),
                frame: None,
            };
            // A genuine prior display survives parse/session/asset/budget failures.
            let source = protocol::parse(include_bytes!(
                "../fixtures/tile-batches/valid-tiles-text-player-ui.json"
            ))
            .unwrap();
            let Message::FrameV2(source) = source.message else {
                panic!()
            };
            state.replace(PreparedFrame::prepare_v2(source, config.grid, &assets).unwrap());
            let parsed = protocol::parse(&std::fs::read(dir.join(name)).unwrap());
            if stage == "parse" {
                assert!(parsed.is_err(), "{name}");
                continue;
            }
            let incoming = parsed.unwrap_or_else(|e| panic!("{name}: {e}"));
            let Message::FrameV2(ref frame) = incoming.message else {
                panic!("{name}")
            };
            if stage == "frame" {
                assert!(frame.validate(config.grid).is_err(), "{name}");
            } else {
                frame
                    .validate(config.grid)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            let update = session.prepare(incoming);
            if stage == "accept" || stage == "session" {
                let Update::Frame(frame) = update.unwrap_or_else(|e| panic!("{name}: {e}")) else {
                    panic!()
                };
                state.replace(*frame);
                if name.starts_with("clear-") {
                    assert!(state.frame.as_ref().unwrap().tile_batches.is_empty());
                }
                if name == "valid-full-viewport.json" {
                    assert_eq!(state.frame.as_ref().unwrap().resources.tile_cells, 4860);
                }
            } else {
                assert!(update.is_err(), "{name}");
                assert_eq!(
                    state.frame.as_ref().unwrap().tile_batches.len(),
                    1,
                    "{name}"
                );
                assert_eq!(
                    state.frame.as_ref().unwrap().resources.tile_cells,
                    2,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn traced_handoff_preserves_bounded_order_and_reports_receiver_close() {
        use std::io::Write;
        use std::sync::{Arc, Mutex, mpsc};
        struct Capture {
            bytes: Arc<Mutex<Vec<u8>>>,
            third_begin: mpsc::Sender<()>,
        }
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let record: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                self.bytes.lock().unwrap().extend_from_slice(bytes);
                if record["stage"] == "submit_begin" && record["sequence"] == 3 {
                    self.third_begin.send(()).unwrap();
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for version in [1, 2] {
            for close_receiver in [false, true] {
                let bytes = Arc::new(Mutex::new(Vec::new()));
                let (begin_tx, begin_rx) = mpsc::channel();
                let diagnostics = Diagnostics::with_writer(Capture {
                    bytes: bytes.clone(),
                    third_begin: begin_tx,
                });
                let mut input = hello(version);
                input.push(b'\n');
                for number in [7, 7, 2] {
                    let mut frame = serde_json::json!({"protocol":version,"type":"frame","frame":number,"sprites":[]});
                    frame[if version == 1 { "text" } else { "textLayers" }] = serde_json::json!([]);
                    input.extend(serde_json::to_vec(&frame).unwrap());
                    input.push(b'\n');
                }
                let mut frames = Vec::new();
                read_protocol_observed(input.as_slice(), &diagnostics, |u| {
                    if matches!(u, Update::Frame(_)) {
                        frames.push(u);
                    }
                    true
                });
                let mut frames = frames.into_iter();
                let (tx, rx) = async_channel::bounded(2);
                assert!(submit_update(&tx, frames.next().unwrap(), &diagnostics));
                assert!(submit_update(&tx, frames.next().unwrap(), &diagnostics));
                let third = frames.next().unwrap();
                let worker_diagnostics = diagnostics.clone();
                let worker_tx = tx.clone();
                let (done_tx, done_rx) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    done_tx
                        .send(submit_update(&worker_tx, third, &worker_diagnostics))
                        .unwrap();
                });
                // Observation acknowledges entry while capacity is still exhausted;
                // no elapsed-time threshold or arbitrary sleep controls this test.
                begin_rx.recv().unwrap();
                assert_eq!(tx.len(), 2);
                assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
                if close_receiver {
                    rx.close();
                }
                let first = rx.recv_blocking().unwrap();
                assert!(matches!(first, Update::Frame(frame) if frame.number == 7));
                assert_eq!(done_rx.recv().unwrap(), !close_receiver);
                worker.join().unwrap();
                let second = rx.recv_blocking().unwrap();
                assert!(matches!(second, Update::Frame(frame) if frame.number == 7));
                if !close_receiver {
                    let last = rx.recv_blocking().unwrap();
                    assert!(matches!(last, Update::Frame(frame) if frame.number == 2));
                }
                futures_lite::future::block_on(diagnostics.finish()).unwrap();
                let records: Vec<serde_json::Value> =
                    String::from_utf8(bytes.lock().unwrap().clone())
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect();
                for sequence in 1..=3 {
                    let observed: Vec<_> = records
                        .iter()
                        .filter(|r| {
                            r["sequence"] == sequence
                                && r["stage"]
                                    .as_str()
                                    .is_some_and(|s| s.starts_with("submit_"))
                        })
                        .collect();
                    assert_eq!(observed.len(), 2);
                    assert_eq!(observed[0]["stage"], "submit_begin");
                    assert_eq!(
                        observed[1]["stage"],
                        if sequence == 3 && close_receiver {
                            "submit_failed"
                        } else {
                            "submit_end"
                        }
                    );
                    assert_eq!(observed[0]["queued_updates"], sequence - 1);
                    for record in &observed {
                        assert_eq!(record["protocol"], version);
                        assert_eq!(record["frame"], if sequence == 3 { 2 } else { 7 });
                        assert_eq!(record["queue_capacity"], 2);
                        assert!(record["queued_updates"].as_u64().unwrap() <= 2);
                    }
                    assert!(observed[0]["at_ns"].as_u64() <= observed[1]["at_ns"].as_u64());
                }
                assert_eq!(records.last().unwrap()["dropped_records"], 0);
            }
        }
    }

    #[test]
    fn observed_frames_keep_duplicate_numbers_distinct_and_rejections_unaccepted() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for version in [1, 2] {
            let records = Arc::new(Mutex::new(Vec::new()));
            let diagnostics = Diagnostics::with_writer(Capture(records.clone()));
            let mut bytes = hello(version);
            bytes.push(b'\n');
            for (number, valid) in [(7, true), (8, false), (7, true), (2, true)] {
                let mut frame = serde_json::json!({"protocol":version,"type":"frame","frame":number,"sprites":[]});
                frame[if version == 1 { "text" } else { "textLayers" }] = serde_json::json!([]);
                if !valid {
                    frame["sprites"] = serde_json::json!([{"id":"bad","asset":"missing.png","x":1,"y":1,
                        "width":32,"height":48,"anchor":"bottom_center","layer":100}]);
                }
                bytes.extend_from_slice(&serde_json::to_vec(&frame).unwrap());
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(
                format!("malformed\n{{\"protocol\":{version},\"type\":\"shutdown\"}}\n").as_bytes(),
            );
            let mut updates = vec![];
            read_protocol_observed(bytes.as_slice(), &diagnostics, |u| {
                updates.push(u);
                true
            });
            let accepted: Vec<_> = updates
                .iter()
                .filter_map(|u| match u {
                    Update::Frame(frame) => {
                        assert!(frame.observation.is_some());
                        Some(frame.number)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(accepted, [7, 7, 2]);
            assert_eq!(
                updates
                    .iter()
                    .filter(|u| matches!(u, Update::Error(_)))
                    .count(),
                2
            );
            futures_lite::future::block_on(diagnostics.finish()).unwrap();
            let records: Vec<serde_json::Value> =
                String::from_utf8(records.lock().unwrap().clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            let received: Vec<_> = records
                .iter()
                .filter(|r| r["stage"] == "received")
                .collect();
            assert_eq!(received.len(), 4);
            for (index, frame) in [7, 8, 7, 2].iter().enumerate() {
                assert_eq!(received[index]["sequence"], index + 1);
                assert_eq!(received[index]["frame"], *frame);
                assert_eq!(received[index]["protocol"], version);
            }
            let accepted: Vec<_> = records
                .iter()
                .filter(|r| r["stage"] == "accepted")
                .collect();
            assert_eq!(
                accepted
                    .iter()
                    .map(|r| r["sequence"].as_u64().unwrap())
                    .collect::<Vec<_>>(),
                [1, 3, 4]
            );
            for record in accepted {
                let source = received[(record["sequence"].as_u64().unwrap() - 1) as usize];
                assert!(source["at_ns"].as_u64().unwrap() <= record["at_ns"].as_u64().unwrap());
            }
            assert!(
                !records
                    .iter()
                    .any(|r| r["stage"] == "replaced" || r["stage"] == "render_callback")
            );
            assert_eq!(records.last().unwrap()["dropped_records"], 0);
        }
    }
    #[test]
    fn malformed_input_recovers_then_shutdown_stops_reading() {
        let mut updates = Vec::new();
        read_protocol(
            &b"not json\n{\"protocol\":1,\"type\":\"shutdown\"}\nnot read\n"[..],
            |u| {
                updates.push(u);
                true
            },
        );
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], Update::Error(_)));
        assert!(matches!(updates[1], Update::Shutdown));
    }
    #[test]
    fn eof_is_distinct_from_shutdown() {
        let mut updates = Vec::new();
        read_protocol(&b""[..], |u| {
            updates.push(u);
            true
        });
        assert!(matches!(updates[0], Update::Eof));
    }
    #[test]
    fn oversized_input_is_drained_without_losing_next_message() {
        let mut input = vec![b'x'; protocol::MAX_LINE_BYTES + 10];
        input.extend_from_slice(b"\n{\"protocol\":1,\"type\":\"shutdown\"}\n");
        let mut updates = Vec::new();
        read_protocol(input.as_slice(), |u| {
            updates.push(u);
            true
        });
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], Update::Error(_)));
        assert!(matches!(updates[1], Update::Shutdown));
    }
    #[test]
    fn frame_before_hello_rejected() {
        let mut session = Session::default();
        assert!(
            session
                .prepare(
                    protocol::parse(
                        br#"{"protocol":1,"type":"frame","frame":1,"text":[],"sprites":[]}"#
                    )
                    .unwrap()
                )
                .is_err()
        );
    }
    fn hello(version: u32) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"protocol":version,"type":"hello","title":"Session","assetRoot":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures"),"grid":{"columns":80,"rows":24,"cellWidth":16,"cellHeight":24}})).unwrap()
    }
    #[test]
    fn source_rect_is_opt_in_for_v1_and_advertised_for_v2() {
        for version in [1, 2] {
            for enabled in [false, true] {
                let mut hello: serde_json::Value = serde_json::from_slice(&hello(version)).unwrap();
                if enabled {
                    hello["requiredCapabilities"] = serde_json::json!(["sprite_source_rect"]);
                }
                let mut session = Session::default();
                session
                    .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
                    .unwrap();
                let mut frame = serde_json::json!({"protocol":version,"type":"frame","frame":1,"sprites":[{
                    "id":"player","asset":"test-sprite.png","x":8,"y":4,"width":32,"height":48,
                    "anchor":"bottom_center","layer":100,"sourceRect":{"x":0,"y":0,"width":16,"height":24}
                }]});
                frame[if version == 1 { "text" } else { "textLayers" }] = serde_json::json!([]);
                let cropped =
                    session.prepare(protocol::parse(&serde_json::to_vec(&frame).unwrap()).unwrap());
                assert_eq!(cropped.is_ok(), enabled || version == 2);
                if let Err(error) = cropped {
                    assert!(error.contains("negotiated"));
                }
                frame["sprites"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("sourceRect");
                let Update::Frame(full) = session
                    .prepare(protocol::parse(&serde_json::to_vec(&frame).unwrap()).unwrap())
                    .unwrap()
                else {
                    panic!()
                };
                assert!(full.sprites[0].sprite.source_rect.is_none());
            }
        }
    }
    #[test]
    fn failed_hello_does_not_select_session_then_v2_can_initialize() {
        let mut session = Session::default();
        let mut bad: serde_json::Value = serde_json::from_slice(&hello(2)).unwrap();
        bad["assetRoot"] = "/nonexistent-ichiloto-assets".into();
        assert!(
            session
                .prepare(protocol::parse(&serde_json::to_vec(&bad).unwrap()).unwrap())
                .is_err()
        );
        assert!(session.initialized.is_none());
        assert!(matches!(
            session
                .prepare(protocol::parse(&hello(2)).unwrap())
                .unwrap(),
            Update::Hello(Version::V2, _)
        ));
        assert_eq!(session.initialized.as_ref().unwrap().0, Version::V2);
    }
    #[test]
    fn second_hello_and_mixed_versions_never_change_or_stop_session() {
        for version in [1, 2] {
            let mut session = Session::default();
            session
                .prepare(protocol::parse(&hello(version)).unwrap())
                .unwrap();
            for data in [
                hello(version),
                hello(3 - version),
                format!(r#"{{"protocol":{},"type":"shutdown"}}"#, 3 - version).into_bytes(),
                if version == 2 {
                    br#"{"protocol":1,"type":"frame","frame":1,"text":[],"sprites":[]}"#.to_vec()
                } else {
                    br#"{"protocol":2,"type":"frame","frame":1,"textLayers":[],"sprites":[]}"#
                        .to_vec()
                },
            ] {
                assert!(session.prepare(protocol::parse(&data).unwrap()).is_err());
                assert_eq!(session.initialized.as_ref().unwrap().0.number(), version);
            }
            assert!(matches!(
                session
                    .prepare(
                        protocol::parse(
                            format!(r#"{{"protocol":{version},"type":"shutdown"}}"#).as_bytes()
                        )
                        .unwrap()
                    )
                    .unwrap(),
                Update::Shutdown
            ));
        }
    }
    #[test]
    fn malformed_and_mixed_input_recovers_in_v2() {
        let mut bytes = hello(2);
        bytes.extend_from_slice(b"\nmalformed\n{\"protocol\":1,\"type\":\"shutdown\"}\n{\"protocol\":2,\"type\":\"frame\",\"frame\":1,\"textLayers\":[],\"sprites\":[]}\n{\"protocol\":2,\"type\":\"shutdown\"}\n");
        let mut updates = vec![];
        read_protocol(bytes.as_slice(), |u| {
            updates.push(u);
            true
        });
        assert_eq!(updates.len(), 5);
        assert!(matches!(updates[0], Update::Hello(Version::V2, _)));
        assert!(matches!(updates[1], Update::Error(_)));
        assert!(matches!(updates[2], Update::Error(_)));
        assert!(matches!(updates[3], Update::Frame(_)));
        if let Update::Frame(frame) = &updates[3] {
            assert!(frame.observation.is_none());
        }
        assert!(matches!(updates[4], Update::Shutdown));
    }
}
