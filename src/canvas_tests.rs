use crate::{
    assets::AssetRoot,
    canvas,
    canvas_protocol::*,
    protocol::*,
    state::*,
    transport::{Session, Update},
    viewport::ViewportTransform,
};
use std::{fs, path::PathBuf, sync::Arc};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}
fn fixture(name: &str) -> Vec<u8> {
    fs::read(root().join("graphical-canvas").join(name)).unwrap()
}
fn hello() -> Hello {
    let mut value: serde_json::Value = serde_json::from_slice(&fixture("hello.json")).unwrap();
    value["assetRoot"] = serde_json::json!(root());
    let Message::Hello(hello) = parse(&serde_json::to_vec(&value).unwrap()).unwrap().message else {
        panic!()
    };
    hello
}
fn source(name: &str) -> FrameV2 {
    let Message::FrameV2(frame) = parse(&fixture(name)).unwrap().message else {
        panic!()
    };
    frame
}
fn prepared(name: &str, config: &Hello, assets: &AssetRoot) -> PreparedFrame {
    PreparedFrame::prepare_v2(source(name), config.grid, assets).unwrap()
}

#[test]
fn results_layout_stages_share_art_clip_gauges_and_release_complete_pages() {
    // Real PHP CanvasNineSlice/CanvasTextLayer/StyledPresentationFrame output for
    // the approved 1350x720 Results geometry. Neutral labels and generated PNGs
    // test the contract, not Game rewards, production art or native visual fidelity.
    let dir = tempfile::tempdir().unwrap();
    let base = "Graphics/UI/LastLegend/V1";
    let mut files = vec![
        ("arena.png".to_string(), 2700, 1440),
        ("bust.png".to_string(), 768, 960),
        ("battler.png".to_string(), 512, 512),
    ];
    for i in 0..4 {
        files.push((format!("menu-{i}.png"), 512, 512));
    }
    for (path, w, h) in [
        ("Frames/Panel.png", 96, 96),
        ("Frames/Quiet.png", 96, 96),
        ("Frames/Portrait.png", 96, 96),
        ("Controls/ButtonSelected.png", 96, 48),
        ("Gauges/Track.png", 24, 12),
        ("Gauges/EXPFill.png", 8, 8),
        ("Navigation/Selector.png", 32, 32),
        ("Icons/Skills.png", 32, 32),
    ] {
        files.push((format!("{base}/{path}"), w, h));
    }
    for (path, w, h) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(w, h, image::Rgba([20, 40, 80, 180]))
            .save(path)
            .unwrap();
    }
    let mut config = hello();
    config.asset_root = dir.path().into();
    config.required_capabilities = vec![
        Capability::GraphicalCanvas,
        Capability::SpriteSourceRect,
        Capability::CanvasClipOpacity,
        Capability::CanvasGlyphEffects,
    ];
    let mut session = Session::default();
    session
        .prepare(Incoming {
            version: Version::V2,
            message: Message::Hello(config.clone()),
        })
        .unwrap();
    let frames: Vec<_> = include_str!("../fixtures/results-layout.ndjson")
        .lines()
        .map(|line| {
            let Update::Frame(frame) = session.prepare(parse(line.as_bytes()).unwrap()).unwrap()
            else {
                panic!("Results fixture must contain complete frames");
            };
            frame
        })
        .collect();
    assert_eq!(frames.len(), 8);
    let expected = [
        (95, 34),
        (95, 34),
        (118, 42),
        (35, 9),
        (32, 8),
        (22, 6),
        (2, 0),
        (91, 34),
    ];
    let primary = frames[0].canvas.as_ref().unwrap();
    let field = primary.images[0].clone();
    for (frame, (images, text)) in frames.iter().zip(expected) {
        assert!(frame.plan.is_empty() && frame.tile_batches.is_empty() && frame.sprites.is_empty());
        let canvas = frame.canvas.as_ref().unwrap();
        assert_eq!((canvas.source.width, canvas.source.height), (1350, 720));
        assert_eq!(
            (canvas.source.images.len(), canvas.source.text_layers.len()),
            (images, text)
        );
        assert!(Arc::ptr_eq(&field, &canvas.images[0]));
        assert_eq!(canvas.source.images[0], primary.source.images[0]);
        assert_eq!(canvas.source.images[1], primary.source.images[1]);
        assert!(Arc::ptr_eq(&primary.images[1], &canvas.images[1]));
        assert!(frame.resources.decoded_bytes <= crate::assets::MAX_DECODED_BYTES);
        assert!(frame.resources.region_bytes <= crate::tile_regions::MAX_REGION_BYTES);
    }
    let pieces: Vec<_> = primary
        .source
        .images
        .iter()
        .filter(|item| item.id.starts_with("party-panel-"))
        .collect();
    assert_eq!(pieces.len(), 9);
    assert_eq!(
        pieces
            .iter()
            .map(|item| item.destination.width * item.destination.height)
            .sum::<f64>(),
        754.0 * 502.0
    );
    assert_eq!(
        (
            pieces[0].destination.x,
            pieces[0].destination.y,
            pieces[0].destination.width,
            pieces[0].destination.height
        ),
        (52.0, 146.0, 24.0, 24.0)
    );
    assert_eq!(
        (pieces[8].destination.x, pieces[8].destination.y),
        (782.0, 624.0)
    );
    for row in 0..3 {
        for column in 0..2 {
            let a = pieces[row * 3 + column].destination;
            let b = pieces[row * 3 + column + 1].destination;
            assert_eq!(a.x + a.width, b.x);
        }
    }
    for row in 0..2 {
        for column in 0..3 {
            let a = pieces[row * 3 + column].destination;
            let b = pieces[(row + 1) * 3 + column].destination;
            assert_eq!(a.y + a.height, b.y);
        }
    }
    let settled = frames[1].canvas.as_ref().unwrap();
    for i in 0..4 {
        let id = format!("exp-{i}-fill");
        let index = primary
            .source
            .images
            .iter()
            .position(|image| image.id == id)
            .unwrap();
        let before = &primary.source.images[index];
        let after = &settled.source.images[index];
        assert_eq!(before.destination, after.destination);
        assert_eq!(before.clip_rect.unwrap().width, 106.5);
        assert_eq!(after.clip_rect.unwrap().width, 319.5);
        assert!(Arc::ptr_eq(&primary.images[index], &settled.images[index]));
        for size in [(1350.0, 720.0), (1920.0, 1080.0), (675.0, 720.0)] {
            let transform = ViewportTransform::fit(1350.0, 720.0, size.0, size.1);
            let (mask, content) =
                canvas::clipped_bounds(before.destination, before.clip_rect, transform).unwrap();
            assert_eq!(content.width, 426.0 * transform.scale);
            assert_eq!(mask.width, 106.5 * transform.scale);
            assert_eq!(
                transform.offset_x,
                (size.0 - transform.presented_width) / 2.0
            );
            assert_eq!(
                transform.offset_y,
                (size.1 - transform.presented_height) / 2.0
            );
        }
    }
    let transition = frames[2].canvas.as_ref().unwrap();
    assert!(
        transition.source.images[2..]
            .iter()
            .all(|image| image.opacity
                == if image.id.starts_with("action-") || image.id == "cursor" {
                    1.0
                } else {
                    0.5
                })
    );
    assert!(transition.source.text_layers.iter().all(|text| text.opacity
        == if text.id == "action-label" {
            None
        } else {
            Some(0.5)
        }));
    for image in &transition.source.images {
        if image.id.starts_with("action-") || image.id == "cursor" {
            let original = primary
                .source
                .images
                .iter()
                .find(|item| item.id == image.id)
                .unwrap();
            assert_eq!(image.destination, original.destination);
            assert!(image.layer > 2200);
        }
    }
    // Page bands preserve outgoing-before-incoming order without pretending that
    // individual child alpha implements isolated browser group compositing.
    let mut last_band = 0;
    for item in &transition.plan {
        let id = match *item {
            canvas::PaintItem::Image(i) => &transition.source.images[i].id,
            canvas::PaintItem::Text(i) => &transition.source.text_layers[i].id,
            canvas::PaintItem::Indicator(_) => unreachable!(),
            canvas::PaintItem::Composite(_) => unreachable!(),
        };
        let band = if id == "battlefield" || id == "battle-combatant" {
            0
        } else if id.starts_with("out:") {
            1
        } else if id.starts_with("in:") {
            2
        } else {
            3
        };
        assert!(band >= last_band);
        last_band = band;
    }
    let heading = &primary.source.text_layers[0];
    assert_eq!(
        (heading.grid.cell_width, heading.grid.cell_height),
        (36, 54)
    );
    assert!(heading.glyph_effects.is_some());
    assert!(
        primary.source.text_layers[1..]
            .iter()
            .all(|text| text.glyph_effects.is_none())
    );

    // Retain every tested stage/glyph generation while admitting new ones. This
    // bounds this fixture's overlap, not arbitrary Game field/portrait selection.
    let mut fonts = crate::glyph_raster::FontCatalog::default();
    for density in [1.0, 1.5, 2.0, 3.0] {
        let mut cache = crate::display_cache::DisplayRasterCache::default();
        let mut held = Vec::new();
        let mut peak = 0;
        for frame in &frames {
            let glyphs =
                crate::glyph_cache::prepare(frame, density, &mut fonts, &mut cache).unwrap();
            peak = peak.max(glyphs.reserved_bytes);
            assert!(glyphs.reserved_bytes <= crate::display_cache::MAX_BYTES);
            held.push(glyphs);
        }
        assert_eq!(held[1].builds, 0); // Complete -> Continue changes no heading.
        assert!(Arc::ptr_eq(&held[0].images[&0], &held[1].images[&0]));
        assert!(held[6].images.is_empty());
        // Unused keys are retired on page changes. Re-entry rebuilds identical
        // pixels while admission still accounts for the retained old generation.
        assert_eq!(held[7].builds, 1);
        assert_eq!(
            held[7].images[&0].as_bytes(0),
            held[0].images[&0].as_bytes(0)
        );
        println!("Results layout density={density} max-live glyph admission={peak} bytes");
    }
    println!(
        "Results layout max decoded={} prepared-regions={} bytes",
        frames
            .iter()
            .map(|frame| frame.resources.decoded_bytes)
            .max()
            .unwrap(),
        frames
            .iter()
            .map(|frame| frame.resources.region_bytes)
            .max()
            .unwrap()
    );
    let mut state = RendererState {
        hello: config,
        frame: None,
    };
    for (index, frame) in frames.into_iter().enumerate() {
        state.replace(frame);
        assert_eq!(state.logical_size(), (1350.0, 720.0));
        let current = state.frame.as_ref().unwrap().canvas.as_ref().unwrap();
        if index == 6 {
            assert_eq!(
                current.plan,
                [canvas::PaintItem::Image(0), canvas::PaintItem::Image(1)]
            );
            assert!(current.source.text_layers.is_empty());
        } else if index == 7 {
            assert!(
                current
                    .source
                    .images
                    .iter()
                    .all(|image| !image.id.ends_with("-fill"))
            );
            assert!(
                current
                    .source
                    .images
                    .iter()
                    .all(|image| !image.id.starts_with("in:") && !image.id.starts_with("out:"))
            );
        }
    }
}

#[test]
fn exact_wire_corpus_matches_stages_and_preserves_the_previous_display_on_error() {
    let manifest: serde_json::Value = serde_json::from_slice(&fixture("manifest.json")).unwrap();
    for case in manifest["cases"].as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let stage = case["stage"].as_str().unwrap();
        let mut config = hello();
        let assets = AssetRoot::new(&root()).unwrap();
        let mut state = RendererState {
            hello: config.clone(),
            frame: Some(prepared("valid-full-canvas.json", &config, &assets)),
        };
        let original = state
            .frame
            .as_ref()
            .unwrap()
            .canvas
            .as_ref()
            .unwrap()
            .images[0]
            .clone();
        if let Some(caps) = case.get("capabilities") {
            config.required_capabilities = serde_json::from_value(caps.clone()).unwrap();
        }
        let mut session = Session::default();
        session
            .prepare(Incoming {
                version: Version::V2,
                message: Message::Hello(config.clone()),
            })
            .unwrap();
        let bytes = fixture(name);
        let decoded = serde_json::from_slice::<serde_json::Value>(&bytes);
        let parsed = parse(&bytes);
        match stage {
            "json" => {
                assert!(decoded.is_err(), "{name}");
                assert!(parsed.is_err(), "{name}");
            }
            "schema" => {
                assert!(decoded.is_ok(), "{name}");
                match parsed {
                    Err(_) => (),
                    Ok(Incoming {
                        message: Message::FrameV2(frame),
                        ..
                    }) => {
                        assert!(frame.validate(config.grid).is_err(), "{name}");
                        assert!(
                            session
                                .prepare(Incoming {
                                    version: Version::V2,
                                    message: Message::FrameV2(frame)
                                })
                                .is_err(),
                            "{name}"
                        );
                    }
                    other => panic!("{name}: unexpected {other:?}"),
                }
            }
            "session" | "preparation" => {
                let incoming = parsed.unwrap_or_else(|error| panic!("{name}: {error}"));
                let Message::FrameV2(frame) = &incoming.message else {
                    panic!("{name}")
                };
                frame
                    .validate(config.grid)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                let error = session
                    .prepare(incoming)
                    .err()
                    .unwrap_or_else(|| panic!("{name}: accepted"));
                assert_eq!(
                    error.contains("negotiated"),
                    stage == "session",
                    "{name}: {error}"
                );
            }
            "accept" => {
                let Update::Frame(frame) = session
                    .prepare(parsed.unwrap())
                    .unwrap_or_else(|error| panic!("{name}: {error}"))
                else {
                    panic!("{name}")
                };
                let expected = source(name).canvas;
                state.replace(frame);
                assert_eq!(
                    state
                        .frame
                        .as_ref()
                        .unwrap()
                        .canvas
                        .as_ref()
                        .map(|c| &c.source),
                    expected.as_ref(),
                    "{name}"
                );
                assert!(state.frame.as_ref().unwrap().plan.is_empty(), "{name}");
                continue;
            }
            _ => panic!("unknown fixture stage {stage}"),
        }
        assert!(
            Arc::ptr_eq(
                &state
                    .frame
                    .as_ref()
                    .unwrap()
                    .canvas
                    .as_ref()
                    .unwrap()
                    .images[0],
                &original
            ),
            "{name}"
        );
        assert_eq!(
            state
                .frame
                .as_ref()
                .unwrap()
                .canvas
                .as_ref()
                .unwrap()
                .source
                .images
                .len(),
            2,
            "{name}"
        );
    }
}

#[test]
fn canvas_size_and_fractional_geometry_ignore_the_legacy_cell_metrics() {
    let mut config = hello();
    let assets = AssetRoot::new(&root()).unwrap();
    for (cw, ch) in [(1, 1), (8, 16), (17, 23), (256, 256)] {
        config.grid.cell_width = cw;
        config.grid.cell_height = ch;
        let mut state = RendererState {
            hello: config.clone(),
            frame: Some(prepared("valid-fractional-crop.json", &config, &assets)),
        };
        assert_eq!(state.logical_size(), (1350.0, 720.0));
        let rect = state
            .frame
            .as_ref()
            .unwrap()
            .canvas
            .as_ref()
            .unwrap()
            .source
            .images[0]
            .destination;
        assert_eq!(
            rect,
            Rect {
                x: 968.5,
                y: 224.5,
                width: 143.0,
                height: 214.5
            }
        );
        for (width, height) in [
            (1600.0, 900.0),
            (1012.5, 540.0),
            (675.0, 720.0),
            (1350.0, 360.0),
            (0.0, 0.0),
        ] {
            let transform = ViewportTransform::fit(1350.0, 720.0, width, height);
            let image = rect.paint(transform);
            assert_eq!(image.left, 968.5 * transform.scale);
            assert_eq!(image.top, 224.5 * transform.scale);
            assert_eq!(image.width, 143.0 * transform.scale);
            assert_eq!(image.height, 214.5 * transform.scale);
            assert_eq!(
                transform.offset_x,
                (width - transform.presented_width) / 2.0
            );
            assert_eq!(
                transform.offset_y,
                (height - transform.presented_height) / 2.0
            );
        }
        state.replace(prepared("clear-canvas-omitted.json", &config, &assets));
        assert_eq!(state.logical_size(), ((40 * cw) as f32, (20 * ch) as f32));
    }
}

#[test]
fn full_replacement_reuses_source_images_and_clears_all_canvas_collections() {
    let config = hello();
    let assets = AssetRoot::new(&root()).unwrap();
    let first = prepared("valid-repeated-instance.json", &config, &assets);
    let image = first.canvas.as_ref().unwrap().images[0].clone();
    assert!(Arc::ptr_eq(
        &image,
        &first.canvas.as_ref().unwrap().images[1]
    ));
    assert_eq!(
        (first.resources.png_decodes, first.resources.decoded_images),
        (1, 1)
    );
    let mut state = RendererState {
        hello: config.clone(),
        frame: Some(first),
    };
    let survivor = prepared("valid-reordered-survivor.json", &config, &assets);
    assert_eq!(survivor.resources.png_decodes, 0);
    assert!(Arc::ptr_eq(
        &image,
        &survivor.canvas.as_ref().unwrap().images[0]
    ));
    state.replace(survivor);
    let current = state.frame.as_ref().unwrap().canvas.as_ref().unwrap();
    assert_eq!(current.source.images[0].id, "participant-2");
    assert_eq!(current.images.len(), 1);
    assert!(current.source.indicators.is_empty());
    for name in ["clear-canvas-blank.json", "clear-canvas-omitted.json"] {
        state.replace(prepared(name, &config, &assets));
        let frame = state.frame.as_ref().unwrap();
        assert_eq!(
            (
                frame.resources.canvas_images,
                frame.resources.canvas_indicators,
                frame.resources.canvas_text_layers
            ),
            (0, 0, 0)
        );
        assert!(
            frame
                .canvas
                .as_ref()
                .is_none_or(|canvas| canvas.plan.is_empty())
        );
    }
    // The existing field fixture fits its own larger grid and fully replaces canvas.
    let mut field = source("clear-canvas-omitted.json");
    field.text_layers.push(TextLayer {
        id: "field".into(),
        layer: 0,
        runs: vec![],
    });
    state.replace(PreparedFrame::prepare_v2(field, config.grid, &assets).unwrap());
    assert!(state.frame.as_ref().unwrap().canvas.is_none());
    assert_eq!(
        state.frame.as_ref().unwrap().plan,
        [crate::state::PaintItem::Text(0)]
    );
    assert!(!image.as_bytes(0).unwrap().is_empty());
}

#[test]
fn canvas_ties_and_changed_layer_order_are_stable() {
    let config = hello();
    let assets = AssetRoot::new(&root()).unwrap();
    let mut frame = source("valid-equal-layer-order.json");
    let canvas = frame.canvas.as_mut().unwrap();
    let mut image = canvas.images[0].clone();
    image.id = "second".into();
    canvas.images.push(image);
    let first = PreparedFrame::prepare_v2(frame.clone(), config.grid, &assets).unwrap();
    assert_eq!(
        first.canvas.unwrap().plan,
        [
            canvas::PaintItem::Image(0),
            canvas::PaintItem::Image(1),
            canvas::PaintItem::Indicator(0),
            canvas::PaintItem::Text(0)
        ]
    );
    frame.canvas.as_mut().unwrap().images[1].layer = i32::MAX;
    frame.canvas.as_mut().unwrap().text_layers[0].layer = i32::MIN;
    assert_eq!(
        PreparedFrame::prepare_v2(frame, config.grid, &assets)
            .unwrap()
            .canvas
            .unwrap()
            .plan,
        [
            canvas::PaintItem::Text(0),
            canvas::PaintItem::Image(0),
            canvas::PaintItem::Indicator(0),
            canvas::PaintItem::Image(1)
        ]
    );
}

#[test]
fn indicator_strokes_fit_resolved_bounds_and_keep_distinct_shapes_at_every_scale() {
    let source = source("valid-selected-acting.json").canvas.unwrap();
    for indicator in &source.indicators {
        let strokes = indicator.strokes();
        assert_eq!(
            strokes.len(),
            if indicator.kind == IndicatorKind::Outline {
                4
            } else {
                1
            }
        );
        for stroke in &strokes {
            stroke.validate(&source).unwrap();
            assert!(stroke.x >= indicator.bounds.x && stroke.y >= indicator.bounds.y);
            assert!(stroke.x + stroke.width <= indicator.bounds.x + indicator.bounds.width);
            assert!(stroke.y + stroke.height <= indicator.bounds.y + indicator.bounds.height);
            for scale in [0.0, 0.5, 0.75, 1.0] {
                let transform =
                    ViewportTransform::fit(1350.0, 720.0, 1350.0 * scale, 720.0 * scale);
                assert_eq!(stroke.paint(transform).width, stroke.width as f32 * scale);
            }
        }
    }
}

#[test]
fn nonfinite_programmatic_values_fail_even_without_json_decoding() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
        let mut canvas = source("valid-arbitrary-geometry.json").canvas.unwrap();
        canvas.images[0].destination.x = value;
        assert!(canvas.validate().is_err());
        let mut canvas = source("valid-arbitrary-geometry.json").canvas.unwrap();
        canvas.images[0].opacity = value;
        assert!(canvas.validate().is_err());
    }
}

#[test]
fn canvas_preparation_obeys_shared_decoded_image_budget_and_symlink_confinement() {
    let dir = tempfile::tempdir().unwrap();
    let config = hello();
    let png = image::RgbaImage::from_pixel(2048, 2048, image::Rgba([20, 40, 60, 255]));
    png.save(dir.path().join("source.png")).unwrap();
    let mut frame = source("valid-arbitrary-geometry.json");
    let canvas = frame.canvas.as_mut().unwrap();
    canvas.indicators.clear();
    let item = canvas.images[0].clone();
    canvas.images = (0..5)
        .map(|i| {
            fs::copy(
                dir.path().join("source.png"),
                dir.path().join(format!("{i}.png")),
            )
            .unwrap();
            CanvasImage {
                id: format!("image-{i}"),
                asset: format!("{i}.png").into(),
                source_rect: Some(SourceRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                }),
                ..item.clone()
            }
        })
        .collect();
    let assets = AssetRoot::new(dir.path()).unwrap();
    let error = PreparedFrame::prepare_v2(frame.clone(), config.grid, &assets).unwrap_err();
    assert!(error.contains("64 MiB decoded"), "{error}");
    for image in &mut frame.canvas.as_mut().unwrap().images {
        image.source_rect = None;
    }
    let error = PreparedFrame::prepare_v2(frame, config.grid, &assets).unwrap_err();
    assert!(error.contains("64 MiB prepared-region"), "{error}");
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs::copy(
            root().join("test-sprite.png"),
            outside.path().join("outside.png"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.png"),
            dir.path().join("escape.png"),
        )
        .unwrap();
        let mut frame = source("valid-arbitrary-geometry.json");
        frame.canvas.as_mut().unwrap().images[0].asset = "escape.png".into();
        assert!(
            PreparedFrame::prepare_v2(frame, config.grid, &assets)
                .unwrap_err()
                .contains("escapes assetRoot")
        );
    }
}

#[test]
fn whole_canvas_images_and_crops_have_cached_edge_guards_with_original_alpha() {
    let config = hello();
    let assets = AssetRoot::new(&root()).unwrap();
    for name in ["valid-full-canvas.json", "valid-fractional-crop.json"] {
        let first = prepared(name, &config, &assets);
        let canvas = first.canvas.as_ref().unwrap();
        for (item, region) in canvas.source.images.iter().zip(&canvas.images) {
            let original = assets.load(&item.asset).unwrap();
            let size = original.size(0);
            let rect = item.source_rect.unwrap_or(SourceRect {
                x: 0,
                y: 0,
                width: size.width.0 as u32,
                height: size.height.0 as u32,
            });
            let guard = crate::tile_regions::GUARD;
            let width = rect.width + 2 * guard;
            assert_eq!(region.size(0).width.0 as u32, width);
            assert_eq!(region.size(0).height.0 as u32, rect.height + 2 * guard);
            for (x, y) in [
                (0, 0),
                (width - 1, 0),
                (0, rect.height + 2 * guard - 1),
                (width - 1, rect.height + 2 * guard - 1),
                (guard, guard),
            ] {
                let sx = rect.x + x.saturating_sub(guard).min(rect.width - 1);
                let sy = rect.y + y.saturating_sub(guard).min(rect.height - 1);
                let a = ((sy * size.width.0 as u32 + sx) * 4) as usize;
                let b = ((y * width + x) * 4) as usize;
                assert_eq!(
                    &original.as_bytes(0).unwrap()[a..a + 4],
                    &region.as_bytes(0).unwrap()[b..b + 4]
                );
            }
        }
        let again = prepared(name, &config, &assets);
        assert_eq!(
            (again.resources.png_decodes, again.resources.region_builds),
            (0, 0)
        );
        assert!(Arc::ptr_eq(
            &canvas.images[0],
            &again.canvas.as_ref().unwrap().images[0]
        ));
    }
}
