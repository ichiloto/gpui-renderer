use crate::assets::AssetRoot;
use crate::protocol::{self, Hello, Incoming, Message, Version};
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
                Ok(Update::Frame(PreparedFrame::prepare(
                    frame, hello.grid, assets,
                )?))
            }
            Message::FrameV2(frame) => {
                let (_, hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                Ok(Update::Frame(PreparedFrame::prepare_v2(
                    frame, hello.grid, assets,
                )?))
            }
            Message::Shutdown => Ok(Update::Shutdown),
        }
    }
}

pub fn start_reader() -> async_channel::Receiver<Update> {
    // Backpressure pauses only the reader. No polling loop or presentation timer.
    let (tx, rx) = async_channel::bounded(2);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        read_protocol(stdin.lock(), |update| tx.send_blocking(update).is_ok());
    });
    rx
}

fn read_protocol(mut reader: impl BufRead, mut send: impl FnMut(Update) -> bool) {
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
        let update = protocol::parse(&bytes)
            .and_then(|message| session.prepare(message))
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
        assert!(matches!(updates[4], Update::Shutdown));
    }
}
