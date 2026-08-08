//! The tracking tables and the statements against them (`TRK-010`, `TRK-011`).
//!
//! Every string in this module is a compatibility contract (`COMPAT-003`). Java CDM creates
//! `cdm_run_info` and `cdm_run_details` itself, with `IF NOT EXISTS`, so whichever tool runs
//! first defines the tables and the other must be happy with what it finds. A column renamed, a
//! primary key reordered, or the keyspace quoted differently, and a run started by one tool is
//! invisible to the other — which is exactly the failure mode run tracking exists to prevent.
//!
//! # What is deliberately *not* here
//!
//! No statement in this module ever takes a row value, a column name from the migrated table, or
//! anything derived from user data. The tracking tables record token bounds, statuses, timestamps
//! and counter strings, and nothing else (`SEC-001`, `SEC-002`). The only interpolated values are
//! the keyspace name — quoted — and the fixed table names below.

use cdm_core::{CdmError, ErrorKind, TableRef};

/// The table Java records one row per run in (`TRK-010`).
pub const RUN_INFO_TABLE: &str = "cdm_run_info";

/// The table Java records one row per token range in (`TRK-010`).
pub const RUN_DETAILS_TABLE: &str = "cdm_run_details";

/// The table cdm-rs adds for distributed range claiming (`TRK-011`, `DST-010`).
pub const RUN_LEASES_TABLE: &str = "cdm_run_leases";

/// Names the tracking tables inside one keyspace, and renders the statements against them.
///
/// Constructed from the **target** table: Java derives both the keyspace and the `table_name`
/// column value from the single `keyspace.table` string it is handed, and the value it stores is
/// the *bare* table name, not the qualified one. Storing `ks.tbl` there instead would make every
/// Java-written row unmatchable, so [`TrackingTables::table_name`] is the bare name and the tests
/// say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingTables {
    keyspace: String,
    table_name: String,
}

impl TrackingTables {
    /// Names the tracking tables that belong to `table`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if either identifier is empty, or if the keyspace contains
    /// a double quote. Cassandra allows a quoted identifier to contain a doubled `""`, but no
    /// keyspace cdm-rs can reach is named that way and accepting one here would mean building
    /// CQL by concatenation from a string that can close its own quoting.
    pub fn new(table: &TableRef) -> Result<Self, CdmError> {
        Self::from_parts(table.keyspace(), table.table())
    }

    /// Names the tracking tables from a keyspace and a bare table name.
    ///
    /// # Errors
    ///
    /// As [`TrackingTables::new`].
    pub fn from_parts(keyspace: &str, table: &str) -> Result<Self, CdmError> {
        if keyspace.is_empty() || table.is_empty() {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                "run tracking needs a target keyspace and table; \
                 set schema.target.keyspace_table or schema.origin.keyspace_table",
            ));
        }
        if keyspace.contains('"') {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!(
                    "keyspace `{keyspace}` contains a double quote, which cdm-rs will not \
                         interpolate into tracking DDL"
                ),
            ));
        }
        Ok(Self {
            keyspace: keyspace.to_owned(),
            table_name: table.to_owned(),
        })
    }

    /// The keyspace the tracking tables live in — the target keyspace (`TRK-010`).
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// The value written to the `table_name` column: the bare table name, as Java writes it.
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// The keyspace, double-quoted exactly as Java quotes it.
    fn quoted_keyspace(&self) -> String {
        format!("\"{}\"", self.keyspace)
    }

    /// `"<ks>".cdm_run_info`.
    pub fn run_info(&self) -> String {
        format!("{}.{RUN_INFO_TABLE}", self.quoted_keyspace())
    }

    /// `"<ks>".cdm_run_details`.
    pub fn run_details(&self) -> String {
        format!("{}.{RUN_DETAILS_TABLE}", self.quoted_keyspace())
    }

    /// `"<ks>".cdm_run_leases`.
    pub fn run_leases(&self) -> String {
        format!("{}.{RUN_LEASES_TABLE}", self.quoted_keyspace())
    }

    /// The two `CREATE TABLE IF NOT EXISTS` statements Java issues, in Java's order (`TRK-010`).
    ///
    /// Column order, types and primary keys are transcribed from
    /// `TargetUpsertRunDetailsStatement`'s constructor. They are upper-cased there and here for
    /// the same reason the rest of this module is verbatim: the smallest change that a reviewer
    /// would wave through is the one that silently forks the schema.
    pub fn create_statements(&self) -> Vec<String> {
        vec![
            format!(
                "CREATE TABLE IF NOT EXISTS {} (table_name TEXT, run_id BIGINT, run_type TEXT, \
                 prev_run_id BIGINT, start_time TIMESTAMP, end_time TIMESTAMP, run_info TEXT, \
                 status TEXT, PRIMARY KEY (table_name, run_id))",
                self.run_info()
            ),
            format!(
                "CREATE TABLE IF NOT EXISTS {} (table_name TEXT, run_id BIGINT, \
                 start_time TIMESTAMP, token_min BIGINT, token_max BIGINT, status TEXT, \
                 run_info TEXT, PRIMARY KEY ((table_name, run_id), token_min))",
                self.run_details()
            ),
        ]
    }

    /// The lease table, created only when `cluster.enabled` is set (`TRK-011`).
    ///
    /// Kept out of [`TrackingTables::create_statements`] on purpose: a single-node run must leave
    /// the target keyspace byte-identical to what Java would have left, so an operator comparing
    /// the two tools sees no extra table. `DST-010` owns the semantics of the columns; this crate
    /// owns only the DDL, so that the table exists before the first node tries to claim a range.
    pub fn create_leases_statement(&self) -> String {
        format!(
            "CREATE TABLE IF NOT EXISTS {} (table_name TEXT, run_id BIGINT, token_min BIGINT, \
             node_id TEXT, lease_until TIMESTAMP, attempt INT, \
             PRIMARY KEY ((table_name, run_id), token_min))",
            self.run_leases()
        )
    }

    /// `INSERT` of the run row (`TRK-020`). Bound as `(table_name, run_id, run_type, prev_run_id,
    /// status)`; `start_time` is the coordinator's clock, as in Java.
    pub fn insert_run_info(&self) -> String {
        format!(
            "INSERT INTO {} (table_name, run_id, run_type, prev_run_id, start_time, status) \
             VALUES (?, ?, ?, ?, totimestamp(now()), ?)",
            self.run_info()
        )
    }

    /// `INSERT` of one range row (`TRK-020`). Bound as `(table_name, run_id, token_min,
    /// token_max, status)`.
    pub fn insert_run_detail(&self) -> String {
        format!(
            "INSERT INTO {} (table_name, run_id, token_min, token_max, status) \
             VALUES (?, ?, ?, ?, ?)",
            self.run_details()
        )
    }

    /// `UPDATE` closing the run row (`TRK-022`). Bound as `(run_info, status, table_name,
    /// run_id)`.
    pub fn end_run_info(&self) -> String {
        format!(
            "UPDATE {} SET end_time = totimestamp(now()), run_info = ?, status = ? \
             WHERE table_name = ? AND run_id = ?",
            self.run_info()
        )
    }

    /// `UPDATE` marking a range `STARTED` (`TRK-021`). Bound as `(status, table_name, run_id,
    /// token_min)`.
    ///
    /// A separate statement from [`TrackingTables::update_run_detail`] because it sets
    /// `start_time` and leaves `run_info` alone, which is what stops the start write from
    /// clobbering a metrics string an earlier attempt left behind.
    pub fn start_run_detail(&self) -> String {
        format!(
            "UPDATE {} SET start_time = totimestamp(now()), status = ? \
             WHERE table_name = ? AND run_id = ? AND token_min = ?",
            self.run_details()
        )
    }

    /// `UPDATE` recording a range's terminal status and metrics (`TRK-021`). Bound as `(status,
    /// run_info, table_name, run_id, token_min)`.
    pub fn update_run_detail(&self) -> String {
        format!(
            "UPDATE {} SET status = ?, run_info = ? \
             WHERE table_name = ? AND run_id = ? AND token_min = ?",
            self.run_details()
        )
    }

    /// `SELECT` of one run row. Bound as `(table_name, run_id)`.
    pub fn select_run_info(&self) -> String {
        format!(
            "SELECT run_id, run_type, prev_run_id, start_time, end_time, run_info, status \
             FROM {} WHERE table_name = ? AND run_id = ?",
            self.run_info()
        )
    }

    /// `SELECT` of the newest run for a job (`TRK-030`). Bound as `(table_name, run_type)`.
    ///
    /// `ORDER BY run_id DESC LIMIT 1 ALLOW FILTERING` is Java's, and the ordering is what makes
    /// `TRK-003`'s time-sortable id a correctness requirement rather than an aesthetic one: the
    /// clustering key decides which run is "the most recent", so an id that does not increase
    /// with time picks the wrong run.
    pub fn select_latest_run_info(&self) -> String {
        format!(
            "SELECT run_id, run_type, prev_run_id, start_time, end_time, run_info, status \
             FROM {} WHERE table_name = ? AND run_type = ? ORDER BY run_id DESC LIMIT 1 \
             ALLOW FILTERING",
            self.run_info()
        )
    }

    /// `SELECT` of every range row of a run (`TRK-031`). Bound as `(table_name, run_id)`.
    ///
    /// Java issues one query per status and concatenates. cdm-rs reads the partition once and
    /// filters in memory: it is the same partition either way, it is one round trip instead of
    /// four, and — the reason that matters — a status cdm-rs does not recognise is *visible*
    /// here, where a status-equality filter would silently drop it. See
    /// [`crate::resume`] for what happens to such a row.
    pub fn select_run_details(&self) -> String {
        format!(
            "SELECT token_min, token_max, start_time, status, run_info FROM {} \
             WHERE table_name = ? AND run_id = ?",
            self.run_details()
        )
    }

    /// `INSERT` of the run row **conditionally**, which is the election of `DST-002`.
    ///
    /// Bound as `(table_name, run_id, run_type, prev_run_id, status, run_info)`. The `run_info`
    /// column carries the secret-redacted configuration hash of `DST-003` until `TRK-022`
    /// replaces it with the metrics string at the end of the run.
    ///
    /// Distinct from [`TrackingTables::insert_run_info`] rather than a flag on it: the
    /// unconditional insert is also what moves the run row to `STARTED`, and an `IF NOT EXISTS`
    /// on *that* write would never apply. The two statements do different things and are told
    /// apart by their names rather than by an argument.
    pub fn insert_run_info_if_not_exists(&self) -> String {
        format!(
            "INSERT INTO {} (table_name, run_id, run_type, prev_run_id, start_time, status, \
             run_info) VALUES (?, ?, ?, ?, totimestamp(now()), ?, ?) IF NOT EXISTS",
            self.run_info()
        )
    }

    /// `INSERT` of a lease for a range nobody holds (`DST-011`). Bound as `(table_name, run_id,
    /// token_min, node_id, lease_until)`; `attempt` is 1, because this is the first.
    pub fn claim_lease_if_absent(&self) -> String {
        format!(
            "INSERT INTO {} (table_name, run_id, token_min, node_id, lease_until, attempt) \
             VALUES (?, ?, ?, ?, ?, 1) IF NOT EXISTS",
            self.run_leases()
        )
    }

    /// `UPDATE` taking a range over from a lease that has expired (`DST-011`, `DST-012`).
    ///
    /// Bound as `(node_id, lease_until, attempt, table_name, run_id, token_min, now)`. The
    /// condition is `lease_until < now` with `now` supplied by the reclaiming node, exactly as
    /// `DST-011` specifies: Cassandra will not evaluate a function inside an `IF`, and taking the
    /// time from the server would mean the decision was made by a clock no node can inspect.
    pub fn reclaim_lease_if_expired(&self) -> String {
        format!(
            "UPDATE {} SET node_id = ?, lease_until = ?, attempt = ? \
             WHERE table_name = ? AND run_id = ? AND token_min = ? IF lease_until < ?",
            self.run_leases()
        )
    }

    /// `UPDATE` extending a lease this node still holds (`DST-012`). Bound as `(lease_until,
    /// table_name, run_id, token_min, node_id)`.
    ///
    /// `IF node_id = ?` is what makes a renewal safe after a reclaim: a node whose lease was
    /// taken cannot extend the new holder's, and learns it lost the range from the failed
    /// condition rather than from the data it was about to write.
    pub fn renew_lease(&self) -> String {
        format!(
            "UPDATE {} SET lease_until = ? \
             WHERE table_name = ? AND run_id = ? AND token_min = ? IF node_id = ?",
            self.run_leases()
        )
    }

    /// `SELECT` of one lease row (`DST-010`). Bound as `(table_name, run_id, token_min)`.
    ///
    /// Issued only when a conditional write did **not** apply: a failed `UPDATE ... IF` returns
    /// only the columns its condition names, and "who holds this range, and on which attempt" is
    /// the whole content of a contention diagnostic.
    pub fn select_lease(&self) -> String {
        format!(
            "SELECT token_min, node_id, lease_until, attempt FROM {} \
             WHERE table_name = ? AND run_id = ? AND token_min = ?",
            self.run_leases()
        )
    }

    /// `SELECT` of every lease row of a run (`DST-010`). Bound as `(table_name, run_id)`.
    pub fn select_leases(&self) -> String {
        format!(
            "SELECT token_min, node_id, lease_until, attempt FROM {} \
             WHERE table_name = ? AND run_id = ?",
            self.run_leases()
        )
    }

    /// `SELECT` of every run recorded for the table, newest first (`TRK-034`).
    pub fn select_runs(&self) -> String {
        format!(
            "SELECT run_id, run_type, prev_run_id, start_time, end_time, run_info, status \
             FROM {} WHERE table_name = ? ORDER BY run_id DESC",
            self.run_info()
        )
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

    fn tables() -> TrackingTables {
        TrackingTables::new(&TableRef::new("target_ks", "customers")).unwrap()
    }

    #[test]
    fn trk_010_the_ddl_is_java_s_ddl_character_for_character() {
        let statements = tables().create_statements();
        assert_eq!(
            statements[0],
            "CREATE TABLE IF NOT EXISTS \"target_ks\".cdm_run_info (table_name TEXT, \
             run_id BIGINT, run_type TEXT, prev_run_id BIGINT, start_time TIMESTAMP, \
             end_time TIMESTAMP, run_info TEXT, status TEXT, PRIMARY KEY (table_name, run_id))"
        );
        assert_eq!(
            statements[1],
            "CREATE TABLE IF NOT EXISTS \"target_ks\".cdm_run_details (table_name TEXT, \
             run_id BIGINT, start_time TIMESTAMP, token_min BIGINT, token_max BIGINT, \
             status TEXT, run_info TEXT, PRIMARY KEY ((table_name, run_id), token_min))"
        );
    }

    #[test]
    fn trk_010_the_table_name_column_holds_the_bare_table_name() {
        // Java splits `ks.tbl` and stores `tbl`. Storing the qualified name would make every
        // Java-written row invisible to the `table_name = ?` predicate.
        assert_eq!(tables().table_name(), "customers");
        assert_eq!(tables().keyspace(), "target_ks");
    }

    #[test]
    fn trk_010_the_keyspace_is_quoted_and_the_table_names_are_not() {
        let tables = tables();
        assert_eq!(tables.run_info(), "\"target_ks\".cdm_run_info");
        assert_eq!(tables.run_details(), "\"target_ks\".cdm_run_details");
        assert_eq!(tables.run_leases(), "\"target_ks\".cdm_run_leases");
    }

    #[test]
    fn trk_010_empty_or_quote_bearing_identifiers_are_refused() {
        assert!(TrackingTables::from_parts("", "t").is_err());
        assert!(TrackingTables::from_parts("ks", "").is_err());
        let err = TrackingTables::from_parts("k\"s", "t").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
    }

    #[test]
    fn trk_010_every_statement_binds_the_table_name_first_in_its_key() {
        let tables = tables();
        for statement in [
            tables.insert_run_info(),
            tables.insert_run_detail(),
            tables.end_run_info(),
            tables.start_run_detail(),
            tables.update_run_detail(),
            tables.select_run_info(),
            tables.select_latest_run_info(),
            tables.select_run_details(),
            tables.select_runs(),
        ] {
            assert!(
                statement.contains("table_name"),
                "{statement} does not name the partition key"
            );
        }
    }

    #[test]
    fn trk_011_the_lease_table_is_not_part_of_the_java_compatible_ddl() {
        let tables = tables();
        assert!(!tables
            .create_statements()
            .iter()
            .any(|s| s.contains(RUN_LEASES_TABLE)));
        assert!(tables.create_leases_statement().contains("cdm_run_leases"));
        assert!(tables.create_leases_statement().contains("lease_until"));
    }

    #[test]
    fn dst_002_the_election_insert_is_conditional_and_carries_the_config_hash() {
        let statement = tables().insert_run_info_if_not_exists();
        assert!(statement.ends_with("IF NOT EXISTS"), "{statement}");
        assert!(statement.contains("run_info"), "{statement}");
        // The unconditional insert must stay unconditional: it is also the write that moves the
        // run row to STARTED, which an `IF NOT EXISTS` would make a no-op.
        assert!(!tables().insert_run_info().contains("IF NOT EXISTS"));
    }

    #[test]
    fn dst_010_every_lease_statement_names_the_lease_table_and_its_whole_key() {
        let tables = tables();
        for statement in [
            tables.claim_lease_if_absent(),
            tables.reclaim_lease_if_expired(),
            tables.renew_lease(),
            tables.select_lease(),
            tables.select_leases(),
        ] {
            assert!(statement.contains(RUN_LEASES_TABLE), "{statement}");
            assert!(statement.contains("table_name"), "{statement}");
            assert!(statement.contains("run_id"), "{statement}");
            assert!(statement.contains("token_min"), "{statement}");
        }
    }

    #[test]
    fn dst_011_a_claim_is_conditional_on_absence_or_on_expiry() {
        let tables = tables();
        assert!(tables.claim_lease_if_absent().contains("IF NOT EXISTS"));
        // `DST-011`'s condition, with `now` bound by the claiming node rather than read from the
        // server: Cassandra evaluates no function inside an `IF`.
        assert!(tables
            .reclaim_lease_if_expired()
            .contains("IF lease_until < ?"));
        // A first claim is attempt 1 by construction, so no read is needed to write it.
        assert!(tables.claim_lease_if_absent().contains("attempt"));
    }

    #[test]
    fn dst_012_a_renewal_is_conditional_on_still_holding_the_lease() {
        assert!(tables().renew_lease().contains("IF node_id = ?"));
        assert!(tables().renew_lease().contains("SET lease_until = ?"));
    }

    #[test]
    fn trk_030_the_latest_run_lookup_filters_on_run_type_and_orders_descending() {
        let statement = tables().select_latest_run_info();
        assert!(statement.contains("run_type = ?"));
        assert!(statement.contains("ORDER BY run_id DESC LIMIT 1"));
    }

    #[test]
    fn sec_002_no_statement_can_carry_a_row_value() {
        // Every statement is built from the keyspace, the fixed table names, and `?` markers.
        // Nothing else is interpolated, which is what makes "a migrated row cannot reach the
        // tracking table" a property of this module rather than of its callers.
        let tables = tables();
        for statement in tables
            .create_statements()
            .into_iter()
            .chain([tables.insert_run_detail(), tables.update_run_detail()])
        {
            assert!(!statement.contains('\''), "{statement} inlines a literal");
        }
    }
}
