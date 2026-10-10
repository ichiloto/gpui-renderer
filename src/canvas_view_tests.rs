use crate::{
    canvas_protocol::Canvas,
    canvas_view::{CanvasAssets, CanvasPainter},
    display_cache::DisplayRasterCache,
    glyph_raster::FontCatalog,
    protocol::Grid,
    state::PreparedFrame,
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn grid() -> Grid {
    Grid {
        columns: 135,
        rows: 36,
        cell_width: 10,
        cell_height: 20,
    }
}

fn runtime(source: Canvas, assets: &crate::assets::AssetRoot) -> PreparedFrame {
    let mut frame: crate::protocol::FrameV2 = serde_json::from_value(json!({
        "frame": 1, "textLayers": [], "sprites": []
    }))
    .unwrap();
    frame.canvas = Some(source);
    PreparedFrame::prepare_v2(frame, grid(), assets).unwrap()
}

fn fixture_canvas(name: &str) -> Canvas {
    let frame: Value = serde_json::from_slice(&fs::read(root().join(name)).unwrap()).unwrap();
    serde_json::from_value(frame["canvas"].clone()).unwrap()
}

fn image_canvas(path: &str) -> Value {
    json!({"width": 128, "height": 96, "images": [{"id": "mutable", "asset": path,
        "destination": {"x": 0, "y": 0, "width": 32, "height": 48}, "layer": 0}]})
}

#[test]
fn public_canvas_preparation_matches_runtime_images_composites_and_paint_order() {
    let view_assets = CanvasAssets::new(&root()).unwrap();
    let runtime_assets = crate::assets::AssetRoot::new(&root()).unwrap();
    for fixture in [
        "graphical-canvas/valid-full-canvas.json",
        "compositing/frame.json",
    ] {
        let source = fixture_canvas(fixture);
        let view = view_assets.prepare_canvas(source.clone()).unwrap();
        let runtime = runtime(source.clone(), &runtime_assets);
        let canvas = runtime.canvas.as_ref().unwrap();
        assert_eq!(view.get_size(), (source.width, source.height));
        assert_eq!(view.canvas.source, canvas.source);
        assert_eq!(view.canvas.plan, canvas.plan);
        assert_eq!(view.canvas.images.len(), canvas.images.len());
        assert_eq!(view.canvas.composites.len(), canvas.composites.len());
        for (a, b) in view
            .canvas
            .images
            .iter()
            .chain(&view.canvas.composites)
            .zip(canvas.images.iter().chain(&canvas.composites))
        {
            assert_eq!(a.size(0), b.size(0));
            assert_eq!(a.as_bytes(0), b.as_bytes(0));
        }
        assert_eq!(
            view.get_resources().decoded_bytes,
            runtime.resources.decoded_bytes
        );
        assert_eq!(
            view.get_resources().region_bytes,
            runtime.resources.region_bytes
        );
    }
}

#[test]
fn public_canvas_uses_runtime_crop_tone_flip_and_clone_cache() {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(4, 2, |x, y| {
        image::Rgba([(x * 30) as u8, (y * 60) as u8, 80, 130])
    })
    .save(dir.path().join("sheet.png"))
    .unwrap();
    let mut source = image_canvas("sheet.png");
    source["images"][0]["sourceRect"] = json!({"x": 2, "y": 0, "width": 2, "height": 2});
    source["images"][0]["brightness"] = json!(0.5);
    source["images"][0]["flipX"] = json!(true);
    source["images"][0]["flipY"] = json!(true);
    let source: Canvas = serde_json::from_value(source).unwrap();
    let assets = CanvasAssets::new(dir.path()).unwrap();
    let view = assets.prepare_canvas(source.clone()).unwrap();
    let same = assets.clone().prepare_canvas(source.clone()).unwrap();
    assert!(Arc::ptr_eq(&view.canvas.images[0], &same.canvas.images[0]));
    assert_eq!(same.get_resources().png_decodes, 0);
    let runtime = runtime(source, &crate::assets::AssetRoot::new(dir.path()).unwrap());
    assert_eq!(
        view.canvas.images[0].as_bytes(0),
        runtime.canvas.unwrap().images[0].as_bytes(0)
    );
}

#[test]
fn public_canvas_crop_display_preserves_source_edges_without_showing_region_guards() {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(4, 1, |x, _| image::Rgba([x as u8 * 60, 20, 80, 255]))
        .save(dir.path().join("sheet.png"))
        .unwrap();
    let mut source = image_canvas("sheet.png");
    source["images"][0]["sourceRect"] = json!({"x": 1, "y": 0, "width": 2, "height": 1});
    let frame = CanvasAssets::new(dir.path())
        .unwrap()
        .prepare_canvas(serde_json::from_value(source).unwrap())
        .unwrap();
    let destination = crate::viewport::PaintRect {
        left: 0.0,
        top: 0.0,
        width: 2.0,
        height: 1.0,
    };
    let (display, bounds) =
        DisplayRasterCache::default().prepare(&frame.canvas.images[0], destination, 1.0);
    let width = display.size(0).width.0 as usize;
    let x = (-bounds.left) as usize;
    let y = (-bounds.top) as usize;
    let pixels = display.as_bytes(0).unwrap().as_chunks::<4>().0;
    // RenderImage stores device pixels in BGRA order.
    assert_eq!(pixels[y * width + x], [80, 20, 60, 255]);
    assert_eq!(pixels[y * width + x + 1], [80, 20, 120, 255]);
}

#[test]
fn invalid_public_candidates_do_not_change_the_retained_generation() {
    let assets = CanvasAssets::new(&root()).unwrap();
    let source = fixture_canvas("graphical-canvas/valid-full-canvas.json");
    let accepted = assets.prepare_canvas(source.clone()).unwrap();
    let pixels = accepted.canvas.images[0].as_bytes(0).unwrap().to_vec();
    for bad in ["../Cargo.toml", "/tmp/absolute.png", "missing.png"] {
        let mut rejected = source.clone();
        rejected.images[0].asset = PathBuf::from(bad);
        assert!(assets.prepare_canvas(rejected).is_err());
        assert_eq!(accepted.canvas.images[0].as_bytes(0).unwrap(), pixels);
        assert_eq!(accepted.canvas.source, source);
    }
    let mut rejected = source.clone();
    rejected.images[0].destination.width = f64::NAN;
    assert!(assets.prepare_canvas(rejected).is_err());
    let mut rejected = source;
    rejected.images[0].source_rect = Some(crate::protocol::SourceRect {
        x: 9999,
        y: 0,
        width: 1,
        height: 1,
    });
    assert!(assets.prepare_canvas(rejected).is_err());
}

#[test]
fn public_view_revalidates_replaced_art_without_hash_or_dimension_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replaceable.png");
    image::RgbaImage::from_pixel(2, 3, image::Rgba([80, 20, 40, 255]))
        .save(&path)
        .unwrap();
    let assets = CanvasAssets::new(dir.path()).unwrap();
    let source: Canvas = serde_json::from_value(image_canvas("replaceable.png")).unwrap();
    let original = assets.prepare_canvas(source.clone()).unwrap();
    image::RgbaImage::from_pixel(5, 7, image::Rgba([10, 180, 50, 255]))
        .save(&path)
        .unwrap();
    let replaced = assets.prepare_canvas(source.clone()).unwrap();
    assert_ne!(original.canvas.images[0].id, replaced.canvas.images[0].id);
    assert_ne!(
        original.canvas.images[0].size(0),
        replaced.canvas.images[0].size(0)
    );
    fs::write(&path, b"invalid PNG").unwrap();
    assert!(assets.prepare_canvas(source).is_err());
    assert_eq!(replaced.get_size(), (128, 96));
}

#[test]
fn public_view_enforces_the_same_combined_decode_budget() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["one.png", "two.png"] {
        image::RgbaImage::from_pixel(3000, 3000, image::Rgba([40, 80, 120, 255]))
            .save(dir.path().join(name))
            .unwrap();
    }
    let mut source = image_canvas("one.png");
    let mut second = source["images"][0].clone();
    second["id"] = json!("second");
    second["asset"] = json!("two.png");
    source["images"].as_array_mut().unwrap().push(second);
    let source: Canvas = serde_json::from_value(source).unwrap();
    let error = CanvasAssets::new(dir.path())
        .unwrap()
        .prepare_canvas(source)
        .unwrap_err();
    assert!(error.contains("64 MiB decoded"), "{error}");
}

#[test]
fn public_view_shares_glyph_rasters_with_runtime_and_releases_previous_images() {
    let source = fixture_canvas("canvas-glyph-effects/valid-default.json");
    let assets = CanvasAssets::new(&root()).unwrap();
    let view = assets.prepare_canvas(source.clone()).unwrap();
    let runtime = runtime(source, &crate::assets::AssetRoot::new(&root()).unwrap());
    for density in [0.5, 1.0, 2.0] {
        let a = crate::glyph_cache::prepare_canvas(
            &view.canvas,
            density,
            &mut FontCatalog::default(),
            &mut DisplayRasterCache::default(),
        )
        .unwrap();
        let b = crate::glyph_cache::prepare(
            &runtime,
            density,
            &mut FontCatalog::default(),
            &mut DisplayRasterCache::default(),
        )
        .unwrap();
        assert_eq!(a.keys, b.keys);
        assert_eq!(a.reserved_bytes, b.reserved_bytes);
        for (index, image) in a.images {
            assert_eq!(image.as_bytes(0), b.images[&index].as_bytes(0));
        }
    }
    let mut painter = CanvasPainter::default();
    assert!(
        painter
            .replace_cached_images(&view.canvas.images)
            .is_empty()
    );
    assert!(
        painter
            .replace_cached_images(&view.canvas.images)
            .is_empty()
    );
    let retired = painter.replace_cached_images(&[]);
    assert_eq!(retired.len(), view.canvas.images.len());
    assert!(painter.replace_cached_images(&[]).is_empty());
}

#[test]
fn public_view_evicts_old_cinematic_assets_without_invalidating_accepted_frames() {
    let dir = tempfile::tempdir().unwrap();
    let assets = CanvasAssets::new(dir.path()).unwrap();
    let mut first = None;
    let mut first_pixels = None;
    let mut last = None;
    let mut last_source = None;
    // Sixteen eight-pose atlases exceed the source, crop and tone cache budgets.
    // Keep one accepted frame while changing sequences, as a refused seek does.
    for sequence in 0..16 {
        let path = format!("sequence-{sequence}.png");
        image::RgbaImage::from_fn(512, 4096, |_, y| {
            image::Rgba([sequence * 10, (y / 512) as u8 * 20, 80, 180])
        })
        .save(dir.path().join(&path))
        .unwrap();
        for pose in 0..8 {
            let mut source = image_canvas(&path);
            source["images"][0]["sourceRect"] =
                json!({"x": 0, "y": pose * 512, "width": 512, "height": 512});
            source["images"][0]["brightness"] = json!(0.5);
            source["images"][0]["flipX"] = json!(pose % 2 == 1);
            let source: Canvas = serde_json::from_value(source).unwrap();
            let frame = assets.prepare_canvas(source.clone()).unwrap();
            assert_eq!(frame.get_resources().png_decodes, u64::from(pose == 0));
            assert_eq!(frame.get_resources().decoded_bytes, 512 * 4096 * 4);
            if first.is_none() {
                first_pixels = Some(frame.canvas.images[0].as_bytes(0).unwrap().to_vec());
                first = Some(frame.clone());
            }
            last = Some(frame);
            last_source = Some(source);
        }
    }
    let first = first.unwrap();
    let first_image = Arc::downgrade(&first.canvas.images[0]);
    let same = assets.clone().prepare_canvas(last_source.unwrap()).unwrap();
    let last = last.unwrap();
    assert_eq!(same.get_resources().png_decodes, 0);
    assert!(Arc::ptr_eq(&last.canvas.images[0], &same.canvas.images[0]));
    assert_eq!(
        first.canvas.images[0].as_bytes(0).unwrap(),
        first_pixels.unwrap()
    );
    drop(first);
    assert!(first_image.upgrade().is_none());

    let reloaded: Canvas = serde_json::from_value(image_canvas("sequence-0.png")).unwrap();
    assert_eq!(
        assets
            .prepare_canvas(reloaded)
            .unwrap()
            .get_resources()
            .png_decodes,
        1
    );
    let last_image = Arc::downgrade(&last.canvas.images[0]);
    drop(same);
    drop(last);
    drop(assets);
    assert!(last_image.upgrade().is_none());
}
