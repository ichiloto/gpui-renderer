use crate::{
    assets::AssetRoot,
    display_cache::{DisplayRasterCache, MAX_BYTES, RasterKey},
    glyph_cache,
    glyph_raster::FontCatalog,
    protocol::{self, FrameV2, Grid, Message},
    state::PreparedFrame,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}
fn source() -> FrameV2 {
    let wire = json!({"protocol":2,"type":"frame","frame":1,"sprites":[],"textLayers":[],
        "canvas":{"width":900,"height":400,"textLayers":[{"id":"result","layer":1,
            "origin":{"x":30,"y":40},"grid":{"columns":20,"rows":4,"cellWidth":24,"cellHeight":40},
            "runs":[{"row":0,"column":0,"text":"CRITICAL 1234567890","foreground":{"kind":"rgb","r":255,"g":200,"b":80},"background":null},
                {"row":1,"column":0,"text":"MISS KO HP +420","foreground":{"kind":"rgb","r":80,"g":255,"b":180},"background":null},
                {"row":2,"column":0,"text":"MP -12 0","foreground":null,"background":null},
                {"row":3,"column":0,"text":"Bg","foreground":null,"background":{"kind":"rgb","r":40,"g":70,"b":130}}],
            "glyphEffects":{"outline":{"width":2,"color":{"kind":"rgb","r":8,"g":15,"b":29}},
                "shadow":{"offsetX":0,"offsetY":2,"sigma":1,"opacity":0.7,"color":{"kind":"rgb","r":8,"g":15,"b":29}}}}]}});
    let Message::FrameV2(frame) = protocol::parse(&serde_json::to_vec(&wire).unwrap())
        .unwrap()
        .message
    else {
        panic!()
    };
    frame
}
fn prepared(frame: FrameV2) -> PreparedFrame {
    PreparedFrame::prepare_v2(
        frame,
        Grid {
            columns: 40,
            rows: 20,
            cell_width: 8,
            cell_height: 16,
        },
        &AssetRoot::new(&root()).unwrap(),
    )
    .unwrap()
}

#[test]
fn real_font_contours_are_transparent_bounded_and_cached_across_motion_fade() {
    let mut catalog = FontCatalog::default();
    let mut cache = DisplayRasterCache::default();
    let first = glyph_cache::prepare(&prepared(source()), 2.0, &mut catalog, &mut cache).unwrap();
    assert_eq!(first.builds, 1);
    assert!(first.reserved_bytes < MAX_BYTES);
    let image = first.images[&0].clone();
    let pixels = image.as_bytes(0).unwrap();
    let mut transparent = 0;
    let mut ink = 0;
    let mut contour = 0;
    let mut soft = 0;
    for pixel in pixels.as_chunks::<4>().0 {
        if pixel[3] == 0 {
            transparent += 1;
        }
        if pixel[3] == 255 && pixel[..3] == [80, 200, 255] {
            ink += 1;
        }
        if pixel[3] == 255 && pixel[..3] == [29, 15, 8] {
            contour += 1;
        }
        if pixel[3] > 0 && pixel[3] < 120 {
            soft += 1;
        }
    }
    assert!(transparent > pixels.len() / 8);
    assert!(
        ink > 100 && contour > 100 && soft > 100,
        "ink={ink} contour={contour} soft={soft}"
    );
    let (w, h) = (
        image.size(0).width.0 as usize,
        image.size(0).height.0 as usize,
    );
    for y in 0..h {
        assert_eq!(pixels[(y * w) * 4 + 3], 0);
        assert_eq!(pixels[(y * w + w - 1) * 4 + 3], 0);
    }
    for x in 0..w {
        assert_eq!(pixels[x * 4 + 3], 0);
        assert_eq!(pixels[((h - 1) * w + x) * 4 + 3], 0);
    }
    for step in 0..100 {
        let mut next = source();
        let layer = &mut next.canvas.as_mut().unwrap().text_layers[0];
        layer.id = format!("different-id-{step}");
        layer.origin.y += step as f64 / 10.0;
        layer.opacity = Some(step as f64 / 100.0);
        layer.clip_rect = Some(crate::canvas_protocol::Rect {
            x: 31.0,
            y: 45.0,
            width: 80.0,
            height: 30.0,
        });
        let again = glyph_cache::prepare(&prepared(next), 2.0, &mut catalog, &mut cache).unwrap();
        assert_eq!(again.builds, 0);
        assert!(Arc::ptr_eq(&again.images[&0], &image));
    }
    if let Ok(directory) = std::env::var("ICHILOTO_GLYPH_EVIDENCE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        for density in [0.75, 1.0, 1.5, 2.0, 3.0] {
            let frame =
                glyph_cache::prepare(&prepared(source()), density, &mut catalog, &mut cache)
                    .unwrap();
            let image = &frame.images[&0];
            let mut pixels = image.as_bytes(0).unwrap().to_vec();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            image::RgbaImage::from_raw(
                image.size(0).width.0 as u32,
                image.size(0).height.0 as u32,
                pixels,
            )
            .unwrap()
            .save(PathBuf::from(&directory).join(format!("glyphs-{density}.png")))
            .unwrap();
        }
    }
}

#[test]
fn glyph_display_sampling_preserves_every_content_pixel_including_edges() {
    let mut catalog = FontCatalog::default();
    // Distinct one-pixel cell backgrounds expose cropped edges and stretching
    // without depending on a particular system font's contour placement.
    for padding in [0, 1] {
        let runs: Vec<_> = (0..3)
            .flat_map(|row| {
                (0..3).map(move |column| {
                    json!({"row":row,"column":column,"text":" ","foreground":null,
                        "background":{"kind":"rgb","r":40+column*80,"g":30+row*90,"b":70}})
                })
            })
            .collect();
        let frame: FrameV2 = serde_json::from_value(json!({
            "frame":1,"sprites":[],"textLayers":[],
            "canvas":{"width":100,"height":100,"textLayers":[{
                "id":"edge-pixels","layer":0,"origin":{"x":10,"y":20},
                "grid":{"columns":3,"rows":3,"cellWidth":1,"cellHeight":1},
                "runs":runs,"glyphEffects":{
                    "outline":{"width":padding,"color":{"kind":"rgb","r":0,"g":0,"b":0}},
                    "shadow":{"offsetX":0,"offsetY":0,"sigma":0,"opacity":0,
                        "color":{"kind":"rgb","r":0,"g":0,"b":0}}
                }
            }]}
        }))
        .unwrap();
        let frame = prepared(frame);
        let destination = frame.canvas.as_ref().unwrap().source.text_layers[0]
            .paint_bounds()
            .paint(crate::viewport::ViewportTransform::fit(
                100., 100., 100., 100.,
            ));
        for density in [1.0, 2.0, 3.0] {
            let mut cache = DisplayRasterCache::default();
            let glyphs = glyph_cache::prepare(&frame, density, &mut catalog, &mut cache).unwrap();
            let source = &glyphs.images[&0];
            let (sampled, bounds) = cache.prepare(source, destination, density);
            let source_stride = source.size(0).width.0 as usize;
            let sampled_stride = sampled.size(0).width.0 as usize;
            let offset_x = (destination.left * density - (bounds.left * density).floor()) as usize;
            let offset_y = (destination.top * density - (bounds.top * density).floor()) as usize;
            let guard = crate::glyph_raster::GUARD as usize;
            for y in 0..(destination.height * density) as usize {
                for x in 0..(destination.width * density) as usize {
                    let original = ((y + guard) * source_stride + x + guard) * 4;
                    let displayed = ((y + offset_y) * sampled_stride + x + offset_x) * 4;
                    assert_eq!(
                        &sampled.as_bytes(0).unwrap()[displayed..displayed + 4],
                        &source.as_bytes(0).unwrap()[original..original + 4],
                        "padding={padding}, density={density}, pixel=({x},{y})"
                    );
                }
            }
        }
    }
}

#[test]
fn candidate_failure_does_not_replace_previous_glyphs_and_budget_is_shared() {
    let mut catalog = FontCatalog::default();
    let mut cache = DisplayRasterCache::default();
    let accepted =
        glyph_cache::prepare(&prepared(source()), 1.0, &mut catalog, &mut cache).unwrap();
    let image = accepted.images[&0].clone();
    let mut oversized = source();
    oversized.canvas.as_mut().unwrap().width = 16384;
    oversized.canvas.as_mut().unwrap().height = 16384;
    let layer = &mut oversized.canvas.as_mut().unwrap().text_layers[0];
    layer.grid.columns = 60;
    layer.grid.rows = 60;
    layer.grid.cell_width = 256;
    layer.grid.cell_height = 256;
    assert!(glyph_cache::prepare(&prepared(oversized), 2.0, &mut catalog, &mut cache).is_err());
    assert!(Arc::ptr_eq(&accepted.images[&0], &image));
    let wanted = HashMap::from([(RasterKey::Glyph("over-limit".into()), MAX_BYTES)]);
    assert!(cache.reserve_glyphs(&wanted, 1).is_err());
    assert_eq!(cache.entries.len(), 1);
    let retained = cache.begin_frame(&HashSet::new(), &accepted.keys);
    assert!(retained.is_empty());
    let retired = cache.begin_frame(&HashSet::new(), &HashSet::new());
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].id, image.id);
    assert_eq!(cache.bytes, 0);
}

#[test]
fn glyph_padding_matches_engine_and_never_relaxes_grid_validation() {
    let mut frame = source();
    let canvas = frame.canvas.as_mut().unwrap();
    let layer = &mut canvas.text_layers[0];
    assert_eq!(
        layer.glyph_effects.as_ref().unwrap().padding(),
        [5.0, 5.0, 5.0, 7.0]
    );
    let grid = layer.grid;
    layer.origin.x = 4.0;
    layer.opacity = Some(0.0);
    layer.clip_rect = Some(crate::canvas_protocol::Rect {
        x: 30.0,
        y: 40.0,
        width: 1.0,
        height: 1.0,
    });
    assert!(canvas.validate().is_err());
    assert_eq!(canvas.text_layers[0].grid, grid);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for member in 0..5 {
            let mut canvas = source().canvas.unwrap();
            let e = canvas.text_layers[0].glyph_effects.as_mut().unwrap();
            match member {
                0 => e.outline.width = value,
                1 => e.shadow.offset_x = value,
                2 => e.shadow.offset_y = value,
                3 => e.shadow.sigma = value,
                _ => e.shadow.opacity = value,
            }
            assert!(canvas.validate().is_err());
        }
    }
}

#[test]
fn gaussian_shadow_has_finite_support_and_straight_alpha_composes_correctly() {
    let mut source = vec![0.0; 81];
    source[40] = 1.0;
    let mut temp = vec![0.0; 81];
    let mut out = vec![0.0; 81];
    crate::glyph_pixels::blur(&source, &mut temp, &mut out, 9, 9, 1.0, 3);
    assert!(out[40] > out[41] && out[41] > out[42] && out[42] > out[43]);
    assert_eq!(out[44], 0.0);
    assert!((out.iter().sum::<f32>() - 1.0).abs() < 0.00001);
    let mut pixel = [0; 4];
    crate::glyph_pixels::over(&mut pixel, 0xff8040, 0.5);
    assert_eq!(pixel, [64, 128, 255, 128]);
    crate::glyph_pixels::over(&mut pixel, 0x008000, 1.0);
    assert_eq!(pixel, [0, 128, 0, 255]);
}

#[test]
fn exact_glyph_wire_cases_keep_negotiation_and_schema_strict() {
    let base = root().join("canvas-glyph-effects");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(base.join("manifest.json")).unwrap()).unwrap();
    for case in manifest["cases"].as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let stage = case["stage"].as_str().unwrap();
        let mut hello: serde_json::Value =
            serde_json::from_slice(&std::fs::read(base.join("hello.json")).unwrap()).unwrap();
        hello["assetRoot"] = json!(root());
        if let Some(caps) = case.get("capabilities") {
            hello["requiredCapabilities"] = caps.clone();
        }
        let mut session = crate::transport::Session::default();
        session
            .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
            .unwrap();
        let parsed = protocol::parse(&std::fs::read(base.join(name)).unwrap());
        match stage {
            "schema" => {
                if let Ok(incoming) = parsed {
                    let Message::FrameV2(frame) = &incoming.message else {
                        panic!("{name}")
                    };
                    assert!(
                        frame
                            .validate(Grid {
                                columns: 40,
                                rows: 20,
                                cell_width: 8,
                                cell_height: 16
                            })
                            .is_err(),
                        "{name}"
                    );
                    assert!(session.prepare(incoming).is_err(), "{name}");
                }
            }
            "session" => assert!(
                session
                    .prepare(parsed.unwrap())
                    .err()
                    .unwrap()
                    .contains("negotiated"),
                "{name}"
            ),
            "accept" => {
                let incoming = parsed.unwrap_or_else(|error| panic!("{name}: {error}"));
                if matches!(incoming.message, Message::Hello(_)) {
                    continue;
                }
                assert!(matches!(
                    session
                        .prepare(incoming)
                        .unwrap_or_else(|error| panic!("{name}: {error}")),
                    crate::transport::Update::Frame(_)
                ));
            }
            _ => panic!("{name}: invalid stage"),
        }
    }
}

#[test]
fn retired_and_snapshot_pinned_tiles_stay_in_the_glyph_admission_budget() {
    use crate::display_cache::{MAX_IMAGES, TileKey};
    let mut cache = DisplayRasterCache::default();
    let held = Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::new(2048, 4096),
    )]));
    cache.insert_with_limits(
        RasterKey::Tile(TileKey {
            region: held.id,
            width: 1,
            height: 1,
            phase_x: 0,
            phase_y: 0,
        }),
        held.clone(),
        MAX_BYTES,
        MAX_IMAGES,
    );
    let wanted = HashMap::from([(RasterKey::Glyph("candidate".into()), 40 * 1024 * 1024)]);
    assert!(cache.reserve_glyphs(&wanted, 1024).is_err());
    let retired = cache.begin_frame(&HashSet::new(), &HashSet::new());
    assert_eq!(cache.bytes, 0);
    assert_eq!(cache.live_bytes(), 32 * 1024 * 1024);
    assert!(cache.reserve_glyphs(&wanted, 1024).is_err());
    drop(retired); // GPU retirement alone cannot release snapshot-held storage.
    assert_eq!(cache.live_bytes(), 32 * 1024 * 1024);
    assert!(cache.reserve_glyphs(&wanted, 1024).is_err());
    drop(held);
    assert_eq!(cache.live_bytes(), 0);
    assert!(cache.reserve_glyphs(&wanted, 1024).is_ok());
}

#[test]
fn failed_second_layer_leaves_the_complete_old_generation_and_no_staged_raster_leak() {
    let mut catalog = FontCatalog::default();
    let mut cache = DisplayRasterCache::default();
    let accepted =
        glyph_cache::prepare(&prepared(source()), 1.0, &mut catalog, &mut cache).unwrap();
    let bytes = cache.live_bytes();
    let old = accepted.images[&0].clone();
    let mut next = source();
    let canvas = next.canvas.as_mut().unwrap();
    canvas.text_layers[0].runs[0].text = "FIRST CANDIDATE".into();
    let mut invalid = canvas.text_layers[0].clone();
    invalid.id = "unsupported".into();
    invalid.runs[0].text = "\u{10ffff}".into();
    canvas.text_layers.push(invalid);
    assert!(glyph_cache::prepare(&prepared(next), 1.0, &mut catalog, &mut cache).is_err());
    assert!(Arc::ptr_eq(&accepted.images[&0], &old));
    assert_eq!(cache.live_bytes(), bytes);
    let recovered =
        glyph_cache::prepare(&prepared(source()), 1.0, &mut catalog, &mut cache).unwrap();
    assert_eq!(recovered.images[&0].as_bytes(0), old.as_bytes(0));
}

/// Uses unmodified Game-emitted wire frames and their real assets. Kept separate
/// from hermetic tests because the Game runtime is owned by another repository.
#[test]
#[ignore = "requires ICHILOTO_GAME_FRAME_DIR and ICHILOTO_GLYPH_REPORT"]
fn emitted_game_frames_fit_cold_and_retained_generation_budgets() {
    use crate::{
        glyph_raster::RasterPlan,
        transport::{Session, Update},
    };
    use std::time::Instant;
    let directory = PathBuf::from(std::env::var("ICHILOTO_GAME_FRAME_DIR").unwrap());
    let report = PathBuf::from(std::env::var("ICHILOTO_GLYPH_REPORT").unwrap());
    let hello = std::fs::read(directory.join("hello.json")).unwrap();
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for name in [
        "battle-ordinary",
        "battle-feedback",
        "battle-six-recipient-stress",
        "battle-six-recipient-varied-numbers",
    ] {
        let mut session = Session::default();
        session.prepare(protocol::parse(&hello).unwrap()).unwrap();
        let varied = name == "battle-six-recipient-varied-numbers";
        let source_name = if varied {
            "battle-six-recipient-stress"
        } else {
            name
        };
        let mut wire = std::fs::read(directory.join(format!("{source_name}.frame.json"))).unwrap();
        if varied {
            // Presentation-only derivative: vary numeric glyphs at equal scalar
            // width, so identical recipient results do not hide cache misses.
            // The real-action capture and every input file remain unchanged.
            let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
            for (index, layer) in value["canvas"]["textLayers"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                if layer.get("glyphEffects").is_none() {
                    continue;
                }
                for run in layer["runs"].as_array_mut().unwrap() {
                    run["text"] = json!(
                        run["text"]
                            .as_str()
                            .unwrap()
                            .chars()
                            .map(|ch| {
                                if ch.is_ascii_digit() {
                                    char::from(b'0' + ((ch as u8 - b'0' + (index % 10) as u8) % 10))
                                } else {
                                    ch
                                }
                            })
                            .collect::<String>()
                    );
                }
            }
            wire = serde_json::to_vec(&value).unwrap();
        }
        let Update::Frame(frame) = session.prepare(protocol::parse(&wire).unwrap()).unwrap() else {
            panic!("expected a prepared Game frame");
        };
        for mode in ["cold", "retained-generation"] {
            let mut catalog = FontCatalog::default();
            let mut cache = DisplayRasterCache::default();
            let mut previous = crate::glyph_cache::GlyphFrame::default();
            for density in [1.0, 1.5, 2.0, 3.0] {
                if mode == "cold" {
                    previous = Default::default();
                    cache = Default::default();
                    catalog = Default::default();
                }
                let mut unique = HashMap::new();
                for layer in &frame.canvas.as_ref().unwrap().source.text_layers {
                    if layer.glyph_effects.is_some() {
                        let key = format!(
                            "{:?}|{:?}|{:?}",
                            layer.grid, layer.runs, layer.glyph_effects
                        );
                        unique.insert(key, RasterPlan::new(layer, density).unwrap().work);
                    }
                }
                let planned_work: usize = unique.values().sum();
                let held_bytes = cache.live_bytes();
                let start = Instant::now();
                match glyph_cache::prepare(&frame, density, &mut catalog, &mut cache) {
                    Ok(candidate) => {
                        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                        let warm_start = Instant::now();
                        let cached =
                            glyph_cache::prepare(&frame, density, &mut catalog, &mut cache)
                                .unwrap();
                        let warm_ms = warm_start.elapsed().as_secs_f64() * 1000.0;
                        assert_eq!(cached.builds, 0);
                        for (index, image) in &candidate.images {
                            assert!(Arc::ptr_eq(image, &cached.images[index]));
                        }
                        assert!(candidate.reserved_bytes <= MAX_BYTES);
                        results.push(json!({"frame":name, "mode":mode, "density":density,
                            "uniqueSampleWork":planned_work, "heldBytes":held_bytes,
                            "reservedBytes":candidate.reserved_bytes, "builds":candidate.builds,
                            "elapsedMs":elapsed_ms, "warmMs":warm_ms, "warmBuilds":cached.builds}));
                        // Retirement cannot reclaim the previous generation while
                        // its snapshot is still held during candidate preparation.
                        drop(cache.begin_frame(&HashSet::new(), &candidate.keys));
                        drop(previous);
                        previous = candidate;
                    }
                    Err(error) => {
                        failures.push(format!("{name}/{mode}/{density}: {error}"));
                        results.push(json!({"frame":name, "mode":mode, "density":density,
                            "uniqueSampleWork":planned_work, "heldBytes":held_bytes, "error":error}));
                    }
                }
            }
        }
    }
    std::fs::write(report, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
