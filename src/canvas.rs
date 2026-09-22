//! Prepared canvas images and native painting. PHP owns all authored layout/state.
use crate::canvas_protocol::{Canvas, CanvasImage, Rect};
use crate::color::DEFAULT_FOREGROUND;
use crate::renderer::{FONT_FAMILY, fit_cell_font, positioned};
use crate::viewport::{PaintRect, ViewportTransform};
use gpui::{App, Div, RenderImage, div, font, prelude::*, px, rgb};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

#[derive(Debug, PartialEq, Eq)]
pub enum PaintItem {
    Image(usize),
    Composite(usize),
    Indicator(usize),
    Text(usize),
}

#[derive(Debug)]
pub struct PreparedCanvas {
    pub source: Canvas,
    pub images: Vec<Arc<RenderImage>>,
    pub composites: Vec<Arc<RenderImage>>,
    pub plan: Vec<PaintItem>,
}

/// Mask in surface coordinates and the unchanged content relative to that mask.
/// Clipping a gauge reveals part of its full-width texture; it never rescales it.
pub fn clipped_bounds(
    destination: Rect,
    clip: Option<Rect>,
    transform: ViewportTransform,
) -> Option<(PaintRect, PaintRect)> {
    let clip = clip.unwrap_or(destination);
    let x = destination.x.max(clip.x);
    let y = destination.y.max(clip.y);
    let right = (destination.x + destination.width).min(clip.x + clip.width);
    let bottom = (destination.y + destination.height).min(clip.y + clip.height);
    if right <= x || bottom <= y {
        return None;
    }
    let mask = Rect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
    .paint(transform);
    let content = Rect {
        x: destination.x - x,
        y: destination.y - y,
        ..destination
    }
    .paint(transform);
    Some((mask, content))
}

impl PreparedCanvas {
    pub fn prepare(
        source: Canvas,
        prepare_image: impl FnMut(&CanvasImage) -> Result<Arc<RenderImage>, String>,
        composites: Vec<Arc<RenderImage>>,
    ) -> Result<Self, String> {
        let images = source
            .images
            .iter()
            .map(prepare_image)
            .collect::<Result<Vec<_>, String>>()?;
        let mut plan: Vec<_> = (0..images.len())
            .map(PaintItem::Image)
            .chain((0..composites.len()).map(PaintItem::Composite))
            .chain((0..source.indicators.len()).map(PaintItem::Indicator))
            .chain((0..source.text_layers.len()).map(PaintItem::Text))
            .collect();
        plan.sort_by_key(|item| match *item {
            PaintItem::Image(i) => source.images[i].layer,
            PaintItem::Composite(i) => source.composites.as_ref().unwrap()[i].layer,
            PaintItem::Indicator(i) => source.indicators[i].layer,
            PaintItem::Text(i) => source.text_layers[i].layer,
        });
        Ok(Self {
            source,
            images,
            composites,
            plan,
        })
    }

    pub fn element(
        &self,
        transform: ViewportTransform,
        fonts: &mut HashMap<(u32, u32), f32>,
        glyphs: &HashMap<usize, Arc<RenderImage>>,
        samples: Rc<RefCell<crate::display_cache::DisplayRasterCache>>,
        cx: &App,
    ) -> Div {
        // Retain only active metrics: at most the already validated 64 text layers.
        fonts.retain(|&(w, h), _| {
            self.source
                .text_layers
                .iter()
                .any(|layer| (layer.grid.cell_width, layer.grid.cell_height) == (w, h))
        });
        let mut surface = div().absolute().size_full().overflow_hidden();
        for item in &self.plan {
            match *item {
                PaintItem::Composite(index) => {
                    let item = &self.source.composites.as_ref().unwrap()[index];
                    surface = surface.child(crate::canvas_image::element(
                        self.composites[index].clone(),
                        item.destination,
                        item.clip_rect,
                        item.opacity,
                        transform,
                        samples.clone(),
                    ));
                }
                PaintItem::Image(index) => {
                    let item = &self.source.images[index];
                    surface = surface.child(crate::canvas_image::element(
                        self.images[index].clone(),
                        item.destination,
                        item.clip_rect,
                        item.opacity,
                        transform,
                        samples.clone(),
                    ));
                }
                PaintItem::Indicator(index) => {
                    let item = &self.source.indicators[index];
                    for stroke in item.strokes() {
                        surface = surface.child(
                            positioned(div(), stroke.paint(transform)).bg(rgb(item.color.rgb())),
                        );
                    }
                }
                PaintItem::Text(index) => {
                    let layer = &self.source.text_layers[index];
                    if layer.glyph_effects.is_some() {
                        let image = glyphs
                            .get(&index)
                            .expect("accepted effected text has a prepared raster");
                        surface = surface.child(crate::canvas_image::element(
                            image.clone(),
                            layer.paint_bounds(),
                            layer.clip_rect,
                            layer.opacity.unwrap_or(1.0),
                            transform,
                            samples.clone(),
                        ));
                        continue;
                    }
                    let Some((mask, bounds)) = clipped_bounds(
                        Rect {
                            x: layer.origin.x,
                            y: layer.origin.y,
                            width: f64::from(layer.grid.columns * layer.grid.cell_width),
                            height: f64::from(layer.grid.rows * layer.grid.cell_height),
                        },
                        layer.clip_rect,
                        transform,
                    ) else {
                        continue;
                    };
                    let (cw, ch) = (layer.grid.cell_width as f32, layer.grid.cell_height as f32);
                    let size = *fonts
                        .entry((layer.grid.cell_width, layer.grid.cell_height))
                        .or_insert_with(|| {
                            let text = cx.text_system();
                            let font_id = text.resolve_font(&font(FONT_FAMILY));
                            let advance = text.ch_advance(font_id, px(1.0)).ok().map(f32::from);
                            let line = f32::from(text.ascent(font_id, px(1.0)))
                                + f32::from(text.descent(font_id, px(1.0)));
                            fit_cell_font(cw, ch, advance, line)
                        });
                    let mut text = positioned(div(), bounds)
                        .overflow_hidden()
                        .text_size(px(size * transform.scale))
                        .line_height(px(ch * transform.scale));
                    for run in &layer.runs {
                        for (offset, glyph) in run.text.chars().enumerate() {
                            // Transparent spaces produce no paint; explicit backgrounds still paint.
                            if glyph == ' ' && run.background.is_none() {
                                continue;
                            }
                            let bounds = transform.surface_rect(
                                (run.column as f32 + offset as f32) * cw,
                                run.row as f32 * ch,
                                cw,
                                ch,
                            );
                            let mut cell = positioned(div(), bounds)
                                .overflow_hidden()
                                .text_center()
                                .text_color(rgb(run
                                    .foreground
                                    .as_ref()
                                    .map_or(DEFAULT_FOREGROUND, |color| color.rgb())));
                            if let Some(background) = &run.background {
                                cell = cell.bg(rgb(background.rgb()));
                            }
                            if glyph != ' ' {
                                cell = cell.child(glyph.to_string());
                            }
                            text = text.child(cell);
                        }
                    }
                    surface = surface.child(
                        positioned(div(), mask)
                            .overflow_hidden()
                            .opacity(layer.opacity.unwrap_or(1.0) as f32)
                            .child(text),
                    );
                }
            }
        }
        surface
    }
}
