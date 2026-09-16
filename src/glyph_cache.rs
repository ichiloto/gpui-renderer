//! Transactional glyph generation before a candidate becomes visible.
use crate::{
    display_cache::{DisplayRasterCache, MAX_BYTES, MAX_IMAGES, RasterKey},
    glyph_raster::{self, FontCatalog, RasterPlan},
    state::PreparedFrame,
};
use gpui::RenderImage;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Default)]
pub struct GlyphFrame {
    pub images: HashMap<usize, Arc<RenderImage>>,
    pub keys: HashSet<RasterKey>,
    pub builds: usize,
    pub reserved_bytes: usize,
}

pub fn prepare(
    frame: &PreparedFrame,
    density: f32,
    catalog: &mut FontCatalog,
    cache: &mut DisplayRasterCache,
) -> Result<GlyphFrame, String> {
    let mut result = GlyphFrame::default();
    let Some(canvas) = &frame.canvas else {
        return Ok(result);
    };
    let mut wanted = HashMap::new();
    let mut layers = Vec::new();
    let mut scratch = 0;
    let mut work = 0usize;
    for (index, layer) in canvas.source.text_layers.iter().enumerate() {
        if layer.glyph_effects.is_none() {
            continue;
        }
        let plan = RasterPlan::new(layer, density)?;
        // Catalog is immutable and session-owned. Deliberately exclude world
        // position, layer id/order, clip, opacity and frame/animation clocks.
        let key = RasterKey::Glyph(
            format!(
                "{:?}|{:?}|{:?}|{:08x}",
                layer.grid,
                layer.runs,
                layer.glyph_effects,
                density.to_bits()
            )
            .into(),
        );
        if wanted
            .insert(key.clone(), plan.bytes() + key.extra_bytes())
            .is_none()
            && cache.get(&key).is_none()
        {
            scratch = scratch.max(plan.scratch_bytes);
            work = work
                .checked_add(plan.work)
                .ok_or("glyph work accounting overflow")?;
        }
        layers.push((index, key, plan));
    }
    if layers.is_empty() {
        return Ok(result);
    }
    RasterPlan::validate_work(work)?;
    result.reserved_bytes = cache.reserve_glyphs(&wanted, scratch)?;
    let mut staged = HashMap::new();
    let mut fonts = None;
    for (index, key, plan) in layers {
        let image = if let Some(image) = staged.get(&key).cloned().or_else(|| cache.get(&key)) {
            image
        } else {
            if fonts.is_none() {
                fonts = Some(catalog.system()?);
            }
            let image = glyph_raster::rasterize(
                &canvas.source.text_layers[index],
                plan,
                fonts.as_mut().unwrap(),
            )?;
            staged.insert(key.clone(), image.clone());
            result.builds += 1;
            image
        };
        result.keys.insert(key);
        result.images.insert(index, image);
    }
    // No partially prepared candidate is committed to the LRU or displayed.
    for (key, image) in staged {
        cache.insert_with_limits(key, image, MAX_BYTES, MAX_IMAGES);
    }
    Ok(result)
}
