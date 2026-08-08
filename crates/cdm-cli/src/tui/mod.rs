//! The interactive terminal UI, and the line-based progress it degrades to (`MET-031`).
//!
//! `MET-031` asks for `cdm migrate --tui` to show live throughput, a progress bar, an ETA,
//! per-node status, an error tail and latency sparklines, and to "degrade automatically to
//! line-based progress when stdout is not a TTY". This module is both halves of that sentence.
//!
//! # What `--tui` resolves to, and why
//!
//! The flag is a *request*, not an assertion: whether it can be honoured depends on where the
//! output is going. [`Presentation::resolve`] decides, and it is a pure function of three inputs so
//! that the decision is testable rather than only observable by eye on somebody's laptop.
//!
//! | `--tui` | stdout | `--output` | Result |
//! |---|---|---|---|
//! | absent | anything | anything | [`Presentation::Silent`] — today's behaviour, unchanged |
//! | present | a terminal | `human` | [`Presentation::Tui`] |
//! | present | a pipe, a file, a CI log | anything | [`Presentation::Lines`] |
//! | present | a terminal | `json` | [`Presentation::Lines`] |
//!
//! The last row is the one that is not in the requirement and belongs there anyway. `CLI-005`
//! promises `--output json` a single parseable document on standard output; a UI that painted over
//! it would break every script that asked for both, and failing the command instead would break
//! the ones that pass `--tui` from a wrapper. Falling back is the only option that leaves both
//! promises intact — and the line-based progress goes to standard *error*, so the document is
//! untouched either way.
//!
//! # `--tui` takes effect or says why
//!
//! A flag that parses and is then ignored is a defect this codebase has shipped before. If the
//! terminal cannot be taken after all — raw mode refused on a handle that claimed to be a TTY —
//! the display does not silently vanish: it says so on standard error and continues in line mode.
//! There is no path on which `--tui` is accepted and nothing happens.
//!
//! # Which commands it covers
//!
//! `cdm migrate`, `cdm validate` and `cdm guardrail`. The requirement's example names migrate, but
//! all three run the same scheduler through the same harness over the same token plan, and there
//! is nothing job-specific in what is drawn — a six-hour validate run wants a progress bar exactly
//! as much as a migration does. `cdm plan` deliberately **rejects** the flag rather than accepting
//! and ignoring it: it computes a plan and never runs a range, so there would be nothing to show.

pub mod format;
pub mod lines;
pub mod screen;
pub mod terminal;

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdm_engine::scheduler::RunControl;
use cdm_metrics::event::EventPayload;
use cdm_metrics::{DashboardState, Event, EventStreamError, EventSubscriber};

use crate::cli::OutputFormat;
use crate::tui::terminal::TerminalGuard;

/// How often the display wakes up: to drain events, to sample the sparklines, and — in
/// [`Presentation::Tui`] — to redraw and to read the keyboard.
pub const TICK: Duration = Duration::from_millis(200);

/// How many ticks pass between two lines of [`Presentation::Lines`] progress.
///
/// Five seconds. A log line every 200ms would be forty thousand lines an hour, which is not
/// progress reporting but a second copy of the event stream.
const LINE_EVERY_TICKS: u64 = 25;

/// How many events one tick may drain.
///
/// A bound, not a target. `MET-030`'s bus is bounded precisely so that a slow consumer cannot
/// apply backpressure to a migration, and an unbounded drain here would reintroduce the coupling
/// from the other end: a validate run finding a million discrepancies could keep this loop
/// spinning on a runtime thread that the *workers* need. Draining a budget per tick means the
/// display falls behind instead — which the bus already handles, and which the display already
/// reports as dropped events.
const DRAIN_BUDGET: usize = 512;

/// How many ticks pass between two refreshes of the per-node panel.
///
/// Two seconds. The driver keeps its cluster metadata current from its own topology events, so
/// asking costs no query — but it does take the driver's lock, and a node coming back two seconds
/// later than it might have is not something anybody is watching for at that resolution.
const NODES_EVERY_TICKS: u64 = 10;

/// Where the per-node panel gets its rows (`MET-031`).
///
/// A callback rather than a snapshot, so that a node dropping out mid-run appears on the display
/// instead of being frozen at whatever was true when the run started.
pub type NodeProvider = Arc<dyn Fn() -> Vec<cdm_metrics::NodeStatus> + Send + Sync>;

/// How a run reports its progress while it runs (`MET-031`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// Nothing until the run ends. What every command did before `--tui` existed, and what they
    /// still do without it.
    Silent,
    /// One progress line on standard error every few seconds.
    Lines,
    /// The full interactive display.
    Tui,
}

impl Presentation {
    /// Decides what `--tui` means for this invocation (`MET-031`, `CLI-005`).
    ///
    /// See the module documentation for the table this implements and the reasoning behind the row
    /// that is not in the requirement.
    #[must_use]
    pub const fn resolve(requested: bool, stdout_is_terminal: bool, format: OutputFormat) -> Self {
        if !requested {
            return Self::Silent;
        }
        match format {
            OutputFormat::Human if stdout_is_terminal => Self::Tui,
            // A pipe, a redirect or a CI log — and `--output json`, whose single parseable document
            // owns stdout (`CLI-005`). Progress goes to stderr in both cases.
            OutputFormat::Human | OutputFormat::Json => Self::Lines,
        }
    }

    /// Decides from the process's real standard output (`MET-031`).
    ///
    /// The one place `IsTerminal` is consulted. Everything else takes the answer as a parameter, so
    /// that the branch is testable where the environment is not: `cargo test` captures stdout, CI
    /// has no TTY, and a test that asked the real one would assert the opposite thing depending on
    /// where it ran.
    #[must_use]
    pub fn detect(requested: bool, format: OutputFormat) -> Self {
        Self::resolve(requested, std::io::stdout().is_terminal(), format)
    }

    /// Whether anything is displayed while the run runs.
    #[must_use]
    pub const fn is_live(self) -> bool {
        !matches!(self, Self::Silent)
    }
}

/// A running display, joined before the command prints its report.
///
/// Dropping this without [`LiveDisplay::finish`] leaves the task running; every caller in the
/// harness finishes it on every path, including the failing ones, because the terminal has to be
/// handed back before anything else writes to it.
#[derive(Debug)]
pub struct LiveDisplay {
    stop: Arc<AtomicBool>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl LiveDisplay {
    /// A display that shows nothing and needs no task.
    #[must_use]
    pub fn none() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            handle: None,
        }
    }

    /// Starts the display for a run (`MET-031`).
    ///
    /// The terminal is taken here, on the caller's thread, so that a refusal is a value this
    /// function can act on rather than an error inside a detached task nobody reads. A refusal is
    /// never fatal: it degrades to [`Presentation::Lines`] with a line on standard error saying so.
    #[must_use]
    pub fn start(
        presentation: Presentation,
        state: DashboardState,
        events: EventSubscriber,
        control: RunControl,
        nodes: Option<NodeProvider>,
    ) -> Self {
        let mode = match presentation {
            Presentation::Silent => return Self::none(),
            Presentation::Lines => Mode::Lines,
            Presentation::Tui => match TerminalGuard::enter() {
                Ok(guard) => Mode::Tui(Box::new(guard)),
                Err(error) => {
                    // Visible, not silent: `--tui` was accepted, so the operator is owed an
                    // explanation of why they are looking at lines instead.
                    let _ = writeln!(
                        std::io::stderr(),
                        "warning: the terminal UI could not start ({}); \
                         falling back to line-based progress",
                        error.message()
                    );
                    Mode::Lines
                }
            },
        };

        let stop = Arc::new(AtomicBool::new(false));
        let handle = tokio::spawn(drive(
            mode,
            state,
            events,
            control,
            nodes,
            Arc::clone(&stop),
        ));
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Stops the display and waits for the terminal to be handed back.
    ///
    /// Awaited rather than aborted. Aborting the task would drop the [`TerminalGuard`] on an
    /// arbitrary thread at an arbitrary moment, and — worse — would race the report the command is
    /// about to print onto a screen that is still the alternate one.
    pub async fn finish(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            if handle.await.is_err() {
                // The task panicked. `TerminalGuard`'s hook has already restored the terminal;
                // there is nothing further to do, and failing the run over a display would be
                // exactly backwards.
                terminal::restore();
            }
        }
    }
}

/// What the display is currently doing.
enum Mode {
    /// Boxed because a `TerminalGuard` is far larger than the other variant, and clippy is right
    /// that a two-variant enum should not cost the size of its largest member everywhere.
    Tui(Box<TerminalGuard>),
    Lines,
}

impl std::fmt::Debug for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tui(_) => f.write_str("Tui"),
            Self::Lines => f.write_str("Lines"),
        }
    }
}

/// The display loop: drain, sample, draw, repeat (`MET-031`).
async fn drive(
    mut mode: Mode,
    mut state: DashboardState,
    mut events: EventSubscriber,
    control: RunControl,
    nodes: Option<NodeProvider>,
    stop: Arc<AtomicBool>,
) {
    let mut ticker = tokio::time::interval(TICK);
    // A display that fell behind must not then redraw as fast as it can to catch up: the frames it
    // missed are frames nobody will ever see.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks: u64 = 0;

    loop {
        ticker.tick().await;
        ticks += 1;
        let drained = drain(&mut state, &mut events);
        state.sample();
        if let Some(nodes) = &nodes {
            if ticks == 1 || ticks.is_multiple_of(NODES_EVERY_TICKS) {
                state.set_nodes(nodes());
            }
        }
        let finished = stop.load(Ordering::Relaxed);

        match &mut mode {
            Mode::Tui(guard) => {
                if poll_keys() {
                    // `ENG-010`'s graceful stop, exactly as the signal listener would have caused
                    // it — including the escalation on a second press.
                    control.signalled();
                    state.set_stopping(true);
                }
                if control.is_stopping() {
                    state.set_stopping(true);
                }
                let view = state.snapshot();
                let _ = guard.terminal().draw(|frame| screen::draw(frame, &view));
            }
            Mode::Lines => {
                for event in &drained {
                    report_line_event(event);
                }
                if control.is_stopping() {
                    state.set_stopping(true);
                }
                if ticks.is_multiple_of(LINE_EVERY_TICKS) {
                    let _ = writeln!(
                        std::io::stderr(),
                        "{}",
                        lines::progress_line(&state.snapshot())
                    );
                }
            }
        }

        if finished {
            break;
        }
    }

    // Whatever landed while the last frame was being drawn: the `RunCompleted` event is published
    // on the way out, and a display that missed it would report the run as still running.
    drain(&mut state, &mut events);
    if matches!(mode, Mode::Lines) {
        let _ = writeln!(
            std::io::stderr(),
            "{}",
            lines::final_line(&state.snapshot())
        );
    }
    // `mode` drops here, and with it the `TerminalGuard`: raw mode off, alternate screen left.
}

/// Folds up to [`DRAIN_BUDGET`] events, recording any the bounded bus dropped.
fn drain(state: &mut DashboardState, events: &mut EventSubscriber) -> Vec<Event> {
    let mut applied = Vec::new();
    for _ in 0..DRAIN_BUDGET {
        match events.try_recv() {
            Ok(Some(event)) => {
                state.apply(&event);
                applied.push(event);
            }
            Err(EventStreamError::Lagged(missed)) => state.note_lag(missed),
            // Nothing buffered, or the run is over and the bus has gone: either way, done.
            Ok(None) | Err(EventStreamError::Closed) => break,
        }
    }
    applied
}

/// Prints an error or a warning as it happens, for [`Presentation::Lines`].
///
/// Only these two kinds. A range-level event per range would drown the log the progress lines are
/// meant to be readable in, and a discrepancy carries row-derived data that `SEC-002` keeps to the
/// diff log; both are counted into the next progress line instead.
fn report_line_event(event: &Event) {
    let (EventPayload::Warning { diagnostic } | EventPayload::Error { diagnostic, .. }) =
        &event.payload
    else {
        return;
    };
    let line = cdm_metrics::ErrorLine {
        at: event.at,
        severity: diagnostic.severity,
        code: diagnostic.code.clone(),
        title: diagnostic.title.clone(),
        location: diagnostic.location.clone(),
        range: match &event.payload {
            EventPayload::Error { range, .. } => range
                .as_ref()
                .map(|range| format!("{}..{}", range.min, range.max)),
            _ => None,
        },
    };
    let _ = writeln!(std::io::stderr(), "{}", lines::error_line(&line));
}

/// Reads whatever keys are already pending, returning whether a stop was asked for.
///
/// Non-blocking: a zero-duration poll, drained until it is empty. Raw mode means Ctrl-C arrives
/// here as a key rather than as `SIGINT` (see [`terminal`]), so this is the *only* thing that can
/// honour it while the UI is up.
fn poll_keys() -> bool {
    use crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};

    let mut stop = false;
    while crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
        let Ok(event) = crossterm::event::read() else {
            break;
        };
        let TermEvent::Key(key) = event else { continue };
        // A key press, not its release: a terminal with the kitty protocol enabled reports both,
        // and acting on each would stop the run twice — which escalates a graceful stop into an
        // abandonment (`ENG-010`).
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => stop = true,
            KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                stop = true;
            }
            _ => {}
        }
    }
    stop
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
    use std::time::Instant;

    use cdm_core::{Diagnostic, JobKind, RunId, TokenRange};
    use cdm_metrics::{EventBus, Instruments, ProgressTracker, RangeTimings};

    use super::*;

    #[test]
    fn met_031_without_the_flag_nothing_is_displayed() {
        for terminal in [true, false] {
            for format in [OutputFormat::Human, OutputFormat::Json] {
                assert_eq!(
                    Presentation::resolve(false, terminal, format),
                    Presentation::Silent,
                    "terminal={terminal} format={format:?}"
                );
            }
        }
    }

    #[test]
    fn met_031_the_ui_degrades_to_lines_when_stdout_is_not_a_terminal() {
        // The requirement's own sentence, and the case every pipe, every `>` redirect and every CI
        // job takes. It is a branch rather than an observation precisely so that it can be tested
        // where there is no terminal to observe.
        assert_eq!(
            Presentation::resolve(true, false, OutputFormat::Human),
            Presentation::Lines
        );
        assert_eq!(
            Presentation::resolve(true, true, OutputFormat::Human),
            Presentation::Tui
        );
    }

    #[test]
    fn met_031_cli_005_a_json_run_never_gets_a_ui_over_its_document() {
        // `--output json` promises one parseable document on stdout. Painting a UI over it, on a
        // developer's terminal but not in the CI job that consumes the output, is the kind of
        // difference nobody finds until it breaks a pipeline.
        assert_eq!(
            Presentation::resolve(true, true, OutputFormat::Json),
            Presentation::Lines
        );
        assert_eq!(
            Presentation::resolve(true, false, OutputFormat::Json),
            Presentation::Lines
        );
    }

    #[test]
    fn met_031_detecting_from_the_real_stdout_agrees_with_the_branch() {
        // Under `cargo test` stdout is captured, so this is the not-a-terminal branch; on a
        // developer running `cargo test -- --nocapture` from a terminal it is the other one.
        // Either way it must agree with `resolve`, which is what the rest of the suite pins.
        let expected =
            Presentation::resolve(true, std::io::stdout().is_terminal(), OutputFormat::Human);
        assert_eq!(Presentation::detect(true, OutputFormat::Human), expected);
        assert_eq!(
            Presentation::detect(false, OutputFormat::Human),
            Presentation::Silent
        );
        assert!(Presentation::Tui.is_live());
        assert!(Presentation::Lines.is_live());
        assert!(!Presentation::Silent.is_live());
    }

    fn state() -> DashboardState {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        DashboardState::new(
            JobKind::Migrate,
            RunId::from_raw(1),
            "node-a",
            Arc::new(ProgressTracker::by_token_span(&ranges, start)),
            Arc::new(Instruments::new(start)),
            Arc::new(RangeTimings::new()),
        )
    }

    #[tokio::test]
    async fn met_031_the_drain_is_bounded_and_reports_what_the_bus_dropped() {
        // Two properties at once: one tick never drains more than `DRAIN_BUDGET`, so a flood
        // cannot monopolise a runtime thread the workers need; and whatever the bounded bus threw
        // away is counted rather than lost silently.
        let bus = EventBus::with_capacity(
            RunId::from_raw(1),
            "node-a",
            64,
            cdm_metrics::Redaction::default(),
        );
        let mut subscriber = bus.subscribe();
        let mut state = state();

        for index in 0..2_000 {
            bus.warning(
                chrono::Utc::now(),
                Diagnostic::warning("CDM-INTERNAL", format!("warning {index}")),
            );
        }
        assert_eq!(bus.published(), 2_000, "publishing never blocked");

        let drained = drain(&mut state, &mut subscriber);
        assert!(drained.len() <= DRAIN_BUDGET, "{}", drained.len());
        // Only the last 64 were still buffered; the rest are reported as dropped.
        assert_eq!(drained.len(), 64);
        assert_eq!(state.snapshot().dropped_events, 2_000 - 64);
    }

    #[tokio::test]
    async fn met_031_a_silent_display_starts_no_task_and_finishes_immediately() {
        let display = LiveDisplay::none();
        display.finish().await;
    }

    #[tokio::test]
    async fn met_031_a_line_display_runs_and_hands_control_back() {
        // The fallback path end to end, on the runtime it will really run on: start it, publish
        // through the bus, stop it, and require `finish` to return. A display that could not be
        // stopped would hang every run that used one.
        let bus = Arc::new(EventBus::new(RunId::from_raw(1), "node-a"));
        let subscriber = bus.subscribe();
        let control = RunControl::new();
        let display = LiveDisplay::start(
            Presentation::Lines,
            state(),
            subscriber,
            control.clone(),
            None,
        );
        bus.warning(
            chrono::Utc::now(),
            Diagnostic::warning("CDM-INTERNAL", "a warning nobody has to act on"),
        );
        tokio::time::sleep(TICK * 2).await;
        display.finish().await;
    }
}
