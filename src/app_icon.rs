//! The game's application icon, shown in place of the renderer's own. The
//! renderer is shared by every game, so the icon arrives with the session.
use crate::assets::AssetRoot;
use std::path::Path;

/// Icon file types a hello may name.
pub const FORMATS: [&str; 2] = ["png", "icns"];

pub fn validate(icon: &Path) -> Result<(), String> {
    let supported = icon
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| FORMATS.contains(&extension.to_ascii_lowercase().as_str()));
    if icon.as_os_str().is_empty()
        || icon.is_absolute()
        || icon.as_os_str().len() > crate::protocol::MAX_TILE_ASSET_BYTES
        || !supported
    {
        return Err("icon must be a relative PNG or ICNS path inside assetRoot".into());
    }
    Ok(())
}

/// Resolves the icon inside the asset root and applies it. On failure the
/// caller reports it and the renderer keeps its own icon.
pub fn apply(asset_root: &Path, icon: &Path) -> Result<(), String> {
    let path = AssetRoot::new(asset_root)?.resolve(icon)?;
    set_application_icon(&path)
}

#[cfg(target_os = "macos")]
// The objc 0.2 message macros test a legacy `cargo-clippy` feature.
#[allow(unexpected_cfgs)]
fn set_application_icon(path: &Path) -> Result<(), String> {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::{class, msg_send, sel, sel_impl};

    let text = path.to_str().ok_or("icon path is not valid UTF-8")?;
    // SAFETY: called on the application's main thread after launch; every
    // object created here is released once NSApplication has retained it.
    unsafe {
        let file: id = NSString::alloc(nil).init_str(text);
        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithContentsOfFile: file];
        let _: () = msg_send![file, release];
        if image == nil {
            return Err(format!("{} is not a readable image", path.display()));
        }
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        let _: () = msg_send![image, release];
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn set_application_icon(_: &Path) -> Result<(), String> {
    Err("application icons are not supported on this platform yet".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_relative_png_or_icns_paths() {
        for valid in ["Graphics/System/Game.icns", "Game.PNG"] {
            assert!(validate(Path::new(valid)).is_ok(), "{valid}");
        }
        for invalid in ["", "/Game.png", "Game.txt", "Game"] {
            assert!(validate(Path::new(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn an_icon_outside_the_asset_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("Game.png"), b"png").unwrap();
        let error = apply(&dir.path().join("assets"), Path::new("../Game.png")).unwrap_err();
        assert!(error.contains("escapes"), "{error}");
    }
}
