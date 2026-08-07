//! The machine-readable discrepancy report (`VAL-013`), and its redaction (`SEC-002`).
//!
//! The diff log of `VAL-012` is Java's, and it is a log: one line per finding, shaped for `grep`,
//! carrying no values because `VAL-017` closed that door. This is the other half — a file a program
//! reads, one record per discrepancy, carrying the run, the token range, the primary key, the kind
//! of difference and the differing columns. It is what `VAL-014` will page over, what a spreadsheet
//! opens, and what the run summary of `MET-033` points at.
//!
//! ```text
//!   compare  ──► Comparison::Missing / Mismatch ──┬─► diff log      (VAL-012, never values)
//!                                                 ├─► this report   (VAL-013, values on request)
//!                                                 └─► event bus     (MET-030, key fingerprinted)
//! ```
//!
//! # Values are redacted at construction, not at the sink
//!
//! `validate.report.redact_values` is read once, when the report is opened, and applied while a
//! [`DiscrepancyRecord`] is being *built*. A record therefore never holds a plain value that the
//! policy said to withhold, and there is deliberately no writer, no serialiser and no accessor that
//! could widen one — the same discipline `cdm-metrics`' event bus applies to primary keys, for the
//! same reason.
//!
//! Redaction is **on by default**. A discrepancy report is the artefact somebody attaches to a
//! ticket, so it travels further than a log line ever does, and a default that carries row contents
//! travels with it. Turning it off is the one supported route to the values — the diff log has none
//! and `--compat-java` does not restore them — and it makes the file a copy of the affected rows,
//! which Tier-2 validation says out loud at startup.
//!
//! A redacted value is a **digest**, not an omission: `SEC-002` asks for hashing, and hashing keeps
//! the one property that makes a redacted report useful — two rows wrong in the same way are still
//! visibly wrong in the same way. It is the 64-bit FNV-1a of the event bus, and it carries the same
//! caveat: it is a correlation token, not a commitment. A column with three possible values can be
//! enumerated against it. Saying so is better than implying a guarantee the digest does not give.
//!
//! Null-ness is never hashed. `null` renders as `null` in both modes, because "the target is empty"
//! and "the target holds something else" call for different actions and that distinction is
//! metadata rather than content — the same judgement `VAL-017` makes for the log.
//!
//! # The primary key is present in both modes
//!
//! A key is row data too, and it is written in the clear. That is the whole argument of `VAL-017`
//! stated positively: a finding you cannot attach to a row is not a finding. The key is rendered
//! exactly as the diff log renders it, so a record here and a line there are matchable by string
//! equality, and the report identifies rows in precisely the way the log already does.
//!
//! # Formats
//!
//! `ndjson` is the one to reach for: a record is complete the moment its line is written, so a run
//! that is killed leaves a file that is readable up to the last newline. `json` is a single array,
//! which is friendlier to a small consumer and useless if the run never reaches the closing
//! bracket — so [`DiscrepancyReport::finish`] exists and the caller must call it. `csv` is flat, so
//! it emits one row per differing column rather than per discrepancy; `run_id` and `key` group them
//! back together.
//!
//! Parquet is named by `docs/SPEC.md` and is not implemented; the reasoning is recorded there under
//! `VAL-013`.
//!
//! # An unwritable report does not fail a run
//!
//! Opening it does: a report that cannot be created must be discovered at startup, not after six
//! hours (`VAL-012` takes the same view of the diff log). But a write that fails mid-run is logged
//! once, loudly, and the run carries on — moving data is the job, and filing a report about it is
//! not worth abandoning the job for.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cdm_config::types::ReportFormat;
use cdm_core::{CdmError, ErrorKind, PrimaryKey, RawCell, RunId, TokenRange};
use cdm_metrics::event::fingerprint;
use cdm_metrics::{DiscrepancyKind, DiscrepancyReportRef, EventRange};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::compare::Mismatch;

/// The default path, matching `validate.report.path`.
pub const DEFAULT_REPORT_FILE: &str = "cdm_logs/cdm_discrepancies.json";

/// The prefix a redacted value carries, so a reader can tell a digest from a value.
///
/// Self-describing on purpose: a bare sixteen hexadecimal characters is indistinguishable from a
/// short blob, and a consumer that guessed wrong would be comparing digests against values.
pub const REDACTED_PREFIX: &str = "fnv1a64:";

/// How a null renders, in both redaction modes.
pub const NULL_VALUE: &str = "null";

/// One discrepancy, as the report records it (`VAL-013`).
///
/// Values are already rendered and already redacted by the time this exists; see the
/// [module documentation](self).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscrepancyRecord {
    /// The run that found it.
    pub run_id: RunId,
    /// The token range it was found in, with decimal string bounds (`TOK-002`).
    pub range: EventRange,
    /// The row's primary key, rendered as the diff log renders it.
    pub key: String,
    /// What kind of difference it is.
    pub kind: DiscrepancyKind,
    /// Whether the values below are digests rather than values (`SEC-002`).
    ///
    /// Recorded per record rather than only in the run summary so that a record which has been
    /// lifted out of its file — into a ticket, a database, an API response — still says what it is.
    pub values_redacted: bool,
    /// The differing columns, in target-column order. Empty for a missing row: nothing was
    /// compared, because there was nothing to compare against.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnRecord>,
}

/// One differing column within a discrepancy (`VAL-006`, `VAL-009`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnRecord {
    /// The target column's name.
    pub column: String,
    /// The origin's value: hex, [`NULL_VALUE`], or a [`REDACTED_PREFIX`] digest.
    pub origin: String,
    /// The target's value, in the same three shapes. This is the value as it was **read**, not as
    /// the comparison converted it (`VAL-005`), because the converted form exists nowhere and an
    /// operator who went to look for it would not find it.
    pub target: String,
    /// Why the column could not be compared at all (`VAL-009`), when that is what happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The report sink (`VAL-013`).
///
/// One per run, shared by every worker. Writes are serialised through a mutex and flushed per
/// record: a run that is killed must still have its findings on disk, which is most of the point.
#[derive(Debug)]
pub struct DiscrepancyReport {
    run_id: RunId,
    format: ReportFormat,
    path: PathBuf,
    redact_values: bool,
    state: Mutex<State>,
    warned: AtomicBool,
}

/// The writer and the count, under one lock: the JSON array's separator depends on whether
/// anything has been written yet, so the two cannot be allowed to disagree.
#[derive(Debug)]
struct State {
    sink: Sink,
    records: u64,
    finished: bool,
}

#[derive(Debug)]
enum Sink {
    /// No report was configured (`validate.report.format = none`).
    Disabled,
    /// A file on disk.
    File(BufWriter<File>),
    /// An in-memory buffer, for tests and for an embedded run with no filesystem.
    Buffer(Vec<u8>),
    /// The file failed and the failure has been reported once.
    Broken,
}

impl DiscrepancyReport {
    /// A report that records nothing, which is what `validate.report.format = none` selects.
    ///
    /// The job holds one of these rather than an `Option`, so there is exactly one code path
    /// through the discrepancy handling whether or not a report was asked for.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            run_id: RunId::UNSET,
            format: ReportFormat::None,
            path: PathBuf::new(),
            redact_values: true,
            state: Mutex::new(State {
                sink: Sink::Disabled,
                records: 0,
                finished: true,
            }),
            warned: AtomicBool::new(true),
        }
    }

    /// Opens a report, creating the parent directory and any CSV header (`VAL-013`).
    ///
    /// The file is **truncated**, not appended to: a report describes one run, and two
    /// concatenated JSON documents are not a JSON document. `format = none` returns
    /// [`DiscrepancyReport::disabled`] and touches the filesystem not at all — a report nobody
    /// asked for must not create a file, let alone erase one.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the directory or the file cannot be created, or if the header
    /// cannot be written. A startup error on purpose: finding out at the end of a six-hour
    /// validate run that its findings went nowhere is the failure this exists to prevent.
    pub fn open(
        run_id: RunId,
        format: ReportFormat,
        path: impl AsRef<Path>,
        redact_values: bool,
    ) -> Result<Self, CdmError> {
        if format == ReportFormat::None {
            return Ok(Self::disabled());
        }
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| io_error(&path, &error))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|error| io_error(&path, &error))?;

        let report = Self {
            run_id,
            format,
            path,
            redact_values,
            state: Mutex::new(State {
                sink: Sink::File(BufWriter::new(file)),
                records: 0,
                finished: false,
            }),
            warned: AtomicBool::new(false),
        };
        report.write_preamble()?;
        Ok(report)
    }

    /// A report that keeps its output in memory.
    ///
    /// For tests, and for an embedded run with nowhere to write. The bytes are identical to the
    /// ones [`DiscrepancyReport::open`] would have written, so a test asserting on them is
    /// asserting on the real format.
    #[must_use]
    pub fn in_memory(run_id: RunId, format: ReportFormat, redact_values: bool) -> Self {
        if format == ReportFormat::None {
            return Self::disabled();
        }
        let report = Self {
            run_id,
            format,
            path: PathBuf::from("<memory>"),
            redact_values,
            state: Mutex::new(State {
                sink: Sink::Buffer(Vec::new()),
                records: 0,
                finished: false,
            }),
            warned: AtomicBool::new(true),
        };
        // A buffer cannot fail to accept a header.
        drop(report.write_preamble());
        report
    }

    /// Whether anything is being recorded at all.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.format != ReportFormat::None
    }

    /// The format this report is written in.
    #[must_use]
    pub const fn format(&self) -> ReportFormat {
        self.format
    }

    /// The file being written to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether row values are hashed rather than written (`SEC-002`).
    #[must_use]
    pub const fn redacts_values(&self) -> bool {
        self.redact_values
    }

    /// How many discrepancies have been recorded.
    #[must_use]
    pub fn records(&self) -> u64 {
        self.state.lock().records
    }

    /// Everything written so far, for an in-memory report; empty for a file-backed one.
    ///
    /// Deliberately not available for a file: keeping a whole run's findings in memory is exactly
    /// what `NFR-003`'s bounded-memory requirement forbids.
    #[must_use]
    pub fn captured(&self) -> String {
        match &self.state.lock().sink {
            Sink::Buffer(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Sink::Disabled | Sink::File(_) | Sink::Broken => String::new(),
        }
    }

    /// The pointer the run summary of `MET-033` carries, or `None` when no report was written.
    #[must_use]
    pub fn reference(&self) -> Option<DiscrepancyReportRef> {
        self.is_enabled().then(|| DiscrepancyReportRef {
            path: self.path.clone(),
            format: self.format.as_str().to_owned(),
            records: self.records(),
            values_redacted: self.redact_values,
        })
    }

    /// `VAL-002` — the target has no row for this key.
    pub fn missing(&self, range: TokenRange, key: &PrimaryKey, corrected: bool) {
        let kind = if corrected {
            DiscrepancyKind::CorrectedMissing
        } else {
            DiscrepancyKind::Missing
        };
        self.record(&self.build(range, key, kind, None));
    }

    /// `VAL-006` — the row differs, in these columns.
    pub fn mismatch(
        &self,
        range: TokenRange,
        key: &PrimaryKey,
        mismatch: &Mismatch,
        corrected: bool,
    ) {
        let kind = if corrected {
            DiscrepancyKind::CorrectedMismatch
        } else {
            DiscrepancyKind::Mismatch
        };
        self.record(&self.build(range, key, kind, Some(mismatch)));
    }

    /// Closes the report (`VAL-013`).
    ///
    /// Required for `json`, whose array has to be terminated; harmless and still worth calling for
    /// the others, which it flushes. Calling it twice is a no-op, so a caller with two exit paths
    /// does not have to remember which one already ran.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the closing bytes cannot be written or the file cannot be
    /// flushed. The run's own status is what a caller should exit on; this is worth reporting and
    /// not worth failing a completed run for.
    pub fn finish(&self) -> Result<(), CdmError> {
        let mut state = self.state.lock();
        if state.finished {
            return Ok(());
        }
        state.finished = true;
        let tail = match self.format {
            ReportFormat::Json if state.records == 0 => "[]\n",
            ReportFormat::Json => "\n]\n",
            ReportFormat::None | ReportFormat::Ndjson | ReportFormat::Csv => "",
        };
        let outcome = state
            .sink
            .write_all(tail.as_bytes())
            .and_then(|()| state.sink.flush());
        drop(state);
        outcome.map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!(
                    "cannot close the validate discrepancy report at `{}`: {error} (VAL-013)",
                    self.path.display()
                ),
            )
        })
    }

    /// Builds a record, applying the redaction policy as it goes (`SEC-002`).
    fn build(
        &self,
        range: TokenRange,
        key: &PrimaryKey,
        kind: DiscrepancyKind,
        mismatch: Option<&Mismatch>,
    ) -> DiscrepancyRecord {
        let columns = mismatch.map_or_else(Vec::new, |mismatch| {
            mismatch
                .differences()
                .iter()
                .map(|difference| ColumnRecord {
                    column: difference.column().to_owned(),
                    origin: self.render(difference.origin()),
                    target: self.render(difference.target()),
                    error: difference.error().map(ToOwned::to_owned),
                })
                .collect()
        });
        DiscrepancyRecord {
            run_id: self.run_id,
            range: range.into(),
            key: key.to_string(),
            kind,
            values_redacted: self.redact_values,
            columns,
        }
    }

    /// One value position, under the redaction policy.
    fn render(&self, cell: &RawCell) -> String {
        if cell.is_null() {
            return NULL_VALUE.to_owned();
        }
        let rendered = cell.to_string();
        if self.redact_values {
            format!("{REDACTED_PREFIX}{}", fingerprint(&rendered))
        } else {
            rendered
        }
    }

    /// The header a CSV report opens with; nothing for the others.
    fn write_preamble(&self) -> Result<(), CdmError> {
        if self.format != ReportFormat::Csv {
            return Ok(());
        }
        let mut state = self.state.lock();
        let outcome = state
            .sink
            .write_all(b"run_id,range_min,range_max,key,kind,column,origin,target,error\n")
            .and_then(|()| state.sink.flush());
        drop(state);
        outcome.map_err(|error| io_error(&self.path, &error))
    }

    /// Serialises and emits one record.
    fn record(&self, record: &DiscrepancyRecord) {
        let mut state = self.state.lock();
        if matches!(state.sink, Sink::Disabled) {
            return;
        }
        let bytes = match self.serialise(record, state.records) {
            Ok(bytes) => bytes,
            Err(error) => {
                drop(state);
                self.fallback(&error);
                return;
            }
        };
        let outcome = state
            .sink
            .write_all(bytes.as_bytes())
            .and_then(|()| state.sink.flush());
        match outcome {
            Ok(()) => state.records += 1,
            Err(error) => {
                // The file is not coming back: a full disk stays full. Stop trying, so a run with
                // a million findings does not spend itself on a million failing writes.
                state.sink = Sink::Broken;
                drop(state);
                self.fallback(&error.to_string());
            }
        }
    }

    /// One record's bytes, including whatever separator its position requires.
    fn serialise(&self, record: &DiscrepancyRecord, written: u64) -> Result<String, String> {
        match self.format {
            ReportFormat::None => Ok(String::new()),
            ReportFormat::Ndjson => serde_json::to_string(record)
                .map(|line| format!("{line}\n"))
                .map_err(|error| error.to_string()),
            ReportFormat::Json => serde_json::to_string_pretty(record)
                .map(|body| {
                    // Indented by two so that the array reads as a document rather than as a
                    // stream that happens to have brackets around it.
                    let indented = body.lines().collect::<Vec<_>>().join("\n  ");
                    if written == 0 {
                        format!("[\n  {indented}")
                    } else {
                        format!(",\n  {indented}")
                    }
                })
                .map_err(|error| error.to_string()),
            ReportFormat::Csv => Ok(csv_rows(record)),
        }
    }

    /// The report is unusable; say so once, and never silently.
    fn fallback(&self, error: &str) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::error!(
                target: "cdm::validate",
                path = %self.path.display(),
                error,
                "the validate discrepancy report cannot be written; the run continues and its \
                 findings remain in the difference log (VAL-013)"
            );
        }
    }
}

impl Sink {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::File(writer) => writer.write_all(bytes),
            Self::Buffer(buffer) => {
                buffer.extend_from_slice(bytes);
                Ok(())
            }
            Self::Disabled | Self::Broken => Ok(()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::File(writer) => writer.flush(),
            Self::Disabled | Self::Buffer(_) | Self::Broken => Ok(()),
        }
    }
}

/// One CSV row per differing column, and one row for a discrepancy that has none.
///
/// CSV has no nesting, so "one record per discrepancy" cannot survive the format; what survives is
/// that `run_id` and `key` identify the discrepancy, so the rows of one group back together. A
/// missing row has no columns and still gets a row: a report in which a missing row simply did not
/// appear would be actively misleading.
fn csv_rows(record: &DiscrepancyRecord) -> String {
    let prefix = format!(
        "{},{},{},{},{}",
        record.run_id,
        csv_field(&record.range.min),
        csv_field(&record.range.max),
        csv_field(&record.key),
        record.kind.as_str(),
    );
    if record.columns.is_empty() {
        return format!("{prefix},,,,\n");
    }
    record
        .columns
        .iter()
        .fold(String::new(), |mut rows, column| {
            use std::fmt::Write as _;
            // Infallible into a `String`; the result is discarded rather than unwrapped so that
            // `ERR-004` holds without a targeted allow.
            let _ = writeln!(
                rows,
                "{prefix},{},{},{},{}",
                csv_field(&column.column),
                csv_field(&column.origin),
                csv_field(&column.target),
                csv_field(column.error.as_deref().unwrap_or_default()),
            );
            rows
        })
}

/// RFC 4180 quoting: quote when the field contains a comma, a quote or a line break, and double
/// any quote inside it.
///
/// Hand-written rather than pulled in as a dependency. The rule is four lines and one test, and a
/// crate whose licences, advisories and transitive graph have to be reviewed under `SEC-030` is a
/// poor trade for four lines.
pub(crate) fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn io_error(path: &Path, error: &std::io::Error) -> CdmError {
    CdmError::new(
        ErrorKind::Config,
        format!(
            "cannot open the validate discrepancy report at `{}`: {error} (VAL-013). Set \
             `validate.report.path` to a writable path, or `validate.report.format = \"none\"` to \
             write no report.",
            path.display()
        ),
    )
}
