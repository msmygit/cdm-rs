//! The NDJSON event sink (`MET-030`, `metrics.events.sink`).
//!
//! One JSON object per line, to standard output or to a file — the format `jq`, `vector`,
//! Filebeat and every log pipeline already understand, and the artefact an operator attaches to a
//! support ticket alongside the final counter block.
//!
//! # This sink cannot become an exfiltration path
//!
//! It writes what the bus published, and nothing else. The redaction of `SEC-002` is applied when
//! an event is *constructed* (see the [parent module](super)), so this sink has no policy, no
//! configuration and no way to widen what an event carries — there is deliberately no
//! `write_with_values`, and no access to a record. `sec_002_the_sink_writes_no_row_value` asserts
//! the property over one event of every kind.
//!
//! # A lagging sink says so in the stream
//!
//! A subscriber that falls behind loses events (`ARCHITECTURE.md` §9). Losing them silently would
//! make the file quietly wrong, so a gap is written as a `warning` event naming how many were
//! missed. The file then always accounts for itself.

use std::io::Write;
use std::path::Path;

use cdm_core::{CdmError, Diagnostic, ErrorKind, Severity};
use chrono::Utc;

use super::{Event, EventPayload, EventStreamError, EventSubscriber};

/// Writes events as newline-delimited JSON (`MET-030`).
///
/// ```
/// use cdm_core::{RunId, TokenRange};
/// use chrono::DateTime;
/// use cdm_metrics::{Event, EventPayload, NdjsonSink};
///
/// # fn main() -> Result<(), cdm_core::CdmError> {
/// // In a run the sink drains a subscriber; here one event is written directly.
/// let mut sink = NdjsonSink::new(Vec::new());
/// sink.write_event(&Event {
///     run_id: RunId::from_raw(7),
///     node_id: "node-a".to_owned(),
///     at: DateTime::UNIX_EPOCH,
///     payload: EventPayload::RangeStarted { range: TokenRange::new(0, 99)?.into() },
/// })?;
///
/// let line = String::from_utf8(sink.into_inner()).unwrap_or_default();
/// assert!(line.starts_with(r#"{"run_id":7,"node_id":"node-a""#));
/// assert!(line.ends_with("\n"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct NdjsonSink<W: Write> {
    writer: W,
    written: u64,
}

impl<W: Write> NdjsonSink<W> {
    /// A sink over any writer.
    pub const fn new(writer: W) -> Self {
        Self { writer, written: 0 }
    }

    /// How many events have been written.
    pub const fn written(&self) -> u64 {
        self.written
    }

    /// Writes one event, followed by a newline.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the event cannot be serialised — which it always can — or if the
    /// write fails, naming the underlying I/O error. A caller that is draining a subscriber should
    /// log the failure and stop the sink rather than fail the run: `PLG-006`'s rule that an
    /// exporter must never fail a migration applies to the event sink for the same reason.
    pub fn write_event(&mut self, event: &Event) -> Result<(), CdmError> {
        let line = serde_json::to_string(event).map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot serialise a run event: {error}"),
            )
        })?;
        writeln!(self.writer, "{line}").map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot write a run event: {error}"),
            )
        })?;
        self.written += 1;
        Ok(())
    }

    /// Flushes the underlying writer.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] with the underlying I/O error.
    pub fn flush(&mut self) -> Result<(), CdmError> {
        self.writer.flush().map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot flush the event sink: {error}"),
            )
        })
    }

    /// The writer, for a caller that needs it back — a test asserting on a buffer, or a file that
    /// must be closed explicitly.
    pub fn into_inner(self) -> W {
        self.writer
    }

    /// Drains a subscriber until the run ends, writing every event (`MET-030`).
    ///
    /// Returns when the bus is dropped, or on the first write failure — at which point the run
    /// carries on without an event file, because an unwritable event log is not a reason to stop
    /// moving data.
    ///
    /// # Errors
    ///
    /// The first write or flush failure. Reaching the end of the stream is not an error.
    pub async fn drain(&mut self, mut events: EventSubscriber) -> Result<(), CdmError> {
        loop {
            match events.recv().await {
                Ok(event) => self.write_event(&event)?,
                Err(EventStreamError::Lagged(missed)) => self.write_event(&gap(missed))?,
                Err(EventStreamError::Closed) => break,
            }
        }
        self.flush()
    }
}

impl NdjsonSink<std::io::Stdout> {
    /// The sink `metrics.events.sink = stdout_json` selects.
    #[must_use]
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }
}

impl NdjsonSink<std::io::BufWriter<std::fs::File>> {
    /// The sink `metrics.events.sink = file` selects.
    ///
    /// Creates the parent directory if it is missing, and appends rather than truncating, so a
    /// resumed run (`TRK-031`) adds to the record of the run it resumes rather than erasing it.
    ///
    /// The path comes from the caller rather than being read here, so that this module stays
    /// independent of the configuration model; `metrics.events.path` is what the CLI wiring
    /// passes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the directory cannot be created or the file cannot be opened.
    pub fn create(path: &Path) -> Result<Self, CdmError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "cannot create the event sink directory {}: {error}",
                        parent.display()
                    ),
                )
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!("cannot open the event sink {}: {error}", path.display()),
                )
            })?;
        Ok(Self::new(std::io::BufWriter::new(file)))
    }
}

/// The event written in place of the ones a lagging sink missed.
fn gap(missed: u64) -> Event {
    Event {
        run_id: cdm_core::RunId::UNSET,
        node_id: String::new(),
        at: Utc::now(),
        payload: EventPayload::Warning {
            diagnostic: Diagnostic::new(
                "CDM-INTERNAL",
                Severity::Warning,
                format!("the event sink fell behind and lost {missed} events"),
            )
            .with_rule("MET-030"),
        },
    }
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use cdm_core::{RunId, RunStatus};
    use chrono::DateTime;

    use crate::event::tests::{every_kind, range};
    use crate::event::{DiscrepancyKind, EventBus, Redaction};

    use super::*;

    #[tokio::test]
    async fn met_030_the_sink_writes_one_json_object_per_line() {
        let bus = EventBus::new(RunId::from_raw(7), "node-a");
        let events = bus.subscribe();
        every_kind(&bus);
        drop(bus);

        let mut sink = NdjsonSink::new(Vec::new());
        sink.drain(events).await.unwrap();
        assert_eq!(sink.written(), 7);

        let text = String::from_utf8(sink.into_inner()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 7);
        let kinds: Vec<String> = lines
            .iter()
            .map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                assert!(!line.contains('\n'));
                value["type"].as_str().unwrap_or_default().to_owned()
            })
            .collect();
        assert_eq!(kinds, EventPayload::KINDS);
        assert!(text.ends_with('\n'));
    }

    #[tokio::test]
    async fn sec_002_the_sink_writes_no_row_value() {
        // The whole point of the sink: it is a transcription, and the redaction happened before
        // the event existed. One event of every kind, and not a row value between them.
        let bus = EventBus::new(RunId::from_raw(7), "node-a");
        let events = bus.subscribe();
        every_kind(&bus);
        drop(bus);

        let mut sink = NdjsonSink::new(Vec::new());
        sink.drain(events).await.unwrap();
        let text = String::from_utf8(sink.into_inner()).unwrap();

        // `every_kind` publishes a discrepancy for this key.
        assert!(!text.contains("8f2c1b04"), "{text}");
        assert!(!text.contains("customer_id="), "{text}");
        assert!(text.contains("fingerprint"), "{text}");
        // Column names survive; they are schema, not data.
        assert!(text.contains("\"email\""), "{text}");
        for forbidden in ["password", "AstraCS", "secret"] {
            assert!(!text.contains(forbidden), "{forbidden} reached the sink");
        }
    }

    #[tokio::test]
    async fn sec_002_an_opted_in_run_is_the_only_way_a_key_reaches_the_file() {
        let bus = EventBus::with_capacity(RunId::from_raw(7), "n", 16, Redaction::IncludeKeys);
        let events = bus.subscribe();
        bus.discrepancy(
            DateTime::UNIX_EPOCH,
            range(0, 9),
            DiscrepancyKind::Missing,
            "id=alice",
            Vec::new(),
        );
        drop(bus);

        let mut sink = NdjsonSink::new(Vec::new());
        sink.drain(events).await.unwrap();
        let text = String::from_utf8(sink.into_inner()).unwrap();
        assert!(text.contains("id=alice"), "{text}");
        assert!(text.contains("\"form\":\"plain\""), "{text}");
    }

    #[tokio::test]
    async fn met_030_a_gap_is_recorded_rather_than_hidden() {
        let bus = EventBus::with_capacity(RunId::from_raw(7), "n", 2, Redaction::default());
        let events = bus.subscribe();
        for index in 0..8 {
            bus.range_started(DateTime::UNIX_EPOCH, range(index, index));
        }
        drop(bus);

        let mut sink = NdjsonSink::new(Vec::new());
        sink.drain(events).await.unwrap();
        let text = String::from_utf8(sink.into_inner()).unwrap();

        assert!(text.contains("fell behind and lost 6 events"), "{text}");
        assert!(text.contains("\"type\":\"warning\""), "{text}");
        // The two events still buffered are written after the gap notice.
        assert_eq!(text.lines().count(), 3);
    }

    #[tokio::test]
    async fn met_030_the_file_sink_appends_and_creates_its_directory() {
        let dir = tempdir();
        let path = dir.join("nested").join("events.ndjson");

        let bus = EventBus::new(RunId::from_raw(7), "n");
        {
            let mut sink = NdjsonSink::create(&path).unwrap();
            sink.write_event(&Event {
                run_id: RunId::from_raw(7),
                node_id: "n".to_owned(),
                at: DateTime::UNIX_EPOCH,
                payload: EventPayload::RangeStarted {
                    range: range(0, 9).into(),
                },
            })
            .unwrap();
            sink.flush().unwrap();
        }
        {
            // A resumed run appends rather than erasing the record of the run it resumes.
            let mut sink = NdjsonSink::create(&path).unwrap();
            sink.write_event(&Event {
                run_id: RunId::from_raw(7),
                node_id: "n".to_owned(),
                at: DateTime::UNIX_EPOCH,
                payload: EventPayload::RangeCompleted {
                    range: range(0, 9).into(),
                    status: RunStatus::Pass,
                    run_info: "Read: 1".to_owned(),
                },
            })
            .unwrap();
            sink.flush().unwrap();
        }
        drop(bus);

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("range_started"));
        assert!(text.contains("range_completed"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn met_030_a_write_failure_is_reported_rather_than_panicking() {
        /// A writer that refuses everything, standing in for a full disk.
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("no space left on device"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("no space left on device"))
            }
        }

        let mut sink = NdjsonSink::new(Broken);
        let error = sink
            .write_event(&Event {
                run_id: RunId::from_raw(1),
                node_id: "n".to_owned(),
                at: DateTime::UNIX_EPOCH,
                payload: EventPayload::RangeStarted {
                    range: range(0, 1).into(),
                },
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Internal);
        assert!(error.to_string().contains("no space left"), "{error}");
        assert_eq!(sink.written(), 0);
        assert!(sink.flush().is_err());
    }

    /// A unique temporary directory. `tempfile` is not a dependency of this crate, and one
    /// directory is not worth adding one for.
    fn tempdir() -> std::path::PathBuf {
        let unique = format!(
            "cdm-metrics-ndjson-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
