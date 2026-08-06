//! The dedicated difference log (`VAL-012`), and what it is allowed to contain (`SEC-002`).
//!
//! # Why it is a file and not a `tracing` target
//!
//! Java's log4j2 configuration pins `com.datastax.cdm.job.DiffJobSession` to `ERROR`, points it at
//! `cdm_logs/cdm_diff.log`, and sets `additivity = false`, so a validate run's findings appear in
//! that file and **nowhere else** — not in `cdm.log`, not on the console. Operators have built
//! tooling on exactly that: the diff log is the machine-readable artefact of a validate run, and it
//! is grepped, diffed and archived. `VAL-012` therefore requires the same file, at the same default
//! path, carrying the same lines.
//!
//! Routing it through `tracing` instead would put every discrepancy in the main log too, where a
//! run with a million missing rows drowns everything an operator needs in order to understand
//! *why*. So this writes the file directly, and the main log gets counts rather than findings.
//!
//! # What a line may say (`SEC-002`, `VAL-017`)
//!
//! `SEC-002` forbids logging row values by default. Java's mismatch line contains them. Both cannot
//! hold, and the precedent is already set: `ERR-005` met the same conflict — Java's bind diagnostic
//! prints the offending value — and resolved it in `SEC-002`'s favour, carrying the **primary key**
//! instead. `VAL-017` applies that resolution here.
//!
//! So a line identifies a discrepancy by primary key and column name, with every value position
//! rendered as `<redacted>`. The message shapes are otherwise byte-for-byte Java's, because the
//! shape is what tooling matches on and the contents are what `SEC-002` is about. Nothing is lost
//! operationally: the key is what you need in order to go and look at the row, and looking at the
//! row is a decision a human takes deliberately rather than one a log file takes on their behalf.
//!
//! # Failure to open the file is not silent
//!
//! A diff log that cannot be created must not swallow the run's findings. The sink falls back to
//! `tracing` at `ERROR` and says once, loudly, that it has done so.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cdm_core::{CdmError, ErrorKind, PrimaryKey};
use parking_lot::Mutex;

/// The default path, identical to Java's log4j2 appender (`VAL-012`).
pub const DEFAULT_DIFF_FILE: &str = "cdm_logs/cdm_diff.log";

/// The dedicated difference sink of `VAL-012`.
///
/// One per run, shared by every worker. Writes are serialised through a mutex and flushed per line:
/// a validate run that is killed mid-range must still have its findings on disk, which is the whole
/// reason the file exists.
#[derive(Debug)]
pub struct DiffLog {
    path: PathBuf,
    writer: Mutex<Option<BufWriter<File>>>,
    warned: AtomicBool,
    lines: Mutex<Vec<String>>,
    capture: bool,
}

impl DiffLog {
    /// Opens the diff log at `path`, creating the parent directory if it does not exist.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the directory or the file cannot be created. This is a startup error on
    /// purpose: discovering at the end of a six-hour validate run that its findings went nowhere is
    /// the failure mode `VAL-012` exists to prevent.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CdmError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| io_error(&path, &error))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| io_error(&path, &error))?;
        Ok(Self {
            path,
            writer: Mutex::new(Some(BufWriter::new(file))),
            warned: AtomicBool::new(false),
            lines: Mutex::new(Vec::new()),
            capture: false,
        })
    }

    /// A sink that keeps its lines in memory instead of on disk.
    ///
    /// For tests, and for an embedded run that has no filesystem to write to. The lines are
    /// identical to the ones [`DiffLog::open`] would have written, so a test asserting on them is
    /// asserting on the real format.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            path: PathBuf::from("<memory>"),
            writer: Mutex::new(None),
            warned: AtomicBool::new(true),
            lines: Mutex::new(Vec::new()),
            capture: true,
        }
    }

    /// The file being written to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every line written so far, for an in-memory sink.
    ///
    /// Empty for a file-backed one: capturing a run's whole diff output in memory is precisely what
    /// `NFR-003`'s bounded-memory requirement forbids.
    #[must_use]
    pub fn captured(&self) -> Vec<String> {
        self.lines.lock().clone()
    }

    /// `VAL-002` — the target has no row for this key.
    pub fn missing(&self, label: &str, key: &PrimaryKey) {
        self.write(label, &format!("Missing target row found for key: {key}"));
    }

    /// `VAL-004` — a counter row is missing and `autocorrect.missing_counter` is off.
    ///
    /// The message names both properties, as Java's does, because the operator's next action is to
    /// decide whether double-counting a re-inserted counter row is acceptable for their data.
    pub fn counter_correction_skipped(&self, label: &str, key: &PrimaryKey) {
        self.write(
            label,
            &format!(
                "autocorrect.missing is true, but not Inserting as autocorrect.missing_counter \
                 is not enabled; key : {key}"
            ),
        );
    }

    /// `VAL-003` — autocorrect inserted the missing row.
    pub fn inserted_missing(&self, label: &str, key: &PrimaryKey) {
        self.write(label, &format!("Inserted missing row in target: {key}"));
    }

    /// `VAL-006` — the row differs. `detail` is [`Mismatch::detail`](super::Mismatch::detail),
    /// which is already redacted.
    pub fn mismatch(&self, label: &str, key: &PrimaryKey, detail: &str) {
        self.write(
            label,
            &format!("Mismatch row found for key: {key} Mismatch: {detail}"),
        );
    }

    /// `VAL-007` — autocorrect rewrote the mismatched row.
    pub fn corrected_mismatch(&self, label: &str, key: &PrimaryKey) {
        self.write(label, &format!("Corrected mismatch row in target: {key}"));
    }

    /// Renders and emits one line.
    ///
    /// The layout is Java's log4j2 pattern `%d %-5p [%t] %c{1}:%L - %m%n` with the line number
    /// dropped — a Rust line number in a Java-shaped log would be actively misleading — and the
    /// thread name replaced by the range label of `ENG-012`, which is the identifier that actually
    /// tells an operator which slice of the ring a finding came from.
    fn write(&self, label: &str, message: &str) {
        let line = format!(
            "{} ERROR [{label}] validate - {message}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S,%3f")
        );
        if self.capture {
            self.lines.lock().push(line);
            return;
        }
        let mut guard = self.writer.lock();
        let failed = match guard.as_mut() {
            Some(writer) => writeln!(writer, "{line}")
                .and_then(|()| writer.flush())
                .is_err(),
            None => true,
        };
        drop(guard);
        if failed {
            self.fallback(&line);
        }
    }

    /// The diff log is unusable; say so once and keep the findings.
    fn fallback(&self, line: &str) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::error!(
                target: "cdm::validate",
                path = %self.path.display(),
                "the difference log cannot be written; findings are being logged to the main log \
                 instead (VAL-012)"
            );
        }
        tracing::error!(target: "cdm::validate::diff", "{line}");
    }
}

fn io_error(path: &Path, error: &std::io::Error) -> CdmError {
    CdmError::new(
        ErrorKind::Config,
        format!(
            "cannot open the validate difference log at `{}`: {error} (VAL-012). Set \
             `logging.diff_file` to a writable path.",
            path.display()
        ),
    )
}
