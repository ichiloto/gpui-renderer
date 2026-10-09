//! Canvas-only authoring views use runtime validation, pixels, painting and retirement.
use crate::{
    assets::AssetRoot, canvas::PreparedCanvas, canvas_protocol::Canvas,
    display_cache::DisplayRasterCache, glyph_raster::FontCatalog, viewport::ViewportTransform,
};
use gpui::{App, Div, RenderImage, Window, div, prelude::*, px};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    sync::Arc,
};

pub use crate::state::FrameResources as CanvasResources;

/// Clones share the runtime's bounded decoded-image, region and composite caches.
#[derive(Clone)]
pub struct CanvasAssets(AssetRoot);

impl CanvasAssets {
    pub fn new(root: &Path) -> Result<Self, String> {
        AssetRoot::new(root).map(Self)
    }

    /// Prepare off the UI thread. Failure never replaces a caller's accepted frame.
    pub fn prepare_canvas(&self, source: Canvas) -> Result<CanvasFrame, String> {
        let (canvas, cached_images, resources) = crate::state::prepare_canvas(source, &self.0)?;
        Ok(CanvasFrame {
            canvas: Arc::new(canvas),
            cached_images,
            resources,
        })
    }
}

/// An immutable, completely decoded canvas generation, safe to retain while seeking.
#[derive(Clone, Debug)]
pub struct CanvasFrame {
    pub(crate) canvas: Arc<PreparedCanvas>,
    cached_images: Vec<Arc<RenderImage>>,
    resources: CanvasResources,
}

impl CanvasFrame {
    pub fn get_size(&self) -> (u32, u32) {
        (self.canvas.source.width, self.canvas.source.height)
    }

    pub fn get_resources(&self) -> &CanvasResources {
        &self.resources
    }
}

/// One native view's device rasters and GPU retirement; no animation clock or game state.
#[derive(Default)]
pub struct CanvasPainter {
    fonts: HashMap<(u32, u32), f32>,
    glyph_fonts: FontCatalog,
    samples: Rc<RefCell<DisplayRasterCache>>,
    cached_images: Vec<Arc<RenderImage>>,
}

impl CanvasPainter {
    /// Paint into a fitted, centered surface using the same runtime canvas element.
    pub fn paint(
        &mut self,
        frame: &CanvasFrame,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &App,
    ) -> Result<Div, String> {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return Err("canvas view size must be finite and non-negative".into());
        }
        let (logical_width, logical_height) = frame.get_size();
        let transform =
            ViewportTransform::fit(logical_width as f32, logical_height as f32, width, height);
        let glyphs = crate::glyph_cache::prepare_canvas(
            &frame.canvas,
            transform.scale.max(1.0 / 256.0) * window.scale_factor(),
            &mut self.glyph_fonts,
            &mut self.samples.borrow_mut(),
        )?;
        let active = frame
            .canvas
            .images
            .iter()
            .chain(&frame.canvas.composites)
            .chain(glyphs.images.values())
            .map(|image| image.id)
            .collect();
        let mut retired = self.samples.borrow_mut().begin_frame(&active, &glyphs.keys);
        retired.extend(self.replace_cached_images(&frame.cached_images));
        Self::retire_images(retired, window);
        let surface = crate::renderer::positioned(div(), transform.surface_bounds())
            .overflow_hidden()
            .child(frame.canvas.element(
                transform,
                &mut self.fonts,
                &glyphs.images,
                self.samples.clone(),
                cx,
            ));
        Ok(div()
            .relative()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .child(surface))
    }

    /// Call while the owning window still exists when a preview closes or changes project.
    pub fn clear(&mut self, window: &mut Window) {
        let mut retired = self
            .samples
            .borrow_mut()
            .begin_frame(&HashSet::new(), &HashSet::new());
        retired.extend(self.replace_cached_images(&[]));
        self.fonts.clear();
        Self::retire_images(retired, window);
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
                crate::protocol::diagnostic(format!("cannot retire canvas view image: {error}"));
            }
        }
    }
}
