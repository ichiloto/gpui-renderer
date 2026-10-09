//! Complete field previews share runtime validation, retained resources and painting.
use crate::{
    assets::AssetRoot,
    display_cache::DisplayRasterCache,
    field_motion::FieldMotion,
    glyph_raster::FontCatalog,
    protocol::{Event, Hello, Version, write_event},
    renderer::{FramePaint, paint_frame},
    retained_prepared::PreparedScene,
    retained_protocol::{FrameUpdate, Viewport},
    retained_state::RetainedSession,
    viewport::ViewportTransform,
};
use gpui::{App, Div, RenderImage, Window, div, font, prelude::*, px, rgb};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    path::Path,
    rc::Rc,
    sync::Arc,
};

pub use crate::protocol::Capability as SceneCapability;
pub use crate::protocol::Grid as SceneGrid;
pub use crate::state::FrameResources as SceneResources;

#[cfg(test)]
#[path = "scene_view_tests.rs"]
mod tests;

/// One project's retained authoring session. Prepare off the UI thread.
pub struct SceneSession {
    assets: AssetRoot,
    hello: Hello,
    owner: Arc<()>,
    retained: RetainedSession,
    prepared: Option<Arc<PreparedScene>>,
}

impl SceneSession {
    pub fn new(root: &Path, grid: SceneGrid) -> Result<Self, String> {
        grid.validate()?;
        Ok(Self {
            assets: AssetRoot::new(root)?,
            hello: Hello {
                title: "Scene preview".into(),
                asset_root: root.to_path_buf(),
                grid,
                required_capabilities: Vec::new(),
                icon: None,
            },
            owner: Arc::new(()),
            retained: RetainedSession::default(),
            prepared: None,
        })
    }

    /// The production v2 drawing features, without window/input event subscriptions.
    pub fn get_capabilities(&self) -> Vec<SceneCapability> {
        self.hello.get_enabled_capabilities(Version::V2)
    }

    /// Canonical protocol-2 NDJSON, including its terminating newline.
    pub fn encode_ready(&self) -> io::Result<String> {
        Self::encode_event_line(Event::Ready {
            capabilities: self.get_capabilities(),
        })
    }

    /// Carry the accepted update's identifiers unchanged; staged chunks use presented=false.
    /// Queue the line until the Engine's presentFrame call has returned.
    pub fn encode_acknowledgement(
        generation: u64,
        frame: u64,
        presented: bool,
    ) -> io::Result<String> {
        Self::encode_event_line(Event::FrameAck {
            generation,
            frame,
            presented,
        })
    }

    /// Refuse the candidate at the actual accepted cursor, retaining the original diagnostic.
    pub fn encode_rejection(
        &self,
        generation: u64,
        message: impl Into<String>,
    ) -> io::Result<String> {
        Self::encode_event_line(Event::FrameRejected {
            generation,
            expected_generation: self.get_generation(),
            message: message.into(),
            resync_required: true,
        })
    }

    fn encode_event_line(event: Event) -> io::Result<String> {
        let mut line = Vec::new();
        write_event(&mut line, Version::V2, &event)?;
        String::from_utf8(line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Accept the Engine's retained update body, including staged world chunks.
    /// A rejected candidate leaves the previous visible frame intact and requires a reset.
    pub fn apply_update(&mut self, source: Value) -> Result<Option<SceneFrame>, String> {
        match self.prepare_update(source) {
            Ok(frame) => Ok(frame),
            Err(error) => {
                self.retained.require_reset();
                Err(error)
            }
        }
    }

    fn prepare_update(&mut self, source: Value) -> Result<Option<SceneFrame>, String> {
        let update: FrameUpdate =
            serde_json::from_value(source).map_err(|error| error.to_string())?;
        let generation = update.generation;
        let mut candidate = self.retained.clone();
        let accepted = candidate.apply(update, self.hello.grid)?;
        if !accepted.visible {
            self.retained = candidate;
            return Ok(None);
        }
        let prepared = match &self.prepared {
            Some(previous) if Arc::ptr_eq(&previous.source, &accepted.scene) => previous.clone(),
            previous => Arc::new(PreparedScene::prepare(
                accepted.scene,
                accepted.frame,
                self.hello.grid,
                &self.assets,
                previous.as_deref(),
            )?),
        };
        self.retained = candidate;
        self.prepared = Some(prepared.clone());
        Ok(Some(SceneFrame {
            owner: self.owner.clone(),
            generation,
            reset: accepted.reset,
            grid: self.hello.grid,
            prepared,
            viewport: accepted.viewport,
        }))
    }

    pub fn get_generation(&self) -> u64 {
        self.retained.expected_generation()
    }
}

/// Immutable field, actors, camera and screen overlay from one accepted generation.
#[derive(Clone, Debug)]
pub struct SceneFrame {
    owner: Arc<()>,
    generation: u64,
    reset: bool,
    grid: SceneGrid,
    pub(crate) prepared: Arc<PreparedScene>,
    pub(crate) viewport: Option<Viewport>,
}

impl SceneFrame {
    pub fn get_generation(&self) -> u64 {
        self.generation
    }

    pub fn get_size(&self) -> (f32, f32) {
        self.prepared.screen.get_logical_size(self.grid)
    }

    pub fn get_resources(&self) -> &SceneResources {
        &self.prepared.screen.resources
    }
}

/// One view's GPU resources. The host owns the playhead; painting never advances gameplay.
#[derive(Default)]
pub struct ScenePainter {
    fonts: HashMap<(u32, u32), f32>,
    glyph_fonts: FontCatalog,
    samples: Rc<RefCell<DisplayRasterCache>>,
    cached_images: Vec<Arc<RenderImage>>,
    motion: FieldMotion,
    previous: Option<(Arc<()>, u64)>,
    time: f64,
}

impl ScenePainter {
    /// Time is the host's preview playhead, not a second wall clock. Backward seeks reset slides.
    pub fn paint(
        &mut self,
        frame: &SceneFrame,
        time: f64,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &App,
    ) -> Result<Div, String> {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return Err("scene view size must be finite and non-negative".into());
        }
        if !time.is_finite() || time < 0.0 {
            return Err("scene view time must be finite and non-negative".into());
        }
        let (logical_width, logical_height) = frame.get_size();
        let transform = ViewportTransform::fit(logical_width, logical_height, width, height);
        let glyphs = crate::glyph_cache::prepare(
            &frame.prepared.screen,
            transform.scale.max(1.0 / 256.0) * window.scale_factor(),
            &mut self.glyph_fonts,
            &mut self.samples.borrow_mut(),
        )?;
        let mut active: HashSet<_> = frame.prepared.images.iter().map(|image| image.id).collect();
        active.extend(glyphs.images.values().map(|image| image.id));
        let mut retired = self.samples.borrow_mut().begin_frame(&active, &glyphs.keys);
        retired.extend(self.replace_cached_images(&frame.prepared.images));
        Self::retire_images(retired, window);
        self.observe_frame(frame, time);
        let text = cx.text_system();
        let font_id = text.resolve_font(&font(crate::renderer::FONT_FAMILY));
        let advance = text.ch_advance(font_id, px(1.0)).ok().map(f32::from);
        let line =
            f32::from(text.ascent(font_id, px(1.0))) + f32::from(text.descent(font_id, px(1.0)));
        let background = frame
            .prepared
            .source
            .canvas
            .as_ref()
            .and_then(|canvas| canvas.background.as_ref())
            .map_or(
                crate::color::DEFAULT_BACKGROUND,
                crate::color::ColorSpec::rgb,
            );
        let mut view = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .bg(rgb(background));
        if transform.scale > 0.0 {
            view = view.child(paint_frame(
                FramePaint {
                    frame: Some(&frame.prepared.screen),
                    scene: Some(&frame.prepared),
                    viewport: frame.viewport.as_ref(),
                    grid: frame.grid,
                    transform,
                    metrics: (advance, line),
                    motion: &self.motion,
                    time,
                    fonts: &mut self.fonts,
                    glyphs: &glyphs.images,
                    samples: &self.samples,
                },
                cx,
            ));
        }
        Ok(view)
    }

    fn observe_frame(&mut self, frame: &SceneFrame, time: f64) {
        let same_session = self
            .previous
            .as_ref()
            .is_some_and(|(owner, _)| Arc::ptr_eq(owner, &frame.owner));
        let new_frame = !same_session
            || self
                .previous
                .as_ref()
                .is_none_or(|(_, generation)| *generation != frame.generation);
        if !same_session || time < self.time || (new_frame && frame.reset) {
            self.motion = FieldMotion::default();
        }
        if new_frame || time < self.time {
            self.motion
                .observe(&frame.prepared.source, frame.viewport.as_ref(), time);
        }
        self.previous = Some((frame.owner.clone(), frame.generation));
        self.time = time;
    }

    /// Call with the owning window before closing the preview or changing project.
    pub fn clear(&mut self, window: &mut Window) {
        let mut retired = self
            .samples
            .borrow_mut()
            .begin_frame(&HashSet::new(), &HashSet::new());
        retired.extend(self.replace_cached_images(&[]));
        Self::retire_images(retired, window);
        self.fonts.clear();
        self.motion = FieldMotion::default();
        self.previous = None;
        self.time = 0.0;
    }

    pub(crate) fn replace_cached_images(
        &mut self,
        images: &[Arc<RenderImage>],
    ) -> Vec<Arc<RenderImage>> {
        let retained: HashSet<_> = images.iter().map(|image| image.id).collect();
        let retired = self
            .cached_images
            .iter()
            .filter(|image| !retained.contains(&image.id))
            .cloned()
            .collect();
        self.cached_images = images.to_vec();
        retired
    }

    fn retire_images(images: Vec<Arc<RenderImage>>, window: &mut Window) {
        let mut seen = HashSet::new();
        for image in images {
            if seen.insert(image.id)
                && let Err(error) = window.drop_image(image)
            {
                crate::protocol::diagnostic(format!("cannot retire scene view image: {error}"));
            }
        }
    }
}
