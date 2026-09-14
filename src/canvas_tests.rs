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
