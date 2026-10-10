//! Platform-independent premultiplied sampling, coverage and compositing math.
use crate::{
    canvas_protocol::Rect,
    composite_protocol::{Blend, Brush, Displacement, Point, Stop},
};

pub type Pixel = [f32; 4]; // premultiplied BGRA, sRGB channels

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl PixelRect {
    pub fn intersect(r: Rect, width: u32, height: u32) -> Self {
        let x = r.x.floor().clamp(0.0, width as f64) as u32;
        let y = r.y.floor().clamp(0.0, height as f64) as u32;
        let right = (r.x + r.width).ceil().clamp(0.0, width as f64) as u32;
        let bottom = (r.y + r.height).ceil().clamp(0.0, height as f64) as u32;
        Self {
            x,
            y,
            width: right.saturating_sub(x),
            height: bottom.saturating_sub(y),
        }
    }
    pub fn get_area(self) -> usize {
        self.width as usize * self.height as usize
    }
}
#[inline]
pub fn unpack(bytes: &[u8]) -> Pixel {
    let a = bytes[3] as f32 / 255.0;
    [
        bytes[0] as f32 / 255.0 * a,
        bytes[1] as f32 / 255.0 * a,
        bytes[2] as f32 / 255.0 * a,
        a,
    ]
}
#[inline]
pub fn write(bytes: &mut [u8], p: Pixel) {
    let inverse = if p[3] > 0.0 { 255.0 / p[3] } else { 0.0 };
    for i in 0..3 {
        bytes[i] = (p[i] * inverse).round().clamp(0.0, 255.0) as u8;
    }
    bytes[3] = (p[3] * 255.0).round().clamp(0.0, 255.0) as u8;
}
#[inline]
pub fn blend(s: Pixel, d: Pixel, mode: Blend) -> Pixel {
    let mut p = [0.0; 4];
    for i in 0..3 {
        p[i] = match mode {
            Blend::SourceOver => s[i] + d[i] * (1.0 - s[3]),
            // W3C screen with straight-alpha inputs reduces to this expression
            // in premultiplied form, including a transparent backdrop.
            Blend::Screen => s[i] + d[i] - s[i] * d[i],
        };
    }
    p[3] = s[3] + d[3] * (1.0 - s[3]);
    p
}
#[inline]
pub fn paint(target: &mut [u8], source: Pixel, mode: Blend) {
    if target[3] == 0 || (mode == Blend::SourceOver && source[3] == 1.0) {
        write(target, source);
    } else {
        write(target, blend(source, unpack(target), mode));
    }
}
#[inline]
fn mix(a: Pixel, b: Pixel, t: f32) -> Pixel {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

pub fn sample(bytes: &[u8], width: u32, height: u32, source: Rect, uv: Point) -> Pixel {
    // Pixel centers. Clamp every tap to the selected source extent, preventing
    // transparent-edge colour fringes and sampling neighbouring atlas entries.
    let min_x = (source.x * width as f64)
        .floor()
        .clamp(0.0, (width - 1) as f64) as u32;
    let min_y = (source.y * height as f64)
        .floor()
        .clamp(0.0, (height - 1) as f64) as u32;
    let max_x = (((source.x + source.width) * width as f64)
        .ceil()
        .clamp(1.0, width as f64) as u32
        - 1)
    .max(min_x);
    let max_y = (((source.y + source.height) * height as f64)
        .ceil()
        .clamp(1.0, height as f64) as u32
        - 1)
    .max(min_y);
    let x = ((source.x + uv[0].clamp(0.0, 1.0) * source.width) * width as f64 - 0.5)
        .clamp(min_x as f64, max_x as f64);
    let y = ((source.y + uv[1].clamp(0.0, 1.0) * source.height) * height as f64 - 0.5)
        .clamp(min_y as f64, max_y as f64);
    let (ix, iy) = (x.floor() as u32, y.floor() as u32);
    let read = |x: u32, y: u32| unpack(&bytes[((y * width + x) as usize) * 4..][..4]);
    mix(
        mix(
            read(ix, iy),
            read((ix + 1).min(max_x), iy),
            (x - ix as f64) as f32,
        ),
        mix(
            read(ix, (iy + 1).min(max_y)),
            read((ix + 1).min(max_x), (iy + 1).min(max_y)),
            (x - ix as f64) as f32,
        ),
        (y - iy as f64) as f32,
    )
}
#[inline]
pub fn rect_coverage(r: Rect, p: Point) -> f64 {
    let w = (r.x + r.width).min(p[0] + 0.5) - r.x.max(p[0] - 0.5);
    let h = (r.y + r.height).min(p[1] + 0.5) - r.y.max(p[1] - 0.5);
    w.clamp(0.0, 1.0) * h.clamp(0.0, 1.0)
}
pub fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    segment_distance_squared(p, a, b).sqrt()
}

/// Conservative capsule extent at one scanline. Outside it, edge coverage is zero.
pub fn segment_row_extent(a: Point, b: Point, y: f64, radius: f64) -> Option<(f64, f64)> {
    let low = a[1].min(b[1]).max(y - radius);
    let high = a[1].max(b[1]).min(y + radius);
    if low > high {
        return None;
    }
    let (left, right) = if a[1] == b[1] {
        (a[0].min(b[0]), a[0].max(b[0]))
    } else {
        let at = |value: f64| a[0] + (value - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
        let (x, z) = (at(low), at(high));
        (x.min(z), x.max(z))
    };
    Some((left - radius, right + radius))
}

pub fn row_pixel_span(left: f64, right: f64, bounds: PixelRect) -> std::ops::Range<u32> {
    let start = bounds.x as f64;
    let end = (bounds.x + bounds.width) as f64;
    (left - 0.5).ceil().clamp(start, end) as u32..(right + 0.5).ceil().clamp(start, end) as u32
}

fn segment_distance_squared(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length
    } else {
        0.0
    }
    .clamp(0.0, 1.0);
    let x = p[0] - a[0] - t * dx;
    let y = p[1] - a[1] - t * dy;
    x * x + y * y
}
pub fn polygon_distance(p: Point, vertices: &[Point]) -> f64 {
    let (mut inside, mut distance) = (false, f64::INFINITY);
    for (i, &a) in vertices.iter().enumerate() {
        let b = vertices[(i + 1) % vertices.len()];
        distance = distance.min(segment_distance_squared(p, a, b));
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    if inside {
        distance.sqrt()
    } else {
        -distance.sqrt()
    }
}
pub fn ellipse_distance(p: Point, center: Point, radius: Point) -> f64 {
    let x = p[0] - center[0];
    let y = p[1] - center[1];
    let r = (x / radius[0]).hypot(y / radius[1]);
    // Signed distance along the radial ray; exact for circles. This is the
    // documented feather metric, not a claim of Euclidean ellipse distance.
    if r > 0.0 {
        (r.recip() - 1.0) * x.hypot(y)
    } else {
        radius[0].min(radius[1])
    }
}
pub fn coverage(distance: f64, feather: f64, invert: bool) -> f64 {
    let d = if invert { -distance } else { distance };
    if feather == 0.0 {
        (d + 0.5).clamp(0.0, 1.0)
    } else {
        let t = (d / feather).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}
fn stop_color(s: &Stop) -> Pixel {
    color(s.color.rgb(), s.opacity as f32)
}
fn color(rgb: u32, opacity: f32) -> Pixel {
    [
        (rgb & 255) as f32 / 255.0 * opacity,
        ((rgb >> 8) & 255) as f32 / 255.0 * opacity,
        ((rgb >> 16) & 255) as f32 / 255.0 * opacity,
        opacity,
    ]
}

pub enum PreparedBrush {
    Solid(Pixel),
    Linear {
        start: Point,
        direction: Point,
        length: f64,
        stops: Vec<(f64, Pixel)>,
    },
    Radial {
        center: Point,
        radius: Point,
        stops: Vec<(f64, Pixel)>,
    },
}
impl PreparedBrush {
    pub fn prepare(brush: &Brush) -> Self {
        let prepare_stops =
            |stops: &[Stop]| stops.iter().map(|s| (s.offset, stop_color(s))).collect();
        match brush {
            Brush::Solid { color: c } => Self::Solid(color(c.rgb(), 1.0)),
            Brush::Linear { start, end, stops } => {
                let dx = end[0] - start[0];
                let dy = end[1] - start[1];
                let length = dx.hypot(dy);
                Self::Linear {
                    start: *start,
                    direction: [dx / length, dy / length],
                    length,
                    stops: prepare_stops(stops),
                }
            }
            Brush::Radial {
                center,
                radius,
                stops,
            } => Self::Radial {
                center: *center,
                radius: *radius,
                stops: prepare_stops(stops),
            },
        }
    }

    #[inline]
    pub fn sample(&self, p: Point) -> Pixel {
        let (t, stops) = match self {
            Self::Solid(pixel) => return *pixel,
            Self::Linear {
                start,
                direction,
                length,
                stops,
            } => (
                ((p[0] - start[0]) * direction[0] + (p[1] - start[1]) * direction[1]) / length,
                stops,
            ),
            Self::Radial {
                center,
                radius,
                stops,
            } => (
                ((p[0] - center[0]) / radius[0]).hypot((p[1] - center[1]) / radius[1]),
                stops,
            ),
        };
        let t = t.clamp(0.0, 1.0);
        for pair in stops.windows(2) {
            if t <= pair[1].0 {
                return mix(
                    pair[0].1,
                    pair[1].1,
                    ((t - pair[0].0) / (pair[1].0 - pair[0].0)) as f32,
                );
            }
        }
        stops.last().unwrap().1
    }
}

#[cfg(test)]
pub fn brush(b: &Brush, p: Point) -> Pixel {
    let (t, stops) = match b {
        Brush::Solid { color: c } => return color(c.rgb(), 1.0),
        Brush::Linear { start, end, stops } => {
            let dx = end[0] - start[0];
            let dy = end[1] - start[1];
            let length = dx.hypot(dy);
            (
                ((p[0] - start[0]) * (dx / length) + (p[1] - start[1]) * (dy / length)) / length,
                stops,
            )
        }
        Brush::Radial {
            center,
            radius,
            stops,
        } => (
            ((p[0] - center[0]) / radius[0]).hypot((p[1] - center[1]) / radius[1]),
            stops,
        ),
    };
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        if t <= pair[1].offset {
            return mix(
                stop_color(&pair[0]),
                stop_color(&pair[1]),
                ((t - pair[0].offset) / (pair[1].offset - pair[0].offset)) as f32,
            );
        }
    }
    stop_color(stops.last().unwrap())
}
pub fn displacement(d: &Displacement, uv: Point) -> Point {
    let x = uv[0].clamp(0.0, 1.0) * (d.columns - 1) as f64;
    let y = uv[1].clamp(0.0, 1.0) * (d.rows - 1) as f64;
    let ix = (x.floor() as u32).min(d.columns - 2);
    let iy = (y.floor() as u32).min(d.rows - 2);
    let tx = x - ix as f64;
    let ty = y - iy as f64;
    let at = |x, y| d.offsets[(y * d.columns + x) as usize];
    std::array::from_fn(|i| {
        let a = at(ix, iy)[i] * (1.0 - tx) + at(ix + 1, iy)[i] * tx;
        let b = at(ix, iy + 1)[i] * (1.0 - tx) + at(ix + 1, iy + 1)[i] * tx;
        a * (1.0 - ty) + b * ty
    })
}
