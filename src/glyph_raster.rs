//! Runtime font contours. Foreground, outline and shadow share resolved glyphs.
use crate::{canvas_protocol::CanvasText, color::DEFAULT_FOREGROUND, glyph_pixels};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Wrap};
use gpui::RenderImage;
use std::{collections::HashMap, sync::Arc};
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Join, Stroke, Vector};

pub const GUARD: u32 = 1;
// A finite work bound for the opt-in raster path, independent of cache hits.
const MAX_SAMPLE_WORK: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct RasterPlan {
    pub density: f32,
    pub width: u32,
    pub height: u32,
    pub cell_width: usize,
    pub cell_height: usize,
    pub scratch_bytes: usize,
    pub work: usize,
}

impl RasterPlan {
    pub fn new(layer: &CanvasText, density: f32) -> Result<Self, String> {
        let effects = layer
            .glyph_effects
            .as_ref()
            .ok_or("missing glyph effects")?;
        if !density.is_finite() || density <= 0.0 {
            return Err("invalid glyph device density".into());
        }
        let bounds = layer.paint_bounds();
        let dimension = |logical: f64| -> Result<u32, String> {
            let value = (logical * f64::from(density)).ceil() + f64::from(2 * GUARD);
            if !value.is_finite() || !(1.0..=16384.0).contains(&value) {
                return Err("glyph raster dimension exceeds 16384 device pixels".into());
            }
            Ok(value as u32)
        };
        let width = dimension(bounds.width)?;
        let height = dimension(bounds.height)?;
        let [l, t, r, b] = effects.padding();
        // An extra pixel on each side allows fractional cell phase and AA.
        let cell_width = dimension(f64::from(layer.grid.cell_width) + l + r)? as usize + 2;
        let cell_height = dimension(f64::from(layer.grid.cell_height) + t + b)? as usize + 2;
        let pixels = cell_width
            .checked_mul(cell_height)
            .ok_or("glyph cell size overflow")?;
        let radius = (3.0 * effects.shadow.sigma * f64::from(density)).ceil() as usize;
        let scalars: usize = layer.runs.iter().map(|run| run.text.chars().count()).sum();
        let taps = if effects.shadow.sigma > 0.0 && effects.shadow.opacity > 0.0 {
            2 * (2 * radius + 1)
        } else {
            1
        };
        let work = pixels
            .checked_mul(scalars)
            .and_then(|n| n.checked_mul(taps))
            .ok_or("glyph work overflow")?;
        if work > MAX_SAMPLE_WORK {
            return Err("glyph effects exceed bounded raster work".into());
        }
        // Two u8 masks, three f32 blur planes, and one live Swash u8 mask.
        // The sixteenth byte/pixel covers mask rounding; kernel/shape keys extra.
        let scratch_bytes = pixels
            .checked_mul(16)
            .and_then(|n| n.checked_add((2 * radius + 1) * 4 + scalars * 2048))
            .ok_or("glyph scratch size overflow")?;
        Ok(Self {
            density,
            width,
            height,
            cell_width,
            cell_height,
            scratch_bytes,
            work,
        })
    }

    pub fn bytes(self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    pub fn validate_work(total: usize) -> Result<(), String> {
        if total > MAX_SAMPLE_WORK {
            Err("glyph frame exceeds bounded raster work".into())
        } else {
            Ok(())
        }
    }
}

/// Keep only the discovered catalog between raster generations. FontSystem's
/// loaded faces, shaping buffers and Swash scratch are released after each miss
/// batch, so changing strings cannot grow an unbounded second glyph cache.
#[derive(Default)]
pub struct FontCatalog {
    source: Option<(String, cosmic_text::fontdb::Database)>,
}

impl FontCatalog {
    pub fn system(&mut self) -> Result<FontSystem, String> {
        if self.source.is_none() {
            let mut fonts = FontSystem::new();
            let mut families: Vec<_> = fonts
                .db()
                .faces()
                .filter(|face| face.monospaced)
                .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
                .collect();
            families.sort();
            families.dedup();
            let family = families
                .iter()
                .find(|name| name.as_str() == crate::renderer::FONT_FAMILY)
                .or_else(|| families.first())
                .ok_or("no system monospace font available for glyph effects")?
                .clone();
            fonts.db_mut().set_monospace_family(family);
            self.source = Some(fonts.into_locale_and_db());
        }
        let (locale, db) = self.source.as_ref().unwrap();
        Ok(FontSystem::new_with_locale_and_db(
            locale.clone(),
            db.clone(),
        ))
    }
}

struct Part {
    font: Arc<cosmic_text::Font>,
    glyph: u16,
    x: f32,
    y: f32,
}
struct Character {
    parts: Vec<Part>,
    left: f32,
    right: f32,
}
struct Characters {
    entries: HashMap<char, Character>,
    size: f32,
    ascent: f32,
    descent: f32,
}

fn characters(layer: &CanvasText, fonts: &mut FontSystem) -> Result<Characters, String> {
    let mut entries = HashMap::new();
    let mut ascent = 0.0_f32;
    let mut descent = 0.0_f32;
    let mut advance = 0.0_f32;
    let mut scaler = ScaleContext::new();
    for ch in layer.runs.iter().flat_map(|run| run.text.chars()) {
        if ch == ' ' || entries.contains_key(&ch) {
            continue;
        }
        let mut buffer = Buffer::new(fonts, Metrics::new(100.0, 100.0));
        buffer.set_wrap(fonts, Wrap::None);
        buffer.set_text(
            fonts,
            &ch.to_string(),
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(fonts, true);
        let mut parts = Vec::new();
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for run in buffer.layout_runs() {
            ascent = ascent.max(run.line_y / 100.0);
            descent = descent.max((run.line_height - run.line_y) / 100.0);
            right = right.max(run.line_w / 100.0);
            for glyph in run.glyphs {
                if glyph.glyph_id == 0 {
                    return Err(format!(
                        "system fonts have no glyph for U+{:04X}",
                        ch as u32
                    ));
                }
                let font = fonts
                    .get_font(glyph.font_id)
                    .ok_or("cannot load resolved glyph font")?;
                let x = glyph.x / 100.0 + glyph.x_offset;
                let y = glyph.y / 100.0 - glyph.y_offset;
                let outline = scaler
                    .builder(font.as_swash())
                    .size(100.0)
                    .hint(false)
                    .build()
                    .scale_outline(glyph.glyph_id)
                    .ok_or("resolved glyph has no scalable contour")?;
                let bounds = outline.bounds();
                left = left.min(x + bounds.min.x / 100.0);
                right = right.max(x + bounds.max.x / 100.0);
                ascent = ascent.max(bounds.max.y / 100.0 - y);
                descent = descent.max(y - bounds.min.y / 100.0);
                parts.push(Part {
                    font,
                    glyph: glyph.glyph_id,
                    x,
                    y,
                });
            }
        }
        if parts.is_empty() {
            return Err("visible scalar produced no glyph contour".into());
        }
        advance = advance.max(right - left);
        entries.insert(ch, Character { parts, left, right });
    }
    let size = crate::renderer::fit_cell_font(
        layer.grid.cell_width as f32,
        layer.grid.cell_height as f32,
        Some(advance.max(0.01)),
        (ascent + descent).max(0.01),
    );
    Ok(Characters {
        entries,
        size,
        ascent,
        descent,
    })
}

fn mask_into(
    target: &mut [u8],
    width: usize,
    height: usize,
    image: swash::scale::image::Image,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let p = image.placement;
    if image.data.len() != p.width as usize * p.height as usize || image.data.len() > target.len() {
        return Err("glyph mask exceeds reserved scratch or is not alpha".into());
    }
    let left = x + p.left;
    let top = y - p.top;
    for sy in 0..p.height as usize {
        for sx in 0..p.width as usize {
            let alpha = image.data[sy * p.width as usize + sx];
            if alpha == 0 {
                continue;
            }
            let dx = left + sx as i32;
            let dy = top + sy as i32;
            if dx < 0 || dy < 0 || dx as usize >= width || dy as usize >= height {
                return Err("glyph contour escaped its measured scratch bounds".into());
            }
            let old = &mut target[dy as usize * width + dx as usize];
            *old = (255 - (255 - u32::from(*old)) * (255 - u32::from(alpha)) / 255) as u8;
        }
    }
    Ok(())
}

pub fn rasterize(
    layer: &CanvasText,
    plan: RasterPlan,
    fonts: &mut FontSystem,
) -> Result<Arc<RenderImage>, String> {
    let chars = characters(layer, fonts)?;
    let effects = layer.glyph_effects.as_ref().unwrap();
    let [l, t, _, _] = effects.padding().map(|v| v as f32);
    let d = plan.density;
    let cw = layer.grid.cell_width as f32;
    let ch = layer.grid.cell_height as f32;
    let w = plan.cell_width;
    let h = plan.cell_height;
    let mut fill = vec![0_u8; w * h];
    let mut stroke = vec![0_u8; w * h];
    let mut shadow = vec![0_f32; w * h];
    let mut temporary = vec![0_f32; w * h];
    let mut blurred = vec![0_f32; w * h];
    let mut pixels = vec![0_u8; plan.bytes()];
    let mut context = ScaleContext::new();
    let baseline =
        (ch - chars.size * (chars.ascent + chars.descent)) / 2.0 + chars.size * chars.ascent;
    for run in &layer.runs {
        for (column, scalar) in run.text.chars().enumerate() {
            let cell_x = (run.column as usize + column) as f32 * cw;
            let cell_y = run.row as f32 * ch;
            if let Some(bg) = &run.background {
                let x0 = ((l + cell_x) * d).floor() as usize + GUARD as usize;
                let y0 = ((t + cell_y) * d).floor() as usize + GUARD as usize;
                let x1 = ((l + cell_x + cw) * d).ceil() as usize + GUARD as usize;
                let y1 = ((t + cell_y + ch) * d).ceil() as usize + GUARD as usize;
                for y in y0..y1.min(plan.height as usize) {
                    for x in x0..x1.min(plan.width as usize) {
                        let i = (y * plan.width as usize + x) * 4;
                        glyph_pixels::over(&mut pixels[i..i + 4], bg.rgb(), 1.0);
                    }
                }
            }
            let Some(character) = chars.entries.get(&scalar) else {
                continue;
            };
            fill.fill(0);
            stroke.fill(0);
            let offset_x = (cell_x * d).floor() as i32 - 1;
            let offset_y = (cell_y * d).floor() as i32 - 1;
            let center = (cw - (character.right - character.left) * chars.size) / 2.0
                - character.left * chars.size;
            for part in &character.parts {
                let x = (l + cell_x + center + part.x * chars.size) * d - offset_x as f32;
                let y = (t + cell_y + baseline + part.y * chars.size) * d - offset_y as f32;
                let mut scaler = context
                    .builder(part.font.as_swash())
                    .size(chars.size * d)
                    .hint(false)
                    .build();
                let mut render = Render::new(&[Source::Outline]);
                render
                    .format(Format::Alpha)
                    .offset(Vector::new(x.fract(), y.fract()));
                let image = render
                    .render(&mut scaler, part.glyph)
                    .ok_or("cannot rasterize foreground contour")?;
                mask_into(&mut fill, w, h, image, x.floor() as i32, y.floor() as i32)?;
                if effects.outline.width > 0.0 {
                    let mut contour = Stroke::new(effects.outline.width as f32 * 2.0 * d);
                    contour.join(Join::Round);
                    render.style(contour);
                    let image = render
                        .render(&mut scaler, part.glyph)
                        .ok_or("cannot rasterize glyph stroke")?;
                    mask_into(&mut stroke, w, h, image, x.floor() as i32, y.floor() as i32)?;
                }
            }
            for (i, value) in shadow.iter_mut().enumerate() {
                *value = 1.0 - (1.0 - fill[i] as f32 / 255.0) * (1.0 - stroke[i] as f32 / 255.0);
            }
            if effects.shadow.opacity > 0.0 {
                glyph_pixels::blur(
                    &shadow,
                    &mut temporary,
                    &mut blurred,
                    w,
                    h,
                    effects.shadow.sigma as f32 * d,
                    (3.0 * effects.shadow.sigma * f64::from(d)).ceil() as usize,
                );
            } else {
                blurred.fill(0.0);
            }
            for y in 0..h {
                for x in 0..w {
                    let dx = offset_x + x as i32 + GUARD as i32;
                    let dy = offset_y + y as i32 + GUARD as i32;
                    if dx < GUARD as i32
                        || dy < GUARD as i32
                        || dx >= (plan.width - GUARD) as i32
                        || dy >= (plan.height - GUARD) as i32
                    {
                        continue;
                    }
                    let pixel =
                        &mut pixels[(dy as usize * plan.width as usize + dx as usize) * 4..][..4];
                    let s = glyph_pixels::sample(
                        &blurred,
                        w,
                        h,
                        x as f32 - effects.shadow.offset_x as f32 * d,
                        y as f32 - effects.shadow.offset_y as f32 * d,
                    ) * effects.shadow.opacity as f32;
                    glyph_pixels::over(pixel, effects.shadow.color.rgb(), s);
                    glyph_pixels::over(
                        pixel,
                        effects.outline.color.rgb(),
                        stroke[y * w + x] as f32 / 255.0,
                    );
                    glyph_pixels::over(
                        pixel,
                        run.foreground
                            .as_ref()
                            .map_or(DEFAULT_FOREGROUND, |c| c.rgb()),
                        fill[y * w + x] as f32 / 255.0,
                    );
                }
            }
        }
    }
    let rgba = image::RgbaImage::from_raw(plan.width, plan.height, pixels)
        .ok_or("invalid glyph raster storage")?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}
