//! Optional observation only: no protocol fields, no stderr writes on GPUI's thread.
use serde::Serialize;
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Default)]
pub struct Diagnostics(Option<Arc<Inner>>);

struct Inner {
    epoch: Instant,
    next_key: AtomicU64,
    next_frame: AtomicU64,
    dropped: Arc<AtomicUsize>,
    records: async_channel::Sender<Record>,
    done: async_channel::Receiver<Result<(), String>>,
}

#[derive(Serialize)]
#[serde(tag = "diagnostic", rename_all = "snake_case")]
enum Record {
    ClockAnchor {
        stage: &'static str,
        clock: &'static str,
        pid: u32,
        elapsed_before_ns: u64,
        host_ns: u64,
        elapsed_after_ns: u64,
    },
    ClockAnchorUnavailable {
        stage: &'static str,
        reason: &'static str,
    },
    Frame {
        sequence: u64,
        protocol: u32,
        frame: u64,
        stage: &'static str,
        at_ns: u64,
    },
    Key {
        id: u64,
        stage: &'static str,
        at_ns: u64,
        key: String,
        is_held: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pending_before_enqueue: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pending_after_dequeue: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        in_flight: Option<bool>,
    },
    Geometry {
        stage: &'static str,
        at_ns: u64,
        values: Vec<(&'static str, f64)>,
    },
    Summary {
        dropped_records: usize,
    },
}

fn write_record(writer: &mut impl Write, record: &Record) -> std::io::Result<()> {
    // Serialize on the worker, then submit one whole line. Stderr also carries
    // ordinary error messages; fragmented serde writes could interleave with them.
    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[derive(Clone)]
pub struct KeyTrace {
    diagnostics: Diagnostics,
    id: u64,
    key: String,
    is_held: bool,
}

/// Renderer-local observation identity; never serialized into protocol payloads.
/// Source frame numbers may repeat or decrease, so they are not unique trace IDs.
#[derive(Clone, Copy, Debug)]
pub struct FrameTrace {
    sequence: u64,
    protocol: u32,
    frame: u64,
}

#[cfg(target_os = "macos")]
fn host_clock_ns() -> Option<u64> {
    // Darwin SDK _time.h declares this API since macOS 10.12. Use libc's actual
    // clockid_t/CLOCK_UPTIME_RAW rather than duplicating its ABI or numeric value.
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
    }
    let ns = unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) };
    (ns != 0).then_some(ns)
}

#[cfg(not(target_os = "macos"))]
fn host_clock_ns() -> Option<u64> {
    None
}

impl Diagnostics {
    pub fn from_environment() -> Self {
        if std::env::var("ICHILOTO_GPUI_TRACE").as_deref() == Ok("1") {
            let diagnostics = Self::with_writer(std::io::stderr());
            diagnostics.clock_anchor("start");
            diagnostics
        } else {
            Self::default()
        }
    }

    pub(crate) fn with_writer(mut writer: impl Write + Send + 'static) -> Self {
        let (tx, rx) = async_channel::bounded(4096);
        let (done_tx, done_rx) = async_channel::bounded(1);
        let dropped = Arc::new(AtomicUsize::new(0));
        let worker_dropped = dropped.clone();
        std::thread::spawn(move || {
            let result: std::io::Result<()> = (|| {
                while let Ok(record) = rx.recv_blocking() {
                    write_record(&mut writer, &record)?;
                }
                write_record(
                    &mut writer,
                    &Record::Summary {
                        dropped_records: worker_dropped.load(Ordering::Relaxed),
                    },
                )
            })();
            let _ = done_tx.try_send(result.map_err(|e| e.to_string()));
        });
        Self(Some(Arc::new(Inner {
            epoch: Instant::now(),
            next_key: AtomicU64::new(1),
            next_frame: AtomicU64::new(1),
            dropped,
            records: tx,
            done: done_rx,
        })))
    }

    pub fn now(&self) -> Option<u64> {
        self.0.as_ref().map(|inner| {
            inner
                .epoch
                .elapsed()
                .as_nanos()
                .try_into()
                .unwrap_or(u64::MAX)
        })
    }

    /// Bracket the host clock read so alignment has an explicit uncertainty bound.
    /// Only valid for another process using the SAME host clock and nanosecond unit.
    pub fn clock_anchor(&self, stage: &'static str) {
        let Some(elapsed_before_ns) = self.now() else {
            return;
        };
        let host_ns = host_clock_ns();
        let elapsed_after_ns = self.now().expect("enabled trace clock");
        match host_ns {
            Some(host_ns) => self.record(Record::ClockAnchor {
                stage,
                clock: "CLOCK_UPTIME_RAW",
                pid: std::process::id(),
                elapsed_before_ns,
                host_ns,
                elapsed_after_ns,
            }),
            None => self.record(Record::ClockAnchorUnavailable {
                stage,
                reason: "CLOCK_UPTIME_RAW is unavailable on this platform or its read failed",
            }),
        }
    }

    /// Timestamp is sampled when a complete bounded line has been read, before
    /// parsing. A record is emitted only if parsing identifies a frame envelope.
    pub fn frame_received(
        &self,
        protocol: u32,
        frame: u64,
        at_ns: Option<u64>,
    ) -> Option<FrameTrace> {
        let at_ns = at_ns?;
        let inner = self.0.as_ref()?;
        let trace = FrameTrace {
            sequence: inner.next_frame.fetch_add(1, Ordering::Relaxed),
            protocol,
            frame,
        };
        self.frame_record(trace, "received", at_ns);
        Some(trace)
    }

    pub fn frame_stage(&self, trace: Option<FrameTrace>, stage: &'static str) {
        if let Some(trace) = trace
            && let Some(at_ns) = self.now()
        {
            self.frame_record(trace, stage, at_ns);
        }
    }

    fn frame_record(&self, trace: FrameTrace, stage: &'static str, at_ns: u64) {
        self.record(Record::Frame {
            sequence: trace.sequence,
            protocol: trace.protocol,
            frame: trace.frame,
            stage,
            at_ns,
        });
    }

    /// Called first in the native callback: timestamp precedes key cloning/formatting.
    pub fn native_key(&self, key: &str, is_held: bool) -> Option<KeyTrace> {
        let at_ns = self.now()?;
        let inner = self.0.as_ref()?;
        let trace = KeyTrace {
            diagnostics: self.clone(),
            id: inner.next_key.fetch_add(1, Ordering::Relaxed),
            key: key.to_owned(),
            is_held,
        };
        trace.record("native", at_ns, None, None, None);
        Some(trace)
    }

    pub fn geometry(&self, stage: &'static str, values: impl FnOnce() -> Vec<(&'static str, f64)>) {
        if let Some(at_ns) = self.now() {
            self.record(Record::Geometry {
                stage,
                at_ns,
                values: values(),
            });
        }
    }

    fn record(&self, record: Record) {
        if let Some(inner) = &self.0
            && inner.records.try_send(record).is_err()
        {
            inner.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub async fn finish(&self) -> Result<(), String> {
        if let Some(inner) = &self.0 {
            inner.records.close();
            inner.done.recv().await.map_err(|e| e.to_string())?
        } else {
            Ok(())
        }
    }
}

impl KeyTrace {
    pub fn normalized(&mut self, key: Option<&str>) {
        let at_ns = self.now();
        if let Some(key) = key {
            self.key = key.to_owned();
        }
        self.record(
            if key.is_some() {
                "normalized"
            } else {
                "ignored"
            },
            at_ns,
            None,
            None,
            None,
        );
    }

    pub fn now(&self) -> u64 {
        self.diagnostics.now().expect("enabled trace clock")
    }

    /// Submission timestamp is captured immediately before try_send. The record is
    /// emitted only after success; the consumer may already have received it by then.
    pub fn queued(&self, at_ns: u64, pending_before: usize) {
        self.record("queued", at_ns, Some(pending_before), None, None);
    }

    pub fn write_started(&self, pending_after_dequeue: usize) {
        self.record(
            "write_started",
            self.now(),
            None,
            Some(pending_after_dequeue),
            Some(true),
        );
    }

    pub fn written(&self) {
        self.record("written", self.now(), None, None, Some(false));
    }

    fn record(
        &self,
        stage: &'static str,
        at_ns: u64,
        before: Option<usize>,
        after: Option<usize>,
        in_flight: Option<bool>,
    ) {
        self.diagnostics.record(Record::Key {
            id: self.id,
            stage,
            at_ns,
            key: self.key.clone(),
            is_held: self.is_held,
            pending_before_enqueue: before,
            pending_after_dequeue: after,
            in_flight,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone)]
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

    #[test]
    fn disabled_diagnostics_never_create_a_trace() {
        let d = Diagnostics::default();
        assert!(d.native_key("right", true).is_none());
        assert!(d.frame_received(2, 1, d.now()).is_none());
        d.clock_anchor("test");
        d.frame_stage(None, "accepted");
        d.geometry("test", || {
            panic!("disabled must not build geometry records")
        });
        futures_lite::future::block_on(d.finish()).unwrap();
    }

    #[test]
    fn worker_submits_complete_json_lines_to_the_shared_stderr_sink() {
        struct Lines(Arc<AtomicUsize>);
        impl Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                assert_eq!(bytes.last(), Some(&b'\n'));
                let _: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let lines = Arc::new(AtomicUsize::new(0));
        let d = Diagnostics::with_writer(Lines(lines.clone()));
        d.native_key("right", false);
        d.frame_received(2, 1, d.now());
        futures_lite::future::block_on(d.finish()).unwrap();
        assert_eq!(lines.load(Ordering::Relaxed), 3);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clock_anchor_pairs_host_time_with_a_bounded_relative_interval() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let d = Diagnostics::with_writer(Capture(bytes.clone()));
        let host_before = host_clock_ns().unwrap();
        let relative_before = d.now().unwrap();
        d.clock_anchor("test");
        let relative_after = d.now().unwrap();
        let host_after = host_clock_ns().unwrap();
        futures_lite::future::block_on(d.finish()).unwrap();
        let records: Vec<serde_json::Value> = String::from_utf8(bytes.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records[0]["diagnostic"], "clock_anchor");
        assert_eq!(records[0]["clock"], "CLOCK_UPTIME_RAW");
        assert_eq!(records[0]["pid"], std::process::id());
        let before = records[0]["elapsed_before_ns"].as_u64().unwrap();
        let after = records[0]["elapsed_after_ns"].as_u64().unwrap();
        assert!(relative_before <= before && before <= after && after <= relative_after);
        let host = records[0]["host_ns"].as_u64().unwrap();
        assert!(host_before <= host && host <= host_after);
        assert_eq!(records[1]["dropped_records"], 0);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_host_clock_is_reported_without_a_fabricated_anchor() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let d = Diagnostics::with_writer(Capture(bytes.clone()));
        d.clock_anchor("test");
        futures_lite::future::block_on(d.finish()).unwrap();
        let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let record: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(record["diagnostic"], "clock_anchor_unavailable");
        assert!(record.get("host_ns").is_none());
    }

    #[test]
    fn blocked_stderr_drops_observations_without_blocking_the_producer() {
        struct BlockedWriter {
            started: std::sync::mpsc::Sender<()>,
            release: Option<std::sync::mpsc::Receiver<()>>,
        }
        impl Write for BlockedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Some(release) = self.release.take() {
                    self.started.send(()).unwrap();
                    release.recv().unwrap();
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let d = Diagnostics::with_writer(BlockedWriter {
            started: started_tx,
            release: Some(release_rx),
        });
        d.native_key("right", false);
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let producer = d.clone();
        let worker = std::thread::spawn(move || {
            for _ in 0..5000 {
                producer.native_key("right", true);
            }
            finished_tx.send(()).unwrap();
        });
        let finished_while_blocked = finished_rx.recv_timeout(std::time::Duration::from_secs(2));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        futures_lite::future::block_on(d.finish()).unwrap();
        assert!(finished_while_blocked.is_ok());
        assert!(d.0.as_ref().unwrap().dropped.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn stage_identity_repeat_and_monotonic_times_survive_async_recording() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let d = Diagnostics::with_writer(Capture(bytes.clone()));
        let mut trace = d.native_key("right", true).unwrap();
        trace.normalized(Some("right"));
        trace.queued(trace.now(), 2);
        trace.write_started(1);
        trace.written();
        let mut ignored = d.native_key("a", false).unwrap();
        ignored.normalized(None);
        futures_lite::future::block_on(d.finish()).unwrap();
        let records: Vec<serde_json::Value> = String::from_utf8(bytes.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 8);
        for (record, stage) in
            records
                .iter()
                .zip(["native", "normalized", "queued", "write_started", "written"])
        {
            assert_eq!(record["stage"], stage);
            assert_eq!(record["id"], 1);
            assert_eq!(record["key"], "right");
            assert_eq!(record["is_held"], true);
        }
        for pair in records[..5].windows(2) {
            assert!(pair[0]["at_ns"].as_u64().unwrap() <= pair[1]["at_ns"].as_u64().unwrap());
        }
        assert_eq!(records[2]["pending_before_enqueue"], 2);
        assert_eq!(records[3]["pending_after_dequeue"], 1);
        assert_eq!(records[3]["in_flight"], true);
        assert_eq!(records[4]["in_flight"], false);
        assert_eq!(records[6]["stage"], "ignored");
        assert_eq!(records[6]["id"], 2);
        assert_eq!(records[7]["dropped_records"], 0);
    }
}
