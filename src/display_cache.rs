//! One device-raster LRU for terrain samples and runtime glyph-effect images.
use gpui::{ImageId, RenderImage};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Weak},
};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IMAGES: usize = crate::protocol::MAX_TILE_CELLS;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum RasterKey {
    Tile(TileKey),
    Glyph(Arc<str>),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TileKey {
    pub region: ImageId,
    pub width: u32,
    pub height: u32,
    pub phase_x: u32,
    pub phase_y: u32,
}

impl RasterKey {
    pub fn extra_bytes(&self) -> usize {
        match self {
            Self::Glyph(text) => text.len() + std::mem::size_of::<Self>(),
            Self::Tile(_) => 0,
        }
    }
}

pub(crate) struct Entry {
    image: Arc<RenderImage>,
    used: u64,
    bytes: usize,
}

struct LiveRaster {
    image: Weak<RenderImage>,
    bytes: usize,
}

#[derive(Default)]
pub struct DisplayRasterCache {
    pub(crate) entries: HashMap<RasterKey, Entry>,
    lru: BTreeMap<u64, RasterKey>,
    pub(crate) bytes: usize,
    clock: u64,
    pub(crate) retired: Vec<Arc<RenderImage>>,
    live: HashMap<ImageId, LiveRaster>,
}

impl DisplayRasterCache {
    pub fn begin_frame(
        &mut self,
        regions: &HashSet<ImageId>,
        glyphs: &HashSet<RasterKey>,
    ) -> Vec<Arc<RenderImage>> {
        self.live.retain(|_, entry| entry.image.strong_count() != 0);
        let stale: Vec<_> = self
            .entries
            .keys()
            .filter(|key| match key {
                RasterKey::Tile(key) => !regions.contains(&key.region),
                RasterKey::Glyph(_) => !glyphs.contains(*key),
            })
            .cloned()
            .collect();
        for key in stale {
            self.remove(&key);
        }
        std::mem::take(&mut self.retired)
    }

    pub fn get(&mut self, key: &RasterKey) -> Option<Arc<RenderImage>> {
        let entry = self.entries.get_mut(key)?;
        self.lru.remove(&entry.used);
        self.clock += 1;
        entry.used = self.clock;
        self.lru.insert(self.clock, key.clone());
        Some(entry.image.clone())
    }

    /// Check the whole candidate plus peak reusable scratch before allocating.
    /// LRU eviction does not release a raster held by a snapshot or retirement
    /// queue. Account those lifetimes too, without keeping them alive ourselves.
    pub fn reserve_glyphs(
        &mut self,
        wanted: &HashMap<RasterKey, usize>,
        scratch: usize,
    ) -> Result<usize, String> {
        let live_bytes = self.live_bytes();
        let new: Vec<_> = wanted
            .iter()
            .filter(|(key, _)| !self.entries.contains_key(*key))
            .collect();
        let total = new
            .iter()
            .try_fold(
                live_bytes
                    .checked_add(scratch)
                    .ok_or("glyph scratch accounting overflow")?,
                |sum, (_, bytes)| sum.checked_add(**bytes),
            )
            .ok_or("glyph raster byte accounting overflow")?;
        if total > MAX_BYTES || self.live.len() + new.len() > MAX_IMAGES {
            return Err(
                "glyph candidate and scratch exceed shared 64 MiB display-raster limit".into(),
            );
        }
        let stale: Vec<_> = self
            .entries
            .keys()
            .filter(|key| !wanted.contains_key(*key))
            .cloned()
            .collect();
        for key in stale {
            self.remove(&key);
        }
        Ok(total)
    }

    pub fn live_bytes(&mut self) -> usize {
        self.live.retain(|_, entry| entry.image.strong_count() != 0);
        self.live.values().map(|entry| entry.bytes).sum()
    }

    pub fn insert_with_limits(
        &mut self,
        key: RasterKey,
        image: Arc<RenderImage>,
        byte_limit: usize,
        count_limit: usize,
    ) {
        let bytes = image.as_bytes(0).unwrap().len() + key.extra_bytes();
        self.live.insert(
            image.id,
            LiveRaster {
                image: Arc::downgrade(&image),
                bytes,
            },
        );
        if bytes > byte_limit || count_limit == 0 {
            // Existing terrain-only oversized behavior. Glyph admission rejects
            // over-budget candidates before reaching this method.
            self.retired.push(image);
            return;
        }
        if self.entries.contains_key(&key) {
            self.remove(&key);
        }
        while self.bytes + bytes > byte_limit || self.entries.len() >= count_limit {
            self.remove(&self.lru.first_key_value().unwrap().1.clone());
        }
        self.clock += 1;
        self.bytes += bytes;
        self.lru.insert(self.clock, key.clone());
        self.entries.insert(
            key,
            Entry {
                image,
                used: self.clock,
                bytes,
            },
        );
    }

    fn remove(&mut self, key: &RasterKey) {
        let entry = self.entries.remove(key).unwrap();
        self.lru.remove(&entry.used);
        self.bytes -= entry.bytes;
        self.retired.push(entry.image);
    }
}
