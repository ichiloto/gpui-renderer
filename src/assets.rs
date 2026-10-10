//! Resolve and decode PNGs before any presentation reaches the UI thread.
use gpui::{ImageId, RenderImage};
use std::collections::VecDeque;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHED_IMAGES: usize = 1024;
const MAX_TONED_BYTES: usize = 64 * 1024 * 1024;
const MAX_TONED_IMAGES: usize = 1024;
const PNG_HEADER_BYTES: usize = 33;

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    header: [u8; PNG_HEADER_BYTES],
}

#[derive(Debug)]
struct CachedImage {
    path: PathBuf,
    stamp: FileStamp,
    image: Arc<RenderImage>,
}

#[derive(Debug, Default)]
struct ImageCache {
    entries: VecDeque<CachedImage>,
    decoded_bytes: usize,
}

impl ImageCache {
    fn get(&mut self, path: &Path, stamp: &FileStamp) -> Option<Arc<RenderImage>> {
        let index = self.entries.iter().position(|entry| entry.path == path)?;
        let entry = self.entries.remove(index)?;
        if entry.stamp != *stamp {
            self.decoded_bytes -= entry.image.as_bytes(0).unwrap().len();
            return None;
        }
        let image = entry.image.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    fn insert(&mut self, entry: CachedImage, byte_limit: usize, count_limit: usize) {
        let bytes = entry.image.as_bytes(0).unwrap().len();
        if bytes > byte_limit || count_limit == 0 {
            return;
        }
        while self.decoded_bytes + bytes > byte_limit || self.entries.len() >= count_limit {
            let oldest = self.entries.pop_front().unwrap();
            self.decoded_bytes -= oldest.image.as_bytes(0).unwrap().len();
        }
        self.decoded_bytes += bytes;
        self.entries.push_back(entry);
    }
}

#[derive(Debug)]
struct ImageVariant {
    source: ImageId,
    brightness: u64,
    flip_x: bool,
    flip_y: bool,
    image: Arc<RenderImage>,
}

/// Reuse immutable color variants across frames without retaining every
/// requested brightness value indefinitely. The original guarded crop remains
/// in the region cache and never needs to be decoded or sampled again.
#[derive(Debug, Default)]
struct ImageVariantCache {
    entries: VecDeque<ImageVariant>,
    bytes: usize,
}

impl ImageVariantCache {
    fn prepare(
        &mut self,
        source: &Arc<RenderImage>,
        brightness: f64,
        flip_x: bool,
        flip_y: bool,
    ) -> Result<Arc<RenderImage>, String> {
        if !brightness.is_finite() || !(0.0..=1.0).contains(&brightness) {
            return Err("canvas brightness must be finite and between 0 and 1".into());
        }
        if brightness == 1.0 && !flip_x && !flip_y {
            return Ok(source.clone());
        }
        let key = if brightness == 0.0 {
            0.0f64.to_bits()
        } else {
            brightness.to_bits()
        };
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.source == source.id
                && entry.brightness == key
                && entry.flip_x == flip_x
                && entry.flip_y == flip_y
        }) {
            let entry = self.entries.remove(index).unwrap();
            let image = entry.image.clone();
            self.entries.push_back(entry);
            return Ok(image);
        }

        // Source pixels are already in GPUI's BGRA order. Multiplying the
        // three color bytes equally leaves transparency and edge guards intact.
        let size = source.size(0);
        let mut bytes = source
            .as_bytes(0)
            .ok_or("prepared canvas image has no pixels")?
            .to_vec();
        for pixel in bytes.as_chunks_mut::<4>().0 {
            for channel in &mut pixel[..3] {
                *channel = (f64::from(*channel) * brightness).round() as u8;
            }
        }
        let mut pixels =
            image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)
                .ok_or("prepared canvas image has invalid pixel dimensions")?;
        if flip_x {
            image::imageops::flip_horizontal_in_place(&mut pixels);
        }
        if flip_y {
            image::imageops::flip_vertical_in_place(&mut pixels);
        }
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
        let length = image.as_bytes(0).unwrap().len();
        if length <= MAX_TONED_BYTES {
            while self.bytes + length > MAX_TONED_BYTES || self.entries.len() >= MAX_TONED_IMAGES {
                let oldest = self.entries.pop_front().unwrap();
                self.bytes -= oldest.image.as_bytes(0).unwrap().len();
            }
            self.bytes += length;
            self.entries.push_back(ImageVariant {
                source: source.id,
                brightness: key,
                flip_x,
                flip_y,
                image: image.clone(),
            });
        }
        Ok(image)
    }
}

#[derive(Clone, Debug)]
pub struct AssetRoot {
    path: PathBuf,
    cache: Arc<Mutex<ImageCache>>,
    regions: Arc<Mutex<crate::tile_regions::RegionCache>>,
    variants: Arc<Mutex<ImageVariantCache>>,
    decodes: Arc<AtomicU64>,
    composites: Arc<Mutex<crate::composite_cache::CompositeCache>>,
}

impl AssetRoot {
    pub fn new(root: &Path) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("assetRoot must be absolute".into());
        }
        let root = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve assetRoot: {e}"))?;
        if !root.is_dir() {
            return Err("assetRoot must be a directory".into());
        }
        Ok(Self {
            path: root,
            cache: Arc::default(),
            regions: Arc::default(),
            variants: Arc::default(),
            decodes: Arc::default(),
            composites: Arc::default(),
        })
    }

    pub fn resolve(&self, asset: &Path) -> Result<PathBuf, String> {
        if asset.is_absolute() || asset.as_os_str().is_empty() {
            return Err("asset must be a nonempty relative path".into());
        }
        let resolved = self
            .path
            .join(asset)
            .canonicalize()
            .map_err(|e| format!("cannot resolve asset: {e}"))?;
        if !resolved.starts_with(&self.path) {
            return Err("asset escapes assetRoot".into());
        }
        if !resolved.is_file() {
            return Err("asset must be a regular file".into());
        }
        Ok(resolved)
    }

    pub fn load(&self, asset: &Path) -> Result<Arc<RenderImage>, String> {
        let path = self.resolve(asset)?;
        // Only a validated path is opened. GPUI receives decoded bytes, never a path/URL.
        let mut file = fs::File::open(&path).map_err(|e| format!("cannot open asset: {e}"))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("cannot inspect asset: {e}"))?;
        if metadata.len() > 16 * 1024 * 1024 {
            return Err("PNG exceeds 16 MiB encoded limit".into());
        }
        // Current dimensions can change while length and timestamps stay equal.
        // Probe the signature and complete IHDR on hits; decoded pixels remain shared.
        let mut header = [0; PNG_HEADER_BYTES];
        file.read_exact(&mut header)
            .map_err(|e| format!("invalid PNG header: {e}"))?;
        let stamp = FileStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            header,
        };
        if let Some(image) = self.cache.lock().unwrap().get(&path, &stamp) {
            return Ok(image);
        }
        let mut bytes = header.to_vec();
        file.take(16 * 1024 * 1024 + 1 - header.len() as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read asset: {e}"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("PNG exceeds 16 MiB encoded limit".into());
        }
        let mut reader =
            image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let mut pixels = reader
            .decode()
            .map_err(|e| format!("invalid PNG: {e}"))?
            .into_rgba8();
        // GPUI 0.2.2 RenderImage's documented channel order is BGRA.
        for pixel in pixels.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
        self.decodes.fetch_add(1, Ordering::Relaxed);
        self.cache.lock().unwrap().insert(
            CachedImage {
                path,
                stamp,
                image: image.clone(),
            },
            MAX_DECODED_BYTES,
            MAX_CACHED_IMAGES,
        );
        Ok(image)
    }

    /// Retain this bounded cache generation with the snapshot. The UI can retire
    /// old atlas entries when it next draws without racing queued snapshots.
    pub fn cached_images(&self) -> Vec<Arc<RenderImage>> {
        self.cache
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.image.clone())
            .collect()
    }

    pub fn prepare_region(
        &self,
        atlas: &Arc<RenderImage>,
        rect: crate::protocol::SourceRect,
    ) -> Result<Arc<RenderImage>, String> {
        self.regions.lock().unwrap().prepare(atlas, rect)
    }

    pub fn cached_regions(&self) -> Vec<Arc<RenderImage>> {
        self.regions.lock().unwrap().images()
    }

    pub fn prepare_canvas_image(
        &self,
        source: &Arc<RenderImage>,
        brightness: f64,
        flip_x: bool,
        flip_y: bool,
    ) -> Result<Arc<RenderImage>, String> {
        self.variants
            .lock()
            .unwrap()
            .prepare(source, brightness, flip_x, flip_y)
    }

    pub fn preparation_totals(&self) -> (u64, u64) {
        (
            self.decodes.load(Ordering::Relaxed),
            self.regions.lock().unwrap().builds,
        )
    }

    pub fn prepare_composites(
        &self,
        composites: &[crate::composite_protocol::Composite],
        sources: &crate::composite_cache::Sources,
    ) -> Result<(Vec<Arc<RenderImage>>, crate::composite_cache::Stats), String> {
        self.composites.lock().unwrap().prepare(composites, sources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_cache_reuses_color_variants_and_keeps_alpha_and_sources_unchanged() {
        let source = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_fn(2, 1, |x, _| {
                image::Rgba(if x == 0 {
                    [200, 101, 50, 37]
                } else {
                    [12, 34, 56, 255]
                })
            }),
        )]));
        let original = source.as_bytes(0).unwrap().to_vec();
        let mut cache = ImageVariantCache::default();
        assert!(Arc::ptr_eq(
            &source,
            &cache.prepare(&source, 1.0, false, false).unwrap()
        ));
        let toned = cache.prepare(&source, 0.6, false, false).unwrap();
        assert_eq!(
            toned.as_bytes(0).unwrap(),
            &[120, 61, 30, 37, 7, 20, 34, 255]
        );
        assert_eq!(source.as_bytes(0).unwrap(), original);
        assert!(Arc::ptr_eq(
            &toned,
            &cache.prepare(&source, 0.6, false, false).unwrap()
        ));
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, toned.as_bytes(0).unwrap().len());
        assert_eq!(
            cache
                .prepare(&source, 0.0, false, false)
                .unwrap()
                .as_bytes(0)
                .unwrap(),
            &[0, 0, 0, 37, 0, 0, 0, 255]
        );
        for invalid in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
            assert!(cache.prepare(&source, invalid, false, false).is_err());
        }
    }

    #[test]
    fn tone_cache_evicts_old_variants_at_its_count_limit() {
        let source = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_pixel(1, 1, image::Rgba([100, 150, 200, 90])),
        )]));
        let mut cache = ImageVariantCache::default();
        let first = cache.prepare(&source, 0.001, false, false).unwrap();
        for i in 2..=MAX_TONED_IMAGES + 1 {
            cache
                .prepare(
                    &source,
                    i as f64 / (MAX_TONED_IMAGES + 2) as f64,
                    false,
                    false,
                )
                .unwrap();
        }
        assert_eq!(cache.entries.len(), MAX_TONED_IMAGES);
        assert!(cache.bytes <= MAX_TONED_BYTES);
        assert!(!cache.entries.iter().any(|entry| entry.image.id == first.id));
        // Prepared snapshots still own evicted variants until they retire.
        assert_eq!(first.as_bytes(0).unwrap(), &[0, 0, 0, 90]);
    }

    #[test]
    fn image_flips_preserve_alpha_and_reuse_cropped_color_variants() {
        let source = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_fn(2, 2, |x, y| {
                let value = (1 + x + 2 * y) as u8;
                image::Rgba([value * 20, value * 10, value * 5, value * 50])
            }),
        )]));
        let original = source.as_bytes(0).unwrap().to_vec();
        let mut cache = ImageVariantCache::default();
        for (flip_x, flip_y, order) in [
            (true, false, [2, 1, 4, 3]),
            (false, true, [3, 4, 1, 2]),
            (true, true, [4, 3, 2, 1]),
        ] {
            let flipped = cache.prepare(&source, 0.5, flip_x, flip_y).unwrap();
            let expected: Vec<u8> = order
                .into_iter()
                .flat_map(|v| {
                    [
                        v * 10,
                        v * 5,
                        ((v as f64 * 5.0) * 0.5).round() as u8,
                        v * 50,
                    ]
                })
                .collect();
            assert_eq!(flipped.as_bytes(0).unwrap(), expected);
            assert!(Arc::ptr_eq(
                &flipped,
                &cache.prepare(&source, 0.5, flip_x, flip_y).unwrap()
            ));
        }
        assert_eq!(source.as_bytes(0).unwrap(), original);
        assert_eq!(cache.entries.len(), 3);
    }
    #[test]
    fn cache_reuses_canonical_assets_and_reloads_changed_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sheet.png");
        image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let root = AssetRoot::new(dir.path()).unwrap();
        let first = root.load(Path::new("sheet.png")).unwrap();
        let alias = root.load(Path::new("./sheet.png")).unwrap();
        assert!(Arc::ptr_eq(&first, &alias));
        assert_eq!(root.cached_images().len(), 1);
        let old_len = fs::metadata(&path).unwrap().len();
        image::RgbaImage::from_fn(7, 9, |x, y| image::Rgba([x as u8, y as u8, 255, 255]))
            .save(&path)
            .unwrap();
        assert_ne!(fs::metadata(&path).unwrap().len(), old_len);
        let changed = root.load(Path::new("sheet.png")).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!((changed.size(0).width.0, changed.size(0).height.0), (7, 9));
        assert_eq!(root.cache.lock().unwrap().decoded_bytes, 7 * 9 * 4);
        // The prior snapshot still owns valid immutable bytes after eviction.
        assert_eq!((first.size(0).width.0, first.size(0).height.0), (4, 2));
        fs::remove_file(path).unwrap();
        assert!(root.load(Path::new("sheet.png")).is_err());
    }

    #[test]
    fn cache_reloads_current_headers_when_length_and_full_mtime_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sheet.png");
        let write_bytes = |bytes: &[u8]| {
            fs::write(&path, bytes).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
                ))
                .unwrap();
        };
        let write = |width, height, color| {
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba(color),
            ))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
            assert!(bytes.len() < 512);
            bytes.resize(512, 0);
            write_bytes(&bytes);
        };
        let facts = || {
            let metadata = fs::metadata(&path).unwrap();
            (metadata.len(), metadata.modified().unwrap())
        };
        write(4, 2, [255, 0, 0, 255]);
        let root = AssetRoot::new(dir.path()).unwrap();
        let first = root.load(Path::new("sheet.png")).unwrap();
        let first_facts = facts();
        let rect = crate::protocol::SourceRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        let first_region = root.prepare_region(&first, rect).unwrap();
        let first_tone = root
            .prepare_canvas_image(&first_region, 0.5, true, false)
            .unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &root.load(Path::new("./sheet.png")).unwrap()
        ));
        write(2, 4, [0, 0, 255, 255]);
        assert_eq!(facts(), first_facts);
        let changed = root.load(Path::new("sheet.png")).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!((changed.size(0).width.0, changed.size(0).height.0), (2, 4));
        assert_eq!(&changed.as_bytes(0).unwrap()[..4], &[255, 0, 0, 255]);
        let changed_region = root.prepare_region(&changed, rect).unwrap();
        let changed_tone = root
            .prepare_canvas_image(&changed_region, 0.5, true, false)
            .unwrap();
        assert!(!Arc::ptr_eq(&first_region, &changed_region));
        assert!(!Arc::ptr_eq(&first_tone, &changed_tone));
        assert!(Arc::ptr_eq(
            &changed,
            &root.load(Path::new("sheet.png")).unwrap()
        ));
        // Old prepared snapshots retain immutable pixels after cache invalidation.
        assert_eq!((first.size(0).width.0, first.size(0).height.0), (4, 2));
        assert_eq!(&first.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert_eq!(root.preparation_totals().0, 2);
        let mut invalid_mode = fs::read(&path).unwrap();
        invalid_mode[24] = 0;
        write_bytes(&invalid_mode);
        assert_eq!(facts(), first_facts);
        // Invalid bit depth must not hide behind unchanged signature/dimensions.
        assert!(root.load(Path::new("sheet.png")).is_err());
        let mut invalid = vec![0; 512];
        invalid[..8].copy_from_slice(b"BADIMAGE");
        write_bytes(&invalid);
        assert_eq!(facts(), first_facts);
        assert!(root.load(Path::new("sheet.png")).is_err());
        write(4, 2, [0, 255, 0, 255]);
        let repaired = root.load(Path::new("sheet.png")).unwrap();
        assert_eq!(
            (repaired.size(0).width.0, repaired.size(0).height.0),
            (4, 2)
        );
        assert_eq!(&repaired.as_bytes(0).unwrap()[..4], &[0, 255, 0, 255]);
        assert_eq!(root.cache.lock().unwrap().decoded_bytes, 4 * 2 * 4);
        assert_eq!(root.cached_images().len(), 1);
        assert_eq!(root.preparation_totals().0, 3);
    }

    #[test]
    fn cache_evicts_least_recent_images_with_byte_and_count_bounds() {
        let entry = |name: &str| CachedImage {
            path: name.into(),
            stamp: FileStamp {
                len: 16,
                modified: None,
                header: [0; PNG_HEADER_BYTES],
            },
            image: Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::new(2, 2),
            )])),
        };
        for (byte_limit, count_limit) in [(32, 1024), (64, 2)] {
            let mut cache = ImageCache::default();
            cache.insert(entry("a"), byte_limit, count_limit);
            cache.insert(entry("b"), byte_limit, count_limit);
            let a = cache
                .get(
                    Path::new("a"),
                    &FileStamp {
                        len: 16,
                        modified: None,
                        header: [0; PNG_HEADER_BYTES],
                    },
                )
                .unwrap();
            cache.insert(entry("c"), byte_limit, count_limit);
            assert_eq!(cache.decoded_bytes, 32);
            assert_eq!(
                cache
                    .entries
                    .iter()
                    .map(|e| e.path.to_str().unwrap())
                    .collect::<Vec<_>>(),
                ["a", "c"]
            );
            assert!(
                cache
                    .get(
                        Path::new("b"),
                        &FileStamp {
                            len: 16,
                            modified: None,
                            header: [0; PNG_HEADER_BYTES],
                        }
                    )
                    .is_none()
            );
            assert!(Arc::ptr_eq(
                &a,
                &cache
                    .get(
                        Path::new("a"),
                        &FileStamp {
                            len: 16,
                            modified: None,
                            header: [0; PNG_HEADER_BYTES],
                        }
                    )
                    .unwrap()
            ));
        }
        let mut cache = ImageCache::default();
        cache.insert(entry("too-large"), 15, 1);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.decoded_bytes, 0);
    }
    #[test]
    fn root_and_regular_file_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sprite.png");
        fs::write(&path, b"test").unwrap();
        let root = AssetRoot::new(dir.path()).unwrap();
        assert_eq!(
            root.resolve(Path::new("sprite.png")).unwrap(),
            path.canonicalize().unwrap()
        );
        assert!(root.resolve(Path::new(".")).is_err());
        assert!(AssetRoot::new(Path::new("relative")).is_err());
        assert!(AssetRoot::new(&path).is_err());
        assert!(
            root.load(Path::new("sprite.png"))
                .unwrap_err()
                .contains("invalid PNG")
        );
    }
    #[test]
    fn rejects_absolute_and_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::create_dir(dir.path().join("assets-other")).unwrap();
        let outside = dir.path().join("assets-other/secret.png");
        fs::write(&outside, b"test").unwrap();
        let root = AssetRoot::new(&dir.path().join("assets")).unwrap();
        assert!(root.resolve(&outside).is_err());
        assert!(
            root.resolve(Path::new("../assets-other/secret.png"))
                .unwrap_err()
                .contains("escapes")
        );
    }
    #[cfg(unix)]
    #[test]
    fn rejects_file_and_directory_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::create_dir(dir.path().join("outside")).unwrap();
        fs::write(dir.path().join("outside/secret.png"), b"test").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside"), dir.path().join("assets/link"))
            .unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside/secret.png"),
            dir.path().join("assets/file.png"),
        )
        .unwrap();
        let root = AssetRoot::new(&dir.path().join("assets")).unwrap();
        assert!(
            root.resolve(Path::new("link/secret.png"))
                .unwrap_err()
                .contains("escapes")
        );
        assert!(
            root.resolve(Path::new("file.png"))
                .unwrap_err()
                .contains("escapes")
        );
    }
    #[test]
    fn fixture_png_decodes_transparency_and_bgra() {
        let root = AssetRoot::new(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")).unwrap();
        let image = root.load(Path::new("test-sprite.png")).unwrap();
        assert_eq!(image.size(0).width.0, 32);
        assert_eq!(image.size(0).height.0, 48);
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[0, 0, 0, 0]);
    }
}
