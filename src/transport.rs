use crate::assets::AssetRoot;
use crate::protocol::{self, Hello, Message};
use crate::state::PreparedFrame;
use std::io::{BufRead, Read};

pub enum Update {
    Hello(Hello),
    Frame(PreparedFrame),
    Error(String),
    Fatal(String),
    Shutdown,
    Eof,
}

#[derive(Default)]
pub struct Session {
    initialized: Option<(Hello, AssetRoot)>,
}

impl Session {
    pub fn prepare(&mut self, message: Message) -> Result<Update, String> {
        match message {
            Message::Hello(hello) => {
                if self.initialized.is_some() {
                    return Err("hello has already initialized this session".into());
                }
                let assets = AssetRoot::new(&hello.asset_root)?;
                self.initialized = Some((hello.clone(), assets));
                Ok(Update::Hello(hello))
            }
            Message::Frame(frame) => {
                let (hello, assets) = self
                    .initialized
                    .as_ref()
                    .ok_or("frame requires a successful hello")?;
                Ok(Update::Frame(PreparedFrame::prepare(
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
}
