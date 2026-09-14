//! Cached source regions isolate bilinear sampling from neighbouring atlas tiles.
//! Derived pixels are prepared once per decoded atlas identity/rectangle, never
//! per destination cell. The original PNG cache and this cache have separate bounds.
use crate::protocol::SourceRect;
use gpui::{ImageId, RenderImage};
use std::collections::VecDeque;
use std::sync::Arc;

pub const GUARD: u32 = 2;
pub const MAX_REGION_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_REGIONS: usize = 4096;

#[derive(Debug)]
struct Entry {
    atlas: ImageId,
    rect: SourceRect,
    image: Arc<RenderImage>,
}

#[derive(Debug, Default)]
pub struct RegionCache {
    entries: VecDeque<Entry>,
    bytes: usize,
    pub builds: u64,
}

impl RegionCache {
    pub fn prepare(
        &mut self,
        atlas: &Arc<RenderImage>,
        rect: SourceRect,
    ) -> Result<Arc<RenderImage>, String> {
        let size = atlas.size(0);
        rect.validate_image(size.width.0 as u32, size.height.0 as u32)?;
        if let Some(index) = self
            .entries
            .iter()
            .position(|e| e.atlas == atlas.id && e.rect == rect)
        {
            let entry = self.entries.remove(index).unwrap();
            let image = entry.image.clone();
            self.entries.push_back(entry);
            return Ok(image);
        }
        let width = rect.width + 2 * GUARD;
        let height = rect.height + 2 * GUARD;
        let bytes = width as usize * height as usize * 4;
        if bytes > MAX_REGION_BYTES {
            return Err("source region exceeds 64 MiB prepared-region limit".into());
        }
        let source = atlas.as_bytes(0).ok_or("decoded atlas has no pixels")?;
        let mut pixels = image::RgbaImage::new(width, height);
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let sx = rect.x + x.saturating_sub(GUARD).min(rect.width - 1);
            let sy = rect.y + y.saturating_sub(GUARD).min(rect.height - 1);
            let offset = (sy as usize * size.width.0 as usize + sx as usize) * 4;
            // Source is already GPUI's BGRA, including unchanged alpha.
            pixel.0.copy_from_slice(&source[offset..offset + 4]);
        }
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
        self.builds += 1;
        self.insert(
            Entry {
                atlas: atlas.id,
                rect,
                image: image.clone(),
            },
            MAX_REGION_BYTES,
            MAX_REGIONS,
        );
        Ok(image)
    }

    fn insert(&mut self, entry: Entry, byte_limit: usize, count_limit: usize) {
        let bytes = entry.image.as_bytes(0).unwrap().len();
        if bytes > byte_limit || count_limit == 0 {
            return;
        }
        while self.bytes + bytes > byte_limit || self.entries.len() >= count_limit {
            self.bytes -= self
                .entries
                .pop_front()
                .unwrap()
                .image
                .as_bytes(0)
                .unwrap()
                .len();
        }
        self.bytes += bytes;
        self.entries.push_back(entry);
    }

    pub fn images(&self) -> Vec<Arc<RenderImage>> {
        self.entries.iter().map(|e| e.image.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn atlas() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_fn(4, 2, |x, y| {
                image::Rgba(if x < 2 {
                    [255, 0, y as u8, 128]
                } else {
                    [0, 255, y as u8, 255]
                })
            }),
        )]))
    }
    #[test]
    fn guards_copy_only_selected_pixels_and_alpha_and_reuse_across_cells_frames() {
        let atlas = atlas();
        let mut cache = RegionCache::default();
        let rect = SourceRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        let first = cache.prepare(&atlas, rect).unwrap();
        for _ in 0..4860 {
            assert!(Arc::ptr_eq(&first, &cache.prepare(&atlas, rect).unwrap()));
        }
        assert_eq!(cache.builds, 1);
        for pixel in first.as_bytes(0).unwrap().as_chunks::<4>().0 {
            assert_eq!((pixel[0], pixel[1], pixel[3]), (255, 0, 128));
        }
        assert_eq!((first.size(0).width.0, first.size(0).height.0), (6, 6));
        let other = cache.prepare(&atlas, SourceRect { x: 2, ..rect }).unwrap();
        assert_ne!(first.id, other.id);
        assert_eq!(&other.as_bytes(0).unwrap()[..4], &[0, 255, 0, 255]);
        let new_atlas = self::atlas();
        assert_ne!(cache.prepare(&new_atlas, rect).unwrap().id, first.id);
        assert_eq!(cache.builds, 3);
    }
    #[test]
    fn region_eviction_is_bounded_and_does_not_invalidate_prepared_snapshots() {
        let atlas = atlas();
        let rect = SourceRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        let mut cache = RegionCache::default();
        let retained = cache.prepare(&atlas, rect).unwrap();
        let weak = Arc::downgrade(&retained);
        let entry = Entry {
            atlas: atlas.id,
            rect: SourceRect { x: 2, ..rect },
            image: retained.clone(),
        };
        cache.insert(entry, 144, 1);
        assert_eq!(cache.bytes, 144);
        assert_eq!(cache.entries.len(), 1);
        cache.entries.clear();
        assert!(weak.upgrade().is_some());
        drop(retained);
        assert!(weak.upgrade().is_none());
    }
}
