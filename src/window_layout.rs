//! The desktop owns window placement; the renderer fits the actual content viewport.
use crate::{diagnostics::Diagnostics, protocol::Grid, viewport::ViewportTransform};
use gpui::{App, Bounds, DisplayId, Pixels, WindowBounds, point, px, size};

pub struct InitialWindow {
    pub bounds: WindowBounds,
    pub display_id: DisplayId,
}

pub fn initial_window(
    grid: Grid,
    cx: &App,
    diagnostics: &Diagnostics,
) -> Result<InitialWindow, String> {
    let display = cx
        .primary_display()
        .or_else(|| cx.displays().into_iter().next())
        .ok_or("no native display is available")?;
    let bounds = initial_bounds(grid, display.default_bounds())?;
    let restore = bounds.get_bounds();
    diagnostics.geometry("initial_window", || {
        vec![
            ("display_id", u32::from(display.id()).into()),
            ("maximized", 1.0),
            ("restore_left", f32::from(restore.origin.x).into()),
            ("restore_top", f32::from(restore.origin.y).into()),
            ("restore_width", f32::from(restore.size.width).into()),
            ("restore_height", f32::from(restore.size.height).into()),
            ("logical_width", (grid.columns * grid.cell_width).into()),
            ("logical_height", (grid.rows * grid.cell_height).into()),
        ]
    });
    Ok(InitialWindow {
        bounds,
        display_id: display.id(),
    })
}

fn initial_bounds(grid: Grid, suggested: Bounds<Pixels>) -> Result<WindowBounds, String> {
    let x = f32::from(suggested.origin.x);
    let y = f32::from(suggested.origin.y);
    let width = f32::from(suggested.size.width);
    let height = f32::from(suggested.size.height);
    if ![x, y, width, height].into_iter().all(f32::is_finite) || width < 1.0 || height < 1.0 {
        return Err("native display has no valid default window bounds".into());
    }
    let fitted = ViewportTransform::fit(
        (grid.columns * grid.cell_width) as f32,
        (grid.rows * grid.cell_height) as f32,
        width,
        height,
    );
    let restore_width = fitted.presented_width.round().max(1.0).min(width.floor());
    let restore_height = fitted.presented_height.round().max(1.0).min(height.floor());
    let restore = Bounds::new(
        point(
            px(x + (width - restore_width) / 2.0),
            px(y + (height - restore_height) / 2.0),
        ),
        size(px(restore_width), px(restore_height)),
    );

    // Display/default bounds are only a restore-size hint, never a work-area
    // measurement. GPUI delegates maximization to the native window manager on
    // macOS, Windows, X11 and Wayland (including WSLg). It supplies the real content
    // viewport after accounting for panels, decorations and display scaling.
    // Renderer::render fits that viewport on every resize. The player can restore,
    // move or resize the window without changing the logical grid or canvas.
    Ok(WindowBounds::Maximized(restore))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(columns: u32, rows: u32) -> Grid {
        Grid {
            columns,
            rows,
            cell_width: 10,
            cell_height: 20,
        }
    }

    #[test]
    fn desktop_controls_initial_area_and_restore_bounds_stay_centered() {
        for (x, y) in [(0.0, 0.0), (-1920.0, 300.0), (2560.0, -1080.0)] {
            let hint = Bounds::new(point(px(x), px(y)), size(px(1000.0), px(700.0)));
            let WindowBounds::Maximized(restore) = initial_bounds(grid(135, 36), hint).unwrap()
            else {
                panic!("initial sizing must be delegated to the desktop");
            };
            assert_eq!(restore.size, size(px(1000.0), px(533.0)));
            assert_eq!(restore.center(), hint.center());
        }
    }

    #[test]
    fn restore_geometry_handles_small_portrait_and_fractional_displays() {
        for (width, height) in [(320.0, 240.0), (600.5, 1000.25), (1.0, 1.0)] {
            let hint = Bounds::new(point(px(-320.5), px(17.25)), size(px(width), px(height)));
            for grid in [grid(135, 36), grid(2, 2), grid(1, 800), grid(800, 1)] {
                let restore = initial_bounds(grid, hint).unwrap().get_bounds();
                assert!(restore.size.width >= px(1.0) && restore.size.width <= hint.size.width);
                assert!(restore.size.height >= px(1.0) && restore.size.height <= hint.size.height);
                assert_eq!(restore.center(), hint.center());
            }
        }
        let hint = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(700.0)));
        assert_eq!(
            initial_bounds(grid(2, 2), hint).unwrap().get_bounds().size,
            size(px(20.0), px(40.0))
        );
    }

    #[test]
    fn invalid_display_metadata_is_an_error_independent_of_operating_system() {
        for (x, y, width, height) in [
            (0.0, 0.0, 0.0, 720.0),
            (0.0, 0.0, 1350.0, -1.0),
            (f32::NAN, 0.0, 1350.0, 720.0),
            (0.0, f32::INFINITY, 1350.0, 720.0),
            (0.0, 0.0, f32::INFINITY, 720.0),
        ] {
            let hint = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
            assert!(initial_bounds(grid(135, 36), hint).is_err());
        }
    }
}
