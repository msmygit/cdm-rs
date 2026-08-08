//! The Java-compatible backend: `cdm_run_info` and `cdm_run_details` in the target keyspace
//! (`TRK-010`, `TRK-020`..`TRK-022`, `TRK-030`).
//!
//! # Why this module names `scylla`
//!
//! `ARCHITECTURE.md` §3 reserves the driver for `cdm-cql`, and this is the one place that rule is
//! bent. The tracking tables live in the **target** keyspace and are written through the target
//! session that `cdm-cql` already built, which it hands out as an
//! `Arc<scylla::client::session::Session>`; there is no statement facade over it yet. Wrapping
//! the session in a new trait here would mean inventing a second session type, which the design
//! explicitly does not want. When `cdm-cql` grows the statement layer of `SCH-020`, this module
//! becomes a caller of it and the dependency goes away.
//!
//! # Token bounds and `bigint`
//!
//! `cdm_run_details.token_min` and `token_max` are `bigint`, because Java stores
//! `partition.getMin().longValue()`. A Murmur3 token fits; a RandomPartitioner token — an
//! unsigned 127-bit value — does not, and Java truncates it silently, which turns a resume into a
//! plan for a *different* part of the ring. cdm-rs refuses instead: [`token_bound`] returns a
//! Tier-1-shaped error naming the range, so a RandomPartitioner run with tracking enabled fails
//! at initialisation rather than corrupting its own resume. See the report on this in
//! `docs/TRACEABILITY.md`.

use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{
    CdmError, ErrorKind, JobKind, LeaseOutcome, LeaseRecord, LeaseStore, Plugin, RangeRecord,
    RunClaim, RunId, RunRecord, RunStatus, Side, TableRef, TokenRange, TrackingStore,
};
use cdm_cql::connect::ClusterSession;
use chrono::{DateTime, Utc};
use scylla::client::session::Session;
use scylla::frame::types::SerialConsistency;
use scylla::statement::prepared::PreparedStatement;
use scylla::value::{CqlTimestamp, CqlValue, MaybeUnset, Row};
use tokio::sync::OnceCell;

use crate::compat::{job_from_run_type, run_type, status as status_string};
use crate::schema::TrackingTables;
use crate::store::decode_status;

/// The shape of a `cdm_run_info` row as this module selects it.
type InfoRow = (
    i64,
    Option<String>,
    Option<i64>,
    Option<CqlTimestamp>,
    Option<CqlTimestamp>,
    Option<String>,
    Option<String>,
);

/// The shape of a `cdm_run_leases` row as this module selects it (`DST-010`).
type LeaseRow = (i64, Option<String>, Option<CqlTimestamp>, Option<i32>);

/// The shape of a `cdm_run_details` row as this module selects it.
type DetailRow = (
    i64,
    i64,
    Option<CqlTimestamp>,
    Option<String>,
    Option<String>,
);

/// Every statement, prepared once.
///
/// Preparing is not free and a run issues these thousands of times, so they are prepared on first
/// use and cached. `OnceCell` rather than a `Mutex<Option<_>>` because two workers reaching this
/// at once should both wait for one preparation, not race to do it twice.
#[derive(Debug)]
struct Statements {
    insert_run_info: PreparedStatement,
    insert_run_detail: PreparedStatement,
    end_run_info: PreparedStatement,
    start_run_detail: PreparedStatement,
    update_run_detail: PreparedStatement,
    select_run_info: PreparedStatement,
    select_latest_run_info: PreparedStatement,
    select_run_details: PreparedStatement,
    select_runs: PreparedStatement,
}

/// The statements distributed mode adds, prepared once and only when it is used.
///
/// A second cache rather than five more fields on [`Statements`]: a single-node run never
/// prepares them, never creates `cdm_run_leases`, and therefore never asks the target for a
/// table Java would not have created (`TRK-011`).
#[derive(Debug)]
struct LeaseStatements {
    elect: PreparedStatement,
    claim_if_absent: PreparedStatement,
    reclaim_if_expired: PreparedStatement,
    renew: PreparedStatement,
    select_lease: PreparedStatement,
    select_leases: PreparedStatement,
}

/// Tracking in the target keyspace, in Java's schema (`TRK-010`, `TRK-036`).
#[derive(Debug)]
pub struct CassandraStore {
    session: Arc<Session>,
    tables: TrackingTables,
    create_leases: bool,
    statements: OnceCell<Statements>,
    lease_statements: OnceCell<LeaseStatements>,
}

impl CassandraStore {
    /// A store over the connected **target** cluster (`TRK-010`).
    ///
    /// The supported constructor: it takes `cdm-cql`'s own connected session rather than a bare
    /// driver handle, and refuses the origin outright. Pointing run tracking at the origin is a
    /// mistake that only shows up much later — the tables are created, the run is recorded, and
    /// then the resume reads a keyspace that has nothing to do with what was written — so it is
    /// caught here, where the two sessions are still distinguishable.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if `session` is the origin, or if the table reference
    /// cannot name a tracking table.
    pub fn for_target(session: &ClusterSession, table: &TableRef) -> Result<Self, CdmError> {
        if session.side() != Side::Target {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                "run tracking writes cdm_run_info and cdm_run_details into the target keyspace \
                 (TRK-010), but it was handed the origin session",
            ));
        }
        Self::new(Arc::clone(session.session()), table)
    }

    /// A store over `session`, tracking the target table `table`.
    ///
    /// `session` must be the **target** session: `TRK-010` puts both tables in the target
    /// keyspace, so that a run is recorded next to the data it produced and survives an origin
    /// that goes away.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if the table reference cannot name a tracking table — see
    /// [`TrackingTables::new`].
    pub fn new(session: Arc<Session>, table: &TableRef) -> Result<Self, CdmError> {
        Ok(Self {
            session,
            tables: TrackingTables::new(table)?,
            create_leases: false,
            statements: OnceCell::new(),
            lease_statements: OnceCell::new(),
        })
    }

    /// Also create `cdm_run_leases` on [`TrackingStore::initialise`] (`TRK-011`).
    ///
    /// Off by default, and it must stay off for a single-node run: the target keyspace of a
    /// tracked cdm-rs run then holds exactly the two tables Java would have created, and nothing
    /// else, which is what lets an operator point either tool at it.
    #[must_use]
    pub fn with_leases(mut self, create_leases: bool) -> Self {
        self.create_leases = create_leases;
        self
    }

    /// The tables this store reads and writes.
    pub fn tables(&self) -> &TrackingTables {
        &self.tables
    }

    /// The prepared statements, preparing them on first use.
    async fn statements(&self) -> Result<&Statements, CdmError> {
        self.statements
            .get_or_try_init(|| async {
                Ok(Statements {
                    insert_run_info: self.prepare(&self.tables.insert_run_info()).await?,
                    insert_run_detail: self.prepare(&self.tables.insert_run_detail()).await?,
                    end_run_info: self.prepare(&self.tables.end_run_info()).await?,
                    start_run_detail: self.prepare(&self.tables.start_run_detail()).await?,
                    update_run_detail: self.prepare(&self.tables.update_run_detail()).await?,
                    select_run_info: self.prepare(&self.tables.select_run_info()).await?,
                    select_latest_run_info: self
                        .prepare(&self.tables.select_latest_run_info())
                        .await?,
                    select_run_details: self.prepare(&self.tables.select_run_details()).await?,
                    select_runs: self.prepare(&self.tables.select_runs()).await?,
                })
            })
            .await
    }

    async fn prepare(&self, cql: &str) -> Result<PreparedStatement, CdmError> {
        self.session.prepare(cql).await.map_err(|e| {
            // The statement text is safe to log: it holds only identifiers and `?` markers
            // (`SEC-001`).
            CdmError::new(
                ErrorKind::Tracking,
                format!("cannot prepare tracking statement `{cql}`: {e}"),
            )
        })
    }

    /// Reads one `cdm_run_info` row into a [`RunRecord`].
    fn decode_run(&self, row: InfoRow) -> RunRecord {
        let (run_id, run_type_value, prev_run_id, start_time, end_time, info, status) = row;
        RunRecord {
            run_id: RunId::from_raw(run_id),
            // Java writes `prev_run_id = 0` for "none", and cdm-rs keeps writing 0 for
            // compatibility; both mean the same thing, and neither is a run.
            previous_run_id: prev_run_id.filter(|id| *id != 0).map(RunId::from_raw),
            table: TableRef::new(self.tables.keyspace(), self.tables.table_name()),
            // A `run_type` this build does not recognise cannot be guessed at, and mapping it to
            // Migrate would let a validate run be resumed as a migration. The selection in
            // `latest_run` filters by run_type in CQL, so this only fires for a direct lookup.
            job: run_type_value
                .as_deref()
                .and_then(job_from_run_type)
                .unwrap_or(JobKind::Migrate),
            status: decode_status(status.as_deref()),
            started_at: start_time.and_then(from_cql_timestamp),
            ended_at: end_time.and_then(from_cql_timestamp),
            info,
        }
    }

    /// The lease statements, prepared on first use with `SERIAL` on every conditional one.
    ///
    /// `DST-011` requires a claim to be a lightweight transaction at `SERIAL`, and the driver
    /// otherwise applies whatever serial consistency the execution profile carries. Pinning it on
    /// the statement means the guarantee travels with the statement: a deployment that edits its
    /// profile cannot quietly downgrade a lease to `LOCAL_SERIAL`, under which two datacentres
    /// can each elect a holder for the same range and both believe they are alone.
    async fn lease_statements(&self) -> Result<&LeaseStatements, CdmError> {
        self.lease_statements
            .get_or_try_init(|| async {
                Ok(LeaseStatements {
                    elect: self
                        .prepare_lwt(&self.tables.insert_run_info_if_not_exists())
                        .await?,
                    claim_if_absent: self
                        .prepare_lwt(&self.tables.claim_lease_if_absent())
                        .await?,
                    reclaim_if_expired: self
                        .prepare_lwt(&self.tables.reclaim_lease_if_expired())
                        .await?,
                    renew: self.prepare_lwt(&self.tables.renew_lease()).await?,
                    select_lease: self.prepare(&self.tables.select_lease()).await?,
                    select_leases: self.prepare(&self.tables.select_leases()).await?,
                })
            })
            .await
    }

    /// Prepares a conditional statement and pins its serial consistency to `SERIAL` (`DST-011`).
    async fn prepare_lwt(&self, cql: &str) -> Result<PreparedStatement, CdmError> {
        let mut prepared = self.prepare(cql).await.map_err(|e| lease_error_from(&e))?;
        prepared.set_serial_consistency(Some(SerialConsistency::Serial));
        Ok(prepared)
    }

    /// One lease row, or `None` if the range has never been claimed.
    ///
    /// A plain read at the session's consistency, deliberately not a `SERIAL` one: it decides
    /// nothing. It supplies the `attempt` a takeover increments and the holder a denial reports,
    /// and every claim it informs is still settled by the conditional write, which is the only
    /// thing that can grant a range. A stale read therefore costs a wasted round trip at worst,
    /// never a second holder.
    async fn lease_row(
        &self,
        run_id: RunId,
        token_min: i64,
    ) -> Result<Option<LeaseRecord>, CdmError> {
        let statements = self.lease_statements().await?;
        let result = self
            .session
            .execute_unpaged(
                &statements.select_lease,
                (self.tables.table_name(), run_id.as_i64(), token_min),
            )
            .await
            .map_err(|e| lease_error("selecting a lease row", &e))?
            .into_rows_result()
            .map_err(|e| lease_error("reading a lease row", &e))?;
        let row = result
            .maybe_first_row::<LeaseRow>()
            .map_err(|e| lease_error("reading a lease row", &e))?;
        row.map(decode_lease).transpose()
    }

    /// Every run recorded for this table, newest first (`TRK-034`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if the query or the deserialization fails.
    pub async fn all_runs(&self) -> Result<Vec<RunRecord>, CdmError> {
        let statements = self.statements().await?;
        let result = self
            .session
            .execute_unpaged(&statements.select_runs, (self.tables.table_name(),))
            .await
            .map_err(|e| tracking_error("selecting the run list", &e))?
            .into_rows_result()
            .map_err(|e| tracking_error("reading the run list", &e))?;
        let mut runs = Vec::new();
        for row in result
            .rows::<InfoRow>()
            .map_err(|e| tracking_error("reading the run list", &e))?
        {
            runs.push(self.decode_run(row.map_err(|e| tracking_error("reading a run row", &e))?));
        }
        Ok(runs)
    }
}

/// The `bigint` a token bound is stored as, refusing to truncate (`TRK-010`).
///
/// # Errors
///
/// Returns [`ErrorKind::Tracking`] when the token does not fit in 64 bits, which happens only
/// under the RandomPartitioner. Java truncates here; truncation produces a resume that re-plans
/// the wrong part of the ring, so cdm-rs would rather fail at initialisation.
pub fn token_bound(token: i128, range: TokenRange) -> Result<i64, CdmError> {
    i64::try_from(token).map_err(|_| {
        CdmError::new(
            ErrorKind::Tracking,
            format!(
                "token {token} of range {range} does not fit in the bigint column \
                 cdm_run_details uses. This is a RandomPartitioner ring; Java CDM truncates the \
                 value and resumes the wrong ranges. Disable run tracking, or migrate with a \
                 Murmur3 origin."
            ),
        )
        .with_context(|ctx| ctx.with_config_key("track_run.enabled"))
    })
}

/// `chrono` from the driver's timestamp, dropping a value no calendar can represent.
fn from_cql_timestamp(value: CqlTimestamp) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(value.0)
}

/// The driver's timestamp from `chrono`, at the millisecond resolution CQL stores.
///
/// Truncation towards the past is deliberate: a `lease_until` rounded *down* expires no later
/// than it was meant to, which costs at most a millisecond of lease and never grants one.
fn to_cql_timestamp(value: DateTime<Utc>) -> CqlTimestamp {
    CqlTimestamp(value.timestamp_millis())
}

fn tracking_error(doing: &str, error: &impl std::fmt::Display) -> CdmError {
    CdmError::new(ErrorKind::Tracking, format!("{doing}: {error}"))
}

fn lease_error(doing: &str, error: &impl std::fmt::Display) -> CdmError {
    CdmError::new(ErrorKind::Lease, format!("{doing}: {error}"))
}

/// Re-labels a preparation failure as a lease failure, so that a distributed run's first symptom
/// names the subsystem that is missing rather than run tracking, which is working.
fn lease_error_from(error: &CdmError) -> CdmError {
    CdmError::new(ErrorKind::Lease, error.to_string())
}

/// Turns a selected `cdm_run_leases` row into a [`LeaseRecord`].
///
/// A null `node_id` or `lease_until` describes a row no cdm-rs wrote in one piece. It is reported
/// rather than defaulted: defaulting `lease_until` would either hand a live range away or make a
/// free one permanently unclaimable, and both are worse than an operator seeing the row that
/// cdm-rs cannot explain.
fn decode_lease(row: LeaseRow) -> Result<LeaseRecord, CdmError> {
    let (token_min, node_id, lease_until, attempt) = row;
    let (Some(node_id), Some(lease_until)) = (node_id, lease_until.and_then(from_cql_timestamp))
    else {
        return Err(CdmError::new(
            ErrorKind::Lease,
            format!(
                "the cdm_run_leases row for token_min {token_min} has no node_id or no \
                 lease_until; it was not written by cdm-rs and cannot be reasoned about"
            ),
        ));
    };
    Ok(LeaseRecord {
        token_min,
        node_id,
        lease_until,
        // `attempt` is only ever written by cdm-rs, as a positive count. A negative or absent one
        // is read as "no attempts recorded", so `DST-013` bounds the range by the attempts it can
        // actually account for rather than abandoning it over a value it cannot explain.
        attempt: attempt
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
    })
}

/// The `[applied]` column of a conditional write.
///
/// Read positionally out of a dynamic row, because the arity varies: a transaction that applied
/// returns `[applied]` alone, and one that did not returns it followed by columns of the row that
/// blocked it. Only the first column is guaranteed to be there, and only it is needed — what
/// blocked the write is read back separately where it matters, since a failed `UPDATE ... IF`
/// returns only the columns its condition names, which is never the whole row.
fn lwt_applied(result: scylla::response::query_result::QueryResult) -> Result<bool, CdmError> {
    let rows = result
        .into_rows_result()
        .map_err(|e| lease_error("reading the result of a lightweight transaction", &e))?;
    let row = rows
        .first_row::<Row>()
        .map_err(|e| lease_error("reading the result of a lightweight transaction", &e))?;
    match row.columns.first() {
        Some(Some(CqlValue::Boolean(applied))) => Ok(*applied),
        _ => Err(CdmError::new(
            ErrorKind::Lease,
            "a lightweight transaction returned no [applied] column; the target cluster is not \
             answering conditional writes the way CQL specifies",
        )),
    }
}

impl Plugin for CassandraStore {
    fn name(&self) -> &'static str {
        "cassandra"
    }

    fn provider(&self) -> &'static str {
        "cdm-track"
    }
}

#[async_trait]
impl TrackingStore for CassandraStore {
    /// Creates the tables if absent (`TRK-010`, `TRK-011`).
    ///
    /// `IF NOT EXISTS` throughout, so a Java-created table is adopted rather than fought over,
    /// and so several cdm-rs nodes starting together do not race (`DST-002`).
    async fn initialise(&self) -> Result<(), CdmError> {
        for statement in self.tables.create_statements() {
            self.session
                .query_unpaged(statement.clone(), &[])
                .await
                .map_err(|e| {
                    tracking_error(&format!("creating tracking table (`{statement}`)"), &e)
                })?;
        }
        if self.create_leases {
            let statement = self.tables.create_leases_statement();
            self.session
                .query_unpaged(statement.clone(), &[])
                .await
                .map_err(|e| tracking_error(&format!("creating `{statement}`"), &e))?;
        }
        // The DDL is visible on the coordinator before it is visible anywhere else. A run that
        // starts inserting range rows against a node that has not seen the table yet fails in a
        // way that looks like a transient write error, so the wait is not optional.
        self.session
            .await_schema_agreement()
            .await
            .map_err(|e| tracking_error("waiting for the tracking schema to agree", &e))?;
        Ok(())
    }

    /// Java's `initCdmRun`, in Java's order (`TRK-020`).
    ///
    /// Reject an existing run id; write the run row as `NOT_STARTED`; write one range row per
    /// planned range as `NOT_STARTED`; only then move the run row to `STARTED`. The order is the
    /// contract `DST-002` and `TRK-032` both read: a run row at `STARTED` means every range row
    /// exists, so a resume that sees `STARTED` can trust the range list it finds.
    async fn create_run(&self, run: &RunRecord, ranges: &[RangeRecord]) -> Result<(), CdmError> {
        // Fail before writing anything if any bound cannot be stored, rather than half-way
        // through inserting range rows.
        let bounds: Vec<(i64, i64)> = ranges
            .iter()
            .map(|record| {
                Ok((
                    token_bound(record.range.min(), record.range)?,
                    token_bound(record.range.max(), record.range)?,
                ))
            })
            .collect::<Result<_, CdmError>>()?;

        let statements = self.statements().await?;
        let table_name = self.tables.table_name();
        let run_id = run.run_id.as_i64();

        if self.run(run.run_id).await?.is_some() {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!(
                    "run id {} already exists for table {table_name}; \
                     pick another track_run.run_id or omit it to have one allocated",
                    run.run_id
                ),
            )
            .with_context(|ctx| ctx.with_config_key("track_run.run_id")));
        }

        let previous = run.previous_run_id.as_ref().map_or(0, RunId::as_i64);
        let job = run_type(run.job);
        self.session
            .execute_unpaged(
                &statements.insert_run_info,
                (
                    table_name,
                    run_id,
                    job,
                    previous,
                    status_string(RunStatus::NotStarted),
                ),
            )
            .await
            .map_err(|e| tracking_error("inserting the run row", &e))?;

        for (min, max) in bounds {
            self.session
                .execute_unpaged(
                    &statements.insert_run_detail,
                    (
                        table_name,
                        run_id,
                        min,
                        max,
                        status_string(RunStatus::NotStarted),
                    ),
                )
                .await
                .map_err(|e| tracking_error("inserting a range row", &e))?;
        }

        self.session
            .execute_unpaged(
                &statements.insert_run_info,
                (
                    table_name,
                    run_id,
                    job,
                    previous,
                    status_string(RunStatus::Started),
                ),
            )
            .await
            .map_err(|e| tracking_error("marking the run started", &e))?;
        Ok(())
    }

    /// Java's `endCdmRun` (`TRK-022`), generalised to any run status so that `INTERRUPTED` and
    /// `ABORTED` can be recorded too (`TRK-012`).
    ///
    /// `info` is bound `Unset` when absent rather than `None` (`TRK-037`). The statement is
    /// `SET ... run_info = ?`, so binding `None` would write a `NULL` — a tombstone that erases
    /// whatever metrics string the run had already recorded. [`RunManager::cancel`] passes `None`
    /// precisely because it has no new metrics to offer, which is the opposite of asking for the
    /// old ones to be deleted. `Unset` says "leave this column alone", which is what a status-only
    /// write means, and it writes no tombstone — the same reasoning as `MIG-012` on the data path.
    ///
    /// [`RunManager::cancel`]: crate::manage::RunManager::cancel
    async fn update_run(
        &self,
        run_id: RunId,
        status: RunStatus,
        info: Option<&str>,
    ) -> Result<(), CdmError> {
        let statements = self.statements().await?;
        self.session
            .execute_unpaged(
                &statements.end_run_info,
                (
                    info.map_or(MaybeUnset::Unset, MaybeUnset::Set),
                    status_string(status),
                    self.tables.table_name(),
                    run_id.as_i64(),
                ),
            )
            .await
            .map_err(|e| tracking_error("updating the run row", &e))?;
        Ok(())
    }

    /// Java's `updateCdmRun` (`TRK-021`).
    ///
    /// `STARTED` takes the statement that sets `start_time` and leaves `run_info` alone; every
    /// other status takes the one that writes the metrics string. Java branches the same way, and
    /// the reason is not cosmetic: a start write that also wrote `run_info` would erase the
    /// metrics of the attempt that just failed, which is the only record of what it managed.
    async fn update_range(&self, run_id: RunId, range: &RangeRecord) -> Result<(), CdmError> {
        let statements = self.statements().await?;
        let min = token_bound(range.range.min(), range.range)?;
        let table_name = self.tables.table_name();
        if range.status == RunStatus::Started {
            self.session
                .execute_unpaged(
                    &statements.start_run_detail,
                    (
                        status_string(range.status),
                        table_name,
                        run_id.as_i64(),
                        min,
                    ),
                )
                .await
                .map_err(|e| tracking_error("marking a range started", &e))?;
        } else {
            self.session
                .execute_unpaged(
                    &statements.update_run_detail,
                    (
                        status_string(range.status),
                        range.info.as_deref(),
                        table_name,
                        run_id.as_i64(),
                        min,
                    ),
                )
                .await
                .map_err(|e| tracking_error("recording a range outcome", &e))?;
        }
        Ok(())
    }

    async fn run(&self, run_id: RunId) -> Result<Option<RunRecord>, CdmError> {
        let statements = self.statements().await?;
        let result = self
            .session
            .execute_unpaged(
                &statements.select_run_info,
                (self.tables.table_name(), run_id.as_i64()),
            )
            .await
            .map_err(|e| tracking_error("selecting the run row", &e))?
            .into_rows_result()
            .map_err(|e| tracking_error("reading the run row", &e))?;
        let row = result
            .maybe_first_row::<InfoRow>()
            .map_err(|e| tracking_error("reading the run row", &e))?;
        Ok(row.map(|row| self.decode_run(row)))
    }

    /// Every range row of a run, in one read of one partition (`TRK-031`).
    ///
    /// Java issues four status-filtered queries and concatenates them, which drops any row whose
    /// status is not one of the four it asked for. Reading the partition whole is one round trip
    /// instead of four *and* keeps unrecognised statuses visible, so [`decode_status`] can turn
    /// them into pending work rather than into silence.
    async fn ranges(&self, run_id: RunId) -> Result<Vec<RangeRecord>, CdmError> {
        let statements = self.statements().await?;
        let result = self
            .session
            .execute_unpaged(
                &statements.select_run_details,
                (self.tables.table_name(), run_id.as_i64()),
            )
            .await
            .map_err(|e| tracking_error("selecting the range rows", &e))?
            .into_rows_result()
            .map_err(|e| tracking_error("reading the range rows", &e))?;

        let mut records = Vec::with_capacity(result.rows_num());
        for row in result
            .rows::<DetailRow>()
            .map_err(|e| tracking_error("reading the range rows", &e))?
        {
            let (min, max, start_time, status, info) =
                row.map_err(|e| tracking_error("reading a range row", &e))?;
            // A row whose bounds are inverted cannot be turned into a `TokenRange`. Dropping it
            // would remove the range from the resume, so it is an error the operator sees.
            let range = TokenRange::new(i128::from(min), i128::from(max)).map_err(|e| {
                CdmError::new(
                    ErrorKind::Tracking,
                    format!("cdm_run_details holds an unusable range [{min}, {max}]: {e}"),
                )
            })?;
            records.push(RangeRecord {
                range,
                status: decode_status(status.as_deref()),
                started_at: start_time.and_then(from_cql_timestamp),
                info,
            });
        }
        Ok(records)
    }

    /// The newest run for `(table_name, run_type)` (`TRK-030`).
    ///
    /// `table` is accepted for trait conformance and checked against the table this store was
    /// built for: a store pointed at one table cannot answer for another, and silently answering
    /// for the wrong one would resume the wrong data.
    async fn latest_run(
        &self,
        table: &TableRef,
        job: JobKind,
    ) -> Result<Option<RunRecord>, CdmError> {
        if table.table() != self.tables.table_name() {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!(
                    "this tracking store records {}, not {table}",
                    self.tables.table_name()
                ),
            ));
        }
        let statements = self.statements().await?;
        let result = self
            .session
            .execute_unpaged(
                &statements.select_latest_run_info,
                (self.tables.table_name(), run_type(job)),
            )
            .await
            .map_err(|e| tracking_error("selecting the most recent run", &e))?
            .into_rows_result()
            .map_err(|e| tracking_error("reading the most recent run", &e))?;
        let row = result
            .maybe_first_row::<InfoRow>()
            .map_err(|e| tracking_error("reading the most recent run", &e))?;
        Ok(row.map(|row| self.decode_run(row)))
    }
}

/// The conditional half of distributed mode (`DST-002`, `DST-003`, `DST-010`..`DST-013`).
///
/// Every method here is one lightweight transaction, or one transaction and the read that
/// describes why it did not apply. The policy that decides *whether* to claim — how long a lease
/// runs for, how often it is renewed, how many attempts a range gets — is not here: it belongs to
/// `cdm-cluster`'s `Coordinator`, and keeping it there is what lets the same policy be tested
/// against the in-memory store without a cluster.
#[async_trait]
impl LeaseStore for CassandraStore {
    async fn initialise_leases(&self) -> Result<(), CdmError> {
        let statement = self.tables.create_leases_statement();
        self.session
            .query_unpaged(statement.clone(), &[])
            .await
            .map_err(|e| lease_error(&format!("creating `{statement}`"), &e))?;
        self.session
            .await_schema_agreement()
            .await
            .map_err(|e| lease_error("waiting for the lease schema to agree", &e))?;
        Ok(())
    }

    /// `TRK-020`'s initialisation, entered by exactly one node (`DST-002`, `DST-003`).
    ///
    /// The `INSERT ... IF NOT EXISTS` is the election: Paxos orders the contending inserts and
    /// exactly one applies, so the winner is decided by the same mechanism that decides a lease,
    /// with no separate leader protocol to get wrong. The winner then walks `TRK-020` in
    /// `TRK-020`'s order — range rows, and only then `STARTED` — because every other node is
    /// waiting on `STARTED` as its signal that the range rows exist.
    ///
    /// The loser is handed the row it lost to. Its `run_info` carries the initialiser's
    /// configuration hash until `TRK-022` overwrites it with the run's metrics string, which is
    /// why `DST-003`'s check belongs at *join* time and cannot be repeated later in the run.
    async fn initialise_run(
        &self,
        run: &RunRecord,
        ranges: &[RangeRecord],
        config_hash: &str,
    ) -> Result<RunClaim, CdmError> {
        // Refuse before writing anything if a bound cannot be stored, rather than half-way
        // through the range rows — the same order `create_run` uses, for the same reason.
        let bounds: Vec<(i64, i64)> = ranges
            .iter()
            .map(|record| {
                Ok((
                    token_bound(record.range.min(), record.range)?,
                    token_bound(record.range.max(), record.range)?,
                ))
            })
            .collect::<Result<_, CdmError>>()?;

        let statements = self.lease_statements().await?;
        let table_name = self.tables.table_name();
        let run_id = run.run_id.as_i64();
        let previous = run.previous_run_id.as_ref().map_or(0, RunId::as_i64);
        let job = run_type(run.job);

        let result = self
            .session
            .execute_unpaged(
                &statements.elect,
                (
                    table_name,
                    run_id,
                    job,
                    previous,
                    status_string(RunStatus::NotStarted),
                    config_hash,
                ),
            )
            .await
            .map_err(|e| lease_error("electing the initialising node", &e))?;

        if !lwt_applied(result)? {
            let existing = self.run(run.run_id).await?.ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Lease,
                    format!(
                        "the run row for {run_id} blocked this node's initialisation and then \
                         could not be read back; the target keyspace is not answering \
                         consistently"
                    ),
                )
            })?;
            return Ok(RunClaim::Lost(existing));
        }

        let tracking = self.statements().await?;
        for (min, max) in bounds {
            self.session
                .execute_unpaged(
                    &tracking.insert_run_detail,
                    (
                        table_name,
                        run_id,
                        min,
                        max,
                        status_string(RunStatus::NotStarted),
                    ),
                )
                .await
                .map_err(|e| tracking_error("inserting a range row", &e))?;
        }

        // Unconditional, and naming no `run_info`: an INSERT writes only the columns it lists, so
        // the configuration hash `DST-003` just recorded survives the move to STARTED.
        self.session
            .execute_unpaged(
                &tracking.insert_run_info,
                (
                    table_name,
                    run_id,
                    job,
                    previous,
                    status_string(RunStatus::Started),
                ),
            )
            .await
            .map_err(|e| tracking_error("marking the run started", &e))?;
        Ok(RunClaim::Won)
    }

    async fn lease(
        &self,
        run_id: RunId,
        range: TokenRange,
    ) -> Result<Option<LeaseRecord>, CdmError> {
        self.lease_row(run_id, token_bound(range.min(), range)?)
            .await
    }

    /// One claim: `INSERT ... IF NOT EXISTS`, or `UPDATE ... IF lease_until < now` (`DST-011`).
    ///
    /// Two nodes racing to reclaim the same expired lease are ordered by Paxos: the first sets
    /// `lease_until` into the future, and the second's condition is then false. `observed` picks
    /// the statement and the attempt number and settles nothing, as the trait says.
    async fn claim_range(
        &self,
        run_id: RunId,
        range: TokenRange,
        node_id: &str,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        observed: Option<&LeaseRecord>,
    ) -> Result<LeaseOutcome, CdmError> {
        let token_min = token_bound(range.min(), range)?;
        let statements = self.lease_statements().await?;
        let table_name = self.tables.table_name();

        let Some(current) = observed else {
            let result = self
                .session
                .execute_unpaged(
                    &statements.claim_if_absent,
                    (
                        table_name,
                        run_id.as_i64(),
                        token_min,
                        node_id,
                        to_cql_timestamp(lease_until),
                    ),
                )
                .await
                .map_err(|e| lease_error("claiming an unclaimed range", &e))?;
            if lwt_applied(result)? {
                return Ok(LeaseOutcome::Granted(LeaseRecord {
                    token_min,
                    node_id: node_id.to_owned(),
                    lease_until,
                    attempt: 1,
                }));
            }
            // Another node inserted between the read and the transaction. Report whoever won.
            return Ok(LeaseOutcome::Denied(
                self.lease_row(run_id, token_min).await?.ok_or_else(|| {
                    CdmError::new(
                        ErrorKind::Lease,
                        format!(
                            "the lease on token_min {token_min} blocked this node's claim and \
                             then could not be read back; the target keyspace is not answering \
                             consistently"
                        ),
                    )
                })?,
            ));
        };

        if current.lease_until > now {
            return Ok(LeaseOutcome::Denied(current.clone()));
        }

        let attempt = current.attempt.saturating_add(1);
        let result = self
            .session
            .execute_unpaged(
                &statements.reclaim_if_expired,
                (
                    node_id,
                    to_cql_timestamp(lease_until),
                    i32::try_from(attempt).unwrap_or(i32::MAX),
                    table_name,
                    run_id.as_i64(),
                    token_min,
                    to_cql_timestamp(now),
                ),
            )
            .await
            .map_err(|e| lease_error("reclaiming an expired range", &e))?;
        if lwt_applied(result)? {
            Ok(LeaseOutcome::Granted(LeaseRecord {
                token_min,
                node_id: node_id.to_owned(),
                lease_until,
                attempt,
            }))
        } else {
            Ok(LeaseOutcome::Denied(
                self.lease_row(run_id, token_min)
                    .await?
                    .unwrap_or_else(|| current.clone()),
            ))
        }
    }

    async fn renew_lease(
        &self,
        run_id: RunId,
        range: TokenRange,
        node_id: &str,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, CdmError> {
        let token_min = token_bound(range.min(), range)?;
        let statements = self.lease_statements().await?;
        let result = self
            .session
            .execute_unpaged(
                &statements.renew,
                (
                    to_cql_timestamp(lease_until),
                    self.tables.table_name(),
                    run_id.as_i64(),
                    token_min,
                    node_id,
                ),
            )
            .await
            .map_err(|e| lease_error("renewing a lease", &e))?;
        lwt_applied(result)
    }

    /// Releasing is renewing to the epoch: the row keeps its `attempt`, and the range is
    /// claimable again immediately.
    ///
    /// The epoch rather than "now" on purpose. A released lease must read as expired to *every*
    /// node, and a `lease_until` taken from this node's clock is expired only relative to a clock
    /// that agrees with it — the one assumption `DST-012` cannot make. There is no timestamp a
    /// live lease could ever hold that is below the epoch, so the release cannot be mistaken for
    /// one, however far the clocks have drifted.
    async fn release_lease(
        &self,
        run_id: RunId,
        range: TokenRange,
        node_id: &str,
    ) -> Result<bool, CdmError> {
        self.renew_lease(run_id, range, node_id, DateTime::UNIX_EPOCH)
            .await
    }

    async fn leases(&self, run_id: RunId) -> Result<Vec<LeaseRecord>, CdmError> {
        let statements = self.lease_statements().await?;
        let result = self
            .session
            .execute_unpaged(
                &statements.select_leases,
                (self.tables.table_name(), run_id.as_i64()),
            )
            .await
            .map_err(|e| lease_error("selecting the lease rows", &e))?
            .into_rows_result()
            .map_err(|e| lease_error("reading the lease rows", &e))?;
        let mut records = Vec::with_capacity(result.rows_num());
        for row in result
            .rows::<LeaseRow>()
            .map_err(|e| lease_error("reading the lease rows", &e))?
        {
            records.push(decode_lease(
                row.map_err(|e| lease_error("reading a lease row", &e))?,
            )?);
        }
        Ok(records)
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

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    #[test]
    fn trk_010_a_murmur3_token_fits_the_bigint_column() {
        let full = range(i128::from(i64::MIN) + 1, i128::from(i64::MAX));
        assert_eq!(token_bound(full.min(), full).unwrap(), i64::MIN + 1);
        assert_eq!(token_bound(full.max(), full).unwrap(), i64::MAX);
    }

    #[test]
    fn trk_010_a_random_partitioner_token_is_refused_rather_than_truncated() {
        let wide = range(0, i128::from(i64::MAX) + 1);
        let err = token_bound(wide.max(), wide).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
        assert!(err.to_string().contains("RandomPartitioner"));
        assert_eq!(
            err.context().config_key.as_deref(),
            Some("track_run.enabled")
        );
    }

    #[test]
    fn trk_010_a_timestamp_outside_the_calendar_is_dropped_rather_than_panicking() {
        assert!(from_cql_timestamp(CqlTimestamp(0)).is_some());
        assert!(from_cql_timestamp(CqlTimestamp(i64::MAX)).is_none());
    }
}
