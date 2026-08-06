//! Structured logging (`MET-032`, `SEC-001`, `SEC-002`).
//!
//! Every log line cdm-rs emits goes through `tracing`, and this module builds the subscriber that
//! renders them. Three formats, matching `logging.format`:
//!
//! | `logging.format` | Shape | For |
//! |---|---|---|
//! | `pretty` | multi-line, coloured, human | an operator watching a run |
//! | `compact` | one line per record, human | a terminal that is also doing something else |
//! | `json` | one JSON object per record | a log pipeline |
//!
//! The `json` format is the one `MET-032` is really about: it emits the span fields of `ENG-011` —
//! `run_id`, `range_min`, `range_max`, `node_id` — as structured fields rather than as text
//! embedded in a message, so a log search can filter a run down to one token range without a
//! regular expression.
//!
//! # Why `LogFormat` is declared here and not read from `cdm-config`
//!
//! `ARCHITECTURE.md` §3 has `cdm-metrics` depending on `cdm-core` alone; `cdm-config` is not in
//! its dependency set, and adding an edge that is not in that graph is forbidden. The enum is
//! therefore restated here, with the same three spellings, and `cdm-cli` maps one to the other in
//! a single `match`. `met_032_the_format_spellings_match_the_configuration_enum` pins the
//! spellings so the two cannot drift.
//!
//! # `SEC-001` and `SEC-002` are upstream of this module
//!
//! A subscriber renders what it is given. Secrets never reach it because `Secret<T>` renders as
//! `***` in `Debug`, `Display` and `Serialize` alike (`CFG-012`), and row values never reach it
//! because no call site logs one outside the validate diff path (`VAL-012`). This module's
//! contribution is to make neither of those accidental: it installs no field formatter that could
//! reflect over a config, and `logging::init` takes a level and a format rather than a
//! configuration object.

use std::str::FromStr;

use cdm_core::{CdmError, ErrorKind};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

/// The shape of a log record (`MET-032`, `logging.format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LogFormat {
    /// Human-readable, multi-line, coloured when the terminal supports it.
    #[default]
    Pretty,
    /// One JSON object per record, for ingestion.
    Json,
    /// Human-readable, one line per record.
    Compact,
}

impl LogFormat {
    /// Every format, in declaration order.
    pub const ALL: [Self; 3] = [Self::Pretty, Self::Json, Self::Compact];

    /// The configuration spelling, identical to `cdm_config::types::LogFormat`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pretty => "pretty",
            Self::Json => "json",
            Self::Compact => "compact",
        }
    }

    /// Whether this format emits structured records rather than prose (`MET-032`).
    #[must_use]
    pub const fn is_structured(self) -> bool {
        matches!(self, Self::Json)
    }
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogFormat {
    type Err = CdmError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str().eq_ignore_ascii_case(value.trim()))
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Config,
                    format!("unknown log format `{value}`; expected pretty, json or compact"),
                )
                .with_context(|ctx| ctx.with_config_key("logging.format"))
            })
    }
}

/// How logging is set up for a run (`MET-032`).
///
/// Deliberately three fields wide. A logging setup that took the whole configuration would be one
/// refactor away from rendering it, and `SEC-001` is easier to keep when there is nothing to keep
/// it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingSetup {
    /// The `tracing` filter directive of `logging.level`, e.g. `info` or `cdm_engine=debug,info`.
    pub level: String,
    /// The record shape.
    pub format: LogFormat,
    /// Whether to colour the output. Callers set this from whether stdout is a terminal; the
    /// `json` format ignores it.
    pub ansi: bool,
}

impl Default for LoggingSetup {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LogFormat::default(),
            ansi: true,
        }
    }
}

impl LoggingSetup {
    /// A setup at the given level, otherwise default.
    #[must_use]
    pub fn new(level: impl Into<String>, format: LogFormat) -> Self {
        Self {
            level: level.into(),
            format,
            ansi: true,
        }
    }

    /// Turns colouring off, for a pipe or a CI log.
    #[must_use]
    pub const fn without_ansi(mut self) -> Self {
        self.ansi = false;
        self
    }

    /// The filter this setup describes.
    ///
    /// `RUST_LOG` wins when it is set, which is the convention every Rust operator already knows
    /// and the only way to raise the level of a binary that is already running under an init
    /// system that owns its arguments.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the directive does not parse, naming `logging.level`.
    pub fn filter(&self) -> Result<EnvFilter, CdmError> {
        if std::env::var_os(EnvFilter::DEFAULT_ENV).is_some() {
            if let Ok(from_env) = EnvFilter::try_from_default_env() {
                return Ok(from_env);
            }
        }
        EnvFilter::try_new(&self.level).map_err(|error| {
            CdmError::new(
                ErrorKind::Config,
                format!("`{}` is not a valid log filter: {error}", self.level),
            )
            .with_context(|ctx| ctx.with_config_key("logging.level"))
        })
    }
}

/// Installs the process-wide subscriber (`MET-032`).
///
/// Called once, as early in `main` as the configuration allows — anything logged before it is
/// discarded, which is why the loader logs nothing it cannot repeat.
///
/// # Errors
///
/// [`ErrorKind::Config`] for an unparseable filter directive, and [`ErrorKind::Internal`] if a
/// subscriber has already been installed. The second case is a programming error rather than an
/// operational one: it means two call sites both believed they owned logging.
pub fn init(setup: &LoggingSetup) -> Result<(), CdmError> {
    install(setup, std::io::stdout)
}

/// Installs a subscriber writing somewhere other than standard output.
///
/// The seam the tests use, and the one a `cdm serve` deployment would use to write to a file.
///
/// # Errors
///
/// As [`init`].
pub fn install<W>(setup: &LoggingSetup, writer: W) -> Result<(), CdmError>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let filter = setup.filter()?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_target(true);

    let installed = match setup.format {
        // One JSON object per record, with the span fields of `ENG-011` flattened alongside the
        // message so that a log pipeline can filter on `run_id` without parsing prose.
        LogFormat::Json => builder
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .try_init(),
        LogFormat::Compact => builder.compact().with_ansi(setup.ansi).try_init(),
        LogFormat::Pretty => builder.pretty().with_ansi(setup.ansi).try_init(),
    };

    installed.map_err(|error| {
        CdmError::new(
            ErrorKind::Internal,
            format!("a tracing subscriber is already installed: {error}"),
        )
    })
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
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::format::FmtSpan;

    use super::*;

    /// A `MakeWriter` collecting into a shared buffer, so a rendered record can be asserted on.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl std::io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Renders one event through a subscriber built exactly as [`install`] builds it, but scoped
    /// to this thread so the tests do not fight over the global subscriber.
    fn render(format: LogFormat) -> String {
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("info"))
            .with_writer(buffer.clone())
            .with_target(true)
            .with_span_events(FmtSpan::NONE);

        match format {
            LogFormat::Json => {
                let subscriber = subscriber
                    .json()
                    .flatten_event(true)
                    .with_current_span(true)
                    .with_span_list(false)
                    .finish();
                tracing::subscriber::with_default(subscriber, emit);
            }
            LogFormat::Compact => {
                let subscriber = subscriber.compact().with_ansi(false).finish();
                tracing::subscriber::with_default(subscriber, emit);
            }
            LogFormat::Pretty => {
                let subscriber = subscriber.pretty().with_ansi(false).finish();
                tracing::subscriber::with_default(subscriber, emit);
            }
        }
        buffer.text()
    }

    /// One event inside a range span, as `ENG-011` shapes it.
    fn emit() {
        let span = tracing::info_span!(
            "cdm.range",
            run_id = 1_712_345_678_i64,
            range_min = "-9223372036854775808",
            range_max = "-4611686018427387905",
            node_id = "node-a"
        );
        let _guard = span.enter();
        tracing::info!(target: "cdm::engine", rows = 10_u64, "the range completed");
    }

    #[test]
    fn met_032_the_json_format_emits_one_structured_object_per_record() {
        let text = render(LogFormat::Json);
        let line = text.lines().next().expect("one record");
        let record: serde_json::Value = serde_json::from_str(line).expect("valid JSON");

        assert_eq!(record["level"], "INFO");
        assert_eq!(record["target"], "cdm::engine");
        assert_eq!(record["message"], "the range completed");
        assert_eq!(record["rows"], 10);
        // The span fields of `ENG-011`, structured rather than embedded in the message.
        assert_eq!(record["span"]["run_id"], 1_712_345_678_i64);
        assert_eq!(record["span"]["range_min"], "-9223372036854775808");
        assert_eq!(record["span"]["node_id"], "node-a");
        assert!(LogFormat::Json.is_structured());
    }

    #[test]
    fn met_032_the_human_formats_stay_human() {
        for format in [LogFormat::Pretty, LogFormat::Compact] {
            let text = render(format);
            assert!(text.contains("the range completed"), "{format}: {text}");
            assert!(
                serde_json::from_str::<serde_json::Value>(text.lines().next().unwrap_or(""))
                    .is_err(),
                "{format} must not be JSON"
            );
            assert!(!format.is_structured());
        }
    }

    #[test]
    fn met_032_the_format_spellings_match_the_configuration_enum() {
        // `cdm_config::types::LogFormat`'s spellings, which `cdm-cli` maps to these. The two
        // enums are separate because the dependency graph says so, not because they may differ.
        assert_eq!(
            LogFormat::ALL.map(LogFormat::as_str),
            ["pretty", "json", "compact"]
        );
        assert_eq!(LogFormat::default(), LogFormat::Pretty);
        for format in LogFormat::ALL {
            assert_eq!(LogFormat::from_str(format.as_str()).unwrap(), format);
            assert_eq!(format.to_string(), format.as_str());
        }
        assert_eq!(LogFormat::from_str(" JSON ").unwrap(), LogFormat::Json);

        let error = LogFormat::from_str("logfmt").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(
            error.context().config_key.as_deref(),
            Some("logging.format")
        );
    }

    #[test]
    fn met_032_an_unparseable_filter_names_the_property_it_came_from() {
        let setup = LoggingSetup::new("cdm_engine=chatty", LogFormat::Json);
        let error = setup.filter().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(error.context().config_key.as_deref(), Some("logging.level"));

        assert!(LoggingSetup::default().filter().is_ok());
        assert_eq!(LoggingSetup::default().level, "info");
        assert!(LoggingSetup::default().ansi);
        assert!(!LoggingSetup::default().without_ansi().ansi);
    }

    #[test]
    fn sec_001_the_logging_setup_cannot_carry_a_configuration_object() {
        // Three fields, all of them strings the operator typed on purpose. There is no
        // constructor that takes a `CdmConfig`, which is what keeps a resolved password from
        // arriving here in the first place.
        let setup = LoggingSetup::new("info", LogFormat::Json);
        let rendered = format!("{setup:?}");
        assert!(rendered.contains("level"));
        assert!(rendered.contains("format"));
        assert!(rendered.contains("ansi"));
        assert_eq!(rendered.matches(": ").count(), 3);
    }
}
