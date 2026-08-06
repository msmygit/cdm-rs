//! The migrate job's counter tokens, resolved once (`MET-002`, `MET-003`).
//!
//! `MET-003` makes an unregistered counter a *startup* error rather than a failure on an arbitrary
//! row: [`JobCounters::counter`] returns a token, and every later operation takes the token and is
//! infallible. Resolving all five here means the row loop has no error branch for counting at all.

use cdm_core::CdmError;
use cdm_metrics::{Counter, CounterKind, CounterView, JobCounters};

/// The five counters a migrate range keeps (`MET-002`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateCounters {
    read: Counter,
    write: Counter,
    skipped: Counter,
    error: Counter,
    unflushed: Counter,
}

impl MigrateCounters {
    /// Resolves every token from a range's registry.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`](cdm_core::ErrorKind::Internal) if the registry is not a migrate
    /// registry, which is a programming error and must stop the run.
    pub fn resolve(counters: &JobCounters) -> Result<Self, CdmError> {
        Ok(Self {
            read: counters.counter(CounterKind::Read)?,
            write: counters.counter(CounterKind::Write)?,
            skipped: counters.counter(CounterKind::Skipped)?,
            error: counters.counter(CounterKind::Error)?,
            unflushed: counters.counter(CounterKind::Unflushed)?,
        })
    }

    /// Rows read from the origin (`MIG-001`).
    #[must_use]
    pub const fn read(self) -> Counter {
        self.read
    }

    /// Rows written to the target, credited only on a flush (`MIG-005`).
    #[must_use]
    pub const fn write(self) -> Counter {
        self.write
    }

    /// Rows a filter rejected or that produced no statement (`MIG-002`, `MIG-003`).
    #[must_use]
    pub const fn skipped(self) -> Counter {
        self.skipped
    }

    /// Rows lost to a record-level failure (`ERR-005`).
    #[must_use]
    pub const fn error(self) -> Counter {
        self.error
    }

    /// Writes issued but not yet credited to `WRITE` (`MIG-004`).
    #[must_use]
    pub const fn unflushed(self) -> Counter {
        self.unflushed
    }

    /// The **interim** `UNFLUSHED` count — the only level at which it is ever non-zero.
    ///
    /// Named rather than left to a call site because reading the committed level here is precisely
    /// the Java defect `MIG-004` documents, and a call site that spells out
    /// `count(unflushed, CounterView::Committed)` looks just as deliberate as one that does not.
    #[must_use]
    pub fn unflushed_count(self, counters: &JobCounters) -> u64 {
        counters.count(self.unflushed, CounterView::Interim)
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
    use cdm_core::JobKind;

    use super::*;

    #[test]
    fn met_003_every_migrate_counter_resolves_at_startup() {
        let registry = JobCounters::new(JobKind::Migrate);
        let counters = MigrateCounters::resolve(&registry).unwrap();
        assert_eq!(counters.read().kind(), CounterKind::Read);
        assert_eq!(counters.write().kind(), CounterKind::Write);
        assert_eq!(counters.skipped().kind(), CounterKind::Skipped);
        assert_eq!(counters.error().kind(), CounterKind::Error);
        assert_eq!(counters.unflushed().kind(), CounterKind::Unflushed);
    }

    #[test]
    fn met_003_a_registry_of_the_wrong_job_is_refused_before_any_row_moves() {
        let registry = JobCounters::new(JobKind::Validate);
        let error = MigrateCounters::resolve(&registry).unwrap_err();
        assert!(error.kind().is_fatal());
    }

    #[test]
    fn mig_004_the_unflushed_count_is_read_at_the_interim_level() {
        let registry = JobCounters::new(JobKind::Migrate);
        let counters = MigrateCounters::resolve(&registry).unwrap();
        registry.increment_by(counters.unflushed(), 7);

        assert_eq!(counters.unflushed_count(&registry), 7);
        assert_eq!(
            registry.count(counters.unflushed(), CounterView::Committed),
            0,
            "the committed level is the one Java reads, and it is structurally always zero"
        );
    }
}
