use crate::{
    canvas_image::get_paint_bounds,
    canvas_protocol::Rect,
    display_cache::{DisplayRasterCache, RasterKey},
    protocol::Grid,
    state::PreparedFrame,
    viewport::{PaintRect, ViewportTransform},
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

fn get_device_bounds(rect: PaintRect, dpi: f32) -> PaintRect {
    // The public GPUI image painter floors origin and ceils extent after its
    // logical-to-device conversion. Assert the resulting bounds, not intent.
    PaintRect {
        left: (rect.left * dpi).floor(),
        top: (rect.top * dpi).floor(),
        width: (rect.width * dpi).ceil(),
        height: (rect.height * dpi).ceil(),
    }
}

#[test]
fn static_and_composite_surfaces_keep_registration_across_resolution_scale_and_dpi() {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(1618, 1024, |x, y| {
        image::Rgba([
            (((x as f64 + 0.5) / 1618.0) * 255.0).round() as u8,
            (((y as f64 + 0.5) / 1024.0) * 255.0).round() as u8,
            90,
            255,
        ])
    })
    .save(dir.path().join("replaceable.png"))
    .unwrap();
    let destination = Rect {
        x: 409.0,
        y: 16.0,
        width: 532.0,
        height: 336.69221260815823,
    };
    let destination_json = json!({"x":destination.x,"y":destination.y,"width":destination.width,"height":destination.height});
    let grid = Grid {
        columns: 135,
        rows: 36,
        cell_width: 10,
        cell_height: 20,
    };
    let assets = crate::assets::AssetRoot::new(dir.path()).unwrap();
    let static_frame = PreparedFrame::prepare_v2(serde_json::from_value(json!({
        "frame":1,"textLayers":[],"sprites":[],"canvas":{"width":1350,"height":720,
        "images":[{"id":"image","asset":"replaceable.png","destination":destination_json,"layer":0}]}
    })).unwrap(),grid,&assets).unwrap();
    let composite_frame = PreparedFrame::prepare_v2(serde_json::from_value(json!({
        "frame":2,"textLayers":[],"sprites":[],"canvas":{"width":1350,"height":720,
        "composites":[{"id":"effect","width":532,"height":337,"destination":destination_json,"layer":0,
        "operations":[{"type":"image","asset":"replaceable.png","destination":{"x":0,"y":0,"width":532,"height":337}}]}]}
    })).unwrap(),grid,&assets).unwrap();
    let original = &static_frame.canvas.as_ref().unwrap().images[0];
    let composited = &composite_frame.canvas.as_ref().unwrap().composites[0];
    let clip = Rect {
        x: 425.25,
        y: 30.125,
        width: 460.5,
        height: 285.75,
    };
    for fit in [1.0, 0.8, 0.6666667, 0.5, 0.125] {
        let transform = ViewportTransform::fit(1350., 720., 1350. * fit, 720. * fit);
        for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let mut samples = DisplayRasterCache::default();
            let origin = (31.25, 9.375);
            let (mask, full) = get_paint_bounds(destination, None, transform, origin).unwrap();
            let (clipped, unclipped_content) =
                get_paint_bounds(destination, Some(clip), transform, origin).unwrap();
            // Clip reveals the original mapping and never scales/recenters it.
            for (a, b) in [
                (full.left, unclipped_content.left),
                (full.top, unclipped_content.top),
            ] {
                assert!((a - b).abs() < 0.0001);
            }
            assert!(clipped.width < mask.width && clipped.height < mask.height);
            let (before, before_bounds) = samples.prepare(original, full, dpi);
            let (during, during_bounds) = samples.prepare(composited, full, dpi);
            let (after, after_bounds) = samples.prepare(original, full, dpi);
            assert!(Arc::ptr_eq(&before, &after));
            let expected = get_device_bounds(before_bounds, dpi);
            assert_eq!(get_device_bounds(during_bounds, dpi), expected);
            assert_eq!(get_device_bounds(after_bounds, dpi), expected);
            assert_eq!(expected.width, before.size(0).width.0 as f32);
            assert_eq!(expected.height, before.size(0).height.0 as f32);
            // A linear coordinate ramp detects translation/stretch; two
            // resampling stages can differ by at most one 8-bit rounding step.
            for (a, b) in before
                .as_bytes(0)
                .unwrap()
                .iter()
                .zip(during.as_bytes(0).unwrap())
            {
                assert!(a.abs_diff(*b) <= 1, "fit={fit} dpi={dpi}: {a} vs {b}");
            }
        }
    }
}

#[test]
fn canvas_samples_survive_unchanged_glyph_preparation_and_retire_with_source() {
    let region = Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::from_pixel(12, 12, image::Rgba([20, 40, 60, 255])),
    )]));
    let mut cache = DisplayRasterCache::default();
    let bounds = PaintRect {
        left: 0.25,
        top: 2.5,
        width: 16.0,
        height: 12.0,
    };
    let (first, _) = cache.prepare(&region, bounds, 1.25);
    let glyph = RasterKey::Glyph("retained glyph".into());
    cache
        .reserve_glyphs(&HashMap::from([(glyph, 64)]), 0)
        .unwrap();
    let (same, _) = cache.prepare(&region, bounds, 1.25);
    assert!(Arc::ptr_eq(&first, &same));
    assert!(
        cache
            .begin_frame(&HashSet::from([region.id]), &HashSet::new())
            .is_empty()
    );
    let retired = cache.begin_frame(&HashSet::new(), &HashSet::new());
    assert!(retired.iter().any(|image| image.id == first.id));
}
