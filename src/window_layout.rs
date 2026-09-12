//! Initial native geometry is independent of the immutable protocol grid.
use crate::{diagnostics::Diagnostics, protocol::Grid};
use gpui::{App, Bounds, DisplayId, Pixels};

pub struct InitialWindow {
    pub bounds: Bounds<Pixels>,
    pub display_id: DisplayId,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug)]
struct ScreenRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// AppKit uses global bottom-left coordinates; GPUI 0.2.2 MacWindow::open takes
/// a display-relative OUTER top-left and a CONTENT size. MacDisplay::bounds
/// discards display origins, so do not use it for this conversion.
#[cfg(any(target_os = "macos", test))]
fn centered_origin(
    screen: ScreenRect,
    work: ScreenRect,
    outer_width: f64,
    outer_height: f64,
) -> (f64, f64) {
    (
        work.x - screen.x + (work.width - outer_width) / 2.0,
        screen.y + screen.height - (work.y + work.height) + (work.height - outer_height) / 2.0,
    )
}

#[cfg(target_os = "macos")]
pub fn initial_window(
    grid: Grid,
    cx: &App,
    diagnostics: &Diagnostics,
) -> Result<InitialWindow, String> {
    macos::initial_window(grid, cx, diagnostics)
}

#[cfg(not(target_os = "macos"))]
pub fn initial_window(_: Grid, _: &App, _: &Diagnostics) -> Result<InitialWindow, String> {
    // Pinned GPUI exposes full display bounds, not the work area. Until a native
    // adapter exists, fail explicitly instead of claiming an oversized window fits.
    Err("usable display area fitting is currently implemented only on macOS".into())
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2.7 macros use the legacy cargo-clippy cfg.
mod macos {
    use super::*;
    use crate::viewport::ViewportTransform;
    use cocoa::{
        appkit::{NSScreen, NSWindow, NSWindowStyleMask},
        base::{id, nil},
        foundation::{NSArray, NSAutoreleasePool, NSDictionary, NSPoint, NSRect, NSSize, NSString},
    };
    use gpui::{point, px, size};
    use objc::{class, msg_send, sel, sel_impl};

    fn rect(r: NSRect) -> ScreenRect {
        ScreenRect {
            x: r.origin.x,
            y: r.origin.y,
            width: r.size.width,
            height: r.size.height,
        }
    }

    pub(super) fn initial_window(
        grid: Grid,
        cx: &App,
        diagnostics: &Diagnostics,
    ) -> Result<InitialWindow, String> {
        // Called on GPUI's AppKit main thread after Application initialization.
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let result = fit(grid, cx, diagnostics);
            pool.drain();
            result
        }
    }

    unsafe fn fit(
        grid: Grid,
        cx: &App,
        diagnostics: &Diagnostics,
    ) -> Result<InitialWindow, String> {
        unsafe {
            let mut screen = NSScreen::mainScreen(nil);
            if screen == nil {
                let screens = NSScreen::screens(nil);
                if NSArray::count(screens) == 0 {
                    return Err("no native display is available".into());
                }
                screen = screens.objectAtIndex(0);
            }
            let key = NSString::alloc(nil).init_str("NSScreenNumber");
            let description = screen.deviceDescription();
            let number = description.objectForKey_(key);
            let _: () = msg_send![key, release];
            if number == nil {
                return Err("native screen has no display identifier".into());
            }
            let native_id: u32 = msg_send![number, unsignedIntValue];
            let display_id = cx
                .displays()
                .into_iter()
                .map(|d| d.id())
                .find(|d| u32::from(*d) == native_id)
                .ok_or("native screen is not available in GPUI")?;
            let screen_frame = NSScreen::frame(screen);
            let work = screen.visibleFrame();
            // Match our normal opaque titlebar and GPUI's resizable/minimizable mask.
            let style = NSWindowStyleMask::NSTitledWindowMask
                | NSWindowStyleMask::NSClosableWindowMask
                | NSWindowStyleMask::NSResizableWindowMask
                | NSWindowStyleMask::NSMiniaturizableWindowMask;
            let window_class = class!(NSWindow) as *const _ as id;
            let available = window_class.contentRectForFrameRect_styleMask_(work, style);
            if !available.size.width.is_finite()
                || !available.size.height.is_finite()
                || available.size.width < 1.0
                || available.size.height < 1.0
            {
                return Err("native display has no usable window content area".into());
            }
            let logical_width = (grid.columns * grid.cell_width) as f32;
            let logical_height = (grid.rows * grid.cell_height) as f32;
            let fitted = ViewportTransform::fit(
                logical_width,
                logical_height,
                available.size.width.floor() as f32,
                available.size.height.floor() as f32,
            );
            // Round inward to whole AppKit points; painting uses the actual viewport
            // to preserve aspect ratio even if rounding leaves sub-point letterboxing.
            let width = f64::from(fitted.presented_width.floor());
            let height = f64::from(fitted.presented_height.floor());
            if width < 1.0 || height < 1.0 {
                return Err("fitted window content is empty".into());
            }
            let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
            let outer = window_class.frameRectForContentRect_styleMask_(content, style);
            let (x, y) = centered_origin(
                rect(screen_frame),
                rect(work),
                outer.size.width,
                outer.size.height,
            );
            // Fractional origins make AppKit round its initial content rectangle
            // outward, adding a point to its size. Align the top-left as well.
            let (x, y) = (x.floor(), y.floor());
            diagnostics.geometry("initial_window", || {
                vec![
                    ("display_id", native_id.into()),
                    ("screen_x", screen_frame.origin.x),
                    ("screen_y", screen_frame.origin.y),
                    ("screen_width", screen_frame.size.width),
                    ("screen_height", screen_frame.size.height),
                    ("work_x", work.origin.x),
                    ("work_y", work.origin.y),
                    ("work_width", work.size.width),
                    ("work_height", work.size.height),
                    ("available_content_width", available.size.width),
                    ("available_content_height", available.size.height),
                    ("logical_width", logical_width.into()),
                    ("logical_height", logical_height.into()),
                    ("content_width", width),
                    ("content_height", height),
                    ("outer_width", outer.size.width),
                    ("outer_height", outer.size.height),
                    ("display_relative_left", x),
                    ("display_relative_top", y),
                    (
                        "scale",
                        ViewportTransform::fit(
                            logical_width,
                            logical_height,
                            width as f32,
                            height as f32,
                        )
                        .scale
                        .into(),
                    ),
                ]
            });
            Ok(InitialWindow {
                bounds: Bounds::new(
                    point(px(x as f32), px(y as f32)),
                    size(px(width as f32), px(height as f32)),
                ),
                display_id,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fitted_outer_frame_centers_inside_work_area_in_screen_relative_coordinates() {
        for (x, y) in [(0.0, 0.0), (-1920.0, 300.0), (2560.0, -1080.0)] {
            let screen = ScreenRect {
                x,
                y,
                width: 1920.0,
                height: 1080.0,
            };
            let work = ScreenRect {
                x: x + 60.0,
                y: y + 40.0,
                width: 1860.0,
                height: 1010.0,
            };
            assert_eq!(centered_origin(screen, work, 1350.0, 750.0), (315.0, 160.0));
            assert_eq!(centered_origin(screen, work, 1860.0, 1010.0), (60.0, 30.0));
        }
    }
}
