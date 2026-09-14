use crate::assets::AssetRoot;
use crate::diagnostics::Diagnostics;
use crate::protocol::{self, Capability, Hello, Incoming, Message, Sprite, Version};
use crate::state::PreparedFrame;
use std::io::{BufRead, Read};

pub enum Update {
    Hello(Version, Hello),
    Frame(PreparedFrame),
    Error(String),
    Fatal(String),
    Shutdown,
    Eof,
}

impl Update {
    pub fn observation(&self) -> Option<crate::diagnostics::FrameTrace> {
        match self {
            Self::Frame(frame) => frame.observation,
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Session {
    initialized: Option<(Version, Hello, AssetRoot)>,
}

impl Session {
    pub fn prepare(&mut self, incoming: Incoming) -> Result<Update, String> {
        if let Some((version, _, _)) = &self.initialized
            && *version != incoming.version
        {
            return Err(format!(
                "message protocol {} differs from session protocol {}",
                incoming.version.number(),
                version.number()
            ));
        }
        match incoming.message {
            Message::Hello(hello) => {
                if self.initialized.is_some() {
                    return Err("hello has already initialized this session".into());
                }
                let assets = AssetRoot::new(&hello.asset_root)?;
                self.initialized = Some((incoming.version, hello.clone(), assets));
                Ok(Update::Hello(incoming.version, hello))
            }
            Message::Frame(frame) => {
                let (_, hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                validate_capabilities(&frame.sprites, hello)?;
                Ok(Update::Frame(PreparedFrame::prepare(
                    frame, hello.grid, assets,
                )?))
            }
            Message::FrameV2(frame) => {
                let (_, hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                validate_capabilities(&frame.sprites, hello)?;
                if let Some(canvas) = &frame.canvas {
                    if !hello
                        .required_capabilities
                        .contains(&Capability::GraphicalCanvas)
                    {
                        return Err("canvas requires negotiated graphical_canvas capability".into());
                    }
                    if canvas
                        .images
                        .iter()
                        .any(|image| image.source_rect.is_some())
                        && !hello
                            .required_capabilities
                            .contains(&Capability::SpriteSourceRect)
                    {
                        return Err(
                            "canvas sourceRect requires negotiated sprite_source_rect capability"
                                .into(),
                        );
                    }
                }
                if frame.tile_batches.is_some()
                    && !hello
                        .required_capabilities
                        .contains(&Capability::TileBatches)
                {
                    return Err("tileBatches requires negotiated tile_batches capability".into());
                }
                Ok(Update::Frame(PreparedFrame::prepare_v2(
                    frame, hello.grid, assets,
                )?))
            }
            Message::Shutdown => Ok(Update::Shutdown),
        }
    }
}

fn validate_capabilities(sprites: &[Sprite], hello: &Hello) -> Result<(), String> {
    if sprites.iter().any(|sprite| sprite.source_rect.is_some())
        && !hello
            .required_capabilities
            .contains(&Capability::SpriteSourceRect)
    {
        return Err("sourceRect requires negotiated sprite_source_rect capability".into());
    }
    Ok(())
}

pub fn start_reader(diagnostics: Diagnostics) -> async_channel::Receiver<Update> {
    // Backpressure pauses only the reader. No polling loop or presentation timer.
    let (tx, rx) = async_channel::bounded(2);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        read_protocol_observed(stdin.lock(), &diagnostics, |update| {
            submit_update(&tx, update, &diagnostics)
        });
    });
    rx
}

fn submit_update(
    tx: &async_channel::Sender<Update>,
    update: Update,
    diagnostics: &Diagnostics,
) -> bool {
    let observation = update.observation();
    diagnostics.frame_queue_stage(observation, "submit_begin", || (tx.len(), tx.capacity()));
    let sent = tx.send_blocking(update).is_ok();
    diagnostics.frame_queue_stage(
        observation,
        if sent { "submit_end" } else { "submit_failed" },
        || (tx.len(), tx.capacity()),
    );
    sent
}

#[cfg(test)]
fn read_protocol(reader: impl BufRead, send: impl FnMut(Update) -> bool) {
    read_protocol_observed(reader, &Diagnostics::default(), send);
}

fn read_protocol_observed(
    mut reader: impl BufRead,
    diagnostics: &Diagnostics,
    mut send: impl FnMut(Update) -> bool,
) {
    let mut session = Session::default();
    loop {
        let mut bytes = Vec::new();
        let count = match reader
            .by_ref()
            .take(protocol::MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)
        {
            Ok(count) => count,
            Err(error) => {
                send(Update::Fatal(format!("stdin read failed: {error}")));
                break;
            }
        };
        if count == 0 {
            send(Update::Eof);
            break;
        }
        if count > protocol::MAX_LINE_BYTES {
            if bytes.last() != Some(&b'\n')
                && let Err(error) = reader.skip_until(b'\n')
            {
                send(Update::Fatal(format!("stdin read failed: {error}")));
                break;
            }
            if !send(Update::Error("protocol line exceeds 4 MiB".into())) {
                break;
            }
            continue;
        }
        let received_at = diagnostics.now();
        let update = protocol::parse(&bytes)
            .and_then(|incoming| {
                let observation = match &incoming.message {
                    Message::Frame(frame) => diagnostics.frame_received(
                        incoming.version.number(),
                        frame.frame,
                        received_at,
                    ),
                    Message::FrameV2(frame) => diagnostics.frame_received(
                        incoming.version.number(),
                        frame.frame,
                        received_at,
                    ),
                    _ => None,
                };
                let mut update = session.prepare(incoming)?;
                if let Update::Frame(frame) = &mut update {
                    frame.observation = observation;
                    diagnostics.frame_stage(observation, "accepted");
                    diagnostics.frame_resources(observation, frame.resources);
                }
                Ok(update)
            })
            .unwrap_or_else(Update::Error);
        let shutdown = matches!(update, Update::Shutdown);
        if !send(update) || shutdown {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_language_tile_fixtures_validate_exact_payloads_and_keep_last_display_on_error() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let dir = root.join("tile-batches");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        for case in manifest["cases"].as_array().unwrap() {
            let name = case["file"].as_str().unwrap();
            let stage = case["stage"].as_str().unwrap();
            let mut hello: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("hello.json")).unwrap()).unwrap();
            hello["assetRoot"] = serde_json::json!(root);
            if let Some(caps) = case.get("capabilities") {
                hello["requiredCapabilities"] = caps.clone();
            }
            let mut session = Session::default();
            let Update::Hello(_, config) = session
                .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
                .unwrap()
            else {
                panic!()
            };
            let assets = AssetRoot::new(&root).unwrap();
            let mut state = crate::state::RendererState {
                hello: config.clone(),
                frame: None,
            };
            // A genuine prior display survives parse/session/asset/budget failures.
            let source = protocol::parse(include_bytes!(
                "../fixtures/tile-batches/valid-tiles-text-player-ui.json"
            ))
            .unwrap();
            let Message::FrameV2(source) = source.message else {
                panic!()
            };
            state.replace(PreparedFrame::prepare_v2(source, config.grid, &assets).unwrap());
            let parsed = protocol::parse(&std::fs::read(dir.join(name)).unwrap());
            if stage == "parse" {
                assert!(parsed.is_err(), "{name}");
                continue;
            }
            let incoming = parsed.unwrap_or_else(|e| panic!("{name}: {e}"));
            let Message::FrameV2(ref frame) = incoming.message else {
                panic!("{name}")
            };
            if stage == "frame" {
                assert!(frame.validate(config.grid).is_err(), "{name}");
            } else {
                frame
                    .validate(config.grid)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            let update = session.prepare(incoming);
            if stage == "accept" {
                let Update::Frame(frame) = update.unwrap_or_else(|e| panic!("{name}: {e}")) else {
                    panic!()
                };
                state.replace(frame);
                if name.starts_with("clear-") {
                    assert!(state.frame.as_ref().unwrap().tile_batches.is_empty());
                }
                if name == "valid-full-viewport.json" {
                    assert_eq!(state.frame.as_ref().unwrap().resources.tile_cells, 4860);
                }
            } else {
                assert!(update.is_err(), "{name}");
                assert_eq!(
                    state.frame.as_ref().unwrap().tile_batches.len(),
                    1,
                    "{name}"
                );
                assert_eq!(
                    state.frame.as_ref().unwrap().resources.tile_cells,
                    2,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn traced_handoff_preserves_bounded_order_and_reports_receiver_close() {
        use std::io::Write;
        use std::sync::{Arc, Mutex, mpsc};
        struct Capture {
            bytes: Arc<Mutex<Vec<u8>>>,
            third_begin: mpsc::Sender<()>,
        }
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let record: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                self.bytes.lock().unwrap().extend_from_slice(bytes);
                if record["stage"] == "submit_begin" && record["sequence"] == 3 {
                    self.third_begin.send(()).unwrap();
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for version in [1, 2] {
            for close_receiver in [false, true] {
                let bytes = Arc::new(Mutex::new(Vec::new()));
                let (begin_tx, begin_rx) = mpsc::channel();
                let diagnostics = Diagnostics::with_writer(Capture {
                    bytes: bytes.clone(),
                    third_begin: begin_tx,
                });
                let mut input = hello(version);
                input.push(b'\n');
                for number in [7, 7, 2] {
                    let mut frame = serde_json::json!({"protocol":version,"type":"frame","frame":number,"sprites":[]});
                    frame[if version == 1 { "text" } else { "textLayers" }] = serde_json::json!([]);
                    input.extend(serde_json::to_vec(&frame).unwrap());
                    input.push(b'\n');
                }
                let mut frames = Vec::new();
                read_protocol_observed(input.as_slice(), &diagnostics, |u| {
                    if matches!(u, Update::Frame(_)) {
                        frames.push(u);
                    }
                    true
                });
                let mut frames = frames.into_iter();
                let (tx, rx) = async_channel::bounded(2);
                assert!(submit_update(&tx, frames.next().unwrap(), &diagnostics));
                assert!(submit_update(&tx, frames.next().unwrap(), &diagnostics));
                let third = frames.next().unwrap();
                let worker_diagnostics = diagnostics.clone();
                let worker_tx = tx.clone();
                let (done_tx, done_rx) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    done_tx
                        .send(submit_update(&worker_tx, third, &worker_diagnostics))
                        .unwrap();
                });
                // Observation acknowledges entry while capacity is still exhausted;
                // no elapsed-time threshold or arbitrary sleep controls this test.
                begin_rx.recv().unwrap();
                assert_eq!(tx.len(), 2);
                assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
                if close_receiver {
                    rx.close();
                }
                let first = rx.recv_blocking().unwrap();
                assert!(matches!(first, Update::Frame(frame) if frame.number == 7));
                assert_eq!(done_rx.recv().unwrap(), !close_receiver);
                worker.join().unwrap();
                let second = rx.recv_blocking().unwrap();
                assert!(matches!(second, Update::Frame(frame) if frame.number == 7));
                if !close_receiver {
                    let last = rx.recv_blocking().unwrap();
                    assert!(matches!(last, Update::Frame(frame) if frame.number == 2));
                }
                futures_lite::future::block_on(diagnostics.finish()).unwrap();
                let records: Vec<serde_json::Value> =
                    String::from_utf8(bytes.lock().unwrap().clone())
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect();
                for sequence in 1..=3 {
                    let observed: Vec<_> = records
                        .iter()
                        .filter(|r| {
                            r["sequence"] == sequence
                                && r["stage"]
                                    .as_str()
                                    .is_some_and(|s| s.starts_with("submit_"))
                        })
                        .collect();
                    assert_eq!(observed.len(), 2);
                    assert_eq!(observed[0]["stage"], "submit_begin");
                    assert_eq!(
                        observed[1]["stage"],
                        if sequence == 3 && close_receiver {
                            "submit_failed"
                        } else {
                            "submit_end"
                        }
                    );
                    assert_eq!(observed[0]["queued_updates"], sequence - 1);
                    for record in &observed {
                        assert_eq!(record["protocol"], version);
                        assert_eq!(record["frame"], if sequence == 3 { 2 } else { 7 });
                        assert_eq!(record["queue_capacity"], 2);
                        assert!(record["queued_updates"].as_u64().unwrap() <= 2);
                    }
                    assert!(observed[0]["at_ns"].as_u64() <= observed[1]["at_ns"].as_u64());
                }
                assert_eq!(records.last().unwrap()["dropped_records"], 0);
            }
        }
    }

    #[test]
    fn observed_frames_keep_duplicate_numbers_distinct_and_rejections_unaccepted() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for version in [1, 2] {
            let records = Arc::new(Mutex::new(Vec::new()));
            let diagnostics = Diagnostics::with_writer(Capture(records.clone()));
            let mut bytes = hello(version);
            bytes.push(b'\n');
            for (number, valid) in [(7, true), (8, false), (7, true), (2, true)] {
                let mut frame = serde_json::json!({"protocol":version,"type":"frame","frame":number,"sprites":[]});
                frame[if version == 1 { "text" } else { "textLayers" }] = serde_json::json!([]);
                if !valid {
                    frame["sprites"] = serde_json::json!([{"id":"bad","asset":"missing.png","x":1,"y":1,
                        "width":32,"height":48,"anchor":"bottom_center","layer":100}]);
                }
                bytes.extend_from_slice(&serde_json::to_vec(&frame).unwrap());
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(
                format!("malformed\n{{\"protocol\":{version},\"type\":\"shutdown\"}}\n").as_bytes(),
            );
            let mut updates = vec![];
            read_protocol_observed(bytes.as_slice(), &diagnostics, |u| {
                updates.push(u);
                true
            });
            let accepted: Vec<_> = updates
                .iter()
                .filter_map(|u| match u {
                    Update::Frame(frame) => {
                        assert!(frame.observation.is_some());
                        Some(frame.number)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(accepted, [7, 7, 2]);
            assert_eq!(
                updates
                    .iter()
                    .filter(|u| matches!(u, Update::Error(_)))
                    .count(),
                2
            );
            futures_lite::future::block_on(diagnostics.finish()).unwrap();
            let records: Vec<serde_json::Value> =
                String::from_utf8(records.lock().unwrap().clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            let received: Vec<_> = records
                .iter()
                .filter(|r| r["stage"] == "received")
                .collect();
            assert_eq!(received.len(), 4);
            for (index, frame) in [7, 8, 7, 2].iter().enumerate() {
                assert_eq!(received[index]["sequence"], index + 1);
                assert_eq!(received[index]["frame"], *frame);
                assert_eq!(received[index]["protocol"], version);
            }
            let accepted: Vec<_> = records
                .iter()
                .filter(|r| r["stage"] == "accepted")
                .collect();
            assert_eq!(
                accepted
                    .iter()
                    .map(|r| r["sequence"].as_u64().unwrap())
                    .collect::<Vec<_>>(),
                [1, 3, 4]
            );
            for record in accepted {
                let source = received[(record["sequence"].as_u64().unwrap() - 1) as usize];
                assert!(source["at_ns"].as_u64().unwrap() <= record["at_ns"].as_u64().unwrap());
            }
            assert!(
                !records
                    .iter()
                    .any(|r| r["stage"] == "replaced" || r["stage"] == "render_callback")
            );
            assert_eq!(records.last().unwrap()["dropped_records"], 0);
        }
    }
    #[test]
    fn malformed_input_recovers_then_shutdown_stops_reading() {
        let mut updates = Vec::new();
        read_protocol(
            &b"not json\n{\"protocol\":1,\"type\":\"shutdown\"}\nnot read\n"[..],
            |u| {
                updates.push(u);
                true
            },
        );
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], Update::Error(_)));
        assert!(matches!(updates[1], Update::Shutdown));
    }
    #[test]
    fn eof_is_distinct_from_shutdown() {
        let mut updates = Vec::new();
        read_protocol(&b""[..], |u| {
            updates.push(u);
            true
        });
        assert!(matches!(updates[0], Update::Eof));
    }
    #[test]
    fn oversized_input_is_drained_without_losing_next_message() {
        let mut input = vec![b'x'; protocol::MAX_LINE_BYTES + 10];
        input.extend_from_slice(b"\n{\"protocol\":1,\"type\":\"shutdown\"}\n");
        let mut updates = Vec::new();
        read_protocol(input.as_slice(), |u| {
            updates.push(u);
            true
        });
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], Update::Error(_)));
        assert!(matches!(updates[1], Update::Shutdown));
    }
    #[test]
    fn frame_before_hello_rejected() {
        let mut session = Session::default();
        assert!(
            session
                .prepare(
                    protocol::parse(
                        br#"{"protocol":1,"type":"frame","frame":1,"text":[],"sprites":[]}"#
                    )
                    .unwrap()
                )
                .is_err()
        );
    }
    fn hello(version: u32) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"protocol":version,"type":"hello","title":"Session","assetRoot":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures"),"grid":{"columns":80,"rows":24,"cellWidth":16,"cellHeight":24}})).unwrap()
    }
    #[test]
    fn source_rect_is_opt_in_for_both_versions_and_rejection_does_not_end_session() {
        for version in [1, 2] {
            for enabled in [false, true] {
                let mut hello: serde_json::Value = serde_json::from_slice(&hello(version)).unwrap();
                if enabled {
                    hello["requiredCapabilities"] = serde_json::json!(["sprite_source_rect"]);
                }
                let mut session = Session::default();
                session
                    .prepare(protocol::parse(&serde_json::to_vec(&hello).unwrap()).unwrap())
                    .unwrap();
                let mut frame = serde_json::json!({"protocol":version,"type":"frame","frame":1,"sprites":[{
                    "id":"player","asset":"test-sprite.png","x":8,"y":4,"width":32,"height":48,
                    "anchor":"bottom_center","layer":100,"sourceRect":{"x":0,"y":0,"width":16,"height":24}
                }]});
                frame[if version == 1 { "text" } else { "textLayers" }] = serde_json::json!([]);
                let cropped =
                    session.prepare(protocol::parse(&serde_json::to_vec(&frame).unwrap()).unwrap());
                assert_eq!(cropped.is_ok(), enabled);
                if let Err(error) = cropped {
                    assert!(error.contains("negotiated"));
                }
                frame["sprites"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("sourceRect");
                let Update::Frame(full) = session
                    .prepare(protocol::parse(&serde_json::to_vec(&frame).unwrap()).unwrap())
                    .unwrap()
                else {
                    panic!()
                };
                assert!(full.sprites[0].sprite.source_rect.is_none());
            }
        }
    }
    #[test]
    fn failed_hello_does_not_select_session_then_v2_can_initialize() {
        let mut session = Session::default();
        let mut bad: serde_json::Value = serde_json::from_slice(&hello(2)).unwrap();
        bad["assetRoot"] = "/nonexistent-ichiloto-assets".into();
        assert!(
            session
                .prepare(protocol::parse(&serde_json::to_vec(&bad).unwrap()).unwrap())
                .is_err()
        );
        assert!(session.initialized.is_none());
        assert!(matches!(
            session
                .prepare(protocol::parse(&hello(2)).unwrap())
                .unwrap(),
            Update::Hello(Version::V2, _)
        ));
        assert_eq!(session.initialized.as_ref().unwrap().0, Version::V2);
    }
    #[test]
    fn second_hello_and_mixed_versions_never_change_or_stop_session() {
        for version in [1, 2] {
            let mut session = Session::default();
            session
                .prepare(protocol::parse(&hello(version)).unwrap())
                .unwrap();
            for data in [
                hello(version),
                hello(3 - version),
                format!(r#"{{"protocol":{},"type":"shutdown"}}"#, 3 - version).into_bytes(),
                if version == 2 {
                    br#"{"protocol":1,"type":"frame","frame":1,"text":[],"sprites":[]}"#.to_vec()
                } else {
                    br#"{"protocol":2,"type":"frame","frame":1,"textLayers":[],"sprites":[]}"#
                        .to_vec()
                },
            ] {
                assert!(session.prepare(protocol::parse(&data).unwrap()).is_err());
                assert_eq!(session.initialized.as_ref().unwrap().0.number(), version);
            }
            assert!(matches!(
                session
                    .prepare(
                        protocol::parse(
                            format!(r#"{{"protocol":{version},"type":"shutdown"}}"#).as_bytes()
                        )
                        .unwrap()
                    )
                    .unwrap(),
                Update::Shutdown
            ));
        }
    }
    #[test]
    fn malformed_and_mixed_input_recovers_in_v2() {
        let mut bytes = hello(2);
        bytes.extend_from_slice(b"\nmalformed\n{\"protocol\":1,\"type\":\"shutdown\"}\n{\"protocol\":2,\"type\":\"frame\",\"frame\":1,\"textLayers\":[],\"sprites\":[]}\n{\"protocol\":2,\"type\":\"shutdown\"}\n");
        let mut updates = vec![];
        read_protocol(bytes.as_slice(), |u| {
            updates.push(u);
            true
        });
        assert_eq!(updates.len(), 5);
        assert!(matches!(updates[0], Update::Hello(Version::V2, _)));
        assert!(matches!(updates[1], Update::Error(_)));
        assert!(matches!(updates[2], Update::Error(_)));
        assert!(matches!(updates[3], Update::Frame(_)));
        if let Update::Frame(frame) = &updates[3] {
            assert!(frame.observation.is_none());
        }
        assert!(matches!(updates[4], Update::Shutdown));
    }
}
