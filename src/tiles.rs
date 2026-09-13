//! One layout element per batch. Each visible cell still emits an image primitive;
//! this reduces element/layout overhead and is not a claim of one GPU draw call.
use crate::protocol::Grid;
use crate::state::PreparedTileBatch;
use crate::tile_regions::GUARD;
use crate::viewport::{PaintRect, ViewportTransform};
use gpui::{Bounds, ContentMask, IntoElement, canvas, point, prelude::*, px, size};
use std::sync::Arc;

pub fn element(
    batch: Arc<PreparedTileBatch>,
    grid: Grid,
    transform: ViewportTransform,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let device_scale = window.scale_factor();
            for cell in &batch.batch.cells {
                let source = cell.source as usize;
                let rect = batch.batch.sources[source];
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
                let image_bounds =
                    guarded_bounds(destination, rect.width, rect.height, device_scale);
                let result = window.with_content_mask(
                    Some(ContentMask {
                        bounds: gpui_bounds(destination),
                    }),
                    |window| {
                        window.paint_image(
                            gpui_bounds(image_bounds),
                            Default::default(),
                            batch.regions[source].clone(),
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

/// Pixel-snapped outer guard bounds enclose the exact, unsnapped destination mask.
/// GPUI otherwise floors the origin but ceils only the SIZE, possibly shortening
/// the right edge. Explicitly rounding both endpoints also protects tiny scales.
fn guarded_bounds(
    destination: PaintRect,
    source_width: u32,
    source_height: u32,
    device_scale: f32,
) -> PaintRect {
    let margin_x = (destination.width * GUARD as f32 / source_width as f32).max(1.0 / device_scale);
    let margin_y =
        (destination.height * GUARD as f32 / source_height as f32).max(1.0 / device_scale);
    let left = ((destination.left - margin_x) * device_scale).floor() / device_scale;
    let top = ((destination.top - margin_y) * device_scale).floor() / device_scale;
    let right =
        ((destination.left + destination.width + margin_x) * device_scale).ceil() / device_scale;
    let bottom =
        ((destination.top + destination.height + margin_y) * device_scale).ceil() / device_scale;
    PaintRect {
        left,
        top,
        width: right - left,
        height: bottom - top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_edges_keep_exact_cell_masks_and_samples_inside_isolated_regions() {
        for (sw, sh) in [(1, 1), (16, 32), (4096, 8), (8, 4096)] {
            for scale in [1.0, 0.875, 0.731, 0.5, 0.123, 0.001] {
                let t = ViewportTransform::fit(1350.0, 720.0, 1350.0 * scale + 47.0, 720.0 * scale);
                for (column, row) in [(0, 0), (134, 0), (0, 35), (134, 35), (17, 13)] {
                    let d = t.viewport_rect(column as f32 * 10.0, row as f32 * 20.0, 10.0, 20.0);
                    assert!((d.width - 10.0 * scale).abs() < 0.0001);
                    assert!((d.height - 20.0 * scale).abs() < 0.0001);
                    for device in [1.0, 1.25, 2.0, 3.0] {
                        let p = guarded_bounds(d, sw, sh, device);
                        // Include the painter's actual second f32 scale/round
                        // operation, not just our intended snapped geometry.
                        let p = PaintRect {
                            left: (p.left * device).floor() / device,
                            top: (p.top * device).floor() / device,
                            width: (p.width * device).ceil() / device,
                            height: (p.height * device).ceil() / device,
                        };
                        // Bilinear filtering has a half-texel footprint. Even at
                        // the exact clipping boundary it never reaches another
                        // GPUI atlas allocation, nor another authored source.
                        for (start, len, paint_start, paint_len, source_len) in [
                            (d.left, d.width, p.left, p.width, sw),
                            (d.top, d.height, p.top, p.height, sh),
                        ] {
                            let pixels = (source_len + 2 * GUARD) as f32;
                            let first = (start - paint_start) / paint_len * pixels;
                            let last = (start + len - paint_start) / paint_len * pixels;
                            assert!(first >= 0.5, "{first}: {scale} {device} {source_len}");
                            assert!(
                                last <= pixels - 0.5,
                                "{last}: {scale} {device} {source_len}"
                            );
                        }
                    }
                }
            }
        }
    }
}
