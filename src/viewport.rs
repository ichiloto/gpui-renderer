//! Logical presentation geometry stays immutable. Only these paint coordinates change.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportTransform {
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub logical_width: f32,
    pub logical_height: f32,
    pub presented_width: f32,
    pub presented_height: f32,
}

impl ViewportTransform {
    /// The current policy fits downward, never upscales, and centers the whole grid.
    /// A hidden/zero-sized viewport has zero scale; invalid sizes never produce NaN.
    pub fn fit(logical_width: f32, logical_height: f32, width: f32, height: f32) -> Self {
        let logical_width = nonnegative(logical_width);
        let logical_height = nonnegative(logical_height);
        let width = nonnegative(width);
        let height = nonnegative(height);
        let scale = if logical_width > 0.0 && logical_height > 0.0 {
            1.0_f32
                .min(width / logical_width)
                .min(height / logical_height)
        } else {
            0.0
        };
        let presented_width = logical_width * scale;
        let presented_height = logical_height * scale;
        Self {
            scale,
            offset_x: (width - presented_width).max(0.0) / 2.0,
            offset_y: (height - presented_height).max(0.0) / 2.0,
            logical_width,
            logical_height,
            presented_width,
            presented_height,
        }
    }

    /// Bounds of the clipping surface inside the native content viewport.
    pub fn surface_bounds(self) -> PaintRect {
        PaintRect {
            left: self.offset_x,
            top: self.offset_y,
            width: self.presented_width,
            height: self.presented_height,
        }
    }

    /// Child coordinates inside the scaled surface. Its parent applies centering once.
    pub fn surface_rect(self, left: f32, top: f32, width: f32, height: f32) -> PaintRect {
        PaintRect {
            left: left * self.scale,
            top: top * self.scale,
            width: nonnegative(width) * self.scale,
            height: nonnegative(height) * self.scale,
        }
    }

    /// Absolute viewport coordinates, useful for anchor checks and future hit testing.
    #[cfg(test)]
    pub fn viewport_rect(self, left: f32, top: f32, width: f32, height: f32) -> PaintRect {
        let mut rect = self.surface_rect(left, top, width, height);
        rect.left += self.offset_x;
        rect.top += self.offset_y;
        rect
    }
}

fn nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_legend_fits_at_one_or_scales_uniformly() {
        for (width, height, scale, x, y) in [
            (1600.0, 900.0, 1.0, 125.0, 90.0),
            (1350.0, 720.0, 1.0, 0.0, 0.0),
            (675.0, 720.0, 0.5, 0.0, 180.0),
            (1350.0, 360.0, 0.5, 337.5, 0.0),
            (1012.5, 360.0, 0.5, 168.75, 0.0),
            (1012.5, 540.0, 0.75, 0.0, 0.0),
        ] {
            let t = ViewportTransform::fit(1350.0, 720.0, width, height);
            assert_eq!((t.scale, t.offset_x, t.offset_y), (scale, x, y));
            assert_eq!(
                (t.presented_width, t.presented_height),
                (1350.0 * scale, 720.0 * scale)
            );
            assert!(t.presented_width <= width && t.presented_height <= height);
        }
    }

    #[test]
    fn wide_and_tall_surfaces_center_the_unconstrained_axis() {
        let wide = ViewportTransform::fit(16000.0, 100.0, 1000.0, 500.0);
        assert_eq!(
            (wide.scale, wide.offset_x, wide.offset_y),
            (0.0625, 0.0, 246.875)
        );
        let tall = ViewportTransform::fit(100.0, 16000.0, 500.0, 1000.0);
        assert_eq!(
            (tall.scale, tall.offset_x, tall.offset_y),
            (0.0625, 246.875, 0.0)
        );
    }

    #[test]
    fn minimized_or_invalid_viewports_have_safe_nonnegative_geometry() {
        for (width, height) in [
            (0.0, 0.0),
            (-1.0, 720.0),
            (1350.0, -1.0),
            (f32::NAN, f32::INFINITY),
        ] {
            let t = ViewportTransform::fit(1350.0, 720.0, width, height);
            assert_eq!(t.scale, 0.0);
            assert!(t.offset_x >= 0.0 && t.offset_y >= 0.0);
            assert_eq!((t.presented_width, t.presented_height), (0.0, 0.0));
        }
        assert_eq!(ViewportTransform::fit(0.0, 720.0, 100.0, 100.0).scale, 0.0);
    }

    #[test]
    fn scale_never_exceeds_one_across_representative_sizes() {
        for width in [1.0, 500.0, 1350.0, 16384.0] {
            for height in [1.0, 360.0, 720.0, 16384.0] {
                let t = ViewportTransform::fit(1350.0, 720.0, width, height);
                assert!((0.0..=1.0).contains(&t.scale));
                assert!(t.presented_width <= width && t.presented_height <= height);
            }
        }
    }
}
