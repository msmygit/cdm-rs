//! The run summary (`MET-033`): the artefact a user attaches to a ticket.
//!
//! A run leaves three kinds of evidence behind. The final counter block (`MET-006`) is what Java
//! prints and what people's scripts grep, but it is a screenful of prose. The event stream
//! (`MET-030`) is complete but is a transcript, not a conclusion. This module is the third: **one
//! JSON document that answers "what did this run do?"** — which configuration it ran under, what
//! the plan was, every counter, how long it took, which node did what, and what it found.
//!
//! `--summary-out report.json` is where it goes, and a support ticket is where it ends up. That
//! second fact drives every decision here.
//!
//! # Why it is a plain data structure and not a renderer
//!
//! The summary is assembled by whoever knows the run — the scheduler knows the plan and the
//! outcomes, the CLI knows the configuration and the wall clock — and this crate knows none of
//! that. So [`RunSummary`] is a value with public fields and a `write_to`, not a trait with an
//! implementation somewhere else. `cdm-engine` builds one from a finished run in
//! `RunReport::summary`, and a caller that has extra facts (a config hash, a discrepancy report
//! path) attaches them afterwards.
//!
//! # What it may contain (`SEC-001`, `SEC-002`)
//!
//! Counts, identifiers, statuses, timings — and a **hash** of the configuration, never the
//! configuration. That is not caution for its own sake: this file is the one artefact whose whole
//! purpose is to be sent to somebody else, so a secret or a row value in it is a leak with a
//! delivery mechanism attached. There is deliberately no field that can hold a row, and the
//! discrepancy detail it carries is counts plus a *reference* to the report of `VAL-013` — the
//! path and whether that report redacted its values — rather than the findings themselves.
//!
//! `met_033_the_summary_carries_no_credential_or_row_value` asserts the property over a fully
//! populated summary rather than over the type declaration, so a field added later is covered by
//! it too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cdm_core::{CdmError, ErrorKind, JobKind, RunId, RunStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::counter::CounterKind;
use crate::registry::{CounterView, JobCounters};

/// The value of [`RunSummary::schema`], bumped when a consumer would have to change.
///
/// A summary is read by machines that were written against a particular shape, so the shape says
/// which one it is. Added fields do not bump it — every consumer worth the name ignores what it
/// does not know — but a removed or re-meaning field does.
pub const SUMMARY_SCHEMA: &str = "cdm.run-summary/v1";

/// One run, as `MET-033` requires it to be recorded.
///
/// Serialises to the JSON document `--summary-out` writes. Every field is optional to *populate*
/// and none is optional to *read*: a caller that does not know the config hash leaves it `None`,
/// and the key is then absent rather than present and misleading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    /// The document shape ([`SUMMARY_SCHEMA`]).
    pub schema: String,
    /// The version of cdm-rs that produced it, which is the first thing a support ticket needs.
    pub cdm_version: String,
    /// The run, when run tracking allocated one (`TRK-001`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Which job ran.
    pub job: JobKind,
    /// The run's terminal status (`ENG-009`, `ENG-010`).
    pub status: RunStatus,
    /// The node that wrote this summary (`DST-018`).
    pub node_id: String,
    /// The digest of the effective configuration
    /// (`cdm_config::EffectiveConfig::config_hash`, `CFG-023`).
    ///
    /// A hash, never the configuration: two runs that disagree about a result can be compared for
    /// "did they run the same job?" without either party sending the other their credentials
    /// (`SEC-001`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
    /// When it started, when it ended, and how fast it went.
    pub timings: Timings,
    /// What the plan was and how much of it was done (`TOK-003`, `ENG-002`).
    pub plan: PlanSummary,
    /// Every registered counter at the committed level, under its `MET-001` name.
    pub counters: BTreeMap<String, u64>,
    /// One entry per node that processed ranges (`DST-018`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeSummary>,
    /// What a validate run found, when the job was validate (`VAL-013`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discrepancies: Option<DiscrepancySummary>,
}

impl RunSummary {
    /// A summary of a run that has just ended, with the plan and counters still to be attached.
    ///
    /// `finished_at` and `elapsed` are taken rather than read from the clock so that the value is
    /// a pure function of its inputs, which is what lets a test assert on the whole document.
    #[must_use]
    pub fn new(
        job: JobKind,
        status: RunStatus,
        node_id: impl Into<String>,
        finished_at: DateTime<Utc>,
        elapsed: std::time::Duration,
    ) -> Self {
        Self {
            schema: SUMMARY_SCHEMA.to_owned(),
            cdm_version: crate::VERSION.to_owned(),
            run_id: None,
            job,
            status,
            node_id: node_id.into(),
            config_hash: None,
            timings: Timings::new(finished_at, elapsed),
            plan: PlanSummary::default(),
            counters: BTreeMap::new(),
            nodes: Vec::new(),
            discrepancies: None,
        }
    }

    /// Records the run this summary describes.
    #[must_use]
    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.run_id = Some(run_id);
        self
    }

    /// Records the configuration digest of `CFG-023`.
    #[must_use]
    pub fn with_config_hash(mut self, hash: impl Into<String>) -> Self {
        self.config_hash = Some(hash.into());
        self
    }

    /// Records the plan.
    #[must_use]
    pub fn with_plan(mut self, plan: PlanSummary) -> Self {
        self.plan = plan;
        self
    }

    /// Records every committed counter, and derives the discrepancy summary for a validate run.
    ///
    /// Both at once because they are the same reading of the same registry: taking the counters in
    /// one call and the discrepancies in another invites the two to be taken at different moments,
    /// and a summary whose totals do not add up is worse than no summary.
    #[must_use]
    pub fn with_counters(mut self, counters: &JobCounters) -> Self {
        self.counters = counters
            .registered()
            .iter()
            .map(|&kind| {
                (
                    kind.as_str().to_owned(),
                    counters.count_of(kind, CounterView::Committed),
                )
            })
            .collect();
        if counters.job() == JobKind::Validate {
            self.discrepancies = Some(DiscrepancySummary::from_counters(counters));
        }
        self
    }

    /// Records what one node did (`DST-018`).
    #[must_use]
    pub fn with_node(mut self, node: NodeSummary) -> Self {
        self.nodes.push(node);
        self
    }

    /// Points at the machine-readable discrepancy report of `VAL-013`.
    ///
    /// Ignored when the job is not validate: there is nothing to point at, and inventing an empty
    /// discrepancy section for a migrate run would make "no discrepancies" and "not looked for"
    /// indistinguishable.
    #[must_use]
    pub fn with_discrepancy_report(mut self, report: DiscrepancyReportRef) -> Self {
        if let Some(discrepancies) = self.discrepancies.as_mut() {
            discrepancies.report = Some(report);
        }
        self
    }

    /// The document, pretty-printed with a trailing newline.
    ///
    /// Pretty rather than compact because a human reads this one under time pressure, and `jq` is
    /// not always installed on the machine the incident is happening on.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the document cannot be serialised, which it always can.
    pub fn to_json(&self) -> Result<String, CdmError> {
        let mut json = serde_json::to_string_pretty(self).map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot serialise the run summary: {error}"),
            )
        })?;
        json.push('\n');
        Ok(json)
    }

    /// Writes the document to `path`, creating the parent directory (`MET-033`).
    ///
    /// The file is replaced rather than appended to: a summary describes exactly one run, and two
    /// concatenated JSON documents are not a JSON document.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the directory cannot be created or the file cannot be written,
    /// naming the path. A caller at the end of a run should report this and still exit on the
    /// run's own status: the run happened whether or not its summary could be filed.
    pub fn write_to(&self, path: &Path) -> Result<(), CdmError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "cannot create the run summary directory {}: {error} (MET-033)",
                        parent.display()
                    ),
                )
            })?;
        }
        std::fs::write(path, self.to_json()?).map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!(
                    "cannot write the run summary to {}: {error} (MET-033)",
                    path.display()
                ),
            )
        })
    }
}

/// When a run ran, and how fast (`MET-033`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// When the run started. RFC 3339 UTC on the wire (`NFR-007`).
    pub started_at: DateTime<Utc>,
    /// When it ended.
    pub finished_at: DateTime<Utc>,
    /// How long it took.
    pub elapsed_secs: f64,
    /// Origin rows read per second over the whole run, or `None` for a run too short to divide by.
    ///
    /// The run-long average, deliberately: the instantaneous rates of `MET-010` are for watching a
    /// run, and this is for describing one that is over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows_per_second: Option<f64>,
}

impl Timings {
    /// Timings for a run that ended at `finished_at` after `elapsed`.
    #[must_use]
    pub fn new(finished_at: DateTime<Utc>, elapsed: std::time::Duration) -> Self {
        let started_at = chrono::Duration::from_std(elapsed)
            .ok()
            .and_then(|elapsed| finished_at.checked_sub_signed(elapsed))
            .unwrap_or(finished_at);
        Self {
            started_at,
            finished_at,
            elapsed_secs: elapsed.as_secs_f64(),
            rows_per_second: None,
        }
    }

    /// Derives the run-long throughput from a row count.
    #[must_use]
    pub fn with_rows(mut self, rows: u64) -> Self {
        if self.elapsed_secs > 0.0 {
            #[allow(clippy::cast_precision_loss)] // A row count beyond 2^53 is not a real run.
            let rows = rows as f64;
            self.rows_per_second = Some(rows / self.elapsed_secs);
        }
        self
    }
}

/// The plan, and how much of it the run got through (`TOK-003`, `ENG-002`, `ENG-010`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSummary {
    /// How many ranges the planner produced.
    pub ranges_planned: u64,
    /// How many a worker claimed.
    pub ranges_claimed: u64,
    /// How many reached a successful terminal status.
    pub ranges_passed: u64,
    /// How many failed (`ENG-008`).
    pub ranges_failed: u64,
    /// How many a shutdown abandoned mid-flight (`ENG-010`).
    pub ranges_abandoned: u64,
    /// How many no worker ever claimed, because the run stopped first.
    ///
    /// Together with `ranges_abandoned` this is what a resume has to re-plan (`TRK-031`), and it
    /// is the number that says whether "the run ended" means "the work is done".
    pub ranges_unclaimed: u64,
}

/// What one node contributed (`DST-018`).
///
/// A single-process run has exactly one of these, which is not redundant: it makes the summary of
/// a local run and of a distributed one the same document, so a tool that reads one reads both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSummary {
    /// The node's identity in the membership table.
    pub node_id: String,
    /// How many ranges it claimed.
    pub ranges_claimed: u64,
    /// How many of those succeeded.
    pub ranges_passed: u64,
    /// How many failed.
    pub ranges_failed: u64,
}

/// What a validate run found, in counts (`VAL-002`, `VAL-003`, `VAL-006`, `VAL-007`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscrepancySummary {
    /// Rows absent from the target.
    pub missing: u64,
    /// Of those, how many autocorrect inserted.
    pub corrected_missing: u64,
    /// Rows present and differing.
    pub mismatch: u64,
    /// Of those, how many autocorrect rewrote.
    pub corrected_mismatch: u64,
    /// Rows that are still wrong: what was found, minus what was repaired.
    ///
    /// The number an operator acts on, computed here rather than left to the reader because
    /// `MISSING` counts every missing row *including* the corrected ones (`VAL-016` compares the
    /// two counters for exactly that reason), and subtracting them in one's head at three in the
    /// morning is how a clean run gets reported as a broken one.
    pub outstanding: u64,
    /// Where the per-row detail is, when a report was written (`VAL-013`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<DiscrepancyReportRef>,
}

impl DiscrepancySummary {
    /// Reads the four validate counters at the committed level (`MET-004`).
    #[must_use]
    pub fn from_counters(counters: &JobCounters) -> Self {
        let count = |kind| counters.count_of(kind, CounterView::Committed);
        let missing = count(CounterKind::Missing);
        let corrected_missing = count(CounterKind::CorrectedMissing);
        let mismatch = count(CounterKind::Mismatch);
        let corrected_mismatch = count(CounterKind::CorrectedMismatch);
        Self {
            missing,
            corrected_missing,
            mismatch,
            corrected_mismatch,
            outstanding: missing.saturating_sub(corrected_missing)
                + mismatch.saturating_sub(corrected_mismatch),
            report: None,
        }
    }

    /// Whether the run found anything at all.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.missing == 0 && self.mismatch == 0
    }
}

/// A pointer to the discrepancy report of `VAL-013`.
///
/// A pointer and not the findings: the report is one record per differing row and a large validate
/// run has millions of them, so inlining it would turn the one artefact that must stay attachable
/// to a ticket into one that cannot be. What is inlined is what a reader needs in order to know
/// whether to go and get it — where it is, what shape it is in, how many records it holds, and
/// **whether its values were redacted**, because a report with values in it is a file that has to
/// be handled differently from one without (`SEC-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscrepancyReportRef {
    /// Where the report was written.
    pub path: PathBuf,
    /// Its format, as `validate.report.format` spells it.
    pub format: String,
    /// How many discrepancies it records.
    pub records: u64,
    /// Whether row values were redacted in it (`validate.report.redact_values`, `SEC-002`).
    ///
    /// `true` — the default — means the file is safe to attach to a ticket. `false` means the
    /// operator opted into row contents and the file must be treated as a copy of the data.
    pub values_redacted: bool,
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
    use super::*;

    fn at() -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + chrono::Duration::seconds(90)
    }

    fn validate_counters() -> JobCounters {
        let counters = JobCounters::new(JobKind::Validate);
        for (kind, by) in [
            (CounterKind::Read, 1_000),
            (CounterKind::Valid, 990),
            (CounterKind::Missing, 6),
            (CounterKind::CorrectedMissing, 4),
            (CounterKind::Mismatch, 4),
            (CounterKind::CorrectedMismatch, 1),
            (CounterKind::PartitionsPassed, 8),
        ] {
            counters.increment_by(counters.counter(kind).unwrap(), by);
        }
        counters.flush();
        counters
    }

    /// A summary with every field populated, which is what the security property is asserted over.
    fn full() -> RunSummary {
        RunSummary::new(
            JobKind::Validate,
            RunStatus::Ended,
            "node-a",
            at(),
            std::time::Duration::from_secs(90),
        )
        .with_run_id(RunId::from_raw(1_712_345_678_901_234))
        .with_config_hash("0123456789abcdef")
        .with_plan(PlanSummary {
            ranges_planned: 10,
            ranges_claimed: 9,
            ranges_passed: 8,
            ranges_failed: 1,
            ranges_abandoned: 0,
            ranges_unclaimed: 1,
        })
        .with_counters(&validate_counters())
        .with_node(NodeSummary {
            node_id: "node-a".to_owned(),
            ranges_claimed: 9,
            ranges_passed: 8,
            ranges_failed: 1,
        })
        .with_discrepancy_report(DiscrepancyReportRef {
            path: PathBuf::from("cdm_logs/cdm_discrepancies.ndjson"),
            format: "ndjson".to_owned(),
            records: 10,
            values_redacted: true,
        })
    }

    #[test]
    fn met_033_the_summary_carries_the_configuration_plan_counters_timings_and_findings() {
        let summary = full();
        let json = serde_json::to_value(&summary).unwrap();

        assert_eq!(json["schema"], SUMMARY_SCHEMA);
        assert_eq!(json["config_hash"], "0123456789abcdef");
        assert_eq!(json["job"], "validate");
        assert_eq!(json["status"], "ENDED");
        assert_eq!(json["plan"]["ranges_planned"], 10);
        assert_eq!(json["plan"]["ranges_unclaimed"], 1);
        assert_eq!(json["counters"]["READ"], 1_000);
        assert_eq!(json["counters"]["CORRECTED_MISSING"], 4);
        assert_eq!(json["timings"]["elapsed_secs"], 90.0);
        assert_eq!(json["timings"]["started_at"], "1970-01-01T00:00:00Z");
        assert_eq!(json["timings"]["finished_at"], "1970-01-01T00:01:30Z");
        assert_eq!(json["nodes"][0]["node_id"], "node-a");
        assert_eq!(json["discrepancies"]["outstanding"], 5);
        assert_eq!(json["discrepancies"]["report"]["records"], 10);
        assert_eq!(json["discrepancies"]["report"]["values_redacted"], true);

        // And it reads back: this document is also the body of `GET /v1/runs/{id}/summary`.
        assert_eq!(serde_json::from_value::<RunSummary>(json).unwrap(), summary);
    }

    #[test]
    fn met_033_the_summary_carries_no_credential_or_row_value() {
        // SEC-001 and SEC-002 over the whole document, not over the struct declaration: the
        // summary is the artefact that gets emailed, so what it cannot contain matters more here
        // than anywhere else. Only a *hash* of the configuration appears.
        let text = full().to_json().unwrap();
        for forbidden in [
            "password", "AstraCS", "secret", "token", "username", "0x", "alice",
        ] {
            assert!(!text.contains(forbidden), "{forbidden} reached the summary");
        }
        assert!(text.contains("config_hash"));
    }

    #[test]
    fn met_033_the_outstanding_count_is_what_was_found_minus_what_was_repaired() {
        let summary = DiscrepancySummary::from_counters(&validate_counters());
        assert_eq!(summary.missing, 6);
        assert_eq!(summary.corrected_missing, 4);
        assert_eq!(summary.outstanding, (6 - 4) + (4 - 1));
        assert!(!summary.is_clean());

        let clean = DiscrepancySummary::from_counters(&JobCounters::new(JobKind::Validate));
        assert!(clean.is_clean());
        assert_eq!(clean.outstanding, 0);
    }

    #[test]
    fn met_033_a_migrate_run_has_counters_but_no_discrepancy_section() {
        let counters = JobCounters::new(JobKind::Migrate);
        counters.increment_by(counters.counter(CounterKind::Read).unwrap(), 3);
        counters.flush();

        let summary = RunSummary::new(
            JobKind::Migrate,
            RunStatus::Ended,
            "node-a",
            at(),
            std::time::Duration::from_secs(3),
        )
        .with_counters(&counters)
        // Nothing to point at, and the pointer must not conjure a section into existence.
        .with_discrepancy_report(DiscrepancyReportRef {
            path: PathBuf::from("nowhere"),
            format: "json".to_owned(),
            records: 0,
            values_redacted: true,
        });

        assert!(summary.discrepancies.is_none());
        assert_eq!(summary.counters["READ"], 3);
        assert!(!summary.counters.contains_key("MISSING"), "MET-002");
    }

    #[test]
    fn met_033_throughput_is_the_run_long_average_and_is_absent_for_an_instant_run() {
        let timings = Timings::new(at(), std::time::Duration::from_secs(90)).with_rows(900);
        assert_eq!(timings.rows_per_second, Some(10.0));

        let instant = Timings::new(at(), std::time::Duration::ZERO).with_rows(900);
        assert_eq!(instant.rows_per_second, None);
        assert_eq!(instant.started_at, instant.finished_at);
    }

    #[test]
    fn met_033_the_summary_is_written_to_the_named_file_and_its_directory_is_created() {
        let dir = std::env::temp_dir().join(format!(
            "cdm-metrics-summary-{}-{}",
            std::process::id(),
            at().timestamp_nanos_opt().unwrap_or_default()
        ));
        let path = dir.join("nested").join("report.json");
        full().write_to(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"), "{text}");
        assert!(text.contains("\n  \"job\": \"validate\""), "{text}");
        let parsed: RunSummary = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, full());

        // Written twice, the file is replaced rather than appended to.
        full().write_to(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().len(), text.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn met_033_an_unwritable_path_is_reported_rather_than_panicking() {
        let path = Path::new("/dev/null/report.json");
        let error = full().write_to(path).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Internal);
        assert!(error.to_string().contains("MET-033"), "{error}");
    }
}
