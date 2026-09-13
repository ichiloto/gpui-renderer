//! One layout element per batch. Each visible cell still emits an image primitive;
//! this reduces element/layout overhead and is not a claim of one GPU draw call.
use crate::protocol::Grid;
use crate::state::PreparedTileBatch;
use crate::tile_sampling::TileSamplingCache;
use crate::viewport::{PaintRect, ViewportTransform};
use gpui::{Bounds, ContentMask, IntoElement, canvas, point, prelude::*, px, size};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub fn element(
    batch: Arc<PreparedTileBatch>,
    grid: Grid,
    transform: ViewportTransform,
    samples: Rc<RefCell<TileSamplingCache>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let device_scale = window.scale_factor();
            for cell in &batch.batch.cells {
                let source = cell.source as usize;
                let destination = transform.surface_rect(
                    cell.column as f32 * grid.cell_width as f32,
                    cell.row as f32 * grid.cell_height as f32,
                    grid.cell_width as f32,
                    grid.cell_height as f32,
                );
                let destination = PaintRect {
                    left: f32::from(bounds.origin.x) + destination.left,
                    top: f32::from(bounds.origin.y) + destination.top,
                    ..destination
                };
                let (image, image_bounds) =
                    samples
                        .borrow_mut()
                        .prepare(&batch.regions[source], destination, device_scale);
                let result = window.with_content_mask(
                    Some(ContentMask {
                        bounds: gpui_bounds(destination),
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
                        "cannot paint tile batch {}: {error}",
                        batch.batch.id
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

fn gpui_bounds(rect: PaintRect) -> Bounds<gpui::Pixels> {
    Bounds::new(
        point(px(rect.left), px(rect.top)),
        size(px(rect.width), px(rect.height)),
    )
}
