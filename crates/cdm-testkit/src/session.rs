//! The session seam, and the mock that replaces Java's `CommonMocks` (`TST-100`).
//!
//! `cdm-testkit` cannot execute CQL: only `cdm-cql` may depend on the driver
//! (`ARCHITECTURE.md` §3), and `cdm-cql`'s session type does not exist yet. Rather than wait, the
//! generators here produce statements and this module defines the one-method trait that runs
//! them. Two implementations then exist independently:
//!
//! * [`MockSession`], here, for tests that need no cluster — it records what it was asked to run
//!   and answers with whatever the test primed it with;
//! * a real session, in `cdm-cql`, which is a thin `impl TestSession for SessionHandle` once that
//!   type lands. Nothing in this crate changes when it does.
//!
//! # Why this replaces `CommonMocks`
//!
//! Java's `CommonMocks` is one class with forty-five fields; a test configures the ones it cares
//! about by mutation and inherits whatever the other forty were left as. Reading such a test
//! means reading the fixture. [`MockSession`] instead starts empty and answers nothing, and a
//! test states — in the test — every statement it expects and every answer it wants. What a test
//! does not say, it does not depend on.

use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind};

use crate::data::GeneratedRow;
use crate::schema::{create_keyspace_statement, TableSpec};

/// One row of a result, as raw column bytes.
///
/// Bytes rather than typed values, because typed values would need a driver's type system and
/// this crate has none — and because raw bytes are what `MIG-040`'s passthrough path deals in, so
/// a fixture that spoke in decoded values could not express a passthrough test at all.
///
/// `None` is a null column; `Some(&[])` is an empty value, which is a different thing
/// (`MIG-012`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestRow {
    columns: Vec<(String, Option<Vec<u8>>)>,
}

impl TestRow {
    /// A row from named columns.
    pub fn new(columns: Vec<(String, Option<Vec<u8>>)>) -> Self {
        Self { columns }
    }

    /// A row of text columns, the common case for a fixture assertion.
    pub fn of_text<'a>(columns: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            columns: columns
                .into_iter()
                .map(|(name, value)| (name.to_owned(), Some(value.as_bytes().to_vec())))
                .collect(),
        }
    }

    /// The columns, in projection order.
    pub fn columns(&self) -> &[(String, Option<Vec<u8>>)] {
        &self.columns
    }

    /// How many columns the row has.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether the row has no columns at all.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// A column's raw bytes, or `None` for a null column or a column that is not present. Use
    /// [`TestRow::has_column`] to tell those two apart.
    pub fn bytes(&self, column: &str) -> Option<&[u8]> {
        self.columns
            .iter()
            .find(|(name, _)| name == column)
            .and_then(|(_, bytes)| bytes.as_deref())
    }

    /// Whether the row carries the named column at all.
    pub fn has_column(&self, column: &str) -> bool {
        self.columns.iter().any(|(name, _)| name == column)
    }

    /// A column's bytes as UTF-8.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TypeConversion`] if the column is absent, null, or not valid UTF-8.
    pub fn text(&self, column: &str) -> Result<&str, CdmError> {
        let bytes = self.bytes(column).ok_or_else(|| {
            CdmError::new(
                ErrorKind::TypeConversion,
                format!("column `{column}` is absent or null"),
            )
        })?;
        std::str::from_utf8(bytes).map_err(|e| {
            CdmError::new(
                ErrorKind::TypeConversion,
                format!("column `{column}` is not valid UTF-8: {e}"),
            )
        })
    }
}

/// Runs CQL on behalf of a fixture (`TST-100`).
///
/// The seam between this crate and whatever can actually talk to a cluster. Deliberately tiny:
/// one method that takes a statement and returns rows. A fixture does not prepare, page, or bind
/// — the generators produce complete statements with literals inlined, because a fixture's
/// statements are setup, not the thing under test.
#[async_trait]
pub trait TestSession: fmt::Debug + Send + Sync {
    /// Executes one CQL statement and returns whatever rows it produced.
    ///
    /// # Errors
    ///
    /// Whatever the implementation's cluster or mock reports.
    async fn execute(&self, cql: &str) -> Result<Vec<TestRow>, CdmError>;

    /// Waits for schema agreement, if the implementation can.
    ///
    /// The default does nothing, which is right for a mock and wrong for a real cluster: DDL
    /// applied without waiting is visible on the coordinator and not yet on anybody else, and a
    /// single-node fixture hides that until the day somebody points the suite at three nodes.
    ///
    /// # Errors
    ///
    /// Whatever the implementation reports.
    async fn await_schema_agreement(&self) -> Result<(), CdmError> {
        Ok(())
    }
}

/// Creates the keyspace, the UDTs and the table of `table`, in order (`TST-100`).
///
/// # Errors
///
/// Whatever the session reports, with the failing statement named — a DDL failure whose message
/// does not include the DDL is close to undiagnosable.
pub async fn apply_schema(session: &dyn TestSession, table: &TableSpec) -> Result<(), CdmError> {
    let mut statements = vec![create_keyspace_statement(table.keyspace())];
    statements.extend(table.create_statements());

    for statement in statements {
        session.execute(&statement).await.map_err(|e| {
            CdmError::new(
                e.kind(),
                format!("applying `{statement}` failed: {}", e.message()),
            )
        })?;
        session.await_schema_agreement().await?;
    }
    Ok(())
}

/// Writes generated rows, returning how many statements were executed.
///
/// # Errors
///
/// Whatever the session reports, or [`ErrorKind::Internal`] if a row does not match `table`.
pub async fn seed_rows(
    session: &dyn TestSession,
    table: &TableSpec,
    rows: &[GeneratedRow],
) -> Result<usize, CdmError> {
    for row in rows {
        let statement = row.write_statement(table)?;
        session.execute(&statement).await.map_err(|e| {
            CdmError::new(
                e.kind(),
                format!("seeding with `{statement}` failed: {}", e.message()),
            )
        })?;
    }
    Ok(rows.len())
}

/// A rule the mock applies to a statement it is asked to run.
///
/// A failure is stored as a kind and a message rather than as a [`CdmError`], because a `CdmError`
/// carries a source and is deliberately not [`Clone`] — a rule may fire any number of times, so it
/// has to be able to produce a fresh error each time.
#[derive(Debug, Clone)]
enum Rule {
    /// Answer with these rows.
    Rows(Vec<TestRow>),
    /// Fail with an error of this kind and message, however many times it is asked.
    Fail(ErrorKind, String),
}

/// A [`TestSession`] that records what it was asked and answers what it was told (`TST-100`).
///
/// ```
/// use cdm_testkit::{MockSession, TestRow, TestSession};
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let session = MockSession::new()
///     .responding("SELECT", vec![TestRow::of_text([("key", "a")])]);
///
/// let rows = session.execute("SELECT key FROM ks.t").await?;
/// assert_eq!(rows.first().and_then(|row| row.text("key").ok()), Some("a"));
/// assert_eq!(session.executed().len(), 1);
/// # Ok::<(), cdm_core::CdmError>(())
/// # }).unwrap();
/// ```
#[derive(Debug, Default)]
pub struct MockSession {
    rules: Vec<(String, Rule)>,
    executed: Mutex<Vec<String>>,
}

impl MockSession {
    /// A mock that records everything and answers every statement with no rows.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers statements containing `matching` with `rows`.
    ///
    /// Rules are tried in the order they were added, so a specific rule added first wins over a
    /// general one added later.
    #[must_use]
    pub fn responding(mut self, matching: impl Into<String>, rows: Vec<TestRow>) -> Self {
        self.rules.push((matching.into(), Rule::Rows(rows)));
        self
    }

    /// Fails statements containing `matching` — the seed of the `FaultySession` of `TST-040`,
    /// which extends this with timeout and overload injection in PR #33.
    #[must_use]
    pub fn failing(
        mut self,
        matching: impl Into<String>,
        kind: ErrorKind,
        message: impl Into<String>,
    ) -> Self {
        self.rules
            .push((matching.into(), Rule::Fail(kind, message.into())));
        self
    }

    /// Every statement executed, in order.
    pub fn executed(&self) -> Vec<String> {
        self.lock().clone()
    }

    /// The statements executed that contain `needle`.
    pub fn executed_matching(&self, needle: &str) -> Vec<String> {
        self.lock()
            .iter()
            .filter(|statement| statement.contains(needle))
            .cloned()
            .collect()
    }

    /// How many statements were executed.
    pub fn execution_count(&self) -> usize {
        self.lock().len()
    }

    /// Forgets what has been executed so far, for a test with several phases.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// The recorded statements.
    ///
    /// A poisoned lock is recovered from rather than propagated: the mutex guards a `Vec<String>`
    /// with no invariant to break, and a panic in one test must not turn every later assertion on
    /// the same mock into a second, misleading failure.
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        match self.executed.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[async_trait]
impl TestSession for MockSession {
    async fn execute(&self, cql: &str) -> Result<Vec<TestRow>, CdmError> {
        self.lock().push(cql.to_owned());
        for (matching, rule) in &self.rules {
            if !cql.contains(matching.as_str()) {
                continue;
            }
            return match rule {
                Rule::Rows(rows) => Ok(rows.clone()),
                Rule::Fail(kind, message) => Err(CdmError::new(*kind, message.clone())),
            };
        }
        Ok(Vec::new())
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
    use crate::data::DataGen;
    use crate::schema::SchemaGen;
    use crate::seed::Seed;

    #[tokio::test]
    async fn tst_100_a_mock_records_every_statement_in_order() {
        let session = MockSession::new();
        session.execute("CREATE KEYSPACE ks").await.unwrap();
        session.execute("SELECT * FROM ks.t").await.unwrap();

        assert_eq!(
            session.executed(),
            vec!["CREATE KEYSPACE ks", "SELECT * FROM ks.t"]
        );
        assert_eq!(session.execution_count(), 2);
        assert_eq!(session.executed_matching("SELECT").len(), 1);

        session.clear();
        assert_eq!(session.execution_count(), 0);
    }

    #[tokio::test]
    async fn tst_100_a_mock_answers_only_what_it_was_primed_with() {
        let session = MockSession::new().responding(
            "SELECT key",
            vec![TestRow::of_text([("key", "a"), ("value", "b")])],
        );

        assert!(session
            .execute("SELECT other FROM t")
            .await
            .unwrap()
            .is_empty());

        let rows = session.execute("SELECT key FROM t").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text("key").unwrap(), "a");
        assert_eq!(rows[0].len(), 2);
        assert!(!rows[0].is_empty());
        assert!(rows[0].has_column("value"));
        assert!(!rows[0].has_column("absent"));
    }

    #[tokio::test]
    async fn tst_040_a_mock_can_be_told_to_fail_a_statement() {
        let session = MockSession::new().failing("INSERT", ErrorKind::Write, "write timeout");

        assert!(session.execute("SELECT 1").await.is_ok());
        let err = session
            .execute("INSERT INTO ks.t (k) VALUES (1)")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Write);
        // The failing statement is still recorded: what was attempted matters as much as what
        // succeeded.
        assert_eq!(session.execution_count(), 2);
    }

    #[tokio::test]
    async fn tst_100_the_first_matching_rule_wins() {
        let session = MockSession::new()
            .responding(
                "SELECT key FROM ks.t",
                vec![TestRow::of_text([("key", "specific")])],
            )
            .responding("SELECT", vec![TestRow::of_text([("key", "general")])]);

        let rows = session.execute("SELECT key FROM ks.t").await.unwrap();
        assert_eq!(rows[0].text("key").unwrap(), "specific");
        let rows = session.execute("SELECT other FROM ks.t").await.unwrap();
        assert_eq!(rows[0].text("key").unwrap(), "general");
    }

    #[tokio::test]
    async fn tst_100_applying_a_schema_runs_keyspace_types_and_table_in_order() {
        let table =
            SchemaGen::all_types("cdm_test", "all", crate::Capabilities::portable()).unwrap();
        let session = MockSession::new();

        apply_schema(&session, &table).await.unwrap();

        let executed = session.executed();
        assert_eq!(executed.len(), 3, "{executed:?}");
        assert!(executed[0].starts_with("CREATE KEYSPACE IF NOT EXISTS cdm_test"));
        assert!(executed[1].starts_with("CREATE TYPE IF NOT EXISTS cdm_test.cdm_address"));
        assert!(executed[2].starts_with("CREATE TABLE IF NOT EXISTS cdm_test.all"));
    }

    #[tokio::test]
    async fn tst_100_a_ddl_failure_names_the_statement_that_failed() {
        let table = SchemaGen::simple("cdm_test", "kv").unwrap();
        let session =
            MockSession::new().failing("CREATE TABLE", ErrorKind::SchemaMismatch, "unknown type");

        let err = apply_schema(&session, &table).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::SchemaMismatch);
        assert!(
            err.to_string()
                .contains("CREATE TABLE IF NOT EXISTS cdm_test.kv"),
            "{err}"
        );
        assert!(err.to_string().contains("unknown type"), "{err}");
    }

    #[tokio::test]
    async fn tst_100_seeding_writes_one_statement_per_row() {
        let table = SchemaGen::simple("cdm_test", "kv").unwrap();
        let rows = DataGen::new(Seed::new(1)).rows(&table, 4).unwrap();
        let session = MockSession::new();

        assert_eq!(seed_rows(&session, &table, &rows).await.unwrap(), 4);
        assert_eq!(
            session.executed_matching("INSERT INTO cdm_test.kv").len(),
            4
        );

        let session = MockSession::new().failing("INSERT", ErrorKind::Write, "overloaded");
        let err = seed_rows(&session, &table, &rows).await.unwrap_err();
        assert!(err.to_string().contains("seeding with `INSERT"), "{err}");
    }

    #[tokio::test]
    async fn tst_100_schema_agreement_defaults_to_a_no_op() {
        let session = MockSession::new();
        session.await_schema_agreement().await.unwrap();
    }

    #[test]
    fn mig_012_a_null_column_and_an_empty_one_are_distinguishable() {
        let row = TestRow::new(vec![
            ("null_column".to_owned(), None),
            ("empty_column".to_owned(), Some(Vec::new())),
        ]);
        assert!(row.has_column("null_column"));
        assert_eq!(row.bytes("null_column"), None);
        assert_eq!(row.bytes("empty_column"), Some([].as_slice()));
        assert!(row.text("null_column").is_err());
        assert_eq!(row.text("empty_column").unwrap(), "");
        assert_eq!(TestRow::default().len(), 0);
        assert!(TestRow::default().is_empty());
    }

    #[test]
    fn tst_100_non_utf8_bytes_are_an_error_not_a_lossy_string() {
        let row = TestRow::new(vec![("blob".to_owned(), Some(vec![0xff, 0xfe]))]);
        let err = row.text("blob").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TypeConversion);
        assert!(err.to_string().contains("UTF-8"), "{err}");
        assert_eq!(row.columns().len(), 1);
    }
}
