//! Straight-alpha BGRA composition and a finite-support Gaussian on glyph alpha.
pub fn over(pixel: &mut [u8], color: u32, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a == 0.0 {
        return;
    }
    let old = pixel[3] as f32 / 255.0;
    let combined = a + old * (1.0 - a);
    for (channel, shift) in [0, 8, 16].into_iter().enumerate() {
        let source = ((color >> shift) & 255) as f32;
        pixel[channel] =
            ((source * a + pixel[channel] as f32 * old * (1.0 - a)) / combined).round() as u8;
    }
    pixel[3] = (combined * 255.0).round() as u8;
}

pub fn blur(
    source: &[f32],
    temporary: &mut [f32],
    result: &mut [f32],
    width: usize,
    height: usize,
    sigma: f32,
    radius: usize,
) {
    if sigma <= 0.0 || radius == 0 {
        result.copy_from_slice(source);
        return;
    }
    let mut kernel: Vec<_> = (0..=2 * radius)
        .map(|i| {
            let x = i as f32 - radius as f32;
            (-x * x / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    for value in &mut kernel {
        *value /= total;
    }
    for y in 0..height {
        for x in 0..width {
            temporary[y * width + x] = kernel
                .iter()
                .enumerate()
                .filter_map(|(i, weight)| {
                    let sx = (x + i).checked_sub(radius)?;
                    (sx < width).then(|| source[y * width + sx] * weight)
                })
                .sum();
        }
    }
    for y in 0..height {
        for x in 0..width {
            result[y * width + x] = kernel
                .iter()
                .enumerate()
                .filter_map(|(i, weight)| {
                    let sy = (y + i).checked_sub(radius)?;
                    (sy < height).then(|| temporary[sy * width + x] * weight)
                })
                .sum();
        }
    }
}

pub fn sample(mask: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    let left = x.floor() as i32;
    let top = y.floor() as i32;
    let tx = x - x.floor();
    let ty = y - y.floor();
    let at = |x: i32, y: i32| {
        if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
            0.0
        } else {
            mask[y as usize * width + x as usize]
        }
    };
    (at(left, top) * (1.0 - tx) + at(left + 1, top) * tx) * (1.0 - ty)
        + (at(left, top + 1) * (1.0 - tx) + at(left + 1, top + 1) * tx) * ty
}
