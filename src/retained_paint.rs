//! Visible-only world painting. Camera frames project logical cells; each world
//! cell is one field cell of the world's cell size whatever its text. Tiles
//! layers submit only their visible composed tiles, each one cell tall at its
//! own width and offset.
use crate::{
    display_cache::DisplayRasterCache,
    retained_prepared::PreparedWorld,
    retained_protocol::Viewport,
    retained_world::ProjectedCell,
    viewport::{PaintRect, ViewportTransform},
};
use gpui::{Bounds, ContentMask, IntoElement, canvas, point, prelude::*, px, size};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub fn project_world(world: &PreparedWorld, viewport: &Viewport) -> Arc<Vec<ProjectedCell>> {
    let mut cells = Vec::new();
    world
        .source
        .project_visible(viewport, |projected, _| cells.push(projected));
    Arc::new(cells)
}

/// A projected world cell's row at the field's own pitch, from `left` to
/// `left + width` logical pixels across its left edge.
pub fn cell_bounds(
    cell: ProjectedCell,
    (cell_width, cell_height): (f32, f32),
    (left, width): (f32, f32),
    base: ViewportTransform,
    viewport: &Viewport,
) -> PaintRect {
    base.surface_rect(
        viewport.origin.x + (cell.screen_column as f32 * cell_width + left) * viewport.scale,
        viewport.origin.y + cell.screen_row as f32 * cell_height * viewport.scale,
        width * viewport.scale,
        cell_height * viewport.scale,
    )
}

/// One step of the field's paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldPaint {
    /// Index into the prepared world's sorted layers.
    World(usize),
    /// Index into the screen frame's paint plan.
    Plan(usize),
}

/// Merges world layers with the frame plan by layer, each already in
/// ascending order, so a world layer above characters paints above them. A
/// world layer paints before a plan item of the same layer.
pub fn merge_paint_order(world: &[i32], plan: &[i32]) -> Vec<FieldPaint> {
    let mut order = Vec::with_capacity(world.len() + plan.len());
    let (mut w, mut p) = (0, 0);
    while w < world.len() || p < plan.len() {
        if p == plan.len() || (w < world.len() && world[w] <= plan[p]) {
            order.push(FieldPaint::World(w));
            w += 1;
        } else {
            order.push(FieldPaint::Plan(p));
            p += 1;
        }
    }
    order
}

/// Paints each visible cell of one tiles layer with its tile's current frame,
/// one cell tall at the tile's own width and offset, clipped to the viewport.
pub fn tile_element(
    world: Arc<PreparedWorld>,
    layer_index: usize,
    base: ViewportTransform,
    viewport: Viewport,
    samples: Rc<RefCell<DisplayRasterCache>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let layer = &world.layers[layer_index];
            let field_cell = world.source.cell_size();
            let Some(tileset) = &world.source.definition.tileset else {
                return;
            };
            // Source pixels to logical pixels: a tile is one cell tall.
            let ratio = field_cell.1 / tileset.tile_size as f32;
            let clip = base.surface_rect(
                viewport.clip_rect.x,
                viewport.clip_rect.y,
                viewport.clip_rect.width,
                viewport.clip_rect.height,
            );
            let clip = PaintRect {
                left: f32::from(bounds.origin.x) + clip.left,
                top: f32::from(bounds.origin.y) + clip.top,
                ..clip
            };
            let device_scale = window.scale_factor();
            let mut failed = false;
            world
                .source
                .project_visible_tiles(&layer.id, &viewport, |cell, tile| {
                    if failed {
                        return;
                    }
                    let Some(region) = world.get_tile_image(tile, viewport.tile_frame) else {
                        return;
                    };
                    let definition = &tileset.tiles[tile as usize];
                    let span = (
                        definition.left as f32 * ratio,
                        definition.get_width(tileset.tile_size) as f32 * ratio,
                    );
                    let relative = cell_bounds(cell, field_cell, span, base, &viewport);
                    let destination = PaintRect {
                        left: f32::from(bounds.origin.x) + relative.left,
                        top: f32::from(bounds.origin.y) + relative.top,
                        ..relative
                    };
                    let Some(mask) = intersection(destination, clip) else {
                        return;
                    };
                    let (image, image_bounds) =
                        samples
                            .borrow_mut()
                            .prepare(region, destination, device_scale);
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
                        failed = true;
                    }
                });
        },
    )
    .absolute()
    .left_0()
    .top_0()
    .size_full()
}

fn intersection(a: PaintRect, b: PaintRect) -> Option<PaintRect> {
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
    use FieldPaint::{Plan, World};

    #[test]
    fn world_layers_interleave_with_the_plan_by_layer() {
        // Ground tiles and terrain glyphs below characters, a tiles layer
        // above them and UI text on top. A tie paints the world layer first.
        let world = [-100, -99, 100, 900];
        let plan = [-99, 100, 100, 1000];
        assert_eq!(
            merge_paint_order(&world, &plan),
            [
                World(0),
                World(1),
                Plan(0),
                World(2),
                Plan(1),
                Plan(2),
                World(3),
                Plan(3)
            ]
        );
        assert_eq!(merge_paint_order(&[], &[1, 2]), [Plan(0), Plan(1)]);
        assert_eq!(merge_paint_order(&[5], &[]), [World(0)]);
        assert_eq!(
            merge_paint_order(&[i32::MAX], &[i32::MIN]),
            [Plan(0), World(0)]
        );
    }

    #[test]
    fn intersection_clips_to_the_shared_area() {
        let rect = |left, top, width, height| PaintRect {
            left,
            top,
            width,
            height,
        };
        assert_eq!(
            intersection(rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 2.0, 10.0, 4.0)),
            Some(rect(5.0, 2.0, 5.0, 4.0))
        );
        assert_eq!(
            intersection(rect(0.0, 0.0, 10.0, 10.0), rect(10.0, 0.0, 5.0, 5.0)),
            None
        );
    }
}
