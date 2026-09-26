//! Visible-only world painting. Tile catalogs are prepared on upload; camera
//! frames project logical cells and submit only the visible crops.
use crate::{
    display_cache::DisplayRasterCache,
    protocol::{Grid, TextLayer},
    retained_prepared::PreparedWorld,
    retained_protocol::Viewport,
    retained_world::ProjectedCell,
    viewport::{PaintRect, ViewportTransform},
};
use gpui::{Bounds, ContentMask, IntoElement, canvas, point, prelude::*, px, size};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

#[derive(Clone, Copy)]
struct OccludingCell {
    layer: i32,
    bounds: PaintRect,
}

/// Screen text is opaque even when it contains a space. Keep its coverage in
/// surface coordinates so both zoomed field text and unzoomed UI can clear an
/// intersected wide world glyph without introducing a terminal-only rule.
/// Field-member text uses the square field cell; other text uses the grid.
pub struct TextOcclusion {
    band_height: f32,
    bands: HashMap<i32, Vec<OccludingCell>>,
}

impl TextOcclusion {
    pub fn new(
        layers: &[TextLayer],
        viewport: &Viewport,
        grid: Grid,
        field_cell: f32,
        base: ViewportTransform,
    ) -> Self {
        let band_height = (grid.cell_height as f32 * base.scale).max(1.0);
        let mut bands: HashMap<i32, Vec<OccludingCell>> = HashMap::new();
        let clip = base.surface_rect(
            viewport.clip_rect.x,
            viewport.clip_rect.y,
            viewport.clip_rect.width,
            viewport.clip_rect.height,
        );
        for layer in layers {
            let selected = viewport.text_layer_ids.contains(&layer.id);
            let (scale, origin_x, origin_y, cell_width, cell_height) = if selected {
                (
                    viewport.scale,
                    viewport.origin.x,
                    viewport.origin.y,
                    field_cell,
                    field_cell,
                )
            } else {
                (
                    1.0,
                    0.0,
                    0.0,
                    grid.cell_width as f32,
                    grid.cell_height as f32,
                )
            };
            for cell in crate::state::painted_cells(layer) {
                let rect = base.surface_rect(
                    origin_x + cell.column as f32 * cell_width * scale,
                    origin_y + cell.row as f32 * cell_height * scale,
                    cell_width * scale,
                    cell_height * scale,
                );
                let visible = if selected {
                    intersection(rect, clip)
                } else {
                    Some(rect)
                };
                if let Some(bounds) = visible {
                    for band in vertical_bands(bounds, band_height) {
                        bands.entry(band).or_default().push(OccludingCell {
                            layer: layer.layer,
                            bounds,
                        });
                    }
                }
            }
        }
        Self { band_height, bands }
    }

    pub fn covers_wide_glyph(&self, bounds: PaintRect, owner_layer: i32) -> bool {
        vertical_bands(bounds, self.band_height).any(|band| {
            self.bands.get(&band).is_some_and(|cells| {
                cells.iter().any(|cell| {
                    cell.layer >= owner_layer && intersection(cell.bounds, bounds).is_some()
                })
            })
        })
    }
}

fn vertical_bands(bounds: PaintRect, height: f32) -> impl Iterator<Item = i32> {
    let first = (bounds.top / height).floor() as i32;
    let last = ((bounds.top + bounds.height - f32::EPSILON) / height).floor() as i32;
    first..=last
}

pub fn project_world(world: &PreparedWorld, viewport: &Viewport) -> Arc<Vec<ProjectedCell>> {
    let mut cells = Vec::new();
    world
        .source
        .project_visible(viewport, |projected, _| cells.push(projected));
    Arc::new(cells)
}

/// A projected world cell's square bounds at the field's own pitch.
pub fn cell_bounds(
    cell: ProjectedCell,
    field_cell: f32,
    base: ViewportTransform,
    viewport: &Viewport,
) -> PaintRect {
    let pitch = field_cell * viewport.scale;
    base.surface_rect(
        viewport.origin.x + cell.screen_column as f32 * pitch,
        viewport.origin.y + cell.screen_row as f32 * pitch,
        pitch,
        pitch,
    )
}

pub fn tile_element(
    world: Arc<PreparedWorld>,
    layer_index: usize,
    cells: Arc<Vec<ProjectedCell>>,
    base: ViewportTransform,
    viewport: Viewport,
    samples: Rc<RefCell<DisplayRasterCache>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let layer = &world.layers[layer_index];
            let field_cell = world.source.cell_size();
            let clip = base.surface_rect(
                viewport.clip_rect.x,
                viewport.clip_rect.y,
                viewport.clip_rect.width,
                viewport.clip_rect.height,
            );
            let device_scale = window.scale_factor();
            for &cell in cells.iter() {
                if !cell.tile_eligible {
                    continue;
                }
                let Some(source) = world.get_source(layer, cell.world_column, cell.world_row)
                else {
                    continue;
                };
                let relative = cell_bounds(cell, field_cell, base, &viewport);
                let destination = PaintRect {
                    left: f32::from(bounds.origin.x) + relative.left,
                    top: f32::from(bounds.origin.y) + relative.top,
                    ..relative
                };
                let clip = PaintRect {
                    left: f32::from(bounds.origin.x) + clip.left,
                    top: f32::from(bounds.origin.y) + clip.top,
                    ..clip
                };
                let Some(mask) = intersection(destination, clip) else {
                    continue;
                };
                let (image, image_bounds) =
                    samples
                        .borrow_mut()
                        .prepare(&layer.regions[source], destination, device_scale);
                let result = window.with_content_mask(
                    Some(ContentMask {
                        bounds: gpui_bounds(mask),
                    }),
                    |window| {
                        window.paint_image(
                            gpui_bounds(image_bounds),
                            Default::default(),
                            image,
                            0,
                            false,
                        )
                    },
                );
                if let Err(error) = result {
                    crate::protocol::diagnostic(format!(
                        "cannot paint retained world layer {}: {error}",
                        layer.id,
                    ));
                    break;
                }
            }
        },
    )
    .absolute()
    .left_0()
    .top_0()
    .size_full()
}

pub(crate) fn intersection(a: PaintRect, b: PaintRect) -> Option<PaintRect> {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = (a.left + a.width).min(b.left + b.width);
    let bottom = (a.top + a.height).min(b.top + b.height);
    (right > left && bottom > top).then_some(PaintRect {
        left,
        top,
        width: right - left,
        height: bottom - top,
    })
}

fn gpui_bounds(rect: PaintRect) -> Bounds<gpui::Pixels> {
    Bounds::new(
        point(px(rect.left), px(rect.top)),
        size(px(rect.width), px(rect.height)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TextRun;
    use serde_json::from_value;
    use serde_json::json;

    #[test]
    fn opaque_screen_cell_clears_whole_wide_world_glyph_at_any_zoom() {
        let grid = Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        };
        let base = ViewportTransform::fit(40.0, 40.0, 40.0, 40.0);
        let text = vec![TextLayer {
            id: "ui".into(),
            layer: 1000,
            runs: vec![TextRun {
                row: 0,
                column: 1,
                text: " ".into(),
                foreground: None,
                background: None,
            }],
        }];
        for selected in [false, true] {
            let viewport: Viewport = from_value(json!({"scale":2,
                "origin":{"x":0,"y":0},"worldId":"map",
                "worldOrigin":{"column":0,"row":0},
                "clipRect":{"x":0,"y":0,"width":40,"height":40},
                "textLayerIds":if selected { vec!["ui"] } else { vec![] }
            }))
            .unwrap();
            // A selected (field) layer sits on the square field cell.
            let coverage = TextOcclusion::new(&text, &viewport, grid, 10.0, base);
            let wide = base.surface_rect(0.0, 0.0, 40.0, 40.0);
            assert!(coverage.covers_wide_glyph(wide, -99));
            assert!(!coverage.covers_wide_glyph(wide, 2000));
            assert!(!coverage.covers_wide_glyph(base.surface_rect(0.0, 40.0, 40.0, 40.0), -99));
        }
    }
}
