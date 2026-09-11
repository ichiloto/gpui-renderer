//! Typed NDJSON boundary. No renderer code writes directly to stdout.
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

pub const VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct Incoming {
    pub protocol: u32,
    #[serde(flatten)]
    pub message: Message,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Frame(Frame),
    Shutdown,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub title: String,
    pub asset_root: PathBuf,
    pub grid: Grid,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Grid {
    pub fn validate(self) -> Result<(), String> {
        if self.columns == 0 || self.rows == 0 || self.cell_width == 0 || self.cell_height == 0 {
            return Err("grid and cell dimensions must be positive integers".into());
        }
        if self.columns > 512
            || self.rows > 256
            || self.cell_width > 256
            || self.cell_height > 256
            || self.columns * self.cell_width > 16384
            || self.rows * self.cell_height > 16384
        {
            return Err("grid exceeds S1 presentation limits".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub frame: u64,
    pub text: Vec<String>,
    pub sprites: Vec<Sprite>,
}

impl Frame {
    pub fn validate(&self, grid: Grid) -> Result<(), String> {
        if self.text.len() > grid.rows as usize
            || self
                .text
                .iter()
                .any(|row| row.chars().count() > grid.columns as usize)
        {
            return Err("text exceeds the configured grid".into());
        }
        if self
            .text
            .iter()
            .any(|row| row.chars().any(char::is_control))
        {
            return Err(
                "text rows must not contain control characters (including ANSI or tabs)".into(),
            );
        }
        if self.sprites.len() > 1024 {
            return Err("frame exceeds the S1 limit of 1024 sprites".into());
        }
        let mut ids = std::collections::HashSet::new();
        for sprite in &self.sprites {
            if sprite.id.is_empty() || !ids.insert(&sprite.id) {
                return Err("sprite ids must be nonempty and unique within a frame".into());
            }
            if sprite.width == 0
                || sprite.height == 0
                || sprite.width > 4096
                || sprite.height > 4096
            {
                return Err("sprite dimensions must be between 1 and 4096 logical pixels".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sprite {
    pub id: String,
    pub asset: PathBuf,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub anchor: Anchor,
    pub layer: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    BottomCenter,
}

pub fn parse(line: &[u8]) -> Result<Message, String> {
    let incoming: Incoming =
        serde_json::from_slice(line).map_err(|e| format!("invalid protocol message: {e}"))?;
    if incoming.protocol != VERSION {
        return Err(format!(
            "unsupported protocol version {}; expected {VERSION}",
            incoming.protocol
        ));
    }
    if let Message::Hello(hello) = &incoming.message {
        hello.grid.validate()?;
        if !hello.asset_root.is_absolute() {
            return Err("assetRoot must be an absolute directory path".into());
        }
        if hello.title.is_empty() || hello.title.chars().any(char::is_control) {
            return Err("title must be nonempty and contain no control characters".into());
        }
    }
    Ok(incoming.message)
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ready,
    Key { key: String },
    CloseRequested,
    Error { message: String },
}

#[derive(Serialize)]
struct Outgoing<'a> {
    protocol: u32,
    #[serde(flatten)]
    event: &'a Event,
}

pub fn write_event(mut writer: impl Write, event: &Event) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&Outgoing {
        protocol: VERSION,
        event,
    })?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// Best-effort fatal diagnostics without ever writing a pipe on the UI thread.
/// Routine protocol errors are logged by the existing stdout worker instead.
pub fn diagnostic(message: String) {
    std::thread::spawn(move || eprintln!("{message}"));
}

#[derive(Clone)]
pub struct ProtocolWriter(async_channel::Sender<Event>);

pub struct WriterCompletion {
    pub done: async_channel::Receiver<Result<(), String>>,
    pub failure: async_channel::Receiver<String>,
}

impl ProtocolWriter {
    pub fn close(&self) {
        self.0.close();
    }
    // The UI only enqueues. A slow/unread pipe can never block GPUI.
    pub fn send(&self, event: Event) -> Result<(), String> {
        self.0
            .try_send(event)
            .map_err(|e| format!("protocol output unavailable: {e}"))
    }

    pub fn start() -> (Self, WriterCompletion) {
        let (tx, rx) = async_channel::bounded(256);
        let (done_tx, done_rx) = async_channel::bounded(1);
        let (failure_tx, failure_rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut stdout = stdout.lock();
            let result: std::io::Result<()> = (|| {
                while let Ok(event) = rx.recv_blocking() {
                    write_event(&mut stdout, &event)?;
                    if let Event::Error { message } = event {
                        eprintln!("{message}");
                    }
                }
                Ok(())
            })();
            if let Err(error) = &result {
                diagnostic(format!("stdout write failed: {error}"));
                let _ = failure_tx.try_send(error.to_string());
            }
            let _ = done_tx.try_send(result.map_err(|error| error.to_string()));
        });
        (
            Self(tx),
            WriterCompletion {
                done: done_rx,
                failure: failure_rx,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const HELLO: &str = r#"{"protocol":1,"type":"hello","title":"Home","assetRoot":"/tmp","grid":{"columns":80,"rows":24,"cellWidth":16,"cellHeight":24}}"#;
    const FRAME: &str = r#"{"protocol":1,"type":"frame","frame":1,"text":["Home"],"sprites":[{"id":"test","asset":"test.png","x":8,"y":4,"width":32,"height":48,"anchor":"bottom_center","layer":100}]}"#;

    #[test]
    fn valid_hello() {
        assert!(matches!(
            parse(HELLO.as_bytes()).unwrap(),
            Message::Hello(_)
        ));
    }
    #[test]
    fn unsupported_version() {
        assert!(
            parse(HELLO.replace("\"protocol\":1", "\"protocol\":2").as_bytes())
                .unwrap_err()
                .contains("unsupported")
        );
    }
    #[test]
    fn invalid_grid_dimensions() {
        for field in ["columns", "rows", "cellWidth", "cellHeight"] {
            for bad in ["0", "-1", "1.5", "4294967295"] {
                let mut value: serde_json::Value = serde_json::from_str(HELLO).unwrap();
                value["grid"][field] = serde_json::from_str(bad).unwrap();
                assert!(parse(value.to_string().as_bytes()).is_err());
            }
        }
    }
    #[test]
    fn valid_frame_and_anchor() {
        let Message::Frame(frame) = parse(FRAME.as_bytes()).unwrap() else {
            panic!()
        };
        assert_eq!(frame.sprites[0].anchor, Anchor::BottomCenter);
        frame
            .validate(Grid {
                columns: 80,
                rows: 24,
                cell_width: 16,
                cell_height: 24,
            })
            .unwrap();
    }
    #[test]
    fn malformed_frame_rejected() {
        for bad in [
            FRAME.replace("bottom_center", "top_left"),
            FRAME.replace("\"width\":32,", ""),
            FRAME.replace("\"x\":8", "\"x\":\"8\""),
            FRAME.replace("\"sprites\":", "\"sprite\":"),
            "{".into(),
        ] {
            assert!(parse(bad.as_bytes()).is_err());
        }
    }
    #[test]
    fn shutdown_parsing() {
        assert!(matches!(
            parse(br#"{"protocol":1,"type":"shutdown"}"#).unwrap(),
            Message::Shutdown
        ));
    }
    #[test]
    fn events_are_single_json_lines() {
        for event in [
            Event::Ready,
            Event::CloseRequested,
            Event::Error {
                message: "bad\ninput".into(),
            },
            Event::Key { key: "W".into() },
        ] {
            let mut bytes = Vec::new();
            write_event(&mut bytes, &event).unwrap();
            assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["protocol"], 1);
            if let Event::Key { key } = event {
                assert_eq!(value["key"], key);
            }
        }
    }
}
