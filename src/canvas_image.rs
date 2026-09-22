//! Guarded canvas surfaces share destination sampling, regardless of source size.
//! Keep authored clips out of per-image layout rounding; the existing bounded
//! device-raster cache compensates for GPUI's floor/ceil image painter.
use crate::{
    canvas::clipped_bounds,
    canvas_protocol::Rect,
    display_cache::DisplayRasterCache,
    viewport::{PaintRect, ViewportTransform},
};
use gpui::{
    Bounds, ContentMask, IntoElement, RenderImage, canvas, div, point, prelude::*, px, size,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub fn get_paint_bounds(
    destination: Rect,
    clip: Option<Rect>,
    transform: ViewportTransform,
    origin: (f32, f32),
) -> Option<(PaintRect, PaintRect)> {
    let (mut mask, _) = clipped_bounds(destination, clip, transform)?;
    mask.left += origin.0;
    mask.top += origin.1;
    // Derive the sampling origin directly, so changing a clip cannot even
    // change its floating-point phase through subtract/add cancellation.
    let mut destination = destination.paint(transform);
    destination.left += origin.0;
    destination.top += origin.1;
    Some((mask, destination))
}

pub fn element(
    image: Arc<RenderImage>,
    destination: Rect,
    clip: Option<Rect>,
    opacity: f64,
    transform: ViewportTransform,
    samples: Rc<RefCell<DisplayRasterCache>>,
) -> impl IntoElement {
    div().absolute().size_full().opacity(opacity as f32).child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let Some((mask, destination)) = get_paint_bounds(
                    destination,
                    clip,
                    transform,
                    (bounds.origin.x.into(), bounds.origin.y.into()),
                ) else {
                    return;
                };
                let (image, bounds) =
                    samples
                        .borrow_mut()
                        .prepare(&image, destination, window.scale_factor());
                let result = window.with_content_mask(
                    Some(ContentMask {
                        bounds: get_gpui_bounds(mask),
                    }),
                    |window| {
                        window.paint_image(
                            get_gpui_bounds(bounds),
                            Default::default(),
                            image,
                            0,
                            false,
                        )
                    },
                );
                if let Err(error) = result {
                    crate::protocol::diagnostic(format!("cannot paint canvas surface: {error}"));
                }
            },
        )
        .absolute()
        .size_full(),
    )
}

fn get_gpui_bounds(rect: PaintRect) -> Bounds<gpui::Pixels> {
    Bounds::new(
        point(px(rect.left), px(rect.top)),
        size(px(rect.width), px(rect.height)),
    )
}
