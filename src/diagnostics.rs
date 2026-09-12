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
    dropped: Arc<AtomicUsize>,
    records: async_channel::Sender<Record>,
    done: async_channel::Receiver<Result<(), String>>,
}

#[derive(Serialize)]
#[serde(tag = "diagnostic", rename_all = "snake_case")]
enum Record {
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

#[derive(Clone)]
pub struct KeyTrace {
    diagnostics: Diagnostics,
    id: u64,
    key: String,
    is_held: bool,
}

impl Diagnostics {
    pub fn from_environment() -> Self {
        if std::env::var("ICHILOTO_GPUI_TRACE").as_deref() == Ok("1") {
            Self::with_writer(std::io::stderr())
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
                    serde_json::to_writer(&mut writer, &record)?;
                    writer.write_all(b"\n")?;
                    writer.flush()?;
                }
                serde_json::to_writer(
                    &mut writer,
                    &Record::Summary {
                        dropped_records: worker_dropped.load(Ordering::Relaxed),
                    },
                )?;
                writer.write_all(b"\n")?;
                writer.flush()
            })();
            let _ = done_tx.try_send(result.map_err(|e| e.to_string()));
        });
        Self(Some(Arc::new(Inner {
            epoch: Instant::now(),
            next_key: AtomicU64::new(1),
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
        d.geometry("test", || {
            panic!("disabled must not build geometry records")
        });
        futures_lite::future::block_on(d.finish()).unwrap();
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
