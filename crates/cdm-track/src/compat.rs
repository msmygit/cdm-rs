//! The strings Java and cdm-rs must spell identically (`TRK-013`, `TRK-014`, `COMPAT-003`).
//!
//! Two tools read and write the same two tables. Every value in the `run_type` and `status`
//! columns is therefore a wire format, and the only thing that keeps a Java-started run visible
//! to cdm-rs — and vice versa — is that both sides produce the same bytes.
//!
//! [`run_type`] is a hand-written table rather than a `Display` impl for the reason `TRK-013`
//! gives: `JobKind`'s `Display` is the CLI spelling (`migrate`), it is free to change, and the day
//! it does, `TRK-030`'s `WHERE run_type = ?` predicate silently matches nothing and every
//! previous run becomes invisible. A resume that finds no previous run does not fail; it quietly
//! migrates the whole table again.

use cdm_core::{JobKind, RunStatus};

/// The `cdm_run_info.run_type` value for a job (`TRK-013`).
///
/// Java writes `jobType.toString()` over `enum JobType { MIGRATE, VALIDATE, GUARDRAIL }`
/// (`IJobSessionFactory.java`), so these are upper-case, and that is the whole specification.
pub const fn run_type(job: JobKind) -> &'static str {
    match job {
        JobKind::Migrate => "MIGRATE",
        JobKind::Validate => "VALIDATE",
        JobKind::Guardrail => "GUARDRAIL",
    }
}

/// The job a `run_type` value names, or `None` for one this build does not know.
///
/// Case-insensitive on the way in but not on the way out: cdm-rs must *write* what Java writes,
/// and must *read* anything a reasonable writer produced.
pub fn job_from_run_type(raw: &str) -> Option<JobKind> {
    JobKind::ALL
        .into_iter()
        .find(|job| run_type(*job).eq_ignore_ascii_case(raw.trim()))
}

/// The `status` value for a run or range (`TRK-014`).
///
/// A thin alias for [`RunStatus::as_str`], present so that every column value this crate writes
/// is looked up through this module and a reviewer has one place to check.
pub const fn status(status: RunStatus) -> &'static str {
    status.as_str()
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

    #[test]
    fn trk_013_run_type_is_the_upper_case_java_enum_name() {
        assert_eq!(run_type(JobKind::Migrate), "MIGRATE");
        assert_eq!(run_type(JobKind::Validate), "VALIDATE");
        assert_eq!(run_type(JobKind::Guardrail), "GUARDRAIL");
    }

    #[test]
    fn trk_013_run_type_is_not_derived_from_the_display_impl() {
        // `JobKind`'s Display is the CLI spelling and is allowed to change; `run_type` is not.
        for job in JobKind::ALL {
            assert_ne!(
                run_type(job),
                job.to_string(),
                "run_type must not be JobKind's Display output"
            );
            assert_eq!(run_type(job), job.to_string().to_uppercase());
        }
    }

    #[test]
    fn trk_013_run_type_round_trips_including_a_java_written_value() {
        for job in JobKind::ALL {
            assert_eq!(job_from_run_type(run_type(job)), Some(job));
        }
        assert_eq!(job_from_run_type(" migrate "), Some(JobKind::Migrate));
        assert_eq!(job_from_run_type("BACKUP"), None);
    }

    #[test]
    fn trk_014_every_status_writes_its_exact_java_spelling() {
        assert_eq!(status(RunStatus::NotStarted), "NOT_STARTED");
        assert_eq!(status(RunStatus::DiffCorrected), "DIFF_CORRECTED");
        for value in RunStatus::ALL {
            assert_eq!(
                status(value).parse::<RunStatus>().unwrap(),
                value,
                "{value} must survive a write/read round trip through the status column"
            );
        }
    }

    #[test]
    fn trk_012_the_seven_java_statuses_are_the_ones_a_java_reader_understands() {
        let java: Vec<&str> = RunStatus::ALL
            .into_iter()
            .filter(RunStatus::is_java_compatible)
            .map(status)
            .collect();
        assert_eq!(
            java,
            vec![
                "NOT_STARTED",
                "STARTED",
                "PASS",
                "FAIL",
                "DIFF",
                "DIFF_CORRECTED",
                "ENDED"
            ]
        );
    }
}
