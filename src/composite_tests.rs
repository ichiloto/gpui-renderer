use crate::{
    composite_cache::{CompositeCache, Sources},
    composite_pixels as pixels,
    composite_protocol::{Blend, Composite},
    protocol::{FrameV2, Grid},
    state::PreparedFrame,
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

fn rgb(r: u8, g: u8, b: u8) -> Value {
    json!({"kind":"rgb","r":r,"g":g,"b":b})
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({"x":x,"y":y,"width":w,"height":h})
}
fn composite(ops: Vec<Value>, w: u32, h: u32) -> Value {
    json!({"id":"composition","width":w,"height":h,"destination":rect(0.,0.,w as f64,h as f64),"layer":0,"operations":ops})
}
fn frame(c: Value, w: u32, h: u32) -> FrameV2 {
    serde_json::from_value(json!({"frame":1,"textLayers":[],"sprites":[],"canvas":{"width":w,"height":h,"composites":[c]}})).unwrap()
}
fn grid() -> Grid {
    Grid {
        columns: 135,
        rows: 36,
        cell_width: 10,
        cell_height: 20,
    }
}
fn solid(color: Value, w: u32, h: u32) -> Value {
    json!({"type":"fill","destination":rect(0.,0.,w as f64,h as f64),"brush":{"type":"solid","color":color}})
}
fn raster(c: Value, sources: Sources) -> Arc<gpui::RenderImage> {
    let spec: Composite = serde_json::from_value(c).unwrap();
    CompositeCache::default()
        .prepare(&[spec], &sources)
        .unwrap()
        .0
        .remove(0)
}
fn pixel(image: &gpui::RenderImage, x: u32, y: u32) -> [u8; 4] {
    let offset = (((y + 2) * image.size(0).width.0 as u32 + x + 2) * 4) as usize;
    image.as_bytes(0).unwrap()[offset..offset + 4]
        .try_into()
        .unwrap()
}

#[test]
fn screen_and_source_over_respect_transparency_and_alpha() {
    let s = pixels::unpack(&[255, 0, 0, 128]);
    let d = pixels::unpack(&[0, 255, 0, 128]);
    let over = pixels::blend(s, d, Blend::SourceOver);
    let screen = pixels::blend(s, d, Blend::Screen);
    assert!((over[3] - 0.752).abs() < 0.002);
    assert!(screen[1] > over[1]);
    assert_eq!(screen[3], over[3]);
    assert_eq!(pixels::blend(s, [0.; 4], Blend::Screen), s);
    let mut glow = solid(rgb(100, 50, 0), 2, 2);
    glow["blend"] = json!("screen");
    let image = raster(
        composite(vec![solid(rgb(100, 100, 100), 2, 2), glow], 2, 2),
        Sources::new(),
    );
    assert_eq!(pixel(&image, 0, 0), [100, 130, 161, 255]);
}

#[test]
fn fractional_sampling_preserves_alpha_without_hidden_colour_fringes() {
    let p = pixels::sample(
        &[0, 0, 255, 255, 255, 0, 0, 0],
        2,
        1,
        crate::composite_protocol::whole_source(),
        [0.5, 0.5],
    );
    assert_eq!(p, [0., 0., 0.5, 0.5]);
    // Source sub-rectangle excludes adjacent blue even under displacement.
    let p = pixels::sample(
        &[0, 0, 255, 255, 255, 0, 0, 255],
        2,
        1,
        crate::canvas_protocol::Rect {
            x: 0.,
            y: 0.,
            width: 0.5,
            height: 1.,
        },
        [3., 0.5],
    );
    assert_eq!(p, [0., 0., 1., 1.]);
    // Very small but valid fractional extents must remain bounded, including
    // an upper edge rounded to the same floating-point coordinate as its origin.
    let p = pixels::sample(
        &[0, 0, 255, 255, 255, 0, 0, 255],
        2,
        1,
        crate::canvas_protocol::Rect {
            x: 0.5,
            y: 0.,
            width: f64::MIN_POSITIVE,
            height: 1.,
        },
        [0.5, 0.5],
    );
    assert!(p.into_iter().all(f32::is_finite));
    assert!(pixels::ellipse_distance([100., 100.], [0., 0.], [f64::MIN_POSITIVE; 2]).is_finite());
}

#[test]
fn polygon_union_ellipse_exclusion_feather_and_image_alpha_masks() {
    let mut fill = solid(rgb(255, 0, 0), 8, 8);
    fill["masks"] = json!([
        {"type":"polygon","contours":[[[0,0],[4,0],[4,8],[0,8]],[[4,4],[8,4],[8,8],[4,8]]]},
        {"type":"ellipse","center":[2,4],"radius":[1,1],"invert":true}
    ]);
    let image = raster(composite(vec![fill], 8, 8), Sources::new());
    assert_eq!(pixel(&image, 6, 1)[3], 0);
    assert_eq!(pixel(&image, 6, 6)[3], 255);
    assert!(pixel(&image, 2, 4)[3] < 100);
    assert_eq!(pixel(&image, 0, 0)[3], 255);
    assert_eq!(pixels::coverage(2., 4., false), 0.5);
    assert_eq!(pixels::coverage(-2., 4., true), 0.5);
    assert!((pixels::ellipse_distance([3., 0.], [0., 0.], [5., 5.]) - 2.).abs() < 1e-12);
    let image = Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::from_fn(4, 4, |x, _| {
            image::Rgba([0, 0, 0, if x < 2 { 255 } else { 0 }])
        }),
    )]));
    let mut sources = Sources::new();
    sources.insert("mask.png".into(), image);
    let mut fill = solid(rgb(255, 255, 255), 4, 4);
    fill["masks"] =
        json!([{"type":"image_alpha","asset":"mask.png","destination":rect(0.,0.,4.,4.)}]);
    let image = raster(composite(vec![fill], 4, 4), sources);
    assert_eq!(pixel(&image, 0, 0), [255; 4]);
    assert_eq!(pixel(&image, 3, 0), [0; 4]);
}

#[test]
fn grid_displacement_is_fractional_and_strength_mask_is_independent_of_alpha() {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(8, 4, |x, _| image::Rgba([x as u8 * 30, 0, 0, 255]))
        .save(dir.path().join("source.png"))
        .unwrap();
    let assets = crate::assets::AssetRoot::new(dir.path()).unwrap();
    let op = json!({"type":"image","asset":"source.png","destination":rect(0.,0.,8.,4.),"displacement":{"columns":2,"rows":2,"offsets":[[0.5,0],[0.5,0],[0.5,0],[0.5,0]],"masks":[{"type":"polygon","contours":[[[0,0],[4,0],[4,4],[0,4]]]}]}});
    let prepared =
        PreparedFrame::prepare_v2(frame(composite(vec![op], 8, 4), 8, 4), grid(), &assets).unwrap();
    let image = &prepared.canvas.unwrap().composites[0];
    assert_eq!(pixel(image, 1, 1), [0, 0, 45, 255]);
    assert_eq!(pixel(image, 6, 1), [0, 0, 180, 255]);
}

#[test]
fn gradients_strokes_and_fractional_local_clipping_are_bounded() {
    let image = raster(
        composite(
            vec![
                json!({"type":"stroke","points":[[-3,1],[4,1]],"width":1,"brush":{"type":"linear","start":[0,0],"end":[4,0],"stops":[{"offset":0,"color":rgb(255,0,0)},{"offset":1,"color":rgb(0,0,255)}]}}),
            ],
            4,
            4,
        ),
        Sources::new(),
    );
    assert!(pixel(&image, 0, 0)[2] > pixel(&image, 3, 0)[2]);
    assert_eq!(pixel(&image, 0, 3), [0; 4]);
    assert_eq!(
        pixels::rect_coverage(
            crate::canvas_protocol::Rect {
                x: 0.25,
                y: 0.,
                width: 1.,
                height: 1.
            },
            [0.5, 0.5]
        ),
        0.75
    );
}

#[test]
fn strict_protocol_rejects_invalid_masks_sources_grids_limits_and_unknown_fields() {
    let valid = composite(vec![solid(rgb(1, 2, 3), 8, 8)], 8, 8);
    for (pointer, bad) in [
        ("/width", json!(0)),
        ("/height", json!(4097)),
        ("/opacity", json!(2)),
        ("/operations/0/brush/type", json!("shader")),
        ("/operations/0/masks", json!(null)),
        ("/operations/0/destination/x", json!(-1)),
    ] {
        let mut c = valid.clone();
        if let Some(v) = c.pointer_mut(pointer) {
            *v = bad;
        } else if pointer == "/opacity" {
            c["opacity"] = bad;
        } else {
            c["operations"][0]["masks"] = bad;
        }
        let wire = json!({"frame":1,"textLayers":[],"sprites":[],"canvas":{"width":8,"height":8,"composites":[c]}});
        assert!(
            serde_json::from_value::<FrameV2>(wire).map_or(true, |f| f.validate(grid()).is_err()),
            "{pointer}"
        );
    }
    for extra in [
        json!({"source":null}),
        json!({"source":rect(0.,0.,2.,1.)}),
        json!({"displacement":{"columns":2,"rows":2,"offsets":[[0,0]]}}),
        json!({"unknown":false}),
    ] {
        let mut op = json!({"type":"image","asset":"a.png","destination":rect(0.,0.,8.,8.)});
        op.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let wire = json!({"frame":1,"textLayers":[],"sprites":[],"canvas":{"width":8,"height":8,"composites":[composite(vec![op],8,8)]}});
        assert!(
            serde_json::from_value::<FrameV2>(wire).map_or(true, |f| f.validate(grid()).is_err())
        );
    }
}

#[test]
fn source_replacement_cache_reuse_and_snapshot_clear_retire_dynamic_images() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mutable.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
        .save(&path)
        .unwrap();
    let assets = crate::assets::AssetRoot::new(dir.path()).unwrap();
    let c = composite(
        vec![json!({"type":"image","asset":"mutable.png","destination":rect(0.,0.,4.,4.)})],
        4,
        4,
    );
    let first = PreparedFrame::prepare_v2(frame(c.clone(), 4, 4), grid(), &assets).unwrap();
    let second = PreparedFrame::prepare_v2(frame(c.clone(), 4, 4), grid(), &assets).unwrap();
    assert_eq!(first.resources.compositing.as_ref().unwrap().builds, 1);
    assert_eq!(second.resources.compositing.as_ref().unwrap().builds, 0);
    assert!(Arc::ptr_eq(
        &first.canvas.as_ref().unwrap().composites[0],
        &second.canvas.as_ref().unwrap().composites[0]
    ));
    image::RgbaImage::from_pixel(7, 5, image::Rgba([0, 255, 0, 255]))
        .save(&path)
        .unwrap();
    let changed = PreparedFrame::prepare_v2(frame(c, 4, 4), grid(), &assets).unwrap();
    assert_eq!(changed.resources.compositing.as_ref().unwrap().builds, 1);
    assert_eq!(
        pixel(&changed.canvas.as_ref().unwrap().composites[0], 0, 0),
        [0, 255, 0, 255]
    );
    let clear = serde_json::from_value(
        json!({"frame":2,"textLayers":[],"sprites":[],"canvas":{"width":4,"height":4}}),
    )
    .unwrap();
    assert!(
        PreparedFrame::prepare_v2(clear, grid(), &assets)
            .unwrap()
            .canvas
            .unwrap()
            .composites
            .is_empty()
    );
    assert_eq!(
        pixel(&first.canvas.unwrap().composites[0], 0, 0),
        [0, 0, 255, 255]
    );
    assert!(assets.resolve(Path::new("../missing.png")).is_err());
}

#[test]
fn unrelated_sources_do_not_invalidate_composite_prefixes_or_masks() {
    let source = || {
        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_pixel(4, 4, image::Rgba([100, 120, 140, 255])),
        )]))
    };
    let mut sources = Sources::from([("base.png".into(), source()), ("mask.png".into(), source())]);
    let mask = json!({"type":"image_alpha","asset":"mask.png","destination":rect(0.,0.,4.,4.)});
    let mut overlay = solid(rgb(200, 50, 10), 4, 4);
    overlay["masks"] = json!([mask]);
    let mut c: Composite = serde_json::from_value(composite(vec![
        json!({"type":"image","asset":"base.png","destination":rect(0.,0.,4.,4.),"masks":[mask]}),
        overlay,
    ], 4, 4)).unwrap();
    let mut cache = CompositeCache::default();
    let (first, stats) = cache.prepare(&[c.clone()], &sources).unwrap();
    assert_eq!((stats.builds, stats.mask_builds), (1, 1));
    sources.insert("unrelated.png".into(), source());
    let (same, stats) = cache.prepare(&[c.clone()], &sources).unwrap();
    assert_eq!((stats.builds, stats.mask_builds), (0, 0));
    assert!(Arc::ptr_eq(&first[0], &same[0]));
    if let crate::composite_protocol::Operation::Fill { opacity, .. } = &mut c.operations[1] {
        *opacity = 0.5;
    }
    let (_, stats) = cache.prepare(&[c.clone()], &sources).unwrap();
    assert_eq!((stats.builds, stats.mask_builds), (1, 0));
    // A changed referenced alpha source invalidates both its mask and the
    // first-image prefix that used it; unrelated assets never enter either key.
    sources.insert(
        "mask.png".into(),
        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(4, 4),
        )])),
    );
    let (changed, stats) = cache.prepare(&[c], &sources).unwrap();
    assert_eq!((stats.builds, stats.mask_builds), (1, 1));
    assert_eq!(pixel(&changed[0], 0, 0), [0, 0, 0, 0]);
}

#[test]
fn capability_negotiation_example_fixture_and_clear_are_compatible() {
    use crate::protocol::{Message, parse};
    use crate::transport::{Session, Update};
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    for negotiated in [false, true] {
        let mut capabilities = vec!["graphical_canvas"];
        if negotiated {
            capabilities.push("canvas_compositing");
        }
        let hello = json!({"protocol":2,"type":"hello","title":"compositor test","assetRoot":root,"grid":{"columns":135,"rows":36,"cellWidth":10,"cellHeight":20},"requiredCapabilities":capabilities});
        let mut session = Session::default();
        session
            .prepare(parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
            .unwrap();
        let result =
            session.prepare(parse(include_bytes!("../fixtures/compositing/frame.json")).unwrap());
        assert_eq!(result.is_ok(), negotiated);
        let clear = json!({"protocol":2,"type":"frame","frame":2,"textLayers":[],"sprites":[],"canvas":{"width":64,"height":48,"composites":[]}});
        let result = session.prepare(parse(&serde_json::to_vec(&clear).unwrap()).unwrap());
        assert_eq!(result.is_ok(), negotiated);
        if let Ok(Update::Frame(frame)) = result {
            assert!(frame.canvas.unwrap().composites.is_empty());
        }
        let Message::Hello(_) = parse(&serde_json::to_vec(&hello).unwrap()).unwrap().message else {
            panic!()
        };
    }
    for version in [1, 2] {
        let hello = json!({"protocol":version,"type":"hello","title":"test","assetRoot":root,"grid":{"columns":135,"rows":36,"cellWidth":10,"cellHeight":20},"requiredCapabilities":["canvas_compositing"]});
        assert!(parse(&serde_json::to_vec(&hello).unwrap()).is_err());
    }
}

#[test]
fn work_and_live_generation_limits_reject_before_replacing_valid_outputs() {
    let c: Composite = serde_json::from_value(composite(
        vec![solid(rgb(10, 20, 30), 1350, 720); 80],
        1350,
        720,
    ))
    .unwrap();
    let error = CompositeCache::default()
        .prepare(&[c], &Sources::new())
        .unwrap_err();
    assert!(error.contains("work units"));
    let mut cache = CompositeCache::default();
    let mut held = Vec::new();
    let mut rejected = false;
    for n in 0..9 {
        let c: Composite = serde_json::from_value(composite(
            vec![solid(rgb(n, 10, 20), 1600, 1600)],
            1600,
            1600,
        ))
        .unwrap();
        match cache.prepare(&[c], &Sources::new()) {
            Ok((images, _)) => held.extend(images),
            Err(error) => {
                assert!(error.contains("64 MiB"));
                rejected = true;
                break;
            }
        }
    }
    assert!(rejected);
    assert_eq!(pixel(&held[0], 0, 0), [20, 10, 0, 255]);
    held.clear();
    let c: Composite = serde_json::from_value(composite(
        vec![solid(rgb(90, 10, 20), 1600, 1600)],
        1600,
        1600,
    ))
    .unwrap();
    assert!(cache.prepare(&[c], &Sources::new()).is_ok());
}

// A bounded workload shaped like the approved reference: one sampled backdrop,
// masked sky, three displaced ribbons with 35 screen streaks and mist each,
// plus local masked firelight at night. Synthetic replaceable source images.
fn representative_scene(night: bool, time: f64) -> Composite {
    let asset = if night { "night.png" } else { "day.png" };
    let mut ops = vec![json!({"type":"image","asset":asset,"destination":rect(0.,0.,1350.,720.)})];
    let contours = if night {
        json!([[
            [627, 51],
            [879, 13],
            [1098, 0],
            [1241, 0],
            [1241, 153],
            [1211, 167],
            [1170, 140],
            [1121, 124],
            [1091, 161],
            [1071, 204],
            [1034, 236],
            [964, 246],
            [874, 229],
            [798, 248],
            [706, 218],
            [627, 195]
        ]])
    } else {
        json!([
            [
                [430, 0],
                [1080, 0],
                [1080, 173],
                [1020, 221],
                [955, 264],
                [863, 286],
                [715, 321],
                [609, 282],
                [493, 249],
                [430, 201]
            ],
            [
                [218, 145],
                [401, 155],
                [413, 230],
                [331, 235],
                [281, 251],
                [215, 235]
            ],
            [[1239, 0], [1350, 0], [1350, 101], [1273, 122], [1239, 88]]
        ])
    };
    let masks = vec![json!({"type":"polygon","contours":contours})];
    let mut strength = vec![json!({"type":"polygon","contours":contours,"feather":80})];
    if night {
        strength.push(json!({"type":"ellipse","center":[1162,97],"radius":[66,66],"invert":true,"feather":48}));
    }
    let shift = if night {
        32. * (time / 105.).sin()
    } else {
        48. * (time / 92.).sin()
    };
    ops.push(json!({"type":"image","asset":asset,"destination":rect(0.,0.,1350.,720.),"masks":masks,"displacement":{"columns":2,"rows":2,"offsets":vec![[-shift,0.];4],"masks":strength}}));
    for fall in 0..3 {
        let x = 790. + fall as f64 * 125.;
        let y = 330. + fall as f64 * 42.;
        let h = 70. + fall as f64 * 17.;
        let points = json!([[[x, y], [x + 12., y], [x + 4., y + h], [x - 9., y + h]]]);
        let offsets: Vec<_> = (0..48)
            .flat_map(|r| {
                let p = r as f64 / 47.;
                let v = 2.6 * (p * h / 8. - time * 5.).sin() * (std::f64::consts::PI * p).sin();
                vec![[0., v]; 2]
            })
            .collect();
        ops.push(json!({"type":"image","asset":asset,"destination":rect(x-12.,y,30.,h),"source":rect((x-12.)/1350.,y/720.,30./1350.,h/720.),"opacity":0.88,"masks":[{"type":"polygon","contours":points}],"displacement":{"columns":2,"rows":48,"offsets":offsets}}));
        for lane in 0..7 {
            for streak in 0..5 {
                let p =
                    (time / 2.2 + streak as f64 / 5. + lane as f64 * 0.143 + fall as f64 * 0.217)
                        .fract();
                ops.push(json!({"type":"stroke","points":[[x-3.+lane as f64,y+p*h],[x-4.+lane as f64,y+(p+0.06).min(1.)*h]],"width":0.8,"blend":"screen","opacity":0.45*(std::f64::consts::PI*p).sin(),"brush":{"type":"solid","color":rgb(190,232,255)}}));
            }
        }
        ops.push(json!({"type":"fill","destination":rect(x-15.,y+h-6.,30.,12.),"blend":"screen","brush":{"type":"radial","center":[x,y+h],"radius":[15,6],"stops":[{"offset":0,"color":rgb(178,224,255),"opacity":0.16},{"offset":1,"color":rgb(178,224,255),"opacity":0}]}}));
    }
    if night {
        ops.push(json!({"type":"fill","destination":rect(20.,358.,224.,224.),"blend":"screen","opacity":0.15+0.03*time.sin(),"brush":{"type":"radial","center":[132,470],"radius":[112,112],"stops":[{"offset":0,"color":rgb(255,172,54)},{"offset":0.4,"color":rgb(239,125,35),"opacity":0.2},{"offset":1,"color":rgb(225,116,30),"opacity":0}]}}));
        ops.push(json!({"type":"fill","destination":rect(107.,429.,42.,69.),"blend":"screen","masks":[{"type":"polygon","contours":[[[116,496],[111,478],[128,469],[130.+2.*time.sin(),440.+4.*time.cos()],[139,465],[145,479],[142,497]]]},{"type":"polygon","contours":[[[111,494],[112,446],[142,429],[147,496]]]}],"brush":{"type":"linear","start":[0,440],"end":[0,501],"stops":[{"offset":0,"color":rgb(255,230,155),"opacity":0},{"offset":0.24,"color":rgb(255,236,174),"opacity":0.5},{"offset":0.78,"color":rgb(255,244,208),"opacity":0.7},{"offset":1,"color":rgb(255,166,43),"opacity":0}]}}));
    }
    let mut c = composite(ops, 1350, 720);
    c["id"] = json!(if night { "night" } else { "day" });
    serde_json::from_value(c).unwrap()
}

#[test]
#[ignore = "explicit bounded release-mode CPU benchmark; no window or audio"]
fn benchmark_representative_day_night_composition() {
    let mut sources = Sources::new();
    for (name, base) in [("day.png", 70), ("night.png", 10)] {
        let image = image::RgbaImage::from_fn(1717, 916, |x, y| {
            image::Rgba([
                ((x + y) % 120) as u8 + base,
                (y % 100) as u8 + base,
                (x % 100) as u8 + base,
                255,
            ])
        });
        sources.insert(
            name.into(),
            Arc::new(gpui::RenderImage::new(vec![image::Frame::new(image)])),
        );
    }
    for themes in [vec![false], vec![true], vec![false, true]] {
        let mut cache = CompositeCache::default();
        let mut samples = Vec::new();
        let mut retained = std::collections::VecDeque::new();
        let mut max_bytes = 0;
        let mut max_work = 0;
        let mut cold_masks = 0;
        for i in 0..25 {
            let scenes: Vec<_> = themes
                .iter()
                .map(|night| representative_scene(*night, 6.2 + i as f64 / 30.))
                .collect();
            let canvas: crate::canvas_protocol::Canvas =
                serde_json::from_value(json!({"width":1350,"height":720})).unwrap();
            crate::composite_protocol::validate(&scenes, &canvas).unwrap();
            let start = std::time::Instant::now();
            let (images, stats) = cache.prepare(&scenes, &sources).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.;
            if i == 0 {
                eprintln!(
                    "composition themes={themes:?} cold_ms={ms:.3} masks={}",
                    stats.mask_builds
                );
                cold_masks = stats.mask_builds;
            } else {
                samples.push(ms);
            }
            max_bytes = max_bytes.max(stats.reserved_bytes);
            max_work = max_work.max(stats.work);
            // Simulate current view plus two queued and one rendering snapshot.
            retained.push_back(images);
            if retained.len() > 4 {
                retained.pop_front();
            }
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "composition themes={themes:?} samples={} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} max_reserved_bytes={max_bytes} work={max_work} cold_masks={cold_masks}",
            samples.len(),
            samples.iter().sum::<f64>() / samples.len() as f64,
            samples[samples.len() / 2],
            samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
        );
        assert!(max_bytes <= crate::composite_cache::MAX_BYTES);
    }
}

#[test]
#[ignore = "requires ICHILOTO_COMPOSITE_REPLAY NDJSON and ICHILOTO_COMPOSITE_REPORT directory"]
fn replay_engine_composite_packets_without_a_native_window() {
    use crate::protocol::{Message, parse};
    use crate::transport::{Session, Update};
    let input =
        std::fs::read_to_string(std::env::var("ICHILOTO_COMPOSITE_REPLAY").unwrap()).unwrap();
    let directory = std::path::PathBuf::from(std::env::var("ICHILOTO_COMPOSITE_REPORT").unwrap());
    let capture_every: usize =
        std::env::var("ICHILOTO_COMPOSITE_CAPTURE_EVERY").map_or(30, |value| {
            value
                .parse()
                .expect("capture interval must be a positive integer")
        });
    assert!(capture_every > 0);
    std::fs::create_dir_all(&directory).unwrap();
    let mut session = Session::default();
    let mut results = Vec::new();
    let mut retained = std::collections::VecDeque::new();
    let mut failures = Vec::new();
    for (index, line) in input.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let value: Value = serde_json::from_str(line).unwrap();
        let label = value
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("frame")
            .to_owned();
        let wire = serde_json::to_vec(value.get("message").unwrap_or(&value)).unwrap();
        let parsed = parse(&wire).unwrap();
        if !matches!(parsed.message, Message::FrameV2(_)) {
            session.prepare(parsed).unwrap();
            continue;
        }
        let start = std::time::Instant::now();
        let prepared = session.prepare(parsed);
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        match prepared {
            Ok(Update::Frame(frame)) => {
                results.push(json!({"index":index,"label":label,"elapsedMs":elapsed,"resources":frame.resources}));
                if (results.len() - 1) % capture_every == 0
                    && let Some(canvas) = &frame.canvas
                {
                    for (i, image) in canvas.composites.iter().enumerate() {
                        let spec = &canvas.source.composites.as_ref().unwrap()[i];
                        let rgba = image::RgbaImage::from_fn(spec.width, spec.height, |x, y| {
                            let mut p = pixel(image, x, y);
                            p.swap(0, 2);
                            image::Rgba(p)
                        });
                        rgba.save(directory.join(format!("frame-{index:03}-composite-{i}.png")))
                            .unwrap();
                    }
                }
                retained.push_back(frame);
                if retained.len() > 4 {
                    retained.pop_front();
                }
            }
            Err(error) => {
                failures.push(format!("{index}/{label}: {error}"));
                results
                    .push(json!({"index":index,"label":label,"elapsedMs":elapsed,"error":error}));
            }
            _ => panic!("expected a prepared frame"),
        }
    }
    std::fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    assert!(!results.is_empty());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
