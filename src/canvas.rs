//! Prepared canvas images and native painting. PHP owns all authored layout/state.
use crate::canvas_protocol::{Canvas, CanvasImage, Rect};
use crate::color::DEFAULT_FOREGROUND;
use crate::renderer::{FONT_FAMILY, fit_cell_font, positioned, sheet_bounds};
use crate::viewport::ViewportTransform;
use gpui::{App, Div, ObjectFit, RenderImage, div, font, img, prelude::*, px, rgb};
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, PartialEq, Eq)]
pub enum PaintItem {
    Image(usize),
    Indicator(usize),
    Text(usize),
}

#[derive(Debug)]
pub struct PreparedCanvas {
    pub source: Canvas,
    pub images: Vec<Arc<RenderImage>>,
    pub plan: Vec<PaintItem>,
}

impl PreparedCanvas {
    pub fn prepare(
        source: Canvas,
        prepare_image: impl FnMut(&CanvasImage) -> Result<Arc<RenderImage>, String>,
    ) -> Result<Self, String> {
        let images = source
            .images
            .iter()
            .map(prepare_image)
            .collect::<Result<Vec<_>, String>>()?;
        let mut plan: Vec<_> = (0..images.len())
            .map(PaintItem::Image)
            .chain((0..source.indicators.len()).map(PaintItem::Indicator))
            .chain((0..source.text_layers.len()).map(PaintItem::Text))
            .collect();
        plan.sort_by_key(|item| match *item {
            PaintItem::Image(i) => source.images[i].layer,
            PaintItem::Indicator(i) => source.indicators[i].layer,
            PaintItem::Text(i) => source.text_layers[i].layer,
        });
        Ok(Self {
            source,
            images,
            plan,
        })
    }

    pub fn element(
        &self,
        transform: ViewportTransform,
        fonts: &mut HashMap<(u32, u32), f32>,
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
                PaintItem::Image(index) => {
                    let item = &self.source.images[index];
                    let bounds = item.destination.paint(transform);
                    let image = &self.images[index];
                    let mut clip = positioned(div(), bounds)
                        .overflow_hidden()
                        .opacity(item.opacity as f32);
                    // Every canvas image, whole or cropped, has cached edge guards.
                    // Sample only its authored rectangle, never an adjacent GPU atlas entry.
                    let size = image.size(0);
                    let guard = crate::tile_regions::GUARD;
                    let sheet = sheet_bounds(
                        crate::protocol::SourceRect {
                            x: guard,
                            y: guard,
                            width: size.width.0 as u32 - 2 * guard,
                            height: size.height.0 as u32 - 2 * guard,
                        },
                        size.width.0 as u32,
                        size.height.0 as u32,
                        bounds.width,
                        bounds.height,
                    );
                    let painted = img(image.clone())
                        .object_fit(ObjectFit::Fill)
                        .absolute()
                        .left(px(sheet.left))
                        .top(px(sheet.top))
                        .w(px(sheet.width))
                        .h(px(sheet.height));
                    clip = clip.child(painted);
                    surface = surface.child(clip);
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
                    let mut text = positioned(
                        div(),
                        Rect {
                            x: layer.origin.x,
                            y: layer.origin.y,
                            width: f64::from(layer.grid.columns * layer.grid.cell_width),
                            height: f64::from(layer.grid.rows * layer.grid.cell_height),
                        }
                        .paint(transform),
                    )
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
                    surface = surface.child(text);
                }
            }
        }
        surface
    }
}
