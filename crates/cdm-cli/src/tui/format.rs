//! Turning a [`Dashboard`](cdm_metrics::Dashboard) into text (`MET-031`).
//!
//! Every function here is pure and takes no terminal, which is the point: the numbers a live
//! display shows are the part that can be wrong in a way nobody notices — an ETA rounded to zero,
//! a rate that reads `12345.6789012 rows/s`, a percentage that says `100.0%` on a run with ranges
//! still in flight — and a pure function is a thing a test can pin.

use std::time::Duration;

/// What the display shows in place of an ETA that `MET-011` is withholding.
///
/// `MET-011` withholds the estimate until enough of the run has completed for the extrapolation to
/// mean anything. A display must pass that through rather than paper over it: `unknown` is a
/// statement an operator can act on ("wait a minute and look again"), whereas `00:00:00` or a
/// blank cell is one they will read as a number.
pub const ETA_UNKNOWN: &str = "unknown";

/// A fraction of one, as a percentage with one decimal.
#[allow(clippy::cast_precision_loss)]
pub fn percent(fraction: f64) -> String {
    format!("{:.1}%", fraction.clamp(0.0, 1.0) * 100.0)
}

/// A duration as `HH:MM:SS`, or `NdHH:MM:SS` past a day.
///
/// Days are spelled out rather than rolled into the hours because `73:12:04` is a number people
/// misread, and a migration that runs for three days is the normal case rather than the odd one.
pub fn duration_hms(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    let (days, rest) = (total / 86_400, total % 86_400);
    let (hours, minutes, seconds) = (rest / 3_600, (rest % 3_600) / 60, rest % 60);
    if days > 0 {
        format!("{days}d {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// An ETA, or [`ETA_UNKNOWN`] when `MET-011` is withholding one.
pub fn eta(estimate: Option<Duration>) -> String {
    estimate.map_or_else(|| ETA_UNKNOWN.to_owned(), duration_hms)
}

/// An integer with thousands separators, e.g. `1,234,567`.
///
/// A row count is the number an operator compares against what they expected, and comparing
/// `1234567` with `12345678` by eye is how a factor of ten goes unnoticed.
pub fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A rate as a whole number of events per second, with thousands separators.
///
/// Rounded, and never negative: an exponentially-weighted average of a non-negative quantity
/// cannot be below zero, and displaying five decimal places of one implies a precision the meter
/// does not have.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn rate(per_second: f64) -> String {
    if !per_second.is_finite() || per_second <= 0.0 {
        return "0".to_owned();
    }
    count(per_second.round() as u64)
}

/// Nanoseconds, as the histograms of `MET-010` record them, in milliseconds.
#[allow(clippy::cast_precision_loss)]
pub fn nanos_as_millis(nanos: u64) -> String {
    format!("{:.0}ms", nanos as f64 / 1_000_000.0)
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
    fn met_031_a_withheld_eta_is_shown_as_unknown_and_never_as_zero() {
        // `MET-011` returns `None` until the extrapolation means something. `00:00:00` would read
        // as "about to finish", which is the opposite of what the absence means.
        assert_eq!(eta(None), "unknown");
        assert_eq!(eta(Some(Duration::ZERO)), "00:00:00");
        assert_eq!(eta(Some(Duration::from_secs(754))), "00:12:34");
        assert_eq!(eta(Some(Duration::from_secs(180_000))), "2d 02:00:00");
    }

    #[test]
    fn met_031_progress_is_a_percentage_that_cannot_overshoot() {
        assert_eq!(percent(0.0), "0.0%");
        assert_eq!(percent(0.4213), "42.1%");
        assert_eq!(percent(1.0), "100.0%");
        // A duplicate completion cannot make the display say 103% (`MET-011` clamps too).
        assert_eq!(percent(1.03), "100.0%");
        assert_eq!(percent(-0.5), "0.0%");
    }

    #[test]
    fn met_031_counts_and_rates_are_readable_at_a_glance() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(rate(12_345.678), "12,346");
        assert_eq!(rate(0.0), "0");
        assert_eq!(rate(-1.0), "0", "an average of a non-negative quantity");
        assert_eq!(rate(f64::NAN), "0");
        assert_eq!(rate(f64::INFINITY), "0");
    }

    #[test]
    fn met_031_elapsed_time_rolls_into_days_rather_than_a_hundred_hours() {
        assert_eq!(duration_hms(Duration::ZERO), "00:00:00");
        assert_eq!(duration_hms(Duration::from_secs(3_661)), "01:01:01");
        assert_eq!(duration_hms(Duration::from_secs(263_524)), "3d 01:12:04");
    }

    #[test]
    fn met_031_latency_percentiles_are_shown_in_milliseconds() {
        // `MET-010`'s histograms record nanoseconds; nobody reads a run in nanoseconds.
        assert_eq!(nanos_as_millis(0), "0ms");
        assert_eq!(nanos_as_millis(1_500_000), "2ms");
        assert_eq!(nanos_as_millis(12_000_000_000), "12000ms");
    }
}
