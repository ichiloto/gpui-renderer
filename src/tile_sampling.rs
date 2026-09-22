//! Compensate for GPUI 0.2.2's floor(origin)/ceil(size) image painter.
//!
//! Sample the authored rectangle at destination device-pixel centers, then paint
//! that raster 1:1. The exact cell mask still owns coverage. Moving a cell by an
//! integer device pixel reuses the raster; source identity, size and subpixel
//! phase determine its samples. No PNG encoding or GPUI internals are involved.
use crate::display_cache::{DisplayRasterCache, MAX_BYTES, MAX_IMAGES, RasterKey, TileKey};
use crate::tile_regions::GUARD;
use crate::viewport::PaintRect;
use gpui::RenderImage;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::Arc;

const DEVICE_GUARD: u32 = 1;

impl DisplayRasterCache {
    pub fn prepare(
        &mut self,
        region: &Arc<RenderImage>,
        destination: PaintRect,
        scale: f32,
    ) -> (Arc<RenderImage>, PaintRect) {
        self.prepare_with_limits(region, destination, scale, MAX_BYTES, MAX_IMAGES)
    }

    fn prepare_with_limits(
        &mut self,
        region: &Arc<RenderImage>,
        destination: PaintRect,
        scale: f32,
        byte_limit: usize,
        count_limit: usize,
    ) -> (Arc<RenderImage>, PaintRect) {
        let geometry = SamplingGeometry::new(destination, scale);
        let key = RasterKey::Tile(TileKey {
            region: region.id,
            width: geometry.width.to_bits(),
            height: geometry.height.to_bits(),
            phase_x: geometry.phase_x.to_bits(),
            phase_y: geometry.phase_y.to_bits(),
        });
        if let Some(image) = self.get(&key) {
            return (image, geometry.bounds(scale));
        }
        let image = rasterize(region, geometry);
        self.insert_with_limits(key, image.clone(), byte_limit, count_limit);
        (image, geometry.bounds(scale))
    }
}

#[derive(Clone, Copy)]
struct SamplingGeometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    phase_x: f32,
    phase_y: f32,
}

impl SamplingGeometry {
    fn new(destination: PaintRect, scale: f32) -> Self {
        let left = destination.left * scale;
        let top = destination.top * scale;
        Self {
            left: left.floor() - DEVICE_GUARD as f32,
            top: top.floor() - DEVICE_GUARD as f32,
            width: destination.width * scale,
            height: destination.height * scale,
            phase_x: left - left.floor(),
            phase_y: top - top.floor(),
        }
    }

    fn raster_size(self) -> (u32, u32) {
        (
            (self.phase_x + self.width).ceil() as u32 + 2 * DEVICE_GUARD,
            (self.phase_y + self.height).ceil() as u32 + 2 * DEVICE_GUARD,
        )
    }

    fn bounds(self, scale: f32) -> PaintRect {
        let (width, height) = self.raster_size();
        PaintRect {
            left: snap_input(self.left, scale, true),
            top: snap_input(self.top, scale, true),
            width: snap_input(width as f32, scale, false),
            height: snap_input(height as f32, scale, false),
        }
    }
}

/// Division/multiplication can straddle an integer by an f32 ULP. Supply values
/// whose actual second multiply and floor/ceil yield the intended device bounds.
fn snap_input(device: f32, scale: f32, origin: bool) -> f32 {
    let mut logical = device / scale;
    if origin {
        while (logical * scale).floor() < device {
            logical = logical.next_up();
        }
        while (logical * scale).floor() > device {
            logical = logical.next_down();
        }
    } else {
        while (logical * scale).ceil() > device {
            logical = logical.next_down();
        }
        while (logical * scale).ceil() < device {
            logical = logical.next_up();
        }
    }
    logical
}

fn axis_samples(count: u32, phase: f32, extent: f32, source_extent: u32) -> Vec<(u32, u32, f32)> {
    (0..count)
        .map(|pixel| {
            // Texture coordinates use pixel centers, so source texel zero is at 0.5.
            let position = ((pixel as f64 + 0.5 - DEVICE_GUARD as f64 - phase as f64)
                / extent as f64
                * source_extent as f64
                - 0.5)
                .clamp(0.0, (source_extent - 1) as f64);
            let first = position.floor() as u32;
            (
                first + GUARD,
                (first + 1).min(source_extent - 1) + GUARD,
                (position - first as f64) as f32,
            )
        })
        .collect()
}

fn rasterize(region: &RenderImage, geometry: SamplingGeometry) -> Arc<RenderImage> {
    let size = region.size(0);
    let stride = size.width.0 as u32;
    let source_width = stride - 2 * GUARD;
    let source_height = size.height.0 as u32 - 2 * GUARD;
    let source = region.as_bytes(0).unwrap();
    let (width, height) = geometry.raster_size();
    let xs = axis_samples(width, geometry.phase_x, geometry.width, source_width);
    let ys = axis_samples(height, geometry.phase_y, geometry.height, source_height);
    let mut pixels = image::RgbaImage::new(width, height);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let (x0, x1, tx) = xs[x as usize];
        let (y0, y1, ty) = ys[y as usize];
        let offsets = [
            y0 * stride + x0,
            y0 * stride + x1,
            y1 * stride + x0,
            y1 * stride + x1,
        ];
        for channel in 0..4 {
            let [a, b, c, d] = offsets.map(|offset| source[offset as usize * 4 + channel] as f32);
            let upper = a + (b - a) * tx;
            let lower = c + (d - c) * tx;
            // GPUI filters unorm BGRA and alpha linearly, without premultiplying.
            pixel.0[channel] = (upper + (lower - upper) * ty).round() as u8;
        }
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{protocol::SourceRect, tile_regions::RegionCache, viewport::ViewportTransform};

    fn gradient() -> Arc<RenderImage> {
        let atlas = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_fn(20, 36, |x, y| {
                if (2..18).contains(&x) && (2..34).contains(&y) {
                    let (x, y) = (x - 2, y - 2);
                    image::Rgba([(x * 16) as u8, (y * 7) as u8, 43, (x * 8 + y * 2) as u8])
                } else {
                    image::Rgba([0, 255, 255, 255]) // Adjacent atlas poison.
                }
            }),
        )]));
        RegionCache::default()
            .prepare(
                &atlas,
                SourceRect {
                    x: 2,
                    y: 2,
                    width: 16,
                    height: 32,
                },
            )
            .unwrap()
    }

    // Model the public painter's actual f32 conversion, then its GPU's 1:1
    // device-pixel-center lookup. Do not infer samples from intended bounds.
    fn painted_bounds(bounds: PaintRect, scale: f32) -> PaintRect {
        PaintRect {
            left: (bounds.left * scale).floor(),
            top: (bounds.top * scale).floor(),
            width: (bounds.width * scale).ceil(),
            height: (bounds.height * scale).ceil(),
        }
    }

    fn assert_gradient_samples(destination: PaintRect, scale: f32) -> usize {
        let (image, bounds) =
            DisplayRasterCache::default().prepare(&gradient(), destination, scale);
        let painted = painted_bounds(bounds, scale);
        let size = image.size(0);
        assert_eq!(painted.width, size.width.0 as f32);
        assert_eq!(painted.height, size.height.0 as f32);
        let (left, top) = (destination.left * scale, destination.top * scale);
        let (width, height) = (destination.width * scale, destination.height * scale);
        assert!(left - painted.left >= 1.0);
        assert!(top - painted.top >= 1.0);
        assert!(painted.left + painted.width - left - width >= 0.999);
        assert!(painted.top + painted.height - top - height >= 0.999);
        let bytes = image.as_bytes(0).unwrap();
        let mut checked = 0;
        for y in 0..size.height.0 {
            for x in 0..size.width.0 {
                let (px, py) = (
                    painted.left as f64 + x as f64 + 0.5,
                    painted.top as f64 + y as f64 + 0.5,
                );
                if px < left as f64
                    || py < top as f64
                    || px >= (left + width) as f64
                    || py >= (top + height) as f64
                {
                    continue;
                }
                // The authored fixture is linear in both axes. These analytic
                // colors specify its full source-to-cell transform independently
                // of the sampler's interpolation and guard implementation.
                let sx = ((px - left as f64) / width as f64 * 16.0 - 0.5).clamp(0.0, 15.0);
                let sy = ((py - top as f64) / height as f64 * 32.0 - 0.5).clamp(0.0, 31.0);
                let expected = [sx * 16.0, sy * 7.0, 43.0, sx * 8.0 + sy * 2.0];
                let offset = (y * size.width.0 + x) as usize * 4;
                for channel in 0..4 {
                    assert!(
                        (bytes[offset + channel] as f64 - expected[channel]).abs() <= 0.501,
                        "pixel {px},{py} channel {channel}: {} vs {}",
                        bytes[offset + channel],
                        expected[channel]
                    );
                }
                checked += 1;
            }
        }
        checked
    }

    #[test]
    fn selected_source_fills_16_to_10_cell_without_cropping_or_stretching() {
        let destination = PaintRect {
            left: 0.0,
            top: 0.0,
            width: 10.0,
            height: 20.0,
        };
        assert_eq!(assert_gradient_samples(destination, 1.0), 200);
        let (image, _) = DisplayRasterCache::default().prepare(&gradient(), destination, 1.0);
        // First/last visible red samples are 5/235. The former outward-snapped
        // guarded image gave 17/223, cropping both distinctive source edges.
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(bytes[(12 + 1) * 4], 5);
        assert_eq!(bytes[(12 + 10) * 4], 235);
    }

    #[test]
    fn fractional_resize_dpi_and_viewport_corners_keep_authored_samples() {
        let mut checked = 0;
        for fit in [1.0, 0.875, 0.731, 0.5, 0.123, 0.001] {
            let t = ViewportTransform::fit(1350.0, 720.0, 1350.0 * fit + 47.3, 720.0 * fit);
            for (column, row) in [(0, 0), (134, 0), (0, 35), (134, 35), (17, 13)] {
                let d = t.viewport_rect(column as f32 * 10.0, row as f32 * 20.0, 10.0, 20.0);
                for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
                    checked += assert_gradient_samples(d, dpi);
                }
            }
        }
        assert!(checked > 10000);
    }

    #[test]
    fn singleton_and_extreme_source_aspects_never_sample_neighbouring_tiles() {
        for (width, height) in [(1, 1), (4096, 8), (8, 4096)] {
            let atlas = Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::from_pixel(width, height, image::Rgba([193, 17, 43, 71])),
            )]));
            let region = RegionCache::default()
                .prepare(
                    &atlas,
                    SourceRect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                )
                .unwrap();
            for fit in [1.0, 0.731, 0.001] {
                let destination = PaintRect {
                    left: -0.73,
                    top: 13.21,
                    width: 10.0 * fit,
                    height: 20.0 * fit,
                };
                let (image, _) = DisplayRasterCache::default().prepare(&region, destination, 1.25);
                for pixel in image.as_bytes(0).unwrap().as_chunks::<4>().0 {
                    assert_eq!(pixel, &[193, 17, 43, 71]);
                }
            }
        }
    }

    #[test]
    fn unchanged_cells_and_frames_reuse_samples_but_phase_size_dpi_and_source_do_not() {
        let region = gradient();
        let mut cache = DisplayRasterCache::default();
        let d = PaintRect {
            left: 0.25,
            top: 0.5,
            width: 10.0,
            height: 20.0,
        };
        let (first, _) = cache.prepare(&region, d, 1.0);
        for _ in 0..2 {
            assert!(
                cache
                    .begin_frame(&HashSet::from([region.id]), &HashSet::new())
                    .is_empty()
            );
            for row in 0..36 {
                for col in 0..135 {
                    let (image, _) = cache.prepare(
                        &region,
                        PaintRect {
                            left: d.left + col as f32 * 10.0,
                            top: d.top + row as f32 * 20.0,
                            ..d
                        },
                        1.0,
                    );
                    assert!(Arc::ptr_eq(&first, &image));
                }
            }
        }
        assert_eq!(cache.entries.len(), 1);
        for (destination, dpi, source) in [
            (PaintRect { left: 0.75, ..d }, 1.0, region.clone()),
            (PaintRect { width: 9.0, ..d }, 1.0, region.clone()),
            (d, 2.0, region),
            (d, 1.0, gradient()),
        ] {
            assert_ne!(cache.prepare(&source, destination, dpi).0.id, first.id);
        }
        assert_eq!(cache.entries.len(), 5);
    }

    #[test]
    fn cache_eviction_defers_atlas_retirement_until_the_next_frame() {
        let region = gradient();
        let mut cache = DisplayRasterCache::default();
        let d = PaintRect {
            left: 0.0,
            top: 0.0,
            width: 10.0,
            height: 20.0,
        };
        let bytes = 12 * 22 * 4;
        let (first, _) = cache.prepare_with_limits(&region, d, 1.0, bytes, 1);
        let (second, _) = cache.prepare_with_limits(&gradient(), d, 1.0, bytes, 1);
        assert_eq!(cache.bytes, bytes);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.retired[0].id, first.id);
        let retired = cache.begin_frame(&HashSet::new(), &HashSet::new()); // Empty tile frame.
        assert_eq!(
            retired.iter().map(|image| image.id).collect::<HashSet<_>>(),
            HashSet::from([first.id, second.id])
        );
        assert_eq!(cache.bytes, 0);
        assert!(cache.entries.is_empty());
        assert!(cache.retired.is_empty());
        assert_eq!(first.as_bytes(0).unwrap().len(), bytes);
        let (uncached, _) = cache.prepare_with_limits(&region, d, 1.0, bytes - 1, 1);
        assert!(cache.entries.is_empty());
        assert_eq!(
            cache.begin_frame(&HashSet::from([region.id]), &HashSet::new())[0].id,
            uncached.id
        );
    }
}
