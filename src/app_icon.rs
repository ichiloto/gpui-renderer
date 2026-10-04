//! The game's application icon, shown in place of the renderer's own. The
//! renderer is shared by every game, so the icon arrives with the session.
//! Authoring tools set their own icon through the same call
//! ([`set_application_icon`]). Only macOS takes an icon at run time: Windows
//! reads it from the executable's icon resource and Linux from the desktop
//! entry its window's app id names.
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
    let image = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    set_application_icon(&image).map_err(|error| format!("{}: {error}", path.display()))
}

/// Shows a PNG or ICNS image as the running application's icon, from the
/// main thread after launch.
#[cfg(target_os = "macos")]
// The objc 0.2 message macros test a legacy `cargo-clippy` feature.
#[allow(unexpected_cfgs)]
pub fn set_application_icon(image: &[u8]) -> Result<(), String> {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSData;
    use objc::{class, msg_send, sel, sel_impl};

    // SAFETY: called on the application's main thread after launch; the
    // data is copied, and every object created here is released once
    // NSApplication has retained the image.
    unsafe {
        let data: id = NSData::dataWithBytes_length_(nil, image.as_ptr().cast(), image.len() as _);
        let picture: id = msg_send![class!(NSImage), alloc];
        let picture: id = msg_send![picture, initWithData: data];
        if picture == nil {
            return Err("not a readable image".into());
        }
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: picture];
        let _: () = msg_send![picture, release];
    }
    Ok(())
}

/// Windows and Linux show the icon packaged with the application instead.
#[cfg(not(target_os = "macos"))]
pub fn set_application_icon(_: &[u8]) -> Result<(), String> {
    Err(
        "this platform shows the icon packaged with the application, not one set at run time"
            .into(),
    )
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
