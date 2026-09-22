//! Reader-thread composition with bounded static-mask reuse and live-output accounting.
use crate::{
    composite_pixels::{self as pixels, PixelRect},
    composite_protocol::{self as wire, Composite, Mask, Operation, Point},
};
use gpui::{ImageId, RenderImage};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
const MASK_BYTES: usize = 8 * 1024 * 1024;
const BASE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MASKS: usize = 128;
pub const GUARD: u32 = 2;
pub type Sources = BTreeMap<PathBuf, Arc<RenderImage>>;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub builds: usize,
    pub mask_builds: usize,
    pub work: usize,
    pub reserved_bytes: usize,
}
#[derive(Debug)]
struct MaskEntry {
    key: String,
    pixels: Arc<Vec<u8>>,
}
#[derive(Debug, Default)]
pub struct CompositeCache {
    outputs: HashMap<String, Arc<RenderImage>>,
    live: HashMap<ImageId, (Weak<RenderImage>, usize)>,
    masks: VecDeque<MaskEntry>,
    mask_bytes: usize,
    bases: VecDeque<MaskEntry>,
    base_bytes: usize,
}
struct Plan<'a> {
    composite: &'a Composite,
    key: String,
    bytes: usize,
}

fn source_key<'a>(paths: impl IntoIterator<Item = &'a Path>, sources: &Sources) -> String {
    paths
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|path| format!("{path:?}:{:?};", sources[path].id))
        .collect()
}
fn mask_key(masks: &[Mask], bounds: PixelRect, sources: &Sources) -> String {
    let key = source_key(
        masks.iter().filter_map(|mask| match mask {
            Mask::ImageAlpha { asset, .. } => Some(asset.as_path()),
            _ => None,
        }),
        sources,
    );
    format!("{bounds:?}|{masks:?}|{key}")
}
fn base_key(c: &Composite, sources: &Sources) -> Option<String> {
    match c.operations.first() {
        Some(
            op @ Operation::Image {
                displacement: None, ..
            },
        ) => Some(format!(
            "{}x{}|{op:?}|{}",
            c.width,
            c.height,
            source_key(op.get_assets(), sources)
        )),
        _ => None,
    }
}
fn add_work(work: &mut usize, area: usize, cost: usize) -> Result<(), String> {
    *work = work
        .checked_add(area.checked_mul(cost).ok_or("composite work overflow")?)
        .ok_or("composite work overflow")?;
    if *work > wire::MAX_WORK {
        return Err("composite candidate exceeds 268435456 work units".into());
    }
    Ok(())
}
impl CompositeCache {
    pub fn prepare(
        &mut self,
        composites: &[Composite],
        sources: &Sources,
    ) -> Result<(Vec<Arc<RenderImage>>, Stats), String> {
        self.live.retain(|_, (image, _)| image.strong_count() != 0);
        let live_bytes: usize = self.live.values().map(|(_, bytes)| bytes).sum();
        let mut stats = Stats::default();
        let mut plans = Vec::new();
        let mut new_bases = HashMap::new();
        let (mut additional, mut scratch) = (0usize, 0usize);
        for c in composites {
            // Destination, opacity, layer and ID do not affect local pixels.
            let source_key =
                source_key(c.operations.iter().flat_map(Operation::get_assets), sources);
            let key = format!("{}x{}|{:?}|{}", c.width, c.height, c.operations, source_key);
            let bytes = (c.width as usize + 4) * (c.height as usize + 4) * 4 + key.len();
            if let Some(key) = base_key(c, sources)
                && !self.bases.iter().any(|e| e.key == key)
            {
                let bytes = (c.width as usize + 4) * (c.height as usize + 4) * 4 + key.len();
                if bytes <= BASE_BYTES {
                    new_bases.insert(key, bytes);
                }
            }
            if !self.outputs.contains_key(&key) && !plans.iter().any(|p: &Plan<'_>| p.key == key) {
                additional = additional
                    .checked_add(bytes)
                    .ok_or("composite byte accounting overflow")?;
                // Two simultaneous coverage buffers (paint and displacement),
                // including temporary masks that cannot remain in the LRU.
                scratch = scratch.max(c.width as usize * c.height as usize * 2);
                add_work(&mut stats.work, c.width as usize * c.height as usize, 2)?;
                for op in &c.operations {
                    let bounds = PixelRect::intersect(op.get_bounds(), c.width, c.height);
                    let cost = match op {
                        Operation::Image { displacement, .. } => {
                            16 + usize::from(displacement.is_some()) * 8
                        }
                        Operation::Fill { .. } => 4,
                        Operation::Stroke { points, .. } => 4 + points.len(),
                    };
                    add_work(&mut stats.work, bounds.get_area(), cost)?;
                    let mut lists = vec![op.get_masks()];
                    if let Operation::Image {
                        displacement: Some(d),
                        ..
                    } = op
                    {
                        lists.push(&d.masks);
                    }
                    for masks in lists.into_iter().filter(|m| !m.is_empty()) {
                        // Charge every mask use cold: an earlier operation can
                        // evict a cached mask before this one executes. Cache
                        // hit/miss order must never bypass the work bound.
                        add_work(
                            &mut stats.work,
                            bounds.get_area(),
                            2 + masks.iter().map(Mask::get_work).sum::<usize>(),
                        )?;
                    }
                }
            }
            plans.push(Plan {
                composite: c,
                key,
                bytes,
            });
        }
        let total = live_bytes
            .checked_add(additional)
            .and_then(|n| n.checked_add(scratch))
            .and_then(|n| n.checked_add(MASK_BYTES))
            .and_then(|n| {
                n.checked_add((self.base_bytes + new_bases.values().sum::<usize>()).min(BASE_BYTES))
            })
            .ok_or("composite byte accounting overflow")?;
        if !composites.is_empty() && total > MAX_BYTES {
            return Err(
                "composite live outputs, candidate, masks and scratch exceed 64 MiB".into(),
            );
        }
        stats.reserved_bytes = total;
        let mut staged = HashMap::new();
        let mut images = Vec::new();
        for p in plans {
            let image = if let Some(image) = staged
                .get(&p.key)
                .or_else(|| self.outputs.get(&p.key))
                .cloned()
            {
                image
            } else {
                let image = self.rasterize(p.composite, sources, &mut stats)?;
                self.live
                    .insert(image.id, (Arc::downgrade(&image), p.bytes));
                stats.builds += 1;
                image
            };
            staged.insert(p.key, image.clone());
            images.push(image);
        }
        // Keep only this successful generation. Older snapshots remain counted
        // via Weak references; no elapsed-time animation history accumulates.
        self.outputs = staged;
        if composites.is_empty() {
            self.bases.clear();
            self.base_bytes = 0;
            self.masks.clear();
            self.mask_bytes = 0;
        }
        Ok((images, stats))
    }

    fn store_base(&mut self, key: &str, data: &[u8]) {
        let bytes = key.len() + data.len();
        if bytes > BASE_BYTES {
            return;
        }
        while self.base_bytes + bytes > BASE_BYTES || self.bases.len() >= 8 {
            let old = self.bases.pop_front().unwrap();
            self.base_bytes -= old.key.len() + old.pixels.len();
        }
        self.base_bytes += bytes;
        self.bases.push_back(MaskEntry {
            key: key.into(),
            pixels: Arc::new(data.to_vec()),
        });
    }

    fn prepare_mask(
        &mut self,
        masks: &[Mask],
        bounds: PixelRect,
        sources: &Sources,
        stats: &mut Stats,
    ) -> Option<Arc<Vec<u8>>> {
        if masks.is_empty() {
            return None;
        }
        let key = mask_key(masks, bounds, sources);
        if let Some(index) = self.masks.iter().position(|m| m.key == key) {
            let entry = self.masks.remove(index).unwrap();
            let pixels = entry.pixels.clone();
            self.masks.push_back(entry);
            return Some(pixels);
        }
        let mut data = Vec::with_capacity(bounds.get_area());
        for y in bounds.y..bounds.y + bounds.height {
            for x in bounds.x..bounds.x + bounds.width {
                let p = [x as f64 + 0.5, y as f64 + 0.5];
                let a = masks
                    .iter()
                    .map(|m| mask_alpha(m, p, sources))
                    .product::<f64>();
                data.push((a * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
        stats.mask_builds += 1;
        let pixels = Arc::new(data);
        let bytes = pixels.len() + key.len();
        if bytes <= MASK_BYTES {
            while self.mask_bytes + bytes > MASK_BYTES || self.masks.len() >= MAX_MASKS {
                let old = self.masks.pop_front().unwrap();
                self.mask_bytes -= old.pixels.len() + old.key.len();
            }
            self.mask_bytes += bytes;
            self.masks.push_back(MaskEntry {
                key,
                pixels: pixels.clone(),
            });
        }
        Some(pixels)
    }

    fn rasterize(
        &mut self,
        c: &Composite,
        sources: &Sources,
        stats: &mut Stats,
    ) -> Result<Arc<RenderImage>, String> {
        let stride = c.width + 2 * GUARD;
        let height = c.height + 2 * GUARD;
        let mut output = image::RgbaImage::new(stride, height);
        let data: &mut [u8] = output.as_mut();
        let base_key = base_key(c, sources);
        for (index, op) in c.operations.iter().enumerate() {
            let base_key = if index == 0 {
                base_key.as_deref()
            } else {
                None
            };
            if let Some(key) = base_key
                && let Some(index) = self.bases.iter().position(|e| e.key == key)
            {
                let entry = self.bases.remove(index).unwrap();
                data.copy_from_slice(&entry.pixels);
                self.bases.push_back(entry);
                continue;
            }
            let bounds = PixelRect::intersect(op.get_bounds(), c.width, c.height);
            let masks = self.prepare_mask(op.get_masks(), bounds, sources, stats);
            let displacement_masks = if let Operation::Image {
                displacement: Some(d),
                ..
            } = op
            {
                self.prepare_mask(&d.masks, bounds, sources, stats)
            } else {
                None
            };
            // A whole, unmodified background is a copy rather than four taps
            // and a blend per pixel; this is independent of any scene identity.
            if let Operation::Image {
                asset,
                source,
                destination,
                opacity,
                blend: wire::Blend::SourceOver,
                masks,
                displacement: None,
            } = op
            {
                let image = &sources[asset];
                let size = image.size(0);
                if *opacity == 1.0
                    && masks.is_empty()
                    && source.unwrap_or_else(wire::whole_source) == wire::whole_source()
                    && *destination
                        == (crate::canvas_protocol::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: c.width as f64,
                            height: c.height as f64,
                        })
                    && size.width.0 as u32 == c.width
                    && size.height.0 as u32 == c.height
                    && image
                        .as_bytes(0)
                        .unwrap()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[3] == 255)
                {
                    let source = image.as_bytes(0).unwrap();
                    for y in 0..c.height {
                        let offset = ((y + GUARD) * stride + GUARD) as usize * 4;
                        data[offset..offset + c.width as usize * 4].copy_from_slice(
                            &source[y as usize * c.width as usize * 4
                                ..(y + 1) as usize * c.width as usize * 4],
                        );
                    }
                    if let Some(key) = base_key {
                        self.store_base(key, data);
                    }
                    continue;
                }
            }
            for y in bounds.y..bounds.y + bounds.height {
                for x in bounds.x..bounds.x + bounds.width {
                    let i = ((y - bounds.y) * bounds.width + x - bounds.x) as usize;
                    let alpha =
                        op.get_opacity() * masks.as_ref().map_or(1.0, |m| m[i] as f64 / 255.0);
                    if alpha == 0.0 {
                        continue;
                    }
                    let p = [x as f64 + 0.5, y as f64 + 0.5];
                    let mut pixel = match op {
                        Operation::Image {
                            asset,
                            source,
                            destination,
                            displacement,
                            ..
                        } => {
                            let mut uv = [
                                (p[0] - destination.x) / destination.width,
                                (p[1] - destination.y) / destination.height,
                            ];
                            if let Some(d) = displacement {
                                let offset = pixels::displacement(d, uv);
                                let strength = displacement_masks
                                    .as_ref()
                                    .map_or(1.0, |m| m[i] as f64 / 255.0);
                                uv[0] += offset[0] * strength / destination.width;
                                uv[1] += offset[1] * strength / destination.height;
                            }
                            let image = &sources[asset];
                            let size = image.size(0);
                            let mut sample = pixels::sample(
                                image.as_bytes(0).unwrap(),
                                size.width.0 as u32,
                                size.height.0 as u32,
                                source.unwrap_or_else(wire::whole_source),
                                uv,
                            );
                            let coverage = pixels::rect_coverage(*destination, p) as f32;
                            for v in &mut sample {
                                *v *= coverage;
                            }
                            sample
                        }
                        Operation::Fill {
                            destination, brush, ..
                        } => {
                            let mut sample = pixels::brush(brush, p);
                            let coverage = pixels::rect_coverage(*destination, p) as f32;
                            for v in &mut sample {
                                *v *= coverage;
                            }
                            sample
                        }
                        Operation::Stroke {
                            points,
                            width,
                            brush,
                            ..
                        } => {
                            let d = points
                                .windows(2)
                                .map(|s| pixels::segment_distance(p, s[0], s[1]))
                                .fold(f64::INFINITY, f64::min);
                            let mut sample = pixels::brush(brush, p);
                            let coverage = (width / 2.0 + 0.5 - d).clamp(0.0, 1.0) as f32;
                            for v in &mut sample {
                                *v *= coverage;
                            }
                            sample
                        }
                    };
                    for v in &mut pixel {
                        *v *= alpha as f32;
                    }
                    if pixel[3] == 0.0 {
                        continue;
                    }
                    let offset = ((y + GUARD) * stride + x + GUARD) as usize * 4;
                    let target = &mut data[offset..offset + 4];
                    pixels::write(
                        target,
                        pixels::blend(pixel, pixels::unpack(target), op.get_blend()),
                    );
                }
            }
            if let Some(key) = base_key {
                self.store_base(key, data);
            }
        }
        // Clamp output guards, just like normal cached canvas image regions.
        for y in 0..height {
            for x in 0..stride {
                if x >= GUARD && x < c.width + GUARD && y >= GUARD && y < c.height + GUARD {
                    continue;
                }
                let sx = x.clamp(GUARD, c.width + GUARD - 1);
                let sy = y.clamp(GUARD, c.height + GUARD - 1);
                let src = ((sy * stride + sx) * 4) as usize;
                let dst = ((y * stride + x) * 4) as usize;
                let p: [u8; 4] = data[src..src + 4].try_into().unwrap();
                data[dst..dst + 4].copy_from_slice(&p);
            }
        }
        Ok(Arc::new(RenderImage::new(vec![image::Frame::new(output)])))
    }
}

fn mask_alpha(mask: &Mask, p: Point, sources: &Sources) -> f64 {
    match mask {
        Mask::Polygon {
            contours,
            feather,
            invert,
        } => {
            let d = contours
                .iter()
                .map(|c| pixels::polygon_distance(p, c))
                .fold(f64::NEG_INFINITY, f64::max);
            pixels::coverage(d, *feather, *invert)
        }
        Mask::Ellipse {
            center,
            radius,
            feather,
            invert,
        } => pixels::coverage(
            pixels::ellipse_distance(p, *center, *radius),
            *feather,
            *invert,
        ),
        Mask::ImageAlpha {
            asset,
            source,
            destination,
            invert,
        } => {
            let image = &sources[asset];
            let size = image.size(0);
            let uv = [
                (p[0] - destination.x) / destination.width,
                (p[1] - destination.y) / destination.height,
            ];
            let a = pixels::sample(
                image.as_bytes(0).unwrap(),
                size.width.0 as u32,
                size.height.0 as u32,
                source.unwrap_or_else(wire::whole_source),
                uv,
            )[3] as f64
                * pixels::rect_coverage(*destination, p);
            if *invert { 1.0 - a } else { a }
        }
    }
}
