//! A local SQLite file as the tracking backend (`TRK-036`).
//!
//! `TRK-036` names three backends, and this is the one for the case the other two cannot serve: a
//! target the operator may not create tables in. A read-only application keyspace, a target owned
//! by another team, a managed service where DDL needs a change request — in all of them the
//! Cassandra backend of `TRK-010` is unavailable, and [`MemoryStore`](super::MemoryStore) is no
//! substitute because its whole record dies with the process, which is precisely when a resume is
//! needed. A file on the machine running cdm-rs survives the interruption and asks nothing of the
//! target.
//!
//! # What it gives up
//!
//! Java compatibility and distribution, deliberately. A Java CDM run cannot read this file, and
//! neither can a second cdm-rs node: SQLite is one machine's file, so `cluster.enabled`
//! (`DST-001`) still requires the Cassandra backend. What it keeps is every semantic the tracker
//! and the resume depend on — `TRK-020`'s refusal to reuse a run id, `TRK-021`'s two-phase range
//! write, `TRK-022`'s terminal statuses, and `TRK-030`/`TRK-031`'s view of what is still pending.
//!
//! # Schema
//!
//! The two table names are Java's, so that an operator who has read `TRK-010` recognises what is
//! in the file. Two things differ, and both are forced by the substrate:
//!
//! * a `keyspace_name` column joins the primary key. Cassandra scopes the tracking tables by the
//!   keyspace they live in; one SQLite file has no keyspaces, and without the column two tables
//!   of the same name in different keyspaces would share a partition and corrupt each other's
//!   resume;
//! * timestamps are Unix milliseconds in an `INTEGER`, which is the same resolution CQL's
//!   `timestamp` has, so nothing is lost round-tripping a run through the file.
//!
//! Token bounds go through [`token_bound`] exactly as they do for Cassandra: SQLite's `INTEGER`
//! is also 64-bit, so a RandomPartitioner token would truncate here too, and truncation is what
//! turns a resume into a plan for the wrong part of the ring.
//!
//! # Blocking I/O
//!
//! SQLite is a synchronous, file-backed library, and `ARCHITECTURE.md` §12 does not allow that on
//! the runtime's worker threads: a tracking write that blocks a worker blocks data movement with
//! it. Every statement in this module therefore runs inside [`tokio::task::spawn_blocking`], and
//! the connection lives behind a mutex because `rusqlite`'s `Connection` is `Send` but not
//! `Sync`. The tracker already funnels writes through the bounded queue of `TRK-035`, so the
//! mutex is not on the hot path of a migration.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{
    CdmError, ErrorKind, JobKind, Plugin, RangeRecord, RunId, RunRecord, RunStatus, TableRef,
    TokenRange, TrackingStore,
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row};

use crate::compat::{job_from_run_type, run_type, status as status_string};
use crate::schema::TrackingTables;
use crate::store::cassandra::token_bound;
use crate::store::decode_status;

/// The run table, in SQLite's dialect (`TRK-036`).
const CREATE_RUN_INFO: &str = "CREATE TABLE IF NOT EXISTS cdm_run_info (\
     keyspace_name TEXT NOT NULL, table_name TEXT NOT NULL, run_id INTEGER NOT NULL, \
     run_type TEXT, prev_run_id INTEGER, start_time INTEGER, end_time INTEGER, \
     run_info TEXT, status TEXT, \
     PRIMARY KEY (keyspace_name, table_name, run_id))";

/// The range table, in SQLite's dialect (`TRK-036`).
const CREATE_RUN_DETAILS: &str = "CREATE TABLE IF NOT EXISTS cdm_run_details (\
     keyspace_name TEXT NOT NULL, table_name TEXT NOT NULL, run_id INTEGER NOT NULL, \
     token_min INTEGER NOT NULL, token_max INTEGER NOT NULL, start_time INTEGER, \
     status TEXT, run_info TEXT, \
     PRIMARY KEY (keyspace_name, table_name, run_id, token_min))";

/// The columns a run row is read back through, in the order [`decode_run`] expects.
const RUN_COLUMNS: &str = "run_id, run_type, prev_run_id, start_time, end_time, run_info, status";

/// `INSERT` of the run row (`TRK-020`). Plain, not an upsert: the primary key is what makes a
/// second process reusing a run id fail rather than silently reset the first one's range rows.
const INSERT_RUN_INFO: &str = "INSERT INTO cdm_run_info \
     (keyspace_name, table_name, run_id, run_type, prev_run_id, start_time, status) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// The `NOT_STARTED` → `STARTED` transition that closes run creation (`TRK-020`).
const START_RUN_INFO: &str = "UPDATE cdm_run_info SET status = ?4 \
     WHERE keyspace_name = ?1 AND table_name = ?2 AND run_id = ?3";

/// `INSERT` of one planned range (`TRK-020`).
const INSERT_RUN_DETAIL: &str = "INSERT INTO cdm_run_details \
     (keyspace_name, table_name, run_id, token_min, token_max, status) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

/// The write that closes a run (`TRK-022`).
///
/// `COALESCE(?5, run_info)` is the same refusal to erase metrics that [`UPDATE_RUN_DETAIL`]
/// makes: `cdm runs cancel` (`TRK-034`) updates the status with no metrics string of its own, and
/// a plain assignment would replace the aggregate the run had reported with nothing.
const END_RUN_INFO: &str = "UPDATE cdm_run_info \
     SET end_time = ?4, run_info = COALESCE(?5, run_info), status = ?6 \
     WHERE keyspace_name = ?1 AND table_name = ?2 AND run_id = ?3";

/// The `STARTED` write for a range (`TRK-021`): sets `start_time`, never touches `run_info`.
const START_RUN_DETAIL: &str = "INSERT INTO cdm_run_details \
     (keyspace_name, table_name, run_id, token_min, token_max, start_time, status) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
     ON CONFLICT (keyspace_name, table_name, run_id, token_min) DO UPDATE SET \
     token_max = excluded.token_max, start_time = excluded.start_time, status = excluded.status";

/// The terminal write for a range (`TRK-021`): status and metrics, leaving `start_time` as the
/// claim that produced them.
const UPDATE_RUN_DETAIL: &str = "INSERT INTO cdm_run_details \
     (keyspace_name, table_name, run_id, token_min, token_max, start_time, status, run_info) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
     ON CONFLICT (keyspace_name, table_name, run_id, token_min) DO UPDATE SET \
     token_max = excluded.token_max, \
     start_time = COALESCE(cdm_run_details.start_time, excluded.start_time), \
     status = excluded.status, \
     run_info = COALESCE(excluded.run_info, cdm_run_details.run_info)";

/// Tracking in a local SQLite file (`TRK-036`).
///
/// Construct it with [`SqliteStore::open`] and hand it to the tracker in place of
/// [`CassandraStore`](super::CassandraStore); nothing else in the crate knows the difference.
#[derive(Debug)]
pub struct SqliteStore {
    /// `rusqlite`'s `Connection` is `Send` but not `Sync`, and every use of it happens on a
    /// blocking thread, so one mutex-guarded connection is both what the type system wants and
    /// what SQLite's single-writer model would give us anyway.
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
    tables: TrackingTables,
}

impl SqliteStore {
    /// Opens — creating if absent — the tracking database at `path`, for the target `table`.
    ///
    /// The file is opened but not populated: [`TrackingStore::initialise`] creates the tables, so
    /// that this backend has the same two-step lifecycle the Cassandra one does and a caller can
    /// swap them without reordering anything.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if the table reference cannot name a tracking table (see
    /// [`TrackingTables::new`]), or if SQLite cannot open the file — a missing directory, or a
    /// path the process cannot write.
    pub fn open(path: impl AsRef<Path>, table: &TableRef) -> Result<Self, CdmError> {
        let tables = TrackingTables::new(table)?;
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path).map_err(|e| {
            // The path is configuration, not a credential, and an operator who cannot see which
            // file failed to open cannot fix it (`SEC-001` covers secrets, not file names).
            CdmError::new(
                ErrorKind::Tracking,
                format!(
                    "cannot open the SQLite tracking database at `{}`: {e}",
                    path.display()
                ),
            )
            .with_context(|ctx| ctx.with_config_key("track_run.enabled"))
        })?;
        // A run that is interrupted is the case this backend exists for, so durability is not
        // negotiable: `synchronous = FULL` keeps SQLite's default fsync behaviour, and the
        // write-ahead log lets the reader of a resume run alongside the writer. `busy_timeout`
        // covers the second cdm-rs process an operator starts by mistake — it waits and then
        // fails, instead of failing instantly with a lock error.
        for pragma in [
            "PRAGMA journal_mode = WAL",
            "PRAGMA synchronous = FULL",
            "PRAGMA busy_timeout = 5000",
        ] {
            // `journal_mode` answers with a row, so this cannot be `execute`.
            connection
                .query_row(pragma, [], |_| Ok(()))
                .optional()
                .map_err(|e| tracking_error(&format!("applying `{pragma}`"), &e))?;
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
            tables,
        })
    }

    /// The file this store records into.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The keyspace and table whose runs this store records.
    pub fn tables(&self) -> &TrackingTables {
        &self.tables
    }

    /// The identity every statement is scoped by: `(keyspace_name, table_name)`.
    fn scope(&self) -> (String, String) {
        (
            self.tables.keyspace().to_owned(),
            self.tables.table_name().to_owned(),
        )
    }

    /// Runs `f` against the connection on a blocking thread (`ARCHITECTURE.md` §12).
    ///
    /// Everything the closure needs must be owned, because the work outlives the borrow of
    /// `self`. That is why the callers below clone the scope strings rather than passing `&str`.
    async fn with_connection<T, F>(&self, f: F) -> Result<T, CdmError>
    where
        F: FnOnce(&mut Connection) -> Result<T, CdmError> + Send + 'static,
        T: Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        match tokio::task::spawn_blocking(move || {
            let mut guard = connection.lock();
            f(&mut guard)
        })
        .await
        {
            Ok(result) => result,
            Err(e) => Err(tracking_error("running a tracking statement", &e)),
        }
    }

    /// Every run recorded for this table, newest first (`TRK-034`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if the query or the decoding fails.
    pub async fn all_runs(&self) -> Result<Vec<RunRecord>, CdmError> {
        let (keyspace, table_name) = self.scope();
        let sql =
            format!("SELECT {RUN_COLUMNS} FROM cdm_run_info WHERE keyspace_name = ?1 AND table_name = ?2 ORDER BY run_id DESC");
        self.with_connection(move |conn| {
            let mut statement = conn
                .prepare(&sql)
                .map_err(|e| tracking_error("preparing the run list", &e))?;
            let rows = statement
                .query_map((&keyspace, &table_name), |row| {
                    decode_run(row, &keyspace, &table_name)
                })
                .map_err(|e| tracking_error("selecting the run list", &e))?;
            let mut runs = Vec::new();
            for row in rows {
                runs.push(row.map_err(|e| tracking_error("reading a run row", &e))?);
            }
            Ok(runs)
        })
        .await
    }
}

/// Reads one `cdm_run_info` row into a [`RunRecord`].
///
/// A `run_type` this build does not recognise falls back to `MIGRATE`, as the Cassandra backend's
/// decoder does: the lookup in [`TrackingStore::latest_run`] filters by `run_type` in SQL, so this
/// only fires for a direct lookup by id, where the caller already knows what it asked for.
fn decode_run(row: &Row<'_>, keyspace: &str, table_name: &str) -> rusqlite::Result<RunRecord> {
    let run_id: i64 = row.get(0)?;
    let run_type_value: Option<String> = row.get(1)?;
    let prev_run_id: Option<i64> = row.get(2)?;
    let start_time: Option<i64> = row.get(3)?;
    let end_time: Option<i64> = row.get(4)?;
    let info: Option<String> = row.get(5)?;
    let status: Option<String> = row.get(6)?;
    Ok(RunRecord {
        run_id: RunId::from_raw(run_id),
        // Java writes `prev_run_id = 0` for "none" and cdm-rs keeps doing so; neither is a run.
        previous_run_id: prev_run_id.filter(|id| *id != 0).map(RunId::from_raw),
        table: TableRef::new(keyspace, table_name),
        job: run_type_value
            .as_deref()
            .and_then(job_from_run_type)
            .unwrap_or(JobKind::Migrate),
        status: decode_status(status.as_deref()),
        started_at: from_millis(start_time),
        ended_at: from_millis(end_time),
        info,
    })
}

/// A `DateTime` from stored milliseconds, dropping a value no calendar can represent.
fn from_millis(value: Option<i64>) -> Option<DateTime<Utc>> {
    value.and_then(DateTime::from_timestamp_millis)
}

/// The stored form of a timestamp: Unix milliseconds, CQL's own resolution.
fn to_millis(value: DateTime<Utc>) -> i64 {
    value.timestamp_millis()
}

fn tracking_error(doing: &str, error: &impl std::fmt::Display) -> CdmError {
    CdmError::new(ErrorKind::Tracking, format!("{doing}: {error}"))
}

impl Plugin for SqliteStore {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn provider(&self) -> &'static str {
        "cdm-track"
    }
}

#[async_trait]
impl TrackingStore for SqliteStore {
    /// Creates the two tables if absent (`TRK-036`).
    ///
    /// `IF NOT EXISTS`, like the Cassandra backend, so that reopening a file left by an earlier
    /// run adopts its rows instead of failing.
    async fn initialise(&self) -> Result<(), CdmError> {
        self.with_connection(move |conn| {
            for statement in [CREATE_RUN_INFO, CREATE_RUN_DETAILS] {
                conn.execute(statement, []).map_err(|e| {
                    tracking_error(&format!("creating tracking table (`{statement}`)"), &e)
                })?;
            }
            Ok(())
        })
        .await
    }

    /// Java's `initCdmRun`, in Java's order (`TRK-020`).
    ///
    /// Reject an existing run id; write the run row as `NOT_STARTED`; write one range row per
    /// planned range; only then move the run row to `STARTED`. Unlike Cassandra, SQLite can make
    /// that sequence atomic, and it does: a resume reading a file whose writer was killed
    /// mid-creation sees either no run at all or a complete one, never a `STARTED` run missing
    /// half its ranges — which `TRK-032` would otherwise take at face value.
    async fn create_run(&self, run: &RunRecord, ranges: &[RangeRecord]) -> Result<(), CdmError> {
        // Fail before writing anything if a bound cannot be stored, rather than half-way through.
        let bounds: Vec<(i64, i64)> = ranges
            .iter()
            .map(|record| {
                Ok((
                    token_bound(record.range.min(), record.range)?,
                    token_bound(record.range.max(), record.range)?,
                ))
            })
            .collect::<Result<_, CdmError>>()?;

        let (keyspace, table_name) = self.scope();
        let run_id = run.run_id.as_i64();
        let previous = run.previous_run_id.as_ref().map_or(0, RunId::as_i64);
        let job = run_type(run.job);
        let started_at = to_millis(run.started_at.unwrap_or_else(Utc::now));
        let run_label = run.run_id;

        self.with_connection(move |conn| {
            let transaction = conn
                .transaction()
                .map_err(|e| tracking_error("opening the run-creation transaction", &e))?;

            let existing: Option<i64> = transaction
                .query_row(
                    "SELECT run_id FROM cdm_run_info \
                     WHERE keyspace_name = ?1 AND table_name = ?2 AND run_id = ?3",
                    (&keyspace, &table_name, run_id),
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| tracking_error("looking for an existing run", &e))?;
            if existing.is_some() {
                return Err(CdmError::new(
                    ErrorKind::Tracking,
                    format!(
                        "run id {run_label} already exists for table {table_name}; \
                         pick another track_run.run_id or omit it to have one allocated"
                    ),
                )
                .with_context(|ctx| ctx.with_config_key("track_run.run_id")));
            }

            transaction
                .execute(
                    INSERT_RUN_INFO,
                    (
                        &keyspace,
                        &table_name,
                        run_id,
                        job,
                        previous,
                        started_at,
                        status_string(RunStatus::NotStarted),
                    ),
                )
                .map_err(|e| tracking_error("inserting the run row", &e))?;

            for (min, max) in bounds {
                transaction
                    .execute(
                        INSERT_RUN_DETAIL,
                        (
                            &keyspace,
                            &table_name,
                            run_id,
                            min,
                            max,
                            status_string(RunStatus::NotStarted),
                        ),
                    )
                    .map_err(|e| tracking_error("inserting a range row", &e))?;
            }

            transaction
                .execute(
                    START_RUN_INFO,
                    (
                        &keyspace,
                        &table_name,
                        run_id,
                        status_string(RunStatus::Started),
                    ),
                )
                .map_err(|e| tracking_error("marking the run started", &e))?;

            transaction
                .commit()
                .map_err(|e| tracking_error("committing the run rows", &e))
        })
        .await
    }

    /// Java's `endCdmRun` (`TRK-022`), over any terminal status so that `INTERRUPTED` and
    /// `ABORTED` can be recorded too (`TRK-012`).
    ///
    /// A run row that is not there is an error rather than a silent insert. Cassandra's blind
    /// upsert cannot tell the two apart; SQLite can, and a tracker that has lost its own run row
    /// has lost the thread — writing a fresh row would hide that and leave a run whose ranges are
    /// nowhere.
    async fn update_run(
        &self,
        run_id: RunId,
        status: RunStatus,
        info: Option<&str>,
    ) -> Result<(), CdmError> {
        let (keyspace, table_name) = self.scope();
        let info = info.map(ToOwned::to_owned);
        let ended_at = to_millis(Utc::now());
        self.with_connection(move |conn| {
            let updated = conn
                .execute(
                    END_RUN_INFO,
                    (
                        &keyspace,
                        &table_name,
                        run_id.as_i64(),
                        ended_at,
                        info,
                        status_string(status),
                    ),
                )
                .map_err(|e| tracking_error("updating the run row", &e))?;
            if updated == 0 {
                return Err(CdmError::new(
                    ErrorKind::Tracking,
                    format!("run {run_id} does not exist"),
                ));
            }
            Ok(())
        })
        .await
    }

    /// Java's `updateCdmRun` (`TRK-021`).
    ///
    /// `STARTED` takes the statement that sets `start_time` and leaves `run_info` alone; every
    /// other status takes the one that writes the metrics string. The split is not cosmetic: a
    /// start write that also wrote `run_info` would erase the metrics of the attempt that just
    /// failed, which is the only record of what it managed.
    async fn update_range(&self, run_id: RunId, range: &RangeRecord) -> Result<(), CdmError> {
        let (keyspace, table_name) = self.scope();
        let min = token_bound(range.range.min(), range.range)?;
        let max = token_bound(range.range.max(), range.range)?;
        let status = range.status;
        let info = range.info.clone();
        let started_at = range.started_at.map(to_millis);
        self.with_connection(move |conn| {
            if status == RunStatus::Started {
                conn.execute(
                    START_RUN_DETAIL,
                    (
                        &keyspace,
                        &table_name,
                        run_id.as_i64(),
                        min,
                        max,
                        started_at.unwrap_or_else(|| to_millis(Utc::now())),
                        status_string(status),
                    ),
                )
                .map_err(|e| tracking_error("marking a range started", &e))?;
            } else {
                conn.execute(
                    UPDATE_RUN_DETAIL,
                    (
                        &keyspace,
                        &table_name,
                        run_id.as_i64(),
                        min,
                        max,
                        started_at,
                        status_string(status),
                        info,
                    ),
                )
                .map_err(|e| tracking_error("recording a range outcome", &e))?;
            }
            Ok(())
        })
        .await
    }

    async fn run(&self, run_id: RunId) -> Result<Option<RunRecord>, CdmError> {
        let (keyspace, table_name) = self.scope();
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM cdm_run_info \
             WHERE keyspace_name = ?1 AND table_name = ?2 AND run_id = ?3"
        );
        self.with_connection(move |conn| {
            conn.query_row(&sql, (&keyspace, &table_name, run_id.as_i64()), |row| {
                decode_run(row, &keyspace, &table_name)
            })
            .optional()
            .map_err(|e| tracking_error("reading the run row", &e))
        })
        .await
    }

    /// Every range row of a run, ordered by `token_min` (`TRK-031`).
    ///
    /// The whole set, unfiltered by status, for the reason the Cassandra backend gives: a status
    /// this build does not recognise must stay *visible* so [`decode_status`] can call it pending,
    /// where a status-equality filter would silently drop the range from the resume.
    async fn ranges(&self, run_id: RunId) -> Result<Vec<RangeRecord>, CdmError> {
        let (keyspace, table_name) = self.scope();
        self.with_connection(move |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT token_min, token_max, start_time, status, run_info \
                     FROM cdm_run_details \
                     WHERE keyspace_name = ?1 AND table_name = ?2 AND run_id = ?3 \
                     ORDER BY token_min",
                )
                .map_err(|e| tracking_error("preparing the range query", &e))?;
            let rows = statement
                .query_map((&keyspace, &table_name, run_id.as_i64()), |row| {
                    let min: i64 = row.get(0)?;
                    let max: i64 = row.get(1)?;
                    let start_time: Option<i64> = row.get(2)?;
                    let status: Option<String> = row.get(3)?;
                    let info: Option<String> = row.get(4)?;
                    Ok((min, max, start_time, status, info))
                })
                .map_err(|e| tracking_error("selecting the range rows", &e))?;

            let mut records = Vec::new();
            for row in rows {
                let (min, max, start_time, status, info) =
                    row.map_err(|e| tracking_error("reading a range row", &e))?;
                // Inverted bounds cannot become a `TokenRange`. Dropping the row would remove the
                // range from the resume, so it is an error the operator sees.
                let range = TokenRange::new(i128::from(min), i128::from(max)).map_err(|e| {
                    CdmError::new(
                        ErrorKind::Tracking,
                        format!("cdm_run_details holds an unusable range [{min}, {max}]: {e}"),
                    )
                })?;
                records.push(RangeRecord {
                    range,
                    status: decode_status(status.as_deref()),
                    started_at: from_millis(start_time),
                    info,
                });
            }
            Ok(records)
        })
        .await
    }

    /// The newest run for `(table_name, run_type)` (`TRK-030`).
    ///
    /// `table` is checked against the table this store was built for rather than used as a
    /// filter: a store pointed at one table cannot answer for another, and answering anyway would
    /// resume the wrong data.
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
        let (keyspace, table_name) = self.scope();
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM cdm_run_info \
             WHERE keyspace_name = ?1 AND table_name = ?2 AND run_type = ?3 \
             ORDER BY run_id DESC LIMIT 1"
        );
        self.with_connection(move |conn| {
            conn.query_row(&sql, (&keyspace, &table_name, run_type(job)), |row| {
                decode_run(row, &keyspace, &table_name)
            })
            .optional()
            .map_err(|e| tracking_error("reading the most recent run", &e))
        })
        .await
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
    use cdm_core::TokenRange;
    use tempfile::TempDir;

    use super::*;
    use crate::manage::RunCatalog;
    use crate::schema::{RUN_DETAILS_TABLE, RUN_INFO_TABLE};

    fn table() -> TableRef {
        TableRef::new("target_ks", "customers")
    }

    /// A store over a fresh file in a temporary directory.
    ///
    /// The directory is returned with it: dropping it deletes the file, and a store outliving its
    /// file would fail in ways that have nothing to do with what is being tested. `TempDir` is
    /// also the portable answer — this suite runs on Windows, where no `/tmp` exists.
    async fn store() -> (TempDir, SqliteStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path().join("tracking.db"), &table()).unwrap();
        store.initialise().await.unwrap();
        (dir, store)
    }

    fn run_record(run_id: i64) -> RunRecord {
        RunRecord {
            run_id: RunId::from_raw(run_id),
            previous_run_id: None,
            table: table(),
            job: JobKind::Migrate,
            status: RunStatus::NotStarted,
            started_at: Some(Utc::now()),
            ended_at: None,
            info: None,
        }
    }

    fn range_record(min: i128, max: i128) -> RangeRecord {
        RangeRecord {
            range: TokenRange::new(min, max).unwrap(),
            status: RunStatus::NotStarted,
            started_at: None,
            info: None,
        }
    }

    #[tokio::test]
    async fn trk_036_the_sqlite_backend_answers_the_whole_tracking_store_trait() {
        let (_dir, store) = store().await;
        let store: &dyn TrackingStore = &store;
        assert_eq!(store.name(), "sqlite");
        assert_eq!(store.provider(), "cdm-track");
        assert_eq!(store.run(RunId::from_raw(1)).await.unwrap(), None);
        assert!(store.ranges(RunId::from_raw(1)).await.unwrap().is_empty());
        assert_eq!(
            store.latest_run(&table(), JobKind::Migrate).await.unwrap(),
            None
        );
        // Updating a run that does not exist is an error rather than a silent no-op: a tracker
        // that cannot find its own run row has lost the thread and must say so.
        let err = store
            .update_run(RunId::from_raw(1), RunStatus::Ended, None)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
    }

    #[tokio::test]
    async fn trk_036_initialising_an_existing_database_adopts_it_rather_than_failing() {
        let (_dir, store) = store().await;
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        // The tracker calls `initialise` on every start, including a resume of the same file.
        store.initialise().await.unwrap();
        assert!(store.run(RunId::from_raw(1)).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn trk_036_run_creation_leaves_the_run_started_and_every_range_not_started() {
        let (_dir, store) = store().await;
        store
            .create_run(&run_record(1), &[range_record(0, 9), range_record(10, 19)])
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Started);
        assert_eq!(run.table, table());
        assert_eq!(run.job, JobKind::Migrate);
        assert!(run.started_at.is_some());
        assert_eq!(run.previous_run_id, None, "a stored 0 is not a run");
        let ranges = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(ranges.len(), 2);
        assert!(ranges.iter().all(|r| r.status == RunStatus::NotStarted));
        assert_eq!(ranges[0].range, TokenRange::new(0, 9).unwrap());
    }

    #[tokio::test]
    async fn trk_036_a_run_id_that_already_exists_is_rejected() {
        let (_dir, store) = store().await;
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        let mut second = run_record(1);
        second.previous_run_id = Some(RunId::from_raw(7));
        let err = store
            .create_run(&second, &[range_record(20, 29)])
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
        assert!(err.to_string().contains("already exists"));
        assert_eq!(
            err.context().config_key.as_deref(),
            Some("track_run.run_id")
        );
        // TRK-020: the rejected run must not have touched the first one's rows.
        let ranges = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(ranges.len(), 1);
    }

    #[tokio::test]
    async fn trk_036_a_start_write_does_not_erase_an_earlier_metrics_string() {
        let (_dir, store) = store().await;
        let run_id = RunId::from_raw(1);
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        store
            .update_range(
                run_id,
                &RangeRecord {
                    status: RunStatus::Fail,
                    info: Some("Read: 5; Write: 0".to_owned()),
                    ..range_record(0, 9)
                },
            )
            .await
            .unwrap();
        store
            .update_range(
                run_id,
                &RangeRecord {
                    status: RunStatus::Started,
                    started_at: Some(Utc::now()),
                    info: None,
                    ..range_record(0, 9)
                },
            )
            .await
            .unwrap();
        let ranges = store.ranges(run_id).await.unwrap();
        assert_eq!(ranges[0].status, RunStatus::Started);
        assert_eq!(ranges[0].info.as_deref(), Some("Read: 5; Write: 0"));
        assert!(ranges[0].started_at.is_some());
    }

    #[tokio::test]
    async fn trk_036_a_range_that_was_never_planned_is_recorded_rather_than_lost() {
        // The Cassandra backend upserts, so a range the plan did not contain still lands in the
        // table. This one must behave the same, or a subdivided rerun (TRK-033) would write
        // sub-ranges into nothing.
        let (_dir, store) = store().await;
        let run_id = RunId::from_raw(1);
        store.create_run(&run_record(1), &[]).await.unwrap();
        store
            .update_range(
                run_id,
                &RangeRecord {
                    status: RunStatus::Pass,
                    info: Some("Read: 1".to_owned()),
                    ..range_record(0, 9)
                },
            )
            .await
            .unwrap();
        let ranges = store.ranges(run_id).await.unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].status, RunStatus::Pass);
    }

    #[tokio::test]
    async fn trk_036_stopping_a_run_records_the_status_the_metrics_and_an_end_time() {
        let (_dir, store) = store().await;
        store.create_run(&run_record(1), &[]).await.unwrap();
        // TRK-022: an interrupted run is not an ended one, and both must be recordable.
        store
            .update_run(
                RunId::from_raw(1),
                RunStatus::Interrupted,
                Some("Read: 9; Partitions Failed: 1"),
            )
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Interrupted);
        assert_eq!(run.info.as_deref(), Some("Read: 9; Partitions Failed: 1"));
        assert!(run.ended_at.is_some());

        // And a later status-only write — `cdm runs cancel` — keeps the metrics.
        store
            .update_run(RunId::from_raw(1), RunStatus::Aborted, None)
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Aborted);
        assert_eq!(run.info.as_deref(), Some("Read: 9; Partitions Failed: 1"));
    }

    #[tokio::test]
    async fn trk_036_the_latest_run_is_the_highest_id_for_the_table_and_job() {
        let (_dir, store) = store().await;
        for id in [10_i64, 30, 20] {
            store.create_run(&run_record(id), &[]).await.unwrap();
        }
        let mut other_job = run_record(40);
        other_job.job = JobKind::Validate;
        store.create_run(&other_job, &[]).await.unwrap();

        let latest = store
            .latest_run(&table(), JobKind::Migrate)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.run_id, RunId::from_raw(30), "run_type must filter");
        assert_eq!(
            store
                .latest_run(&table(), JobKind::Validate)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            RunId::from_raw(40)
        );
        // A store built for one table cannot answer for another.
        assert!(store
            .latest_run(&TableRef::new("target_ks", "orders"), JobKind::Migrate)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn trk_036_the_run_catalog_lists_every_run_newest_first() {
        let (_dir, store) = store().await;
        for id in [10_i64, 30, 20] {
            store.create_run(&run_record(id), &[]).await.unwrap();
        }
        let ids: Vec<i64> = store
            .runs(&table(), None)
            .await
            .unwrap()
            .into_iter()
            .map(|run| run.run_id.as_i64())
            .collect();
        assert_eq!(ids, vec![30, 20, 10]);
        assert!(store
            .runs(&table(), Some(JobKind::Validate))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn trk_036_two_tables_sharing_one_file_do_not_share_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tracking.db");
        let customers = SqliteStore::open(&path, &table()).unwrap();
        let orders = SqliteStore::open(&path, &TableRef::new("target_ks", "orders")).unwrap();
        customers.initialise().await.unwrap();
        orders.initialise().await.unwrap();

        customers
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        // The same run id, for a different table, is a different run — not a collision.
        let mut other = run_record(1);
        other.table = TableRef::new("target_ks", "orders");
        orders
            .create_run(&other, &[range_record(50, 59)])
            .await
            .unwrap();

        assert_eq!(
            customers.ranges(RunId::from_raw(1)).await.unwrap()[0].range,
            TokenRange::new(0, 9).unwrap()
        );
        assert_eq!(
            orders.ranges(RunId::from_raw(1)).await.unwrap()[0].range,
            TokenRange::new(50, 59).unwrap()
        );
    }

    #[tokio::test]
    async fn trk_036_a_run_survives_the_process_that_wrote_it() {
        // The whole point of the backend: MemoryStore cannot do this, and an interrupted run is
        // exactly when the record has to still be there.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tracking.db");
        {
            let store = SqliteStore::open(&path, &table()).unwrap();
            store.initialise().await.unwrap();
            store
                .create_run(&run_record(1), &[range_record(0, 9), range_record(10, 19)])
                .await
                .unwrap();
            store
                .update_range(
                    RunId::from_raw(1),
                    &RangeRecord {
                        status: RunStatus::Pass,
                        info: Some("Read: 10; Write: 10".to_owned()),
                        ..range_record(0, 9)
                    },
                )
                .await
                .unwrap();
            store
                .update_run(RunId::from_raw(1), RunStatus::Interrupted, Some("Read: 10"))
                .await
                .unwrap();
        }

        let reopened = SqliteStore::open(&path, &table()).unwrap();
        reopened.initialise().await.unwrap();
        let run = reopened.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Interrupted);
        assert_eq!(run.info.as_deref(), Some("Read: 10"));
        let ranges = reopened.ranges(RunId::from_raw(1)).await.unwrap();
        // TRK-031: one range done, one still pending — which is what a resume re-plans.
        assert_eq!(ranges[0].status, RunStatus::Pass);
        assert_eq!(ranges[1].status, RunStatus::NotStarted);
        assert!(!ranges[0].status.is_pending());
        assert!(ranges[1].status.is_pending());
    }

    #[tokio::test]
    async fn trk_036_a_status_this_build_cannot_read_comes_back_as_pending() {
        let (_dir, store) = store().await;
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        // A newer cdm-rs, or a hand edit: whatever wrote it, the range must not vanish from the
        // resume (TRK-031).
        store
            .with_connection(|conn| {
                conn.execute("UPDATE cdm_run_details SET status = 'QUANTUM'", [])
                    .map_err(|e| tracking_error("rewriting the status", &e))?;
                Ok(())
            })
            .await
            .unwrap();
        let ranges = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(ranges[0].status, RunStatus::Started);
        assert!(ranges[0].status.is_pending());
    }

    #[tokio::test]
    async fn trk_036_a_token_that_does_not_fit_the_column_is_refused_before_anything_is_written() {
        let (_dir, store) = store().await;
        let wide = TokenRange::new(0, i128::from(i64::MAX) + 1).unwrap();
        let err = store
            .create_run(
                &run_record(1),
                &[RangeRecord {
                    range: wide,
                    status: RunStatus::NotStarted,
                    started_at: None,
                    info: None,
                }],
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
        assert!(err.to_string().contains("RandomPartitioner"));
        assert_eq!(store.run(RunId::from_raw(1)).await.unwrap(), None);
    }

    #[test]
    fn trk_036_the_sqlite_tables_carry_the_same_names_as_the_cassandra_ones() {
        // Not a compatibility claim — a Java reader cannot open this file — but an operator who
        // knows TRK-010's schema must recognise what is in it.
        assert!(CREATE_RUN_INFO.contains(RUN_INFO_TABLE));
        assert!(CREATE_RUN_DETAILS.contains(RUN_DETAILS_TABLE));
        assert!(CREATE_RUN_INFO.contains("IF NOT EXISTS"));
        assert!(CREATE_RUN_DETAILS.contains("IF NOT EXISTS"));
    }

    #[test]
    fn trk_036_opening_a_database_in_a_directory_that_does_not_exist_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = SqliteStore::open(dir.path().join("nope").join("t.db"), &table()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
        assert!(err.to_string().contains("SQLite tracking database"));
    }

    #[test]
    fn trk_036_a_store_cannot_be_built_without_a_keyspace_and_table() {
        assert!(SqliteStore::open("unused.db", &TableRef::new("", "t")).is_err());
    }

    #[test]
    fn sec_002_no_statement_in_this_module_can_carry_a_row_value() {
        // Every statement is built from fixed identifiers and `?n` markers; the only values bound
        // are token bounds, statuses, timestamps and counter strings.
        for statement in [
            CREATE_RUN_INFO,
            CREATE_RUN_DETAILS,
            INSERT_RUN_INFO,
            INSERT_RUN_DETAIL,
            START_RUN_INFO,
            END_RUN_INFO,
            START_RUN_DETAIL,
            UPDATE_RUN_DETAIL,
        ] {
            assert!(!statement.contains('\''), "{statement} inlines a literal");
        }
    }

    #[test]
    fn trk_036_timestamps_round_trip_through_the_integer_column() {
        let now = Utc::now();
        let stored = to_millis(now);
        assert_eq!(
            from_millis(Some(stored)),
            DateTime::from_timestamp_millis(stored)
        );
        assert_eq!(from_millis(None), None);
        // A value no calendar can represent is dropped rather than panicking.
        assert_eq!(from_millis(Some(i64::MAX)), None);
    }
}
