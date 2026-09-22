//! Typed NDJSON boundary. No renderer code writes directly to stdout.
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Version {
    #[default]
    V1,
    V2,
}

impl Version {
    pub fn number(self) -> u32 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct Incoming {
    pub version: Version,
    pub message: Message,
}

#[derive(Debug)]
pub enum Message {
    Hello(Hello),
    Frame(Frame),
    FrameV2(FrameV2),
    Shutdown,
}

#[derive(Deserialize)]
struct VersionProbe {
    protocol: u32,
}

#[derive(Deserialize)]
struct Envelope<F> {
    protocol: u32,
    #[serde(flatten)]
    message: WireMessage<F>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireMessage<F> {
    Hello(Hello),
    Frame(F),
    Shutdown(Empty),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub title: String,
    pub asset_root: PathBuf,
    pub grid: Grid,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    SpriteSourceRect,
    TileBatches,
    GraphicalCanvas,
    CanvasClipOpacity,
    CanvasGlyphEffects,
    CanvasCompositing,
    WindowActivation,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Grid {
    pub fn validate(self) -> Result<(), String> {
        if self.columns == 0 || self.rows == 0 || self.cell_width == 0 || self.cell_height == 0 {
            return Err("grid and cell dimensions must be positive integers".into());
        }
        if self.columns > 512
            || self.rows > 256
            || self.cell_width > 256
            || self.cell_height > 256
            || self.columns * self.cell_width > 16384
            || self.rows * self.cell_height > 16384
        {
            return Err("grid exceeds S1 presentation limits".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub frame: u64,
    pub text: Vec<String>,
    pub sprites: Vec<Sprite>,
}

impl Frame {
    pub fn validate(&self, grid: Grid) -> Result<(), String> {
        if self.text.len() > grid.rows as usize
            || self
                .text
                .iter()
                .any(|row| row.chars().count() > grid.columns as usize)
        {
            return Err("text exceeds the configured grid".into());
        }
        if self
            .text
            .iter()
            .any(|row| row.chars().any(char::is_control))
        {
            return Err(
                "text rows must not contain control characters (including ANSI or tabs)".into(),
            );
        }
        validate_sprites(&self.sprites)
    }
}

pub const MAX_TEXT_LAYERS: usize = 64;
pub const MAX_TEXT_RUNS: usize = 32768;
pub const MAX_TEXT_SCALARS: usize = 524288;
pub const MAX_LAYER_ID_BYTES: usize = 256;
pub const MAX_TILE_BATCHES: usize = 64;
pub const MAX_BATCH_SOURCES: usize = 256;
pub const MAX_TILE_SOURCES: usize = 4096;
pub const MAX_TILE_CELLS: usize = 32768;
pub const MAX_TILE_ASSET_BYTES: usize = 4096;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameV2 {
    pub frame: u64,
    pub text_layers: Vec<TextLayer>,
    pub sprites: Vec<Sprite>,
    // Preserve presence for capability gating; omission and [] both clear.
    #[serde(default, deserialize_with = "tile_batches")]
    pub tile_batches: Option<Vec<TileBatch>>,
    #[serde(default, deserialize_with = "crate::canvas_protocol::optional_canvas")]
    pub canvas: Option<crate::canvas_protocol::Canvas>,
}

fn tile_batches<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<TileBatch>>, D::Error> {
    Vec::deserialize(d).map(Some)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TileBatch {
    pub id: String,
    pub asset: PathBuf,
    pub layer: i32,
    pub sources: Vec<SourceRect>,
    pub cells: Vec<TileCell>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TileCell {
    pub column: u32,
    pub row: u32,
    pub source: u32,
}

fn validate_tiles(batches: &[TileBatch], grid: Grid) -> Result<(), String> {
    if batches.len() > MAX_TILE_BATCHES {
        return Err("frame exceeds 64 tile batches".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut source_count = 0;
    let mut cell_count = 0;
    for batch in batches {
        if batch.id.is_empty() || batch.id.len() > MAX_LAYER_ID_BYTES || !ids.insert(&batch.id) {
            return Err(
                "tile batch ids must be nonempty, unique, and at most 256 UTF-8 bytes".into(),
            );
        }
        if batch.asset.as_os_str().is_empty()
            || batch.asset.is_absolute()
            || batch.asset.as_os_str().len() > MAX_TILE_ASSET_BYTES
        {
            return Err(
                "tile asset must be a nonempty relative path of at most 4096 UTF-8 bytes".into(),
            );
        }
        source_count += batch.sources.len();
        cell_count += batch.cells.len();
        if batch.sources.is_empty() || batch.sources.len() > MAX_BATCH_SOURCES {
            return Err("tile batch requires 1..256 sources".into());
        }
        if source_count > MAX_TILE_SOURCES || cell_count > MAX_TILE_CELLS {
            return Err("frame exceeds 4096 tile sources or 32768 tile cells".into());
        }
        for rect in &batch.sources {
            rect.validate()?;
        }
        let mut destinations = std::collections::HashSet::new();
        for cell in &batch.cells {
            if cell.column >= grid.columns || cell.row >= grid.rows {
                return Err("tile cell exceeds the configured grid".into());
            }
            if cell.source as usize >= batch.sources.len() {
                return Err("tile source index exceeds the batch catalog".into());
            }
            if !destinations.insert((cell.column, cell.row)) {
                return Err("tile destinations must be unique within a batch".into());
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextLayer {
    pub id: String,
    pub layer: i32,
    pub runs: Vec<TextRun>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextRun {
    pub row: u32,
    pub column: u32,
    pub text: String,
    // Explicit null is allowed; omission is malformed, not an implicit default.
    #[serde(deserialize_with = "required_color")]
    pub foreground: Option<crate::color::ColorSpec>,
    #[serde(deserialize_with = "required_color")]
    pub background: Option<crate::color::ColorSpec>,
}

fn required_color<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<crate::color::ColorSpec>, D::Error> {
    Option::deserialize(d)
}

impl FrameV2 {
    pub fn validate(&self, grid: Grid) -> Result<(), String> {
        if let Some(canvas) = &self.canvas {
            if !self.text_layers.is_empty()
                || !self.sprites.is_empty()
                || self.tile_batches.as_ref().is_some_and(|b| !b.is_empty())
            {
                return Err("canvas cannot mix with nonempty legacy text, sprites or tiles".into());
            }
            canvas.validate()?;
        }
        if self.text_layers.len() > MAX_TEXT_LAYERS {
            return Err("frame exceeds 64 text layers".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut runs = 0;
        let mut scalars = 0;
        for layer in &self.text_layers {
            if layer.id.is_empty() || layer.id.len() > MAX_LAYER_ID_BYTES || !ids.insert(&layer.id)
            {
                return Err(
                    "text layer ids must be nonempty, unique, and at most 256 UTF-8 bytes".into(),
                );
            }
            runs += layer.runs.len();
            if runs > MAX_TEXT_RUNS {
                return Err("frame exceeds 32768 text runs".into());
            }
            for run in &layer.runs {
                let count = run.text.chars().count();
                scalars += count;
                if scalars > MAX_TEXT_SCALARS {
                    return Err("frame exceeds 524288 text scalars".into());
                }
                if run.row >= grid.rows
                    || run.column >= grid.columns
                    || count > (grid.columns - run.column) as usize
                {
                    return Err("text run exceeds the configured grid".into());
                }
                if run.text.chars().any(char::is_control) {
                    return Err("text runs must not contain control characters (including ANSI, tabs, or newlines)".into());
                }
            }
        }
        validate_sprites(&self.sprites)?;
        validate_tiles(self.tile_batches.as_deref().unwrap_or_default(), grid)
    }
}

fn validate_sprites(sprites: &[Sprite]) -> Result<(), String> {
    if sprites.len() > 1024 {
        return Err("frame exceeds the limit of 1024 sprites".into());
    }
    let mut ids = std::collections::HashSet::new();
    for sprite in sprites {
        if sprite.id.is_empty() || !ids.insert(&sprite.id) {
            return Err("sprite ids must be nonempty and unique within a frame".into());
        }
        if sprite.width == 0 || sprite.height == 0 || sprite.width > 4096 || sprite.height > 4096 {
            return Err("sprite dimensions must be between 1 and 4096 logical pixels".into());
        }
        if let Some(rect) = sprite.source_rect {
            rect.validate()?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Sprite {
    pub id: String,
    pub asset: PathBuf,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub anchor: Anchor,
    pub layer: i32,
    #[serde(default, rename = "sourceRect", deserialize_with = "source_rect")]
    pub source_rect: Option<SourceRect>,
}

pub(crate) fn source_rect<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<SourceRect>, D::Error> {
    // Omission is the legacy whole image; an explicit null is not a rectangle.
    SourceRect::deserialize(d).map(Some)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct SourceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SourceRect {
    pub fn validate(self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("sourceRect width and height must be positive integers".into());
        }
        if self.x.checked_add(self.width).is_none() || self.y.checked_add(self.height).is_none() {
            return Err("sourceRect extent overflows image coordinates".into());
        }
        Ok(())
    }

    pub fn validate_image(self, width: u32, height: u32) -> Result<(), String> {
        self.validate()?;
        if self.x + self.width > width || self.y + self.height > height {
            return Err("sourceRect exceeds the decoded PNG bounds".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    BottomCenter,
}

pub fn parse(line: &[u8]) -> Result<Incoming, String> {
    if line.len() > MAX_LINE_BYTES {
        return Err("protocol line exceeds 4 MiB".into());
    }
    fn decode<F: serde::de::DeserializeOwned>(line: &[u8]) -> Result<WireMessage<F>, String> {
        let envelope: Envelope<F> =
            serde_json::from_slice(line).map_err(|e| format!("invalid protocol message: {e}"))?;
        debug_assert!(matches!(envelope.protocol, 1 | 2));
        Ok(envelope.message)
    }
    let probe: VersionProbe =
        serde_json::from_slice(line).map_err(|e| format!("invalid protocol message: {e}"))?;
    let (version, message) = match probe.protocol {
        1 => (
            Version::V1,
            match decode::<Frame>(line)? {
                WireMessage::Hello(h) => Message::Hello(h),
                WireMessage::Frame(f) => Message::Frame(f),
                WireMessage::Shutdown(_) => Message::Shutdown,
            },
        ),
        2 => (
            Version::V2,
            match decode::<FrameV2>(line)? {
                WireMessage::Hello(h) => Message::Hello(h),
                WireMessage::Frame(f) => Message::FrameV2(f),
                WireMessage::Shutdown(_) => Message::Shutdown,
            },
        ),
        other => {
            return Err(format!(
                "unsupported protocol version {other}; expected 1 or 2"
            ));
        }
    };
    if let Message::Hello(hello) = &message {
        hello.grid.validate()?;
        if !hello.asset_root.is_absolute() {
            return Err("assetRoot must be an absolute directory path".into());
        }
        if hello.title.is_empty() || hello.title.chars().any(char::is_control) {
            return Err("title must be nonempty and contain no control characters".into());
        }
        let unique: std::collections::HashSet<_> = hello.required_capabilities.iter().collect();
        if unique.len() != hello.required_capabilities.len() {
            return Err("requiredCapabilities must not contain duplicates".into());
        }
        if version == Version::V1 && unique.contains(&Capability::TileBatches) {
            return Err("tile_batches requires protocol v2".into());
        }
        if version == Version::V1 && unique.contains(&Capability::GraphicalCanvas) {
            return Err("graphical_canvas requires protocol v2".into());
        }
        if version == Version::V1 && unique.contains(&Capability::WindowActivation) {
            return Err("window_activation requires protocol v2".into());
        }
        if unique.contains(&Capability::CanvasClipOpacity) {
            if version != Version::V2 {
                return Err("canvas_clip_opacity requires protocol v2".into());
            }
            if !unique.contains(&Capability::GraphicalCanvas) {
                return Err("canvas_clip_opacity requires graphical_canvas".into());
            }
        }
        if unique.contains(&Capability::CanvasGlyphEffects)
            && (version != Version::V2 || !unique.contains(&Capability::GraphicalCanvas))
        {
            return Err("canvas_glyph_effects requires protocol v2 and graphical_canvas".into());
        }
        if unique.contains(&Capability::CanvasCompositing)
            && (version != Version::V2 || !unique.contains(&Capability::GraphicalCanvas))
        {
            return Err("canvas_compositing requires protocol v2 and graphical_canvas".into());
        }
    }
    Ok(Incoming { version, message })
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    WindowActivation {
        active: bool,
    },
    Ready {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<Capability>,
    },
    Key {
        key: String,
    },
    CloseRequested,
    Error {
        message: String,
    },
}

#[derive(Serialize)]
struct Outgoing<'a> {
    protocol: u32,
    #[serde(flatten)]
    event: &'a Event,
}

pub fn write_event(mut writer: impl Write, version: Version, event: &Event) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&Outgoing {
        protocol: version.number(),
        event,
    })?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// Best-effort fatal diagnostics without ever writing a pipe on the UI thread.
/// Routine protocol errors are logged by the existing stdout worker instead.
pub fn diagnostic(message: String) {
    std::thread::spawn(move || eprintln!("{message}"));
}

struct QueuedEvent {
    version: Version,
    event: Event,
    trace: Option<crate::diagnostics::KeyTrace>,
}

fn write_queued(
    mut writer: impl Write,
    queued: &QueuedEvent,
    pending_after_dequeue: usize,
) -> std::io::Result<()> {
    if let Some(trace) = &queued.trace {
        trace.write_started(pending_after_dequeue);
    }
    write_event(&mut writer, queued.version, &queued.event)?;
    // This stage is recorded only after the complete line AND flush succeed.
    if let Some(trace) = &queued.trace {
        trace.written();
    }
    Ok(())
}

#[derive(Clone)]
pub struct ProtocolWriter(async_channel::Sender<QueuedEvent>);

pub struct WriterCompletion {
    pub done: async_channel::Receiver<Result<(), String>>,
    pub failure: async_channel::Receiver<String>,
}

impl ProtocolWriter {
    pub fn close(&self) {
        self.0.close();
    }
    // The UI only enqueues. A slow/unread pipe can never block GPUI.
    pub fn send(&self, version: Version, event: Event) -> Result<(), String> {
        self.send_traced(version, event, None)
    }

    pub fn send_traced(
        &self,
        version: Version,
        event: Event,
        trace: Option<crate::diagnostics::KeyTrace>,
    ) -> Result<(), String> {
        let queued_trace = trace
            .as_ref()
            .map(|trace| (trace.clone(), trace.now(), self.0.len()));
        self.0
            .try_send(QueuedEvent {
                version,
                event,
                trace,
            })
            .map_err(|e| format!("protocol output unavailable: {e}"))?;
        if let Some((trace, at_ns, pending_before)) = queued_trace {
            trace.queued(at_ns, pending_before);
        }
        Ok(())
    }

    pub fn start() -> (Self, WriterCompletion) {
        let (tx, rx) = async_channel::bounded(256);
        let (done_tx, done_rx) = async_channel::bounded(1);
        let (failure_tx, failure_rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut stdout = stdout.lock();
            let result: std::io::Result<()> = (|| {
                while let Ok(queued) = rx.recv_blocking() {
                    write_queued(&mut stdout, &queued, rx.len())?;
                    if let Event::Error { message } = queued.event {
                        eprintln!("{message}");
                    }
                }
                Ok(())
            })();
            if let Err(error) = &result {
                diagnostic(format!("stdout write failed: {error}"));
                let _ = failure_tx.try_send(error.to_string());
            }
            let _ = done_tx.try_send(result.map_err(|error| error.to_string()));
        });
        (
            Self(tx),
            WriterCompletion {
                done: done_rx,
                failure: failure_rx,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile_frame() -> FrameV2 {
        let Message::FrameV2(frame) = parse(include_bytes!(
            "../fixtures/tile-batches/valid-tiles-text-player-ui.json"
        ))
        .unwrap()
        .message
        else {
            panic!()
        };
        frame
    }

    #[test]
    fn tile_budgets_are_aggregate_and_independent_of_actor_budget() {
        let grid = Grid {
            columns: 512,
            rows: 256,
            cell_width: 10,
            cell_height: 20,
        };
        let mut frame = tile_frame();
        let batch = frame.tile_batches.as_ref().unwrap()[0].clone();
        let sprite = frame.sprites[0].clone();
        frame.sprites = (0..1024)
            .map(|i| Sprite {
                id: format!("actor-{i}"),
                ..sprite.clone()
            })
            .collect();
        frame.tile_batches.as_mut().unwrap()[0].cells = (0..MAX_TILE_CELLS)
            .map(|i| TileCell {
                column: (i % 512) as u32,
                row: (i / 512) as u32,
                source: 0,
            })
            .collect();
        frame.validate(grid).unwrap();
        frame.tile_batches.as_mut().unwrap()[0]
            .cells
            .push(TileCell {
                column: 0,
                row: 64,
                source: 0,
            });
        assert!(frame.validate(grid).unwrap_err().contains("32768"));
        frame.tile_batches = Some(vec![batch.clone()]);
        frame.sprites.push(Sprite {
            id: "extra".into(),
            ..sprite
        });
        assert!(frame.validate(grid).unwrap_err().contains("1024"));
        frame.sprites.clear();
        let mut empty = batch;
        empty.cells.clear();
        frame.tile_batches = Some(
            (0..64)
                .map(|i| TileBatch {
                    id: format!("batch-{i}"),
                    ..empty.clone()
                })
                .collect(),
        );
        frame.validate(grid).unwrap();
        frame.tile_batches.as_mut().unwrap().push(TileBatch {
            id: "extra".into(),
            ..empty.clone()
        });
        assert!(frame.validate(grid).unwrap_err().contains("64 tile"));
        empty.sources = vec![empty.sources[0]; 256];
        frame.tile_batches = Some(
            (0..16)
                .map(|i| TileBatch {
                    id: format!("batch-{i}"),
                    ..empty.clone()
                })
                .collect(),
        );
        frame.validate(grid).unwrap();
        frame.tile_batches.as_mut().unwrap().push(TileBatch {
            id: "extra".into(),
            ..empty.clone()
        });
        assert!(frame.validate(grid).unwrap_err().contains("4096"));
        empty.sources.push(empty.sources[0]);
        frame.tile_batches = Some(vec![empty]);
        assert!(frame.validate(grid).unwrap_err().contains("1..256"));
    }

    #[test]
    fn tile_integer_types_reject_fraction_exponent_boolean_and_missing_fields() {
        let base = include_str!("../fixtures/tile-batches/valid-tiles-text-player-ui.json");
        for bad in [
            "1.0",
            "1e0",
            "1.5",
            "-1",
            "4294967296",
            "true",
            "null",
            "\"1\"",
        ] {
            for field in ["column", "row", "source"] {
                let wire =
                    base.replacen(&format!("\"{field}\":0"), &format!("\"{field}\":{bad}"), 1);
                assert!(parse(wire.as_bytes()).is_err(), "{field} {bad}");
            }
        }
        let mut frame = tile_frame();
        let batch = &mut frame.tile_batches.as_mut().unwrap()[0];
        batch.id = "é".repeat(128);
        batch.asset = PathBuf::from("a".repeat(4096));
        frame
            .validate(Grid {
                columns: 135,
                rows: 36,
                cell_width: 10,
                cell_height: 20,
            })
            .unwrap();
        frame.tile_batches.as_mut().unwrap()[0].asset = PathBuf::from("a".repeat(4097));
        assert!(frame.validate(grid()).is_err());
    }

    #[test]
    fn tile_capability_negotiation_is_v2_only_and_duplicates_are_invalid() {
        let hello = include_str!("../fixtures/tile-batches/hello.json");
        assert!(parse(hello.as_bytes()).is_ok());
        assert!(parse(hello.replace("\"protocol\":2", "\"protocol\":1").as_bytes()).is_err());
        assert!(
            parse(
                hello
                    .replace("sprite_source_rect", "tile_batches")
                    .as_bytes()
            )
            .is_err()
        );
        assert!(parse(hello.replace("tile_batches", "future_tiles").as_bytes()).is_err());
        for caps in [
            vec![Capability::TileBatches],
            vec![Capability::SpriteSourceRect, Capability::TileBatches],
        ] {
            let mut bytes = vec![];
            write_event(
                &mut bytes,
                Version::V2,
                &Event::Ready {
                    capabilities: caps.clone(),
                },
            )
            .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["capabilities"], serde_json::to_value(caps).unwrap());
        }
    }

    #[test]
    fn total_line_byte_limit_applies_to_tile_frames_before_decoding() {
        let mut wire = include_bytes!("../fixtures/tile-batches/valid-full-viewport.json").to_vec();
        wire.resize(MAX_LINE_BYTES, b' ');
        assert!(parse(&wire).is_ok());
        wire.push(b' ');
        assert!(parse(&wire).unwrap_err().contains("4 MiB"));
    }

    #[test]
    fn traced_output_preserves_schema_and_only_marks_success_after_flush() {
        use crate::diagnostics::Diagnostics;
        use std::sync::{Arc, Mutex};
        #[derive(Clone)]
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
        struct ShortWriter {
            bytes: Vec<u8>,
            fail_flush: bool,
            flushed: bool,
        }
        impl Write for ShortWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let count = bytes.len().min(3);
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushed = true;
                if self.fail_flush {
                    Err(std::io::Error::other("test flush failure"))
                } else {
                    Ok(())
                }
            }
        }
        for version in [Version::V1, Version::V2] {
            for fail_flush in [false, true] {
                let records = Arc::new(Mutex::new(Vec::new()));
                let d = Diagnostics::with_writer(Capture(records.clone()));
                let trace = d.native_key("right", true).unwrap();
                let queued = QueuedEvent {
                    version,
                    event: Event::Key {
                        key: "right".into(),
                    },
                    trace: Some(trace),
                };
                let mut writer = ShortWriter {
                    bytes: vec![],
                    fail_flush,
                    flushed: false,
                };
                assert_eq!(write_queued(&mut writer, &queued, 0).is_err(), fail_flush);
                assert!(writer.flushed);
                let mut expected = vec![];
                write_event(&mut expected, version, &queued.event).unwrap();
                assert_eq!(writer.bytes, expected);
                futures_lite::future::block_on(d.finish()).unwrap();
                let records: Vec<serde_json::Value> =
                    String::from_utf8(records.lock().unwrap().clone())
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect();
                assert_eq!(records.iter().any(|r| r["stage"] == "written"), !fail_flush);
                assert!(records.iter().any(|r| r["stage"] == "write_started"));
            }
        }
    }
    const HELLO: &str = r#"{"protocol":1,"type":"hello","title":"Home","assetRoot":"/tmp","grid":{"columns":80,"rows":24,"cellWidth":16,"cellHeight":24}}"#;
    const FRAME: &str = r#"{"protocol":1,"type":"frame","frame":1,"text":["Home"],"sprites":[{"id":"test","asset":"test.png","x":8,"y":4,"width":32,"height":48,"anchor":"bottom_center","layer":100}]}"#;

    #[test]
    fn valid_hello() {
        assert!(matches!(
            parse(HELLO.as_bytes()).unwrap().message,
            Message::Hello(_)
        ));
    }
    #[test]
    fn source_rect_requires_a_complete_strict_integer_object() {
        let good = r#"{"x":0,"y":1,"width":16,"height":24}"#;
        let make = |rect: &str| {
            FRAME.replace(
                "\"layer\":100",
                &format!("\"layer\":100,\"sourceRect\":{rect}"),
            )
        };
        let Message::Frame(frame) = parse(make(good).as_bytes()).unwrap().message else {
            panic!()
        };
        assert_eq!(
            frame.sprites[0].source_rect,
            Some(SourceRect {
                x: 0,
                y: 1,
                width: 16,
                height: 24
            })
        );
        frame.validate(grid()).unwrap();
        for bad in [
            "null",
            "[]",
            "{}",
            r#"{"x":0,"y":0,"width":1,"height":1,"extra":0}"#,
        ] {
            assert!(parse(make(bad).as_bytes()).is_err(), "{bad}");
        }
        for field in ["x", "y", "width", "height"] {
            for value in [
                "-1",
                "1.5",
                "1.0",
                "1e0",
                "4294967296",
                "1e309",
                "NaN",
                "null",
                "true",
                "\"1\"",
            ] {
                let mut rect = serde_json::json!({"x":0,"y":0,"width":1,"height":1});
                rect.as_object_mut().unwrap().remove(field);
                let mut text = serde_json::to_string(&rect).unwrap();
                text.pop();
                text.push_str(&format!(",\"{field}\":{value}}}"));
                assert!(parse(make(&text).as_bytes()).is_err(), "{field}={value}");
            }
        }
        for rect in [
            SourceRect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            SourceRect {
                x: 0,
                y: 0,
                width: 1,
                height: 0,
            },
            SourceRect {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1,
            },
            SourceRect {
                x: 0,
                y: u32::MAX,
                width: 1,
                height: 1,
            },
        ] {
            assert!(rect.validate().is_err());
        }
    }

    #[test]
    fn capability_request_is_strict_and_legacy_ready_has_no_new_fields() {
        for version in [1, 2] {
            let mut hello: serde_json::Value = serde_json::from_str(HELLO).unwrap();
            hello["protocol"] = version.into();
            for caps in [
                serde_json::json!(["sprite_source_rect"]),
                serde_json::json!([]),
            ] {
                hello["requiredCapabilities"] = caps.clone();
                let Message::Hello(parsed) =
                    parse(&serde_json::to_vec(&hello).unwrap()).unwrap().message
                else {
                    panic!()
                };
                let event = Event::Ready {
                    capabilities: parsed.required_capabilities,
                };
                let mut bytes = vec![];
                write_event(
                    &mut bytes,
                    if version == 1 {
                        Version::V1
                    } else {
                        Version::V2
                    },
                    &event,
                )
                .unwrap();
                let expected = if caps.as_array().unwrap().is_empty() {
                    serde_json::json!({"protocol":version,"type":"ready"})
                } else {
                    serde_json::json!({"protocol":version,"type":"ready","capabilities":caps})
                };
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                    expected
                );
            }
            for caps in [
                serde_json::json!(["unknown"]),
                serde_json::json!(["sprite_source_rect", "sprite_source_rect"]),
                serde_json::Value::Null,
                serde_json::json!("sprite_source_rect"),
            ] {
                hello["requiredCapabilities"] = caps;
                assert!(parse(&serde_json::to_vec(&hello).unwrap()).is_err());
            }
        }
    }
    #[test]
    fn unsupported_version() {
        assert!(
            parse(HELLO.replace("\"protocol\":1", "\"protocol\":3").as_bytes())
                .unwrap_err()
                .contains("unsupported")
        );
    }
    #[test]
    fn invalid_grid_dimensions() {
        for field in ["columns", "rows", "cellWidth", "cellHeight"] {
            for bad in ["0", "-1", "1.5", "4294967295"] {
                let mut value: serde_json::Value = serde_json::from_str(HELLO).unwrap();
                value["grid"][field] = serde_json::from_str(bad).unwrap();
                assert!(parse(value.to_string().as_bytes()).is_err());
            }
        }
    }
    #[test]
    fn valid_frame_and_anchor() {
        let Message::Frame(frame) = parse(FRAME.as_bytes()).unwrap().message else {
            panic!()
        };
        assert_eq!(frame.sprites[0].anchor, Anchor::BottomCenter);
        frame
            .validate(Grid {
                columns: 80,
                rows: 24,
                cell_width: 16,
                cell_height: 24,
            })
            .unwrap();
    }
    #[test]
    fn malformed_frame_rejected() {
        for bad in [
            FRAME.replace("bottom_center", "top_left"),
            FRAME.replace("\"width\":32,", ""),
            FRAME.replace("\"x\":8", "\"x\":\"8\""),
            FRAME.replace("\"sprites\":", "\"sprite\":"),
            "{".into(),
        ] {
            assert!(parse(bad.as_bytes()).is_err());
        }
    }
    #[test]
    fn shutdown_parsing() {
        assert!(matches!(
            parse(br#"{"protocol":1,"type":"shutdown"}"#)
                .unwrap()
                .message,
            Message::Shutdown
        ));
    }
    #[test]
    fn events_are_single_json_lines() {
        for event in [
            Event::Ready {
                capabilities: vec![],
            },
            Event::CloseRequested,
            Event::Error {
                message: "bad\ninput".into(),
            },
            Event::Key { key: "W".into() },
        ] {
            let mut bytes = Vec::new();
            write_event(&mut bytes, Version::V1, &event).unwrap();
            assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["protocol"], 1);
            if let Event::Key { key } = event {
                assert_eq!(value["key"], key);
            }
        }
    }
    fn v2() -> serde_json::Value {
        serde_json::json!({"protocol":2,"type":"frame","frame":1,"textLayers":[{"id":"world","layer":0,"runs":[{"row":4,"column":2,"text":"Hello ","foreground":null,"background":null}]}],"sprites":[]})
    }
    fn grid() -> Grid {
        Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        }
    }
    fn frame_v2(value: serde_json::Value) -> Result<FrameV2, String> {
        let Message::FrameV2(frame) = parse(value.to_string().as_bytes())?.message else {
            panic!()
        };
        frame.validate(grid())?;
        Ok(frame)
    }
    #[test]
    fn v2_hello_and_frame_with_explicit_null_styles() {
        assert_eq!(
            parse(HELLO.replace("protocol\":1", "protocol\":2").as_bytes())
                .unwrap()
                .version,
            Version::V2
        );
        let frame = frame_v2(v2()).unwrap();
        assert_eq!(frame.text_layers[0].runs[0].text, "Hello ");
        assert_eq!(frame.text_layers[0].runs[0].foreground, None);
        assert_eq!(frame.text_layers[0].runs[0].background, None);
    }
    #[test]
    fn cross_version_fields_and_unknown_fields_rejected() {
        for (field, value) in [
            ("textLayers", serde_json::json!([])),
            ("unknown", serde_json::json!(0)),
        ] {
            let mut old: serde_json::Value = serde_json::from_str(FRAME).unwrap();
            old[field] = value;
            assert!(parse(old.to_string().as_bytes()).is_err());
        }
        for path in [
            vec!["extra"],
            vec!["text"],
            vec!["textLayers", "0", "extra"],
            vec!["textLayers", "0", "runs", "0", "extra"],
        ] {
            let mut value = v2();
            let mut node = &mut value;
            for part in &path[..path.len() - 1] {
                node = if *part == "0" {
                    &mut node[0]
                } else {
                    &mut node[*part]
                };
            }
            node[*path.last().unwrap()] = serde_json::json!(0);
            assert!(frame_v2(value).is_err());
        }
        for version in [1, 2] {
            assert!(
                parse(
                    format!(r#"{{"protocol":{version},"type":"shutdown","extra":1}}"#).as_bytes()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn all_v2_fields_are_required_and_typed() {
        for field in ["frame", "textLayers", "sprites"] {
            let mut value = v2();
            value.as_object_mut().unwrap().remove(field);
            assert!(frame_v2(value).is_err());
        }
        for field in ["id", "layer", "runs"] {
            let mut value = v2();
            value["textLayers"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(frame_v2(value).is_err());
        }
        for field in ["row", "column", "text", "foreground", "background"] {
            let mut value = v2();
            value["textLayers"][0]["runs"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(frame_v2(value).is_err(), "{field}");
        }
        for bad in [
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("4"),
            serde_json::json!(4294967296u64),
        ] {
            let mut value = v2();
            value["textLayers"][0]["runs"][0]["row"] = bad;
            assert!(frame_v2(value).is_err());
        }
        let mut value = v2();
        value["textLayers"][0]["layer"] = serde_json::json!(2147483648u64);
        assert!(frame_v2(value).is_err());
    }
    #[test]
    fn runs_must_fit_and_reject_controls() {
        for (row, column, text) in [
            (36, 0, "x"),
            (0, 135, "x"),
            (0, 134, "xx"),
            (0, 0, "a\tb"),
            (0, 0, "a\nb"),
            (0, 0, "\u{1b}[31m"),
            (0, 0, "\u{85}"),
        ] {
            let mut value = v2();
            let run = &mut value["textLayers"][0]["runs"][0];
            run["row"] = row.into();
            run["column"] = column.into();
            run["text"] = text.into();
            assert!(frame_v2(value).is_err(), "{row} {column} {text:?}");
        }
        let mut value = v2();
        let run = &mut value["textLayers"][0]["runs"][0];
        run["row"] = 35.into();
        run["column"] = 133.into();
        run["text"] = "é猫".into();
        assert!(frame_v2(value).is_ok()); // two scalars, independent of bytes/advance
    }
    #[test]
    fn layer_ids_are_nonempty_unique_bounded_utf8() {
        for id in [String::new(), "é".repeat(129)] {
            let mut value = v2();
            value["textLayers"][0]["id"] = id.into();
            assert!(frame_v2(value).is_err());
        }
        let mut value = v2();
        value["textLayers"][0]["id"] = "é".repeat(128).into();
        assert!(frame_v2(value).is_ok());
        let mut value = v2();
        let duplicate = value["textLayers"][0].clone();
        value["textLayers"].as_array_mut().unwrap().push(duplicate);
        assert!(frame_v2(value).is_err());
        assert!(
            parse(b"{\"protocol\":2,\"type\":\"frame\",\"textLayers\":[{\"id\":\"\xff\"}]}")
                .is_err()
        );
    }
    #[test]
    fn v2_resource_boundaries_include_full_current_layouts() {
        let base = frame_v2(v2()).unwrap();
        let mut frame = base.clone();
        frame.text_layers = (0..MAX_TEXT_LAYERS)
            .map(|i| TextLayer {
                id: i.to_string(),
                ..base.text_layers[0].clone()
            })
            .collect();
        assert!(frame.validate(grid()).is_ok());
        frame.text_layers.push(TextLayer {
            id: "overflow".into(),
            ..base.text_layers[0].clone()
        });
        assert!(frame.validate(grid()).is_err());
        let mut frame = base.clone();
        let mut run = frame.text_layers[0].runs[0].clone();
        run.text = String::new();
        frame.text_layers[0].runs = vec![run.clone(); MAX_TEXT_RUNS];
        assert!(frame.validate(grid()).is_ok());
        frame.text_layers[0].runs.push(run.clone());
        assert!(frame.validate(grid()).is_err());
        let mut frame = base.clone();
        run.column = 0;
        run.text = "x".repeat(512);
        frame.text_layers[0].runs = vec![run.clone(); MAX_TEXT_SCALARS / 512];
        let big = Grid {
            columns: 512,
            ..grid()
        };
        assert!(frame.validate(big).is_ok());
        frame.text_layers[0].runs.push(run);
        assert!(frame.validate(big).is_err());
        let mut frame = base;
        frame.text_layers[0].runs = (0..36)
            .map(|row| TextRun {
                row,
                column: 0,
                text: " ".repeat(135),
                foreground: None,
                background: None,
            })
            .collect();
        assert!(frame.validate(grid()).is_ok());
    }
    #[test]
    fn every_event_preserves_both_protocol_versions() {
        for version in [Version::V1, Version::V2] {
            for event in [
                Event::Ready {
                    capabilities: vec![],
                },
                Event::Key { key: "C".into() },
                Event::CloseRequested,
                Event::Error {
                    message: "bad".into(),
                },
            ] {
                let mut bytes = vec![];
                write_event(&mut bytes, version, &event).unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(value["protocol"], version.number());
            }
        }
    }
}
