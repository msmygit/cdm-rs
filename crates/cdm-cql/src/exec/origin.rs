//! An origin-only paged reader, for a job that must not be able to reach a target (`GRD-001`).
//!
//! # Why this is not [`RunExecutor`](super::RunExecutor)
//!
//! [`RunExecutor`](super::RunExecutor) is the right seam for migrate and validate: it carries both
//! sessions and all three prepared statements, because those jobs read one side and write the
//! other. Building the guardrail on it would mean opening a target session and preparing a target
//! upsert for a run that `GRD-001` says never touches a target at all — and "we simply do not call
//! it" is the kind of guarantee that survives right up until somebody refactors.
//!
//! [`OriginReader`] holds an origin session and one prepared statement, the token-range select of
//! `FEA-060`. There is no target field to leave unused, no upsert to accidentally reach and
//! nothing in the type for a reviewer to check. That is the same argument `cdm_engine`'s
//! `OriginRows` makes one layer up, made again at the layer that actually holds a `Session` —
//! which is where it has to hold if it is to mean anything.
//!
//! # The page size is per scan, not per run
//!
//! [`PreparedSet`](super::PreparedSet) bakes `perfops.fetch_size` into the statement once, which is
//! right for a run whose fetch size cannot change. The guardrail's reader is handed a page size per
//! range instead, because that is what its trait method takes, so [`OriginReader::scan`] sets it on
//! the per-scan clone of the statement. A caller that passes the same value every time gets exactly
//! the behaviour `PreparedSet` would have given it.
//!
//! # Specification
//!
//! - `GRD-001` — an origin session and no target, structurally
//! - `ENG-003` — [`OriginReader::scan`] pages at the requested `perfops.fetch_size`
//! - `FEA-060` — the statement it pages is the origin range select
//! - `CON-011` — the scan retries a page with the origin's backoff

use std::sync::Arc;
use std::time::Duration;

use cdm_core::{CdmError, Side, TokenRange};
use scylla::statement::prepared::PreparedStatement;
use scylla::statement::Consistency;

use crate::connect::{Backoff, ClusterSession};
use crate::statement::OriginRangeSelect;

use super::scan::{OwnedRangeScan, TokenWidth};
use super::statements::{page_size, prepare_one};
use super::DriverSession;

/// What preparing an origin-only reader needs beyond the statement text.
///
/// A deliberately smaller thing than [`PreparedSetOptions`](super::PreparedSetOptions): there is no
/// write consistency and no counter flag, because there is no write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OriginReadOptions {
    /// The consistency level origin reads run at.
    pub read_consistency: Consistency,
    /// Per-request timeout (`perfops.request_timeout`).
    pub request_timeout: Duration,
}

impl Default for OriginReadOptions {
    /// The `CFG-160` defaults, so a caller does not have to restate them.
    fn default() -> Self {
        Self {
            read_consistency: Consistency::LocalQuorum,
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// A prepared origin range select and the session to page it against (`GRD-001`, `FEA-060`).
///
/// Prepared once per run, as `ARCHITECTURE.md` §5.5 requires, and shared by every worker: [`scan`]
/// takes `&self` and each call yields an independent [`OwnedRangeScan`].
///
/// [`scan`]: OriginReader::scan
#[derive(Debug)]
pub struct OriginReader {
    session: Arc<DriverSession>,
    prepared: PreparedStatement,
    token_width: TokenWidth,
    backoff: Backoff,
}

impl OriginReader {
    /// Prepares the range select against the origin session.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`](cdm_core::ErrorKind::SchemaMismatch) if the server rejects the
    /// generated statement, which nearly always means the live schema has moved away from the one
    /// introspection reported. It is a startup failure by design: a statement that will never
    /// prepare must not be discovered on the first range.
    pub async fn prepare(
        origin: &ClusterSession,
        select: &OriginRangeSelect,
        options: OriginReadOptions,
        token_width: TokenWidth,
    ) -> Result<Self, CdmError> {
        let session = Arc::clone(origin.session());
        let mut prepared = prepare_one(&session, Side::Origin, select.cql()).await?;
        prepared.set_consistency(options.read_consistency);
        prepared.set_request_timeout(Some(options.request_timeout));
        // CON-011: a `SELECT` has no side effect, so it is always safe for the driver to retry.
        prepared.set_is_idempotent(true);
        Ok(Self {
            session,
            prepared,
            token_width,
            backoff: origin.backoff(),
        })
    }

    /// A paged scan of `range`, reading at most `fetch_size` rows per page (`ENG-003`).
    ///
    /// The returned scan owns everything it needs, so it may be boxed and moved. Nothing is sent
    /// until its first page is asked for.
    #[must_use]
    pub fn scan(&self, range: TokenRange, fetch_size: u32) -> OwnedRangeScan {
        let mut prepared = self.prepared.clone();
        prepared.set_page_size(page_size(fetch_size));
        OwnedRangeScan::for_range(
            Arc::clone(&self.session),
            prepared,
            range,
            self.token_width,
            self.backoff,
        )
    }

    /// The retry backoff the scans it hands out use (`CON-011`).
    #[must_use]
    pub const fn backoff(&self) -> Backoff {
        self.backoff
    }

    /// Which partitioner's token type its scans bind (`TOK-001`).
    #[must_use]
    pub const fn token_width(&self) -> TokenWidth {
        self.token_width
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
    use super::*;

    #[test]
    fn cfg_160_the_origin_read_defaults_are_the_configured_ones() {
        let options = OriginReadOptions::default();
        assert_eq!(options.read_consistency, Consistency::LocalQuorum);
        assert_eq!(options.request_timeout, Duration::from_secs(30));
    }

    #[test]
    fn grd_001_the_origin_reader_holds_no_field_that_could_reach_a_target() {
        // `GRD-001` is a claim about reachability, and the honest test of it is that the struct's
        // own fields name nothing on the target side. `mig_012_no_production_path_can_bind_null`
        // sweeps source for the same kind of reason: a behavioural test cannot distinguish "does
        // not write" from "does not write yet", and a field that does not exist can never be used.
        let fields = include_str!("origin.rs")
            .split("pub struct OriginReader {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("the struct definition is in this file");
        assert!(!fields.to_lowercase().contains("target"), "{fields}");
        assert!(!fields.contains("upsert"), "{fields}");
        assert!(fields.contains("session"), "{fields}");
    }
}
