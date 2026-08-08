//! Drawing one frame of the terminal UI (`MET-031`).
//!
//! `MET-031` names six things the UI must show: live throughput, a progress bar, an ETA, per-node
//! status, an error tail and latency sparklines. Each has a panel here, and the layout is fixed
//! rather than configurable — an operator who has seen one cdm-rs run should not have to find the
//! progress bar again on the next one.
//!
//! ```text
//! ┌ cdm migrate · ks.tbl · run 7 · node-a ────────────────────────────┐
//! │ ███████████████████░░░░░░░░░░░░░░░  42.1%  ETA 00:12:34           │
//! ├ Throughput ─────────────────────┬ Ranges ─────────────────────────┤
//! │ origin       12,345 rows/s      │ planned            5,000        │
//! │ target       11,900 rows/s      │ completed          2,103        │
//! ├ rows/s ─────────────────────────┼ range duration ─────────────────┤
//! │ ▂▃▅▇█▇▅▃▂▁                      │ ▂▂▃▃▂▂▁▁                        │
//! ├ Nodes ────────────────────────────────────────────────────────────┤
//! ├ Errors ───────────────────────────────────────────────────────────┤
//! └ q / Ctrl-C stop the run gracefully ───────────────────────────────┘
//! ```
//!
//! # Nothing drawn here is row data
//!
//! Every value comes from [`Dashboard`], which `SEC-002` already constrains: counts, identifiers,
//! token bounds, one-line diagnostic titles. There is no path from a row, a bound parameter or a
//! configuration secret to a cell on this screen, because there is no such field on the snapshot
//! to draw. See `cdm_metrics::dashboard` for why the error tail is that narrow.

use cdm_core::{RunStatus, Side};
use cdm_metrics::Dashboard;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Sparkline};
use ratatui::Frame;

use crate::tui::format::{count, duration_hms, eta, nanos_as_millis, percent, rate};
use crate::tui::lines::error_line;

/// The keys the footer advertises, and the event loop honours.
pub const KEY_HELP: &str = "q / Esc / Ctrl-C: stop the run gracefully";

/// Draws one frame (`MET-031`).
///
/// Total, not incremental: ratatui diffs the buffer against the last frame and writes only what
/// changed, so redrawing everything twice a second costs a few hundred bytes of terminal output
/// and removes a whole class of "the panel stopped updating" bug.
pub fn draw(frame: &mut Frame<'_>, view: &Dashboard) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // progress bar
            Constraint::Length(7), // throughput, latency and ranges
            Constraint::Length(5), // sparklines
            Constraint::Length(5), // nodes
            Constraint::Min(3),    // error tail
            Constraint::Length(1), // footer
        ])
        .split(area);

    // `split` returns exactly as many rectangles as it was given constraints, but reading them out
    // by index would be an `indexing_slicing` on a slice whose length the type system does not
    // know. A frame that cannot be laid out is simply not drawn, which is a blank screen rather
    // than a panic on a display that must never be able to fail a run.
    let mut regions = chunks.iter();
    let (Some(&progress), Some(&stats), Some(&sparks), Some(&nodes), Some(&errors), Some(&footer)) = (
        regions.next(),
        regions.next(),
        regions.next(),
        regions.next(),
        regions.next(),
        regions.next(),
    ) else {
        return;
    };

    draw_progress(frame, progress, view);
    draw_stats(frame, stats, view);
    draw_sparklines(frame, sparks, view);
    draw_nodes(frame, nodes, view);
    draw_errors(frame, errors, view);
    draw_footer(frame, footer, view);
}

/// The progress bar, the percentage and the ETA (`MET-011`, `MET-031`).
fn draw_progress(frame: &mut Frame<'_>, area: Rect, view: &Dashboard) {
    let title = format!(
        " cdm {job} · {table} · run {run} · {node} ",
        job = view.job.as_str(),
        table = view.table_label(),
        run = view.run_id,
        node = view.node_id,
    );
    // The bar is the *weighted* fraction of `MET-011`, not the range count: on an uneven ring the
    // two differ by an order of magnitude, and the weighted one is the one that predicts the end.
    let label = format!(
        "{}  ·  ETA {}  ·  elapsed {}",
        percent(view.progress.weight_fraction),
        eta(view.progress.eta),
        duration_hms(view.progress.elapsed),
    );
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(title))
        .gauge_style(Style::default().fg(status_colour(view)))
        .ratio(view.progress.weight_fraction.clamp(0.0, 1.0))
        .label(label);
    frame.render_widget(gauge, area);
}

/// Throughput, request latency and ranges by state, side by side (`MET-010`).
///
/// Three columns rather than two: `MET-010`'s latencies are per side *and* per operation, so they
/// are several lines rather than one, and squeezing them under the throughput figures pushed both
/// past the height of the block — a panel that renders nothing at all is worse than one that
/// renders less.
fn draw_stats(frame: &mut Frame<'_>, area: Rect, view: &Dashboard) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);
    let (Some(&left), Some(&middle), Some(&right)) =
        (columns.first(), columns.get(1), columns.get(2))
    else {
        return;
    };

    let throughput = vec![
        labelled(
            "origin",
            format!("{} rows/s", rate(view.rows_per_second(Side::Origin))),
        ),
        labelled(
            "target",
            format!("{} rows/s", rate(view.rows_per_second(Side::Target))),
        ),
        labelled("rows read", count(view.instruments.origin.rows.total)),
        labelled("rows written", count(view.instruments.target.rows.total)),
        labelled("bytes read", count(view.instruments.origin.bytes.total)),
    ];
    frame.render_widget(
        Paragraph::new(throughput)
            .block(Block::default().borders(Borders::ALL).title(" Throughput ")),
        left,
    );

    frame.render_widget(
        Paragraph::new(latency_lines(view))
            .block(Block::default().borders(Borders::ALL).title(" Latency ")),
        middle,
    );

    let mut ranges = vec![
        labelled("planned", count(view.progress.ranges_total)),
        labelled("completed", count(view.progress.ranges_completed)),
        labelled("in flight", count(view.progress.ranges_in_flight)),
        labelled("pending", count(view.progress.ranges_pending)),
    ];
    // `MET-010`'s "ranges in each state", spelled as `TRK-012` spells them.
    let by_status: Vec<String> = view
        .progress
        .ranges_by_status
        .iter()
        .map(|(status, seen)| format!("{status} {seen}"))
        .collect();
    ranges.push(labelled("by status", by_status.join("  ")));
    frame.render_widget(
        Paragraph::new(ranges).block(Block::default().borders(Borders::ALL).title(" Ranges ")),
        right,
    );
}

/// `MET-010`'s request-latency percentiles, per side and per operation.
///
/// One line per operation a job has actually issued: a guardrail run never opens a target session
/// (`GRD-001`), so it contributes no target lines, and a run with no batching contributes no
/// `batch` line. An operation with an empty histogram is not drawn rather than drawn as zero —
/// `SideSnapshot::recorded_latencies` is the same filter the exporters apply.
///
/// Before the first request comes back there is nothing to draw at all, and the range duration is
/// shown instead. That is a real measurement of a real unit of work — see
/// `cdm_metrics::dashboard::RangeTimings` — and it is labelled as what it is rather than as a
/// latency it is not.
fn latency_lines(view: &Dashboard) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = [Side::Origin, Side::Target]
        .into_iter()
        .flat_map(|side| {
            view.instruments
                .side(side)
                .recorded_latencies()
                .into_iter()
                .map(move |(operation, histogram)| {
                    Line::from(vec![
                        Span::styled(
                            format!(
                                "{:<18}",
                                format!("{} {}", side.as_str(), operation.as_str())
                            ),
                            Style::default().add_modifier(Modifier::DIM),
                        ),
                        Span::raw(format!(
                            "{} / {}",
                            nanos_as_millis(histogram.percentile(0.5)),
                            nanos_as_millis(histogram.percentile(0.99))
                        )),
                    ])
                })
                .collect::<Vec<_>>()
        })
        .collect();
    if lines.is_empty() {
        let range = view.range_latency;
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<18}", "range p50/p99"),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw(format!(
                "{} / {}",
                nanos_as_millis(range.percentile(0.5)),
                nanos_as_millis(range.percentile(0.99))
            )),
        ]));
    }
    lines
}

/// The two sparklines `MET-031` asks for.
fn draw_sparklines(frame: &mut Frame<'_>, area: Rect, view: &Dashboard) {
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let (Some(&left), Some(&right)) = (halves.first(), halves.get(1)) else {
        return;
    };

    frame.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" rows/s (origin) "),
            )
            .style(Style::default().fg(Color::Green))
            .data(view.rows_history.clone()),
        left,
    );
    // `MET-010`'s request latency once anything has been recorded, and the range duration until
    // then — each under its own title, because they measure different things and a sparkline with
    // a borrowed label is worse than no sparkline.
    let (title, data) = if view.instruments.origin.latency.iter().any(|h| h.count > 0)
        || view.instruments.target.latency.iter().any(|h| h.count > 0)
    {
        (" request latency (ms) ", &view.request_latency_history)
    } else {
        (" range duration (ms) ", &view.latency_history)
    };
    frame.render_widget(
        Sparkline::default()
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::Cyan))
            .data(data.clone()),
        right,
    );
}

/// Per-node status, as far as it exists today (`MET-031`).
///
/// The nodes are the *cluster* nodes the driver is connected to. The cdm-rs nodes of a distributed
/// run are roadmap items #50–#52 and have no coordinator to be read from; this panel says so
/// rather than showing an empty table that implies there is nothing to show.
fn draw_nodes(frame: &mut Frame<'_>, area: Rect, view: &Dashboard) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Nodes (origin/target clusters) ");
    let body: Vec<Line<'_>> = if view.nodes.is_empty() {
        vec![Line::from(Span::styled(
            "the driver reports no nodes yet",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        view.nodes
            .iter()
            .map(|node| {
                let (state, colour) = if node.connected {
                    ("connected", Color::Green)
                } else {
                    ("no pool", Color::Red)
                };
                Line::from(vec![
                    Span::raw(format!(
                        "{side:<7}{address:<24}{dc}/{rack}  ",
                        side = node.side.as_str(),
                        address = node.address,
                        dc = node.datacenter.as_deref().unwrap_or("?"),
                        rack = node.rack.as_deref().unwrap_or("?"),
                    )),
                    Span::styled(state, Style::default().fg(colour)),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(body).block(block), area);
}

/// The error tail (`MET-031`, `SEC-002`).
fn draw_errors(frame: &mut Frame<'_>, area: Rect, view: &Dashboard) {
    let title = format!(
        " Errors {errors} · warnings {warnings}{discrepancies} ",
        errors = count(view.errors_total),
        warnings = count(view.warnings_total),
        discrepancies = if view.discrepancies_total() > 0 {
            format!(" · discrepancies {}", count(view.discrepancies_total()))
        } else {
            String::new()
        },
    );
    let block = Block::default().borders(Borders::ALL).title(title);

    // Newest last would push the interesting line off the bottom of a short panel, so the tail is
    // drawn newest-first and truncated to what fits.
    let height = usize::from(area.height.saturating_sub(2));
    let body: Vec<Line<'_>> = view
        .errors
        .iter()
        .rev()
        .take(height)
        .map(|line| {
            Line::from(Span::styled(
                format!("{} {}", line.at.format("%H:%M:%S"), error_line(line)),
                Style::default().fg(severity_colour(line.severity)),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(body).block(block), area);
}

/// The one-line footer: what the keys do, and whether the bus dropped anything.
fn draw_footer(frame: &mut Frame<'_>, area: Rect, view: &Dashboard) {
    let mut spans = vec![Span::styled(
        format!(" {KEY_HELP} "),
        Style::default().add_modifier(Modifier::DIM),
    )];
    if view.stopping {
        spans.push(Span::styled(
            " · stopping, draining in-flight ranges ",
            Style::default().fg(Color::Yellow),
        ));
    }
    if view.dropped_events > 0 {
        // `MET-030`'s bus is bounded so a slow display can never apply backpressure to a
        // migration. When it does drop, the operator is told: a tail that is quiet because
        // nothing happened and one that is quiet because the events fell out of the buffer look
        // identical, and only one of them is good news.
        spans.push(Span::styled(
            format!(" · {} events dropped ", count(view.dropped_events)),
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(status) = view.status {
        spans.push(Span::styled(
            format!(" · {} ", status.as_str()),
            Style::default().fg(if status == RunStatus::Ended {
                Color::Green
            } else {
                Color::Red
            }),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// `name    value`, padded so the values line up.
fn labelled(name: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{name:<15}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw(value),
    ])
}

/// Green while the run is healthy, yellow while it is stopping, red once it ended badly.
fn status_colour(view: &Dashboard) -> Color {
    match view.status {
        Some(RunStatus::Ended) => Color::Green,
        Some(_) => Color::Red,
        None if view.stopping => Color::Yellow,
        None => Color::Cyan,
    }
}

/// The colour one tail line is drawn in.
fn severity_colour(severity: cdm_core::Severity) -> Color {
    match severity {
        cdm_core::Severity::Error => Color::Red,
        cdm_core::Severity::Warning => Color::Yellow,
        cdm_core::Severity::Info => Color::Gray,
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
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cdm_core::{Diagnostic, JobKind, RunId, TokenRange};
    use cdm_metrics::event::{EventPayload, EventRange};
    use cdm_metrics::{
        DashboardState, Event, Instruments, NodeStatus, ProgressTracker, RangeTimings,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;

    /// Renders one frame onto an in-memory backend and returns it as text.
    ///
    /// `TestBackend` is what makes the whole renderer testable without a terminal: CI has no TTY,
    /// and a UI nobody can assert on is one that breaks silently.
    fn render(view: &Dashboard, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, view)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn dashboard() -> (
        DashboardState,
        Arc<ProgressTracker>,
        Vec<TokenRange>,
        Instant,
    ) {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(8).unwrap();
        let progress = Arc::new(ProgressTracker::by_token_span(&ranges, start));
        let mut state = DashboardState::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&progress),
            Arc::new(Instruments::new(start)),
            Arc::new(RangeTimings::new()),
        );
        state.apply(&Event {
            run_id: RunId::from_raw(7),
            node_id: "node-a".to_owned(),
            at: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::RunStarted {
                job: JobKind::Migrate,
                keyspace: Some("ks".to_owned()),
                table: Some("tbl".to_owned()),
                ranges_planned: 8,
            },
        });
        (state, progress, ranges, start)
    }

    #[test]
    fn met_031_a_frame_shows_throughput_progress_the_eta_nodes_and_the_error_tail() {
        let (mut state, progress, ranges, start) = dashboard();
        progress.range_completed(ranges[0], RunStatus::Pass);
        progress.range_completed(ranges[1], RunStatus::Pass);
        state.set_nodes(vec![NodeStatus {
            side: Side::Origin,
            address: "10.0.0.1:9042".to_owned(),
            datacenter: Some("dc1".to_owned()),
            rack: Some("rack1".to_owned()),
            connected: true,
        }]);
        state.apply(&Event {
            run_id: RunId::from_raw(7),
            node_id: "node-a".to_owned(),
            at: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::Error {
                diagnostic: Diagnostic::error("CDM-CQL", "the target refused the write"),
                range: Some(EventRange::from(ranges[3])),
            },
        });
        state.sample_at(start + Duration::from_secs(1));

        let frame = render(&state.snapshot_at(start + Duration::from_secs(60)), 100, 30);

        assert!(frame.contains("cdm migrate"), "{frame}");
        assert!(frame.contains("ks.tbl"), "{frame}");
        assert!(frame.contains("25.0%"), "{frame}");
        assert!(frame.contains("ETA 00:03:00"), "{frame}");
        assert!(frame.contains("Throughput"), "{frame}");
        assert!(frame.contains("rows/s"), "{frame}");
        assert!(frame.contains("range duration"), "{frame}");
        assert!(frame.contains("10.0.0.1:9042"), "{frame}");
        assert!(frame.contains("connected"), "{frame}");
        assert!(frame.contains("the target refused the write"), "{frame}");
        assert!(frame.contains("stop the run gracefully"), "{frame}");
    }

    #[test]
    fn met_010_the_latency_panel_draws_request_latency_once_there_is_any() {
        // What this panel showed before the engine fed anything: the range duration, honestly
        // labelled, because the histograms `MET-010` specifies were always empty. Now they are fed,
        // and the panel has to show what the requirement asks for — per side and per operation.
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(8).unwrap();
        let instruments = Arc::new(Instruments::new(start));
        let mut state = DashboardState::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::new(ProgressTracker::by_token_span(&ranges, start)),
            Arc::clone(&instruments),
            Arc::new(RangeTimings::new()),
        );

        // Through the seam `cdm-cql` uses, not through `Instruments`' own methods: the panel has
        // to render what a real request path produces.
        let observer: &dyn cdm_core::RequestObserver = instruments.as_ref();
        for _ in 0..10 {
            observer.request_started(Side::Origin);
            observer.request_finished(
                Side::Origin,
                cdm_metrics::Operation::RangeRead,
                Duration::from_millis(12),
            );
            observer.request_started(Side::Target);
            observer.request_finished(
                Side::Target,
                cdm_metrics::Operation::Write,
                Duration::from_millis(3),
            );
        }
        state.sample_at(start + Duration::from_secs(1));

        let frame = render(&state.snapshot_at(start + Duration::from_secs(1)), 100, 30);
        assert!(frame.contains("origin range_read"), "{frame}");
        assert!(frame.contains("target write"), "{frame}");
        assert!(frame.contains("request latency"), "{frame}");
        // An operation neither side issued contributes no line at all.
        assert!(!frame.contains("origin key_read"), "{frame}");
        assert!(!frame.contains("target batch"), "{frame}");
        // And the fallback is gone, because there is a real latency to draw.
        assert!(!frame.contains("range duration"), "{frame}");
        assert!(!frame.contains("range p50/p99"), "{frame}");
    }

    #[test]
    fn met_010_the_latency_panel_falls_back_to_range_duration_before_the_first_request() {
        // The first frame of a run is drawn before any request has come back. Drawing an empty
        // histogram as a latency would be a display that reads zero and means "no data"; the
        // range duration is a real measurement and is labelled as itself.
        let (state, _, _, start) = dashboard();
        let frame = render(&state.snapshot_at(start), 100, 30);
        assert!(frame.contains("range p50/p99"), "{frame}");
        assert!(frame.contains("range duration"), "{frame}");
        assert!(!frame.contains("request latency"), "{frame}");
    }

    #[test]
    fn met_031_a_frame_withholds_the_eta_rather_than_drawing_a_zero() {
        let (state, _, _, start) = dashboard();
        let frame = render(&state.snapshot_at(start + Duration::from_secs(5)), 100, 30);
        assert!(frame.contains("ETA unknown"), "{frame}");
        assert!(!frame.contains("ETA 00:00:00"), "{frame}");
    }

    #[test]
    fn met_031_a_frame_says_when_the_bounded_bus_dropped_events() {
        let (mut state, _, _, start) = dashboard();
        state.note_lag(412);
        let frame = render(&state.snapshot_at(start), 100, 30);
        assert!(frame.contains("412 events dropped"), "{frame}");
    }

    #[test]
    fn met_031_a_terminal_too_small_to_lay_out_draws_nothing_rather_than_panicking() {
        // A display must not be able to fail a migration, and an operator resizing their window
        // to two rows is not a defect. Every size from 1×1 up must render.
        let (state, _, _, start) = dashboard();
        let view = state.snapshot_at(start);
        for width in [1_u16, 4, 20, 80] {
            for height in [1_u16, 2, 5, 12, 40] {
                let _ = render(&view, width, height);
            }
        }
    }

    #[test]
    fn met_031_sec_002_no_row_data_reaches_a_cell() {
        // The snapshot type makes this structural — there is no field to draw a row from — and
        // this pins it against a future field that would carry one.
        let (mut state, _, ranges, start) = dashboard();
        state.apply(&Event {
            run_id: RunId::from_raw(7),
            node_id: "node-a".to_owned(),
            at: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::Error {
                diagnostic: Diagnostic::error("CDM-CQL", "the write was rejected")
                    .with_detail("value 'sk-live-4711' is not valid")
                    .with_value("sk-live-4711")
                    .with_suggestion("check connect.target.password"),
                range: Some(EventRange::from(ranges[0])),
            },
        });
        let frame = render(&state.snapshot_at(start), 200, 40);
        assert!(!frame.contains("sk-live-4711"), "{frame}");
        assert!(!frame.contains("password"), "{frame}");
        assert!(frame.contains("the write was rejected"), "{frame}");
    }
}
