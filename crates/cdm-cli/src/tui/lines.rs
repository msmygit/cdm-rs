//! The line-based progress `MET-031` degrades to when stdout is not a terminal.
//!
//! `MET-031` requires the interactive UI to "degrade automatically to line-based progress when
//! stdout is not a TTY". This module is that fallback, and it is not a lesser thing: piped into
//! `tee`, redirected into a file, or running under a CI job's log collector is how most long
//! migrations are actually watched, and it is the only mode whose output anybody keeps.
//!
//! # Why every line goes to standard error
//!
//! Standard output belongs to the command's *result* — the counter block, or the single JSON
//! document `--output json` promises (`CLI-005`). A progress line interleaved into that stream
//! would turn a machine-readable document into something that no longer parses, on exactly the
//! runs that are being scripted. Progress is diagnostic output, so it goes where diagnostic output
//! goes.
//!
//! # The shape of a line
//!
//! One line, one instant, no control characters: no carriage-return redraw, no cursor movement, no
//! colour. A file, a pipe and a CI log are all things that get read later by something that is not
//! a terminal, and `\r`-redrawn progress in a log file is a line nobody can read.

use std::fmt::Write as _;

use cdm_metrics::{Dashboard, ErrorLine};

use crate::tui::format::{count, duration_hms, eta, percent, rate};

/// One progress line (`MET-031`).
///
/// ```text
/// cdm migrate ks.tbl: 42.1% (2103/5000 ranges, 8 running) 12,345 rows/s origin, 11,900 rows/s target, elapsed 00:04:11, ETA 00:12:34
/// ```
pub fn progress_line(view: &Dashboard) -> String {
    let progress = &view.progress;
    let mut line = format!(
        "cdm {job} {table}: {percent} ({done}/{total} ranges, {running} running) \
         {origin} rows/s origin, {target} rows/s target, elapsed {elapsed}, ETA {eta}",
        job = view.job.as_str(),
        table = view.table_label(),
        percent = percent(progress.weight_fraction),
        done = progress.ranges_completed,
        total = progress.ranges_total,
        running = progress.ranges_in_flight,
        origin = rate(view.rows_per_second(cdm_core::Side::Origin)),
        target = rate(view.rows_per_second(cdm_core::Side::Target)),
        elapsed = duration_hms(progress.elapsed),
        eta = eta(progress.eta),
    );
    // Writing into a `String` cannot fail, so the results are the one thing here worth ignoring.
    if view.errors_total > 0 {
        let _ = write!(line, ", {} error(s)", count(view.errors_total));
    }
    if view.discrepancies_total() > 0 {
        let _ = write!(
            line,
            ", {} discrepancy(ies)",
            count(view.discrepancies_total())
        );
    }
    if view.dropped_events > 0 {
        // `MET-030`'s bus is bounded so it can never slow the run down; the price is that a slow
        // reader misses part of the narrative, and saying so is the whole difference between an
        // incomplete tail and a misleading one.
        let _ = write!(line, ", {} event(s) dropped", count(view.dropped_events));
    }
    if view.stopping {
        line.push_str(", stopping");
    }
    line
}

/// The line printed once when the run ends.
pub fn final_line(view: &Dashboard) -> String {
    format!(
        "cdm {job} {table}: {status} after {elapsed} — {done}/{total} ranges",
        job = view.job.as_str(),
        table = view.table_label(),
        status = view.status.map_or("finished", |status| status.as_str()),
        elapsed = duration_hms(view.progress.elapsed),
        done = view.progress.ranges_completed,
        total = view.progress.ranges_total,
    )
}

/// One line of the error tail (`SEC-002`).
///
/// Carries only what [`ErrorLine`] carries, which is deliberately not the diagnostic's detail,
/// value or suggestion — see `cdm_metrics::dashboard`.
pub fn error_line(line: &ErrorLine) -> String {
    let mut rendered = format!("{}: [{}] {}", line.severity.as_str(), line.code, line.title);
    if let Some(location) = &line.location {
        let _ = write!(rendered, " ({location})");
    }
    if let Some(range) = &line.range {
        let _ = write!(rendered, " [range {range}]");
    }
    rendered
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
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cdm_core::{Diagnostic, JobKind, RunId, RunStatus, Side, TokenRange};
    use cdm_metrics::event::{EventPayload, EventRange};
    use cdm_metrics::{DashboardState, Event, Instruments, ProgressTracker, RangeTimings};

    use super::*;

    struct Fixture {
        state: DashboardState,
        progress: Arc<ProgressTracker>,
        instruments: Arc<Instruments>,
        ranges: Vec<TokenRange>,
        start: Instant,
    }

    fn fixture() -> Fixture {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(8).unwrap();
        let progress = Arc::new(ProgressTracker::by_token_span(&ranges, start));
        let instruments = Arc::new(Instruments::new(start));
        let mut state = DashboardState::new(
            JobKind::Migrate,
            RunId::from_raw(3),
            "node-a",
            Arc::clone(&progress),
            Arc::clone(&instruments),
            Arc::new(RangeTimings::new()),
        );
        state.apply(&Event {
            run_id: RunId::from_raw(3),
            node_id: "node-a".to_owned(),
            at: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::RunStarted {
                job: JobKind::Migrate,
                keyspace: Some("ks".to_owned()),
                table: Some("tbl".to_owned()),
                ranges_planned: 8,
            },
        });
        Fixture {
            state,
            progress,
            instruments,
            ranges,
            start,
        }
    }

    #[test]
    fn met_031_a_progress_line_names_the_job_the_table_and_the_work_done() {
        let f = fixture();
        for range in f.ranges.iter().take(2) {
            f.progress.range_completed(*range, RunStatus::Pass);
        }
        f.progress.range_started(f.ranges[2]);
        for second in 0..60 {
            f.instruments
                .rows(Side::Origin)
                .mark_at(1_000, f.start + Duration::from_secs(second));
        }

        let view = f.state.snapshot_at(f.start + Duration::from_secs(60));
        let line = progress_line(&view);

        assert!(
            line.starts_with("cdm migrate ks.tbl: 25.0% (2/8 ranges, 1 running)"),
            "{line}"
        );
        assert!(line.contains("rows/s origin"), "{line}");
        assert!(line.contains("elapsed 00:01:00"), "{line}");
        assert!(line.contains("ETA 00:03:00"), "{line}");
        // One line, and nothing a log file cannot hold.
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
        assert!(!line.contains('\u{1b}'), "no escape sequences: {line}");
    }

    #[test]
    fn met_031_a_line_withholds_the_eta_exactly_as_met_011_does() {
        let f = fixture();
        // Nothing completed: `MET-011` offers no estimate, and the line must not invent one.
        let view = f.state.snapshot_at(f.start + Duration::from_secs(5));
        let line = progress_line(&view);
        assert!(line.contains("ETA unknown"), "{line}");
        assert!(!line.contains("ETA 00:00:00"), "{line}");
    }

    #[test]
    fn met_031_a_line_says_when_the_bounded_bus_dropped_events() {
        let mut f = fixture();
        f.state.note_lag(412);
        f.state.set_stopping(true);
        let line = progress_line(&f.state.snapshot_at(f.start));
        assert!(line.contains("412 event(s) dropped"), "{line}");
        assert!(line.ends_with(", stopping"), "{line}");
    }

    #[test]
    fn met_031_the_final_line_reports_the_terminal_status() {
        let mut f = fixture();
        f.state.apply(&Event {
            run_id: RunId::from_raw(3),
            node_id: "node-a".to_owned(),
            at: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::RunCompleted {
                status: RunStatus::Interrupted,
                counters: std::collections::BTreeMap::new(),
                elapsed_secs: 61.0,
            },
        });
        let line = final_line(&f.state.snapshot_at(f.start + Duration::from_secs(61)));
        assert!(line.contains("INTERRUPTED"), "{line}");
        assert!(line.contains("0/8 ranges"), "{line}");
    }

    #[test]
    fn met_031_sec_002_an_error_line_quotes_no_value_back() {
        let mut f = fixture();
        f.state.apply(&Event {
            run_id: RunId::from_raw(3),
            node_id: "node-a".to_owned(),
            at: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::Error {
                diagnostic: Diagnostic::error("CDM-CQL", "the write was rejected")
                    .with_detail("token 'sk-live-4711' is not valid")
                    .with_value("sk-live-4711"),
                range: Some(EventRange::from(f.ranges[0])),
            },
        });

        let view = f.state.snapshot_at(f.start);
        let line = error_line(&view.errors[0]);
        assert_eq!(
            line,
            format!(
                "error: [CDM-CQL] the write was rejected [range {}..{}]",
                f.ranges[0].min(),
                f.ranges[0].max()
            )
        );
        assert!(!line.contains("sk-live-4711"), "{line}");
    }
}
