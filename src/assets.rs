//! Resolve and decode PNGs before any presentation reaches the UI thread.
use gpui::RenderImage;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct AssetRoot(PathBuf);

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
        Ok(Self(root))
    }

    pub fn resolve(&self, asset: &Path) -> Result<PathBuf, String> {
        if asset.is_absolute() || asset.as_os_str().is_empty() {
            return Err("asset must be a nonempty relative path".into());
        }
        let resolved = self
            .0
            .join(asset)
            .canonicalize()
            .map_err(|e| format!("cannot resolve asset: {e}"))?;
        if !resolved.starts_with(&self.0) {
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
        let file = fs::File::open(path).map_err(|e| format!("cannot open asset: {e}"))?;
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1)
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
        Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
