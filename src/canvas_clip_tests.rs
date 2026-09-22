use crate::{
    assets::AssetRoot,
    canvas::clipped_bounds,
    canvas_protocol::Rect,
    protocol::{self, Capability, FrameV2, Hello, Incoming, Message, Version},
    state::{PreparedFrame, RendererState},
    transport::{Session, Update},
    viewport::ViewportTransform,
};
use std::{fs, path::PathBuf, sync::Arc};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn wire(name: &str) -> Vec<u8> {
    fs::read(root().join("canvas-clip-opacity").join(name)).unwrap()
}

fn hello() -> Hello {
    let mut value: serde_json::Value = serde_json::from_slice(&wire("hello.json")).unwrap();
    value["assetRoot"] = serde_json::json!(root());
    let Message::Hello(hello) = protocol::parse(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .message
    else {
        panic!()
    };
    hello
}

fn frame(name: &str) -> FrameV2 {
    let Message::FrameV2(frame) = protocol::parse(&wire(name)).unwrap().message else {
        panic!()
    };
    frame
}

#[test]
fn clipping_wire_corpus_negotiates_validates_and_preserves_accepted_frames() {
    let manifest: serde_json::Value = serde_json::from_slice(&wire("manifest.json")).unwrap();
    for case in manifest["cases"].as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let stage = case["stage"].as_str().unwrap();
        let mut config = hello();
        let assets = AssetRoot::new(&root()).unwrap();
        let accepted =
            PreparedFrame::prepare_v2(frame("valid-combined.json"), config.grid, &assets).unwrap();
        let original = accepted.canvas.as_ref().unwrap().images[0].clone();
        let original_source = accepted.canvas.as_ref().unwrap().source.clone();
        let mut state = RendererState {
            hello: config.clone(),
            frame: Some(accepted),
        };
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
        let parsed = protocol::parse(&wire(name));
        match stage {
            "schema" => match parsed {
                Err(_) => (),
                Ok(incoming) => {
                    let Message::FrameV2(frame) = &incoming.message else {
                        panic!("{name}")
                    };
                    assert!(frame.validate(config.grid).is_err(), "{name}");
                    assert!(session.prepare(incoming).is_err(), "{name}");
                }
            },
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
                    .unwrap_or_else(|| panic!("{name}"));
                assert_eq!(
                    error.contains("negotiated"),
                    stage == "session",
                    "{name}: {error}"
                );
            }
            "accept" => {
                let incoming = parsed.unwrap_or_else(|error| panic!("{name}: {error}"));
                if let Message::Hello(config) = incoming.message {
                    let mut output = Vec::new();
                    protocol::write_event(
                        &mut output,
                        Version::V2,
                        &protocol::Event::Ready {
                            capabilities: config.required_capabilities.clone(),
                        },
                    )
                    .unwrap();
                    let ready: serde_json::Value = serde_json::from_slice(&output).unwrap();
                    assert_eq!(ready["capabilities"], manifest["capabilities"]);
                    assert!(
                        config
                            .required_capabilities
                            .contains(&Capability::CanvasClipOpacity)
                    );
                } else {
                    let Update::Frame(next) = session
                        .prepare(incoming)
                        .unwrap_or_else(|error| panic!("{name}: {error}"))
                    else {
                        panic!("{name}")
                    };
                    let expected = frame(name).canvas;
                    state.replace(next);
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
                }
                continue;
            }
            _ => panic!("{name}: unknown stage {stage}"),
        }
        let retained = state.frame.as_ref().unwrap().canvas.as_ref().unwrap();
        assert_eq!(retained.source, original_source, "{name}");
        assert!(Arc::ptr_eq(&retained.images[0], &original), "{name}");
    }
}

#[test]
fn clipping_retains_full_texture_mapping_and_centers_only_the_parent() {
    let destination = Rect {
        x: 40.0,
        y: 40.0,
        width: 160.0,
        height: 24.0,
    };
    let clip = Rect {
        x: 60.5,
        y: 42.25,
        width: 50.5,
        height: 18.5,
    };
    for (width, height) in [(320.0, 180.0), (160.0, 180.0), (1000.0, 800.0), (0.0, 0.0)] {
        let transform = ViewportTransform::fit(320.0, 180.0, width, height);
        let scale = transform.scale;
        let (mask, content) = clipped_bounds(destination, Some(clip), transform).unwrap();
        assert_eq!(
            (mask.left, mask.top, mask.width, mask.height),
            (60.5 * scale, 42.25 * scale, 50.5 * scale, 18.5 * scale)
        );
        assert_eq!(
            (content.left, content.top, content.width, content.height),
            (-20.5 * scale, -2.25 * scale, 160.0 * scale, 24.0 * scale)
        );
        assert_eq!(
            mask.left + content.left + transform.offset_x,
            destination.x as f32 * scale + transform.offset_x
        );
        assert_eq!(
            mask.top + content.top + transform.offset_y,
            destination.y as f32 * scale + transform.offset_y
        );
        let (full, unchanged) = clipped_bounds(destination, None, transform).unwrap();
        assert_eq!(full, destination.paint(transform));
        assert_eq!((unchanged.left, unchanged.top), (0.0, 0.0));
        let (bounded, _) = clipped_bounds(
            destination,
            Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 320.0,
                height: 180.0,
            }),
            transform,
        )
        .unwrap();
        assert_eq!(bounded, full);
    }
    let transform = ViewportTransform::fit(320.0, 180.0, 320.0, 180.0);
    for (x, y) in [(200.0, 40.0), (40.0, 64.0), (250.0, 100.0)] {
        assert!(
            clipped_bounds(
                destination,
                Some(Rect {
                    x,
                    y,
                    width: 20.0,
                    height: 20.0
                }),
                transform
            )
            .is_none()
        );
    }
}

#[test]
fn gauge_value_and_text_fade_updates_reuse_prepared_sources_and_clear_on_omission() {
    let config = hello();
    let assets = AssetRoot::new(&root()).unwrap();
    let initial =
        PreparedFrame::prepare_v2(frame("valid-combined.json"), config.grid, &assets).unwrap();
    let region = initial.canvas.as_ref().unwrap().images[0].clone();
    for step in 1..=100 {
        let mut source = frame("valid-combined.json");
        let canvas = source.canvas.as_mut().unwrap();
        canvas.images[0].clip_rect = Some(Rect {
            x: 40.0,
            y: 40.0,
            width: 1.6 * f64::from(step),
            height: 24.0,
        });
        canvas.text_layers[0].opacity = Some(f64::from(step) / 100.0);
        let next = PreparedFrame::prepare_v2(source, config.grid, &assets).unwrap();
        assert_eq!(
            (next.resources.png_decodes, next.resources.region_builds),
            (0, 0)
        );
        assert!(Arc::ptr_eq(
            &next.canvas.as_ref().unwrap().images[0],
            &region
        ));
    }
    let next =
        PreparedFrame::prepare_v2(frame("valid-omitted.json"), config.grid, &assets).unwrap();
    let canvas = &next.canvas.as_ref().unwrap().source;
    assert!(canvas.images[0].clip_rect.is_none());
    assert!(canvas.text_layers[0].clip_rect.is_none());
    assert!(canvas.text_layers[0].opacity.is_none());
    assert!(Arc::ptr_eq(
        &next.canvas.as_ref().unwrap().images[0],
        &region
    ));
}

#[test]
fn nonfinite_clip_and_text_opacity_reject_before_preparation() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
        for member in 0..4 {
            let mut source = frame("valid-combined.json").canvas.unwrap();
            let clip = source.images[0].clip_rect.as_mut().unwrap();
            match member {
                0 => clip.x = value,
                1 => clip.y = value,
                2 => clip.width = value,
                _ => clip.height = value,
            }
            assert!(source.validate().is_err());
            let mut source = frame("valid-combined.json").canvas.unwrap();
            let clip = source.text_layers[0].clip_rect.as_mut().unwrap();
            match member {
                0 => clip.x = value,
                1 => clip.y = value,
                2 => clip.width = value,
                _ => clip.height = value,
            }
            assert!(source.validate().is_err());
        }
        let mut source = frame("valid-combined.json").canvas.unwrap();
        source.text_layers[0].opacity = Some(value);
        assert!(source.validate().is_err());
    }
}
