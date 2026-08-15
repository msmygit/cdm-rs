//! The differential corpus: the dataset both implementations are pointed at (`TST-020`).
//!
//! `TST-020` requires the nightly Java-parity harness to run "over a generated corpus covering
//! every CQL type, nesting depth 3, nulls, empty collections, and edge-case values (min/max
//! integers, epoch boundaries, unicode, empty strings)". This module is that corpus.
//!
//! # Why the corpus is the experiment
//!
//! A differential harness proves exactly what its data exercises and nothing else. If a type is
//! never generated, the two implementations are never compared on it — and the run still reports
//! "identical", which is worse than reporting nothing, because somebody will believe it. The
//! corpus is therefore enumerated from [`CqlTypeInfo::PRIMITIVES`] rather than assembled from the
//! types that came to mind, and every type that is *not* in the generated schema is listed in
//! [`Corpus::coverage`] with the reason. A named gap is a finding; a silent one makes the harness
//! misleading.
//!
//! # Everything is a CQL literal
//!
//! The rows are loaded by executing plain CQL — `cqlsh` in a container, or any session — because
//! the Java implementation has to be able to load the identical bytes. Nothing here depends on a
//! cdm-rs capability, and a value that cannot be written as a CQL literal is recorded as a gap
//! rather than quietly dropped. See "Gaps" below.
//!
//! # Determinism
//!
//! Keys are derived from the row index and values from a [`Seed`]-derived [`DataGen`], so the same
//! seed produces a byte-identical corpus (`TST-101`). The edge rows are not random at all: they
//! are a fixed table of literals per type, because the interesting values of `bigint` are not the
//! ones a uniform draw finds.
//!
//! # Counters live in their own table
//!
//! A `counter` column may not share a table with a non-counter one, and counter values are written
//! with `UPDATE ... SET c = c + n`, never `INSERT` (`MIG-030`). So the corpus has two tables:
//! [`Corpus::table`], which is everything else, and [`Corpus::counter_table`]. A caller that only
//! ever reads the four contract methods migrates the first and never sees the second, which is why
//! [`Corpus::tables`] exists and why this paragraph is here.
//!
//! # What the comparison engine reads off this
//!
//! The comparator builds its `SELECT` from [`CorpusTable::key_columns`] and
//! [`CorpusTable::value_columns`] rather than from `system_schema`, on purpose: a harness that
//! introspects the schema it is comparing takes its expectations from the same place as its
//! observations and then agrees with itself.
//!
//! Each column says whether `WRITETIME`/`TTL` may be selected for it as a single `bigint`
//! ([`CorpusColumn::timestamp_eligible`]). Three categories may not be: primary-key columns, which
//! Cassandra rejects outright; non-frozen collections, which answer with a `list<bigint>` of
//! per-cell timestamps rather than one; and counters, whose writetime is the coordinator's clock
//! and therefore differs between two runs by construction. **None of that is a gap in the
//! comparison**: those columns' *values* are still byte-compared, and what goes unread is
//! unreadable or meaningless on both sides equally. See [`CorpusColumn::timestamp_eligible`],
//! every clause of which was measured against `cassandra:5.0.9` rather than assumed — two of the
//! three behave differently from the way they are usually described.
//!
//! # Nulls and empty collections
//!
//! Row `null_row` sets every non-key column to `NULL`, and row `empty_row` sets every column that
//! has an empty form to it: `''`, `0x`, `[]`, `{}`, an all-null tuple and an all-null UDT. That
//! pair is the heart of `MIG-012`: an empty collection must be bound as `UNSET`, not `NULL`, or
//! the migration writes a tombstone onto every row.
//!
//! One subtlety the corpus deliberately carries both sides of: for a **non-frozen** collection,
//! Cassandra stores an empty collection as no cells at all, so it reads back indistinguishably
//! from `NULL`. For a **frozen** one it is a real value and reads back as `[]`. The schema
//! therefore has both `list<int>` and `frozen<list<int>>` columns, so a comparison engine that
//! confuses the two fails here rather than in production.
//!
//! # Gaps
//!
//! These are the things `TST-020` names or implies that this corpus does **not** cover, and why.
//! They are also returned programmatically by [`Corpus::coverage`] and rendered into
//! `tests/differential/coverage.tsv`.
//!
//! * **`counter` is in a second table.** Not a gap in coverage, but a gap in shape: the harness
//!   must migrate two tables to compare it. See [`Corpus::counter_table`].
//! * **The DSE types `PointType`, `LineStringType`, `PolygonType` and `DateRangeType`
//!   (`CDC-003`) are absent by default.** No open-source Cassandra or ScyllaDB image implements
//!   them, so a schema containing one does not apply at all. They appear when the corpus is built
//!   with [`Capabilities::dse_geometry`]/[`Capabilities::date_range`] set, which needs a DSE
//!   image the differential job does not have.
//! * **`vector<T, N>` (`CDC-004`) is absent by default.** Open-source Cassandra 5.0 and later
//!   only, and Java CDM's supported driver does not read it on the older half of the matrix.
//!   Build with [`Capabilities::vectors`] to include it.
//! * **A `counter` delta of `i64::MIN` cannot be written.** Measured against Cassandra 5.0.9, not
//!   assumed: CQL has no negative literal in a counter increment, so the corpus writes
//!   `c = c - 9223372036854775808` and the server answers `Unable to make long from
//!   '9223372036854775808'`. Reaching the value would take two updates to one cell, which breaks
//!   the one-statement-per-row invariant [`CorpusTable::row_count`] rests on. The corpus stops at
//!   `i64::MIN + 1`.
//! * **A lone surrogate cannot be tested.** `TST-020` asks for one "if CQL permits". It does not:
//!   a `text` column is `UTF8Type`, an unpaired surrogate is not valid UTF-8, Cassandra rejects
//!   it, and a Rust `String` cannot hold one either. The corpus covers the reachable neighbours
//!   instead — a 4-byte astral character, a combining mark, a flag sequence — and the case is
//!   recorded as a gap rather than approximated with something that is not a surrogate.
//! * **The 1582 Julian/Gregorian cutover is present for `date` and `timestamp` but is not a
//!   boundary in Cassandra.** `date` is a day count biased by 2^31 and `timestamp` is a
//!   millisecond count, both proleptic Gregorian, so 1582-10-05..14 — the ten days the Gregorian
//!   reform skipped — are ordinary representable dates. The corpus writes both edges of the
//!   reform anyway, because Java's `SimpleDateFormat`/`GregorianCalendar` lineage *does* have a
//!   discontinuity there and a parity harness's job is to catch exactly that.
//! * **`text` values contain no control characters.** The loader flattens a script to the single
//!   line `cqlsh -e` accepts (see [`flatten_cql`](crate::sit::flatten_cql)), so a literal
//!   containing a raw newline would be cut in half by the loader rather than by anything under
//!   test. `\n` in a value is therefore untested by this corpus.
//! * **`TTL` and `writetime` are not part of the corpus.** They are per-cell metadata, not
//!   values; parity for them is `TST-003`'s `smoke/03_ttl_writetime`, not this harness's.

use std::fmt;
use std::fmt::Write as _;

use cdm_codec::{CqlTypeInfo, UdtField};
use cdm_core::{CdmError, ErrorKind};

use crate::containers::Capabilities;
use crate::data::DataGen;
use crate::schema::{
    create_keyspace_statement, type_slug, ColumnKind, ColumnSpec, TableSpec, UdtSpec,
};
use crate::seed::Seed;

/// The keyspace every differential corpus lives in.
pub const KEYSPACE: &str = "cdm_diff";

/// The table under test: one column of every type the engine supports, nested to depth 3.
pub const ALL_TYPES_TABLE: &str = "all_types";

/// The counter table, which exists because a `counter` may not share a table with anything else.
pub const COUNTER_TABLE: &str = "counters";

/// The checked-in rendering of [`Corpus::schema_statements`], under
/// [`corpus_root`](super::corpus_root).
pub const SCHEMA_FILE: &str = "schema.cql";

/// The checked-in rendering of [`Corpus::coverage`], under [`corpus_root`](super::corpus_root).
pub const COVERAGE_FILE: &str = "coverage.tsv";

/// How many rows of pseudo-random filler a [`CorpusScale`] adds beyond the fixed rows.
///
/// The fixed rows — the edge table, the all-null row and the all-empty row — carry the coverage
/// `TST-020` demands. The filler exists so that the harness compares more than one row per
/// partition and more than one token range, which is where an off-by-one in the splitter or the
/// pager shows up.
const FULL_FILLER_ROWS: usize = 128;
/// As [`FULL_FILLER_ROWS`], for the smoke corpus.
const SMOKE_FILLER_ROWS: usize = 4;

/// How many rows share a partition, so that clustering order is exercised at all.
const ROWS_PER_PARTITION: usize = 4;

/// The stride that decides which cells of a filler row are `NULL`.
///
/// Positional rather than probabilistic: a probability makes "did every column get a null?" a
/// matter of luck, and a corpus whose coverage varies with the seed is a corpus whose failures
/// cannot be reproduced by re-reading it.
const FILLER_NULL_STRIDE: usize = 11;

/// The largest number of elements written into one collection literal.
///
/// Edge values are spread across several rows rather than crammed into one literal: a 40-element
/// map literal is unreadable in a diff, and the whole point of the edge rows is that a human can
/// read one and see what it is testing.
const MAX_LITERAL_ELEMENTS: usize = 3;

/// How much data a corpus holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CorpusScale {
    /// A small corpus for fast iteration: the same schema and the same edge rows, less filler.
    Smoke,
    /// The full corpus the nightly job runs.
    Full,
}

impl CorpusScale {
    /// How many pseudo-random filler rows this scale adds.
    #[must_use]
    pub const fn filler_rows(self) -> usize {
        match self {
            Self::Smoke => SMOKE_FILLER_ROWS,
            Self::Full => FULL_FILLER_ROWS,
        }
    }

    /// The name this scale is reported under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Full => "full",
        }
    }
}

impl fmt::Display for CorpusScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a type reaches the generated corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CoverageStatus {
    /// The type has at least one column in the corpus.
    Covered,
    /// The type is deliberately absent, for the reason in [`CoverageEntry::detail`].
    Gap,
}

impl CoverageStatus {
    /// The word this status is written as in the coverage manifest.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Covered => "covered",
            Self::Gap => "gap",
        }
    }
}

impl fmt::Display for CoverageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One line of the type-coverage matrix (`TST-020`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageEntry {
    cql_type: String,
    status: CoverageStatus,
    detail: String,
}

impl CoverageEntry {
    /// The type, as CQL spells it.
    #[must_use]
    pub fn cql_type(&self) -> &str {
        &self.cql_type
    }

    /// Whether the type is in the corpus.
    #[must_use]
    pub const fn status(&self) -> CoverageStatus {
        self.status
    }

    /// Where it is covered, or why it is not.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// One column of a [`CorpusTable`], with the metadata a comparison `SELECT` needs.
///
/// The comparison engine is deliberately not schema-introspecting: a harness that reads the schema
/// it is comparing derives its expectations from the same place as its observations, and then
/// agrees with itself. So the corpus states what it built, and the comparator selects exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusColumn {
    name: String,
    cql_type: CqlTypeInfo,
    kind: ColumnKind,
    timestamp_eligible: bool,
}

impl CorpusColumn {
    /// The column name, as the DDL spells it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The column's type.
    #[must_use]
    pub const fn cql_type(&self) -> &CqlTypeInfo {
        &self.cql_type
    }

    /// The role it plays in the primary key.
    #[must_use]
    pub const fn kind(&self) -> ColumnKind {
        self.kind
    }

    /// Whether `WRITETIME(col)` and `TTL(col)` may be selected for it as a single `bigint`.
    ///
    /// **Not a gap in the comparison.** A column that is not timestamp-eligible still has its
    /// *value* byte-compared; only its per-cell writetime and TTL go unread, and what goes unread
    /// is either something the server refuses to report or something that is guaranteed to differ
    /// between two independently-executed runs — on both sides equally. Nothing can hide behind it.
    ///
    /// Three categories are excluded, and each was **measured against `cassandra:5.0.9`** rather
    /// than read off the documentation, because the received wisdom about two of them is wrong on
    /// that version. `crates/cdm-testkit/tests/differential_corpus_it.rs` reruns the measurement.
    ///
    /// * **Primary-key columns.** Rejected outright: `Cannot use selection function writetime on
    ///   PRIMARY KEY part pk`. The `SELECT` fails, it does not return null, so asking would break
    ///   the whole snapshot query rather than one column of it.
    /// * **Non-frozen collections and non-frozen UDTs.** *Accepted* on 5.0 — and that is the
    ///   problem. They are multi-cell, so the answer is a `list<bigint>` with one entry per
    ///   element (`writetime(c_map_text_int)` → `[1786…, 1786…, 1786…]`): a different *type* from
    ///   the `bigint` every other column returns, and a length that is a function of the value.
    ///   Before Cassandra 4.1 the same query is rejected outright. Excluding them keeps the
    ///   snapshot one uniform shape and keeps this metadata independent of the image, which is
    ///   what makes a comparison reproducible.
    /// * **Counter columns** (`MIG-030`). Also *accepted* on 5.0: `WRITETIME` returns the
    ///   coordinator's clock at the moment of the increment and `TTL` returns null. Comparing that
    ///   between two independently-executed runs would compare two wall clocks and always differ,
    ///   because a counter update cannot carry `USING TIMESTAMP` — which is the same reason cdm-rs
    ///   cannot preserve a counter's writetime across a migration in the first place.
    ///
    /// Everything else — including a *frozen* collection, a tuple, a frozen UDT and a vector, all
    /// of which are single-cell — is eligible.
    #[must_use]
    pub const fn timestamp_eligible(&self) -> bool {
        self.timestamp_eligible
    }
}

/// The rule behind [`CorpusColumn::timestamp_eligible`], which documents it.
fn timestamp_eligible(column: &ColumnSpec) -> bool {
    if column.kind().is_key() {
        return false;
    }
    match column.cql_type() {
        CqlTypeInfo::Counter => false,
        CqlTypeInfo::List { frozen, .. }
        | CqlTypeInfo::Set { frozen, .. }
        | CqlTypeInfo::Map { frozen, .. }
        | CqlTypeInfo::Udt { frozen, .. } => *frozen,
        _ => true,
    }
}

/// One table of a corpus, and the statements that populate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusTable {
    spec: TableSpec,
    writes: Vec<String>,
    columns: Vec<CorpusColumn>,
}

impl CorpusTable {
    /// Describes `spec`, populated by `writes`, deriving the per-column comparison metadata.
    fn new(spec: TableSpec, writes: Vec<String>) -> Self {
        let columns = spec
            .columns()
            .iter()
            .map(|column| CorpusColumn {
                name: column.name().to_owned(),
                cql_type: column.cql_type().clone(),
                kind: column.kind(),
                timestamp_eligible: timestamp_eligible(column),
            })
            .collect();
        Self {
            spec,
            writes,
            columns,
        }
    }

    /// The table's shape.
    #[must_use]
    pub const fn spec(&self) -> &TableSpec {
        &self.spec
    }

    /// Every column, in declaration order.
    #[must_use]
    pub fn columns(&self) -> &[CorpusColumn] {
        &self.columns
    }

    /// The primary-key columns, partition key first and then clustering, in key order.
    ///
    /// The comparison engine builds its `SELECT` from this rather than from
    /// `system_schema`: a harness that introspects the schema it is comparing is a harness that
    /// agrees with itself.
    #[must_use]
    pub fn key_columns(&self) -> Vec<&CorpusColumn> {
        let mut keys: Vec<&CorpusColumn> = self
            .columns
            .iter()
            .filter(|column| column.kind == ColumnKind::Partition)
            .collect();
        keys.extend(
            self.columns
                .iter()
                .filter(|column| column.kind == ColumnKind::Clustering),
        );
        keys
    }

    /// The non-key columns, in declaration order — the ones a comparison reads values from.
    #[must_use]
    pub fn value_columns(&self) -> Vec<&CorpusColumn> {
        self.columns
            .iter()
            .filter(|column| !column.kind.is_key())
            .collect()
    }

    /// `keyspace.table`.
    #[must_use]
    pub fn qualified_name(&self) -> String {
        self.spec.qualified_name()
    }

    /// The statements that populate it: `INSERT`s, or `UPDATE`s for the counter table
    /// (`MIG-030`).
    #[must_use]
    pub fn write_statements(&self) -> &[String] {
        &self.writes
    }

    /// How many rows those statements write. One statement writes one row, by construction.
    #[must_use]
    pub fn row_count(&self) -> u64 {
        u64::try_from(self.writes.len()).unwrap_or(u64::MAX)
    }

    /// Whether this is the counter table.
    #[must_use]
    pub fn is_counter_table(&self) -> bool {
        self.spec.is_counter_table()
    }
}

/// One differential corpus: a schema plus deterministic rows (`TST-020`).
///
/// ```
/// use cdm_testkit::differential::Corpus;
/// use cdm_testkit::Seed;
///
/// let corpus = Corpus::smoke(Seed::new(7))?;
/// assert_eq!(corpus.table(), "cdm_diff.all_types");
/// assert_eq!(corpus, Corpus::smoke(Seed::new(7))?);
/// assert!(corpus.row_count() > 0);
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    seed: Seed,
    scale: CorpusScale,
    capabilities: Capabilities,
    schema_statements: Vec<String>,
    tables: Vec<CorpusTable>,
    coverage: Vec<CoverageEntry>,
    table_name: String,
}

impl Corpus {
    /// The full corpus: every CQL type the engine supports, nesting depth 3, nulls, empty
    /// collections and the edge values `TST-020` enumerates. `seed` makes it reproducible.
    ///
    /// Built with [`Capabilities::portable`] — the set every supported open-source image accepts —
    /// because the differential job runs against stock Cassandra. See the module docs for what
    /// that leaves out, and [`Corpus::with_capabilities`] for how to put it back.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the generated schema is not one a cluster would accept, and
    /// [`ErrorKind::TypeConversion`] if a type in it has no literal syntax. Both are bugs in this
    /// module rather than caller errors, and both are caught by its own tests.
    pub fn full(seed: Seed) -> Result<Self, CdmError> {
        Self::with_capabilities(seed, CorpusScale::Full, Capabilities::portable())
    }

    /// A small corpus for fast iteration; same shape, fewer rows.
    ///
    /// "Same shape" is exact: identical keyspace, identical DDL and the identical fixed rows. Only
    /// the pseudo-random filler shrinks, so a bug the smoke corpus cannot see is a bug that needs
    /// more than one row per partition.
    ///
    /// # Errors
    ///
    /// As [`Corpus::full`].
    pub fn smoke(seed: Seed) -> Result<Self, CdmError> {
        Self::with_capabilities(seed, CorpusScale::Smoke, Capabilities::portable())
    }

    /// A corpus for an engine with the given capabilities.
    ///
    /// # Errors
    ///
    /// As [`Corpus::full`].
    pub fn with_capabilities(
        seed: Seed,
        scale: CorpusScale,
        capabilities: Capabilities,
    ) -> Result<Self, CdmError> {
        let udts = udt_specs();
        let all_types = all_types_table(&udts, capabilities)?;
        let counters = counter_table()?;

        let mut schema_statements = vec![create_keyspace_statement(KEYSPACE)];
        for udt in &udts {
            schema_statements.push(udt.create_statement(KEYSPACE));
        }
        schema_statements.push(all_types.create_table_statement());
        schema_statements.push(counters.create_table_statement());

        let all_types_writes = all_types_rows(&all_types, seed, scale)?;
        let counter_writes = counter_rows(&counters, scale);
        let coverage = coverage_matrix(&all_types, &counters, capabilities);

        Ok(Self {
            seed,
            scale,
            capabilities,
            schema_statements,
            table_name: all_types.qualified_name(),
            tables: vec![
                CorpusTable::new(all_types, all_types_writes),
                CorpusTable::new(counters, counter_writes),
            ],
            coverage,
        })
    }

    /// `CREATE TYPE` / `CREATE TABLE` statements, in dependency order.
    ///
    /// Includes the `CREATE KEYSPACE` that must precede them, and covers **both** tables: the DDL
    /// is shared setup, so a harness that applies this list has somewhere to put the counter rows
    /// as well.
    #[must_use]
    pub fn schema_statements(&self) -> &[String] {
        &self.schema_statements
    }

    /// `INSERT` statements that populate origin.
    ///
    /// These populate [`Corpus::table`] only. The counter table is written with `UPDATE`, not
    /// `INSERT` (`MIG-030`), and its statements are [`CorpusTable::write_statements`] on
    /// [`Corpus::counter_table`].
    #[must_use]
    pub fn insert_statements(&self) -> &[String] {
        self.primary().map_or(&[], CorpusTable::write_statements)
    }

    /// The keyspace-qualified table under test.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table_name
    }

    /// Rows written, for the counter-block assertion.
    ///
    /// Rows of [`Corpus::table`]. The counter table's own row count is
    /// [`CorpusTable::row_count`] on [`Corpus::counter_table`], because the two migrate as two
    /// jobs and produce two counter blocks.
    #[must_use]
    pub fn row_count(&self) -> u64 {
        self.primary().map_or(0, CorpusTable::row_count)
    }

    /// Every table in the corpus, the table under test first.
    #[must_use]
    pub fn tables(&self) -> &[CorpusTable] {
        &self.tables
    }

    /// The counter table (`MIG-030`), which cannot share a table with anything else.
    #[must_use]
    pub fn counter_table(&self) -> Option<&CorpusTable> {
        self.tables.iter().find(|table| table.is_counter_table())
    }

    /// How deeply the deepest column of [`Corpus::table`] nests: a primitive is 0,
    /// `map<text, frozen<list<frozen<set<int>>>>>` is 3.
    ///
    /// `TST-020` asks for depth 3, and a claim a harness makes about its own data is worth being
    /// able to read off rather than infer from the DDL.
    #[must_use]
    pub fn max_nesting_depth(&self) -> usize {
        self.primary().map_or(0, |table| {
            table
                .spec
                .columns()
                .iter()
                .map(|column| nesting_depth(column.cql_type()))
                .max()
                .unwrap_or(0)
        })
    }

    /// The type-coverage matrix: what is covered, and what is not and why (`TST-020`).
    #[must_use]
    pub fn coverage(&self) -> &[CoverageEntry] {
        &self.coverage
    }

    /// The keyspace.
    #[must_use]
    pub fn keyspace(&self) -> &str {
        KEYSPACE
    }

    /// The seed this corpus was built from, for a failure message (`TST-101`).
    #[must_use]
    pub const fn seed(&self) -> Seed {
        self.seed
    }

    /// How much data it holds.
    #[must_use]
    pub const fn scale(&self) -> CorpusScale {
        self.scale
    }

    /// The engine capabilities it was generated for.
    #[must_use]
    pub const fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// The whole corpus as one CQL script: the schema, then every table's rows.
    ///
    /// This is what loads it into a cluster, through
    /// [`ClusterFixture::exec_cql`](crate::ClusterFixture::exec_cql) or any `cqlsh`. Every
    /// statement is on one line and terminated with `;`, because the loader flattens a script to
    /// the single line `cqlsh -e` accepts.
    #[must_use]
    pub fn load_script(&self) -> String {
        let mut script = String::new();
        for statement in &self.schema_statements {
            let _ = writeln!(script, "{statement};");
        }
        for table in &self.tables {
            for statement in &table.writes {
                let _ = writeln!(script, "{statement};");
            }
        }
        script
    }

    /// Just the schema, as a CQL script — the checked-in `tests/differential/schema.cql`.
    #[must_use]
    pub fn schema_script(&self) -> String {
        let mut script = String::new();
        for statement in &self.schema_statements {
            let _ = writeln!(script, "{statement};");
        }
        script
    }

    /// The coverage matrix as the tab-separated `tests/differential/coverage.tsv`.
    ///
    /// A checked-in rendering rather than a second source of truth: a gap that appears or
    /// disappears shows up in a diff, where a reviewer sees it.
    #[must_use]
    pub fn coverage_manifest(&self) -> String {
        let mut out = String::from("status\ttype\tdetail\n");
        for entry in &self.coverage {
            let _ = writeln!(
                out,
                "{}\t{}\t{}",
                entry.status, entry.cql_type, entry.detail
            );
        }
        out
    }

    /// The table under test.
    fn primary(&self) -> Option<&CorpusTable> {
        self.tables.first()
    }
}

/// The UDTs the corpus declares, in dependency order.
///
/// Three of them rather than one, because `TST-020`'s "nesting depth 3" is not only about
/// collections: `diff_person` holds a collection of `diff_contact`, which holds a `diff_address`,
/// which is a UDT inside a collection inside a UDT — the shape `CDC-013`'s recursive conversion
/// planner exists for, and the one a flat `frozen<address>` never reaches.
fn udt_specs() -> Vec<UdtSpec> {
    let address = UdtSpec::new(
        "diff_address",
        vec![
            UdtField::new("street", CqlTypeInfo::Text),
            UdtField::new("zip", CqlTypeInfo::Int),
        ],
    );
    let contact = UdtSpec::new(
        "diff_contact",
        vec![
            UdtField::new("label", CqlTypeInfo::Text),
            UdtField::new("address", address.frozen_type()),
            UdtField::new(
                "aliases",
                CqlTypeInfo::Set {
                    element: Box::new(CqlTypeInfo::Text),
                    frozen: true,
                },
            ),
        ],
    );
    let person = UdtSpec::new(
        "diff_person",
        vec![
            UdtField::new("name", CqlTypeInfo::Text),
            UdtField::new(
                "contacts",
                CqlTypeInfo::List {
                    element: Box::new(contact.frozen_type()),
                    frozen: true,
                },
            ),
        ],
    );
    vec![address, contact, person]
}

/// The structural types the corpus carries, beyond the primitives.
///
/// Every entry says which depth it reaches, because "nesting depth 3" is a claim this list has to
/// make good on and [`nesting_depth`] is what checks it.
fn structural_types(udts: &[UdtSpec], capabilities: Capabilities) -> Vec<CqlTypeInfo> {
    let address = udts.first().map(UdtSpec::frozen_type);
    let person = udts.get(2).map(UdtSpec::frozen_type);

    let mut types = vec![
        // Depth 1: the flat collections.
        CqlTypeInfo::List {
            element: Box::new(CqlTypeInfo::Int),
            frozen: false,
        },
        CqlTypeInfo::Set {
            element: Box::new(CqlTypeInfo::Text),
            frozen: false,
        },
        CqlTypeInfo::Map {
            key: Box::new(CqlTypeInfo::Text),
            value: Box::new(CqlTypeInfo::Int),
            frozen: false,
        },
        CqlTypeInfo::Tuple {
            elements: vec![CqlTypeInfo::Int, CqlTypeInfo::Text],
        },
        // The frozen twins of two of the above. An empty *non-frozen* collection reads back as
        // NULL and an empty *frozen* one reads back as `[]`; a corpus with only one of the two
        // cannot tell a comparison engine that conflates them from one that does not (`MIG-012`).
        CqlTypeInfo::List {
            element: Box::new(CqlTypeInfo::Int),
            frozen: true,
        },
        CqlTypeInfo::Map {
            key: Box::new(CqlTypeInfo::Text),
            value: Box::new(CqlTypeInfo::Int),
            frozen: true,
        },
        // Depth 2.
        CqlTypeInfo::List {
            element: Box::new(CqlTypeInfo::Set {
                element: Box::new(CqlTypeInfo::Int),
                frozen: true,
            }),
            frozen: false,
        },
        CqlTypeInfo::Set {
            element: Box::new(CqlTypeInfo::Tuple {
                elements: vec![CqlTypeInfo::Int, CqlTypeInfo::Text],
            }),
            frozen: false,
        },
        // Depth 3, spelled exactly as `TST-020`'s discussion spells it.
        CqlTypeInfo::Map {
            key: Box::new(CqlTypeInfo::Text),
            value: Box::new(CqlTypeInfo::List {
                element: Box::new(CqlTypeInfo::Set {
                    element: Box::new(CqlTypeInfo::Int),
                    frozen: true,
                }),
                frozen: true,
            }),
            frozen: false,
        },
        // Depth 3 through a tuple rather than a map, because a tuple's components are positional
        // and a map's are not, and the two decode by different paths.
        CqlTypeInfo::Tuple {
            elements: vec![
                CqlTypeInfo::Int,
                CqlTypeInfo::List {
                    element: Box::new(CqlTypeInfo::Map {
                        key: Box::new(CqlTypeInfo::Text),
                        value: Box::new(CqlTypeInfo::Int),
                        frozen: true,
                    }),
                    frozen: true,
                },
            ],
        },
    ];

    if let Some(address) = address {
        // Depth 1 and 2 through a UDT.
        types.push(address.clone());
        types.push(CqlTypeInfo::List {
            element: Box::new(address),
            frozen: false,
        });
    }
    if let Some(person) = person {
        // UDT → collection → UDT → UDT: depth 4, and the only shape in the corpus that puts a
        // user-defined type inside a collection inside another user-defined type.
        types.push(person);
    }

    if capabilities.vectors {
        types.push(CqlTypeInfo::Vector {
            element: Box::new(CqlTypeInfo::Float),
            dimensions: 3,
        });
    }

    types
}

/// Every type that gets a column, in a stable order: primitives first, then structures.
fn corpus_types(udts: &[UdtSpec], capabilities: Capabilities) -> Vec<CqlTypeInfo> {
    let mut types: Vec<CqlTypeInfo> = CqlTypeInfo::PRIMITIVES
        .iter()
        .filter(|cql_type| match cql_type {
            // Its own table; see the module docs.
            CqlTypeInfo::Counter => false,
            CqlTypeInfo::Duration => capabilities.duration,
            _ => true,
        })
        .cloned()
        .collect();

    if capabilities.dse_geometry {
        types.extend([
            CqlTypeInfo::Point,
            CqlTypeInfo::LineString,
            CqlTypeInfo::Polygon,
        ]);
    }
    if capabilities.date_range {
        types.push(CqlTypeInfo::DateRange);
    }

    types.extend(structural_types(udts, capabilities));
    types
}

/// The column name a type gets, e.g. `c_map_text_frozen_list_frozen_set_int`.
fn column_name(cql_type: &CqlTypeInfo) -> String {
    format!("c_{}", type_slug(cql_type))
}

/// The table under test.
fn all_types_table(udts: &[UdtSpec], capabilities: Capabilities) -> Result<TableSpec, CdmError> {
    let mut builder = TableSpec::builder(KEYSPACE, ALL_TYPES_TABLE);
    for udt in udts {
        builder = builder.udt(udt.clone());
    }
    builder = builder
        .partition_key("pk", CqlTypeInfo::Text)
        .clustering_key("ck", CqlTypeInfo::Int)
        // A second, textual clustering column so that an empty string appears in a *key* position
        // and not only in a value. Cassandra permits an empty clustering value; it rejects only an
        // empty partition key, which is why `pk` never gets one.
        .clustering_key("ck_text", CqlTypeInfo::Text);

    for cql_type in corpus_types(udts, capabilities) {
        builder = builder.column(column_name(&cql_type), cql_type);
    }
    builder.build()
}

/// The counter table (`MIG-030`).
fn counter_table() -> Result<TableSpec, CdmError> {
    TableSpec::builder(KEYSPACE, COUNTER_TABLE)
        .partition_key("pk", CqlTypeInfo::Text)
        .clustering_key("ck", CqlTypeInfo::Int)
        .column("c_hits", CqlTypeInfo::Counter)
        .column("c_misses", CqlTypeInfo::Counter)
        .build()
}

/// How deeply a type nests: a primitive is 0, `map<text, frozen<list<frozen<set<int>>>>>` is 3.
fn nesting_depth(cql_type: &CqlTypeInfo) -> usize {
    if cql_type.is_primitive() {
        return 0;
    }
    1 + cql_type
        .element_types()
        .into_iter()
        .map(nesting_depth)
        .max()
        .unwrap_or(0)
}

/// The edge-case literals for a type, in a fixed order.
///
/// This table is the corpus. Everything else here is plumbing: rows, keys, statement rendering.
/// Each list is the set of values that are interesting *because of what the type is* — the ends of
/// its range, the values with a special encoding, the ones a hand-written parser gets wrong — and
/// none of them is reachable by a uniform draw.
///
/// # Errors
///
/// [`ErrorKind::TypeConversion`] for a type with no literal syntax: [`CqlTypeInfo::Custom`], and a
/// UDT whose fields were not resolved. A guess a cluster rejects is worse than an error saying
/// why.
// One arm per CQL type, and the value of the table is that the whole taxonomy is readable in one
// place. Splitting it to satisfy a line count would scatter it.
#[allow(clippy::too_many_lines)]
fn edge_literals(cql_type: &CqlTypeInfo) -> Result<Vec<String>, CdmError> {
    let literals = match cql_type {
        CqlTypeInfo::Ascii => vec![
            "''".to_owned(),
            "' '".to_owned(),
            "'~'".to_owned(),
            "'it''s ascii'".to_owned(),
            "'a--b'".to_owned(),
        ],
        CqlTypeInfo::Text => vec![
            "''".to_owned(),
            // Two-byte, three-byte and four-byte UTF-8, then a combining mark (which is one
            // grapheme in two code points), then a regional-indicator pair (one grapheme in two
            // astral code points). Length in bytes, code points and graphemes is different for
            // every one of these, which is precisely what a text codec gets wrong.
            "'ünïcödé ß'".to_owned(),
            "'日本語'".to_owned(),
            "'🦀𝄞'".to_owned(),
            "'e\u{301}gu\u{308}'".to_owned(),
            "'🇯🇵'".to_owned(),
            "'it''s a -- not a comment'".to_owned(),
        ],
        CqlTypeInfo::Blob => vec![
            // An empty blob is a value and is not a null (`MIG-012`).
            "0x".to_owned(),
            "0x00".to_owned(),
            "0xff".to_owned(),
            "0x000102030405060708090a0b0c0d0e0f".to_owned(),
        ],
        CqlTypeInfo::Boolean => vec!["true".to_owned(), "false".to_owned()],
        CqlTypeInfo::TinyInt => vec![
            i8::MIN.to_string(),
            i8::MAX.to_string(),
            "0".to_owned(),
            "-1".to_owned(),
        ],
        CqlTypeInfo::SmallInt => vec![
            i16::MIN.to_string(),
            i16::MAX.to_string(),
            "0".to_owned(),
            "-1".to_owned(),
        ],
        CqlTypeInfo::Int => vec![
            i32::MIN.to_string(),
            i32::MAX.to_string(),
            "0".to_owned(),
            "-1".to_owned(),
        ],
        CqlTypeInfo::BigInt => vec![
            i64::MIN.to_string(),
            i64::MAX.to_string(),
            "0".to_owned(),
            "-1".to_owned(),
        ],
        CqlTypeInfo::VarInt => vec![
            "0".to_owned(),
            i64::MIN.to_string(),
            i64::MAX.to_string(),
            // Beyond 64 bits in both directions, which is the whole reason `varint` exists and
            // the case a `long`-shaped codec silently truncates.
            i128::MAX.to_string(),
            i128::MIN.to_string(),
            "1".repeat(96),
        ],
        CqlTypeInfo::Decimal => vec![
            "0".to_owned(),
            "-0.0".to_owned(),
            "1.5".to_owned(),
            // An unscaled value beyond 64 bits with a scale beyond a `double`'s precision: the
            // pair that distinguishes a real BigDecimal from a float pretending to be one.
            "123456789012345678901234567890.09876543210987654321098765432109876543210".to_owned(),
            "-123456789012345678901234567890.0000000000000000000000000000001".to_owned(),
            // Trailing zeros are part of a decimal's identity: 1.10 and 1.1 are equal in value and
            // different in scale, and only one of the two survives a round-trip through a double.
            "1.10".to_owned(),
        ],
        // `Infinity`, `-Infinity` and `NaN` are CQL keywords in a float position; `-0.0` is an
        // ordinary literal whose encoding differs from `0.0` in exactly one bit, which is the bit
        // a comparison that uses `==` cannot see.
        CqlTypeInfo::Float => vec![
            "0.0".to_owned(),
            "-0.0".to_owned(),
            "Infinity".to_owned(),
            "-Infinity".to_owned(),
            "NaN".to_owned(),
            "3.4028235E38".to_owned(),
            "1.4E-45".to_owned(),
        ],
        CqlTypeInfo::Double => vec![
            "0.0".to_owned(),
            "-0.0".to_owned(),
            "Infinity".to_owned(),
            "-Infinity".to_owned(),
            "NaN".to_owned(),
            "1.7976931348623157E308".to_owned(),
            "4.9E-324".to_owned(),
        ],
        CqlTypeInfo::Inet => vec![
            "'0.0.0.0'".to_owned(),
            "'255.255.255.255'".to_owned(),
            "'127.0.0.1'".to_owned(),
            // An IPv6 address is 16 bytes where an IPv4 one is 4, in the same column.
            "'::'".to_owned(),
            "'::1'".to_owned(),
            "'2001:db8::ff00:42:8329'".to_owned(),
        ],
        CqlTypeInfo::Uuid => vec![
            "00000000-0000-0000-0000-000000000000".to_owned(),
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned(),
            "123e4567-e89b-42d3-a456-426614174000".to_owned(),
        ],
        // A `timeuuid` column validates the version nibble, so every one of these is a real
        // version 1: the two ends of the ordering Cassandra's `minTimeuuid`/`maxTimeuuid` produce,
        // and one ordinary value.
        CqlTypeInfo::TimeUuid => vec![
            "00000000-0000-1000-8080-808080808080".to_owned(),
            "ffffffff-ffff-1fff-bf7f-7f7f7f7f7f7f".to_owned(),
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_owned(),
        ],
        CqlTypeInfo::Date => vec![
            // The epoch and the day before it: `date` is a day count biased by 2^31, so the sign
            // of the *unbiased* value flips here and an implementation that forgot the bias is
            // wrong by 2^31 days on exactly one side.
            "'1970-01-01'".to_owned(),
            "'1969-12-31'".to_owned(),
            // The Gregorian reform. Not a boundary in Cassandra's proleptic count, and a
            // discontinuity in Java's `GregorianCalendar`; see the module docs.
            "'1582-10-04'".to_owned(),
            "'1582-10-15'".to_owned(),
            "'0001-01-01'".to_owned(),
            "'9999-12-31'".to_owned(),
        ],
        CqlTypeInfo::Time => vec![
            "'00:00:00.000000000'".to_owned(),
            "'23:59:59.999999999'".to_owned(),
            // Nanosecond precision that no millisecond-shaped clock can hold.
            "'12:00:00.000000001'".to_owned(),
        ],
        CqlTypeInfo::Timestamp => vec![
            "'1970-01-01T00:00:00.000Z'".to_owned(),
            // Pre-1970, i.e. a negative epoch millisecond count.
            "'1969-12-31T23:59:59.999Z'".to_owned(),
            "'1900-01-01T00:00:00.000Z'".to_owned(),
            "'1582-10-15T00:00:00.000Z'".to_owned(),
            // The 32-bit `time_t` boundary, which is not a boundary for a 64-bit millisecond
            // count and is one for everything that ever truncated to seconds.
            "'2038-01-19T03:14:08.000Z'".to_owned(),
            "'9999-12-31T23:59:59.999Z'".to_owned(),
        ],
        // Months, days and nanoseconds are three independent signed components, not one duration:
        // `1mo` is not 30 days, and a `duration` that normalised them would be a different value.
        CqlTypeInfo::Duration => vec![
            "0ns".to_owned(),
            "1mo2d3ns".to_owned(),
            "-1mo2d3ns".to_owned(),
            "89h4m48s".to_owned(),
            "P3Y6M4DT12H30M5S".to_owned(),
        ],
        // A counter is never written as a literal in an INSERT; its edge values are the deltas in
        // `counter_rows`, which is the only way CQL lets a counter be written at all (`MIG-030`).
        CqlTypeInfo::Counter => vec!["0".to_owned()],
        CqlTypeInfo::Point => vec![
            "'POINT (0.0 0.0)'".to_owned(),
            "'POINT (1.5 -2.5)'".to_owned(),
        ],
        CqlTypeInfo::LineString => vec!["'LINESTRING (0.0 0.0, 1.0 1.0)'".to_owned()],
        CqlTypeInfo::Polygon => {
            vec!["'POLYGON ((0.0 0.0, 1.0 0.0, 1.0 1.0, 0.0 0.0))'".to_owned()]
        }
        CqlTypeInfo::DateRange => vec![
            "'[1970-01-01 TO 1970-12-31]'".to_owned(),
            "'[* TO *]'".to_owned(),
        ],
        CqlTypeInfo::List { element, .. } => wrap_groups(&edge_literals(element)?, "[", "]", ", "),
        CqlTypeInfo::Set { element, .. } => {
            // Distinct element literals stay distinct as set elements for every element type the
            // corpus puts in a set (`int`, `text`, `tuple`); a float would not, because `-0.0` and
            // `0.0` compare equal, which is why no set here holds one.
            wrap_groups(&edge_literals(element)?, "{", "}", ", ")
        }
        CqlTypeInfo::Map { key, value, .. } => {
            let keys = edge_literals(key)?;
            let values = edge_literals(value)?;
            let entries: Vec<String> = keys
                .iter()
                .enumerate()
                .map(|(index, key)| {
                    let value = values.get(index % values.len().max(1));
                    format!("{key}: {}", value.map_or("null", String::as_str))
                })
                .collect();
            wrap_groups(&entries, "{", "}", ", ")
        }
        CqlTypeInfo::Tuple { elements } => {
            let per_element: Vec<Vec<String>> = elements
                .iter()
                .map(edge_literals)
                .collect::<Result<_, _>>()?;
            positional(&per_element, |values| format!("({})", values.join(", ")))
        }
        CqlTypeInfo::Udt { name, fields, .. } => {
            if fields.is_empty() {
                return Err(CdmError::new(
                    ErrorKind::TypeConversion,
                    format!(
                        "cannot build corpus literals for UDT `{name}`: its fields are unknown, \
                         so resolve it with a UdtResolver first (CDC-014)"
                    ),
                ));
            }
            let per_field: Vec<Vec<String>> = fields
                .iter()
                .map(|field| edge_literals(&field.cql_type))
                .collect::<Result<_, _>>()?;
            positional(&per_field, |values| {
                let rendered: Vec<String> = fields
                    .iter()
                    .zip(values)
                    .map(|(field, value)| format!("{}: {value}", field.name))
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            })
        }
        CqlTypeInfo::Vector {
            element,
            dimensions,
        } => {
            let elements = edge_literals(element)?;
            let mut out = Vec::new();
            for start in (0..elements.len()).step_by(*dimensions.max(&1)) {
                let items: Vec<&str> = (0..*dimensions)
                    .filter_map(|offset| elements.get((start + offset) % elements.len().max(1)))
                    .map(String::as_str)
                    .collect();
                out.push(format!("[{}]", items.join(", ")));
            }
            out
        }
        CqlTypeInfo::Custom(name) => {
            return Err(CdmError::new(
                ErrorKind::TypeConversion,
                format!("cannot build corpus literals for the custom type `{name}`"),
            ))
        }
        // `CqlTypeInfo` is `#[non_exhaustive]`: a type added to `cdm-codec` must fail loudly here
        // rather than silently never appear in the corpus, which is the exact failure this whole
        // module exists to prevent.
        other => {
            return Err(CdmError::new(
                ErrorKind::TypeConversion,
                format!(
                    "the differential corpus has no edge values for `{other}`; add them (TST-020)"
                ),
            ))
        }
    };

    if literals.is_empty() {
        return Err(CdmError::new(
            ErrorKind::Internal,
            format!("the differential corpus produced no literal for `{cql_type}`"),
        ));
    }
    Ok(literals)
}

/// Splits `items` into groups of at most [`MAX_LITERAL_ELEMENTS`] and wraps each in a collection
/// literal, so that every element edge value appears in some row.
fn wrap_groups(items: &[String], open: &str, close: &str, separator: &str) -> Vec<String> {
    items
        .chunks(MAX_LITERAL_ELEMENTS.max(1))
        .map(|chunk| format!("{open}{}{close}", chunk.join(separator)))
        .collect()
}

/// Builds one literal per index across several positional element lists, cycling the shorter ones.
///
/// A tuple or a UDT has no way to vary one component at a time within a single literal, so the
/// variation happens across rows: literal `i` takes element `i` of every component's edge list,
/// wrapping. The number of literals is the longest list, so no component's edge values are lost.
fn positional(per_position: &[Vec<String>], render: impl Fn(Vec<&str>) -> String) -> Vec<String> {
    let count = per_position.iter().map(Vec::len).max().unwrap_or(0);
    (0..count)
        .map(|index| {
            let values: Vec<&str> = per_position
                .iter()
                .map(|values| {
                    values
                        .get(index % values.len().max(1))
                        .map_or("null", String::as_str)
                })
                .collect();
            render(values)
        })
        .collect()
}

/// The literal for the *empty* value of a type, where the type has one.
///
/// `None` means the type has no empty form, and the empty row writes `NULL` there instead. That is
/// the distinction `MIG-012` turns on: `''`, `0x`, `[]` and `{}` are values, and `NULL` is the
/// absence of one, and binding the first as the second writes a tombstone.
fn empty_literal(cql_type: &CqlTypeInfo) -> Option<String> {
    match cql_type {
        CqlTypeInfo::Ascii | CqlTypeInfo::Text => Some("''".to_owned()),
        CqlTypeInfo::Blob => Some("0x".to_owned()),
        CqlTypeInfo::List { .. } => Some("[]".to_owned()),
        CqlTypeInfo::Set { .. } | CqlTypeInfo::Map { .. } => Some("{}".to_owned()),
        // A tuple and a UDT have no empty form, but they do have an all-null one, which is a
        // different value from a null tuple and is the thing a naive encoder collapses.
        CqlTypeInfo::Tuple { elements } => {
            Some(format!("({})", vec!["null"; elements.len()].join(", ")))
        }
        CqlTypeInfo::Udt { fields, .. } => Some(format!(
            "{{{}}}",
            fields
                .iter()
                .map(|field| format!("{}: null", field.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        // A vector has a fixed number of dimensions, so there is no empty one.
        _ => None,
    }
}

/// The rows of the table under test: the edge rows, the null row, the empty row, then filler.
fn all_types_rows(
    table: &TableSpec,
    seed: Seed,
    scale: CorpusScale,
) -> Result<Vec<String>, CdmError> {
    let value_columns: Vec<&ColumnSpec> = table
        .columns()
        .iter()
        .filter(|column| !column.kind().is_key())
        .collect();

    let edges: Vec<Vec<String>> = value_columns
        .iter()
        .map(|column| edge_literals(column.cql_type()))
        .collect::<Result<_, _>>()?;
    let edge_row_count = edges.iter().map(Vec::len).max().unwrap_or(0);

    let mut rows: Vec<Vec<String>> = Vec::new();

    // The edge rows. Row `i` takes edge `i` of every column, wrapping, so the corpus has as many
    // edge rows as the widest edge list and no column's values are lost.
    for index in 0..edge_row_count {
        rows.push(
            edges
                .iter()
                .map(|column_edges| {
                    column_edges
                        .get(index % column_edges.len().max(1))
                        .cloned()
                        .unwrap_or_else(|| "null".to_owned())
                })
                .collect(),
        );
    }

    // Every non-key column NULL.
    rows.push(vec!["null".to_owned(); value_columns.len()]);

    // Every column that has an empty form set to it.
    rows.push(
        value_columns
            .iter()
            .map(|column| empty_literal(column.cql_type()).unwrap_or_else(|| "null".to_owned()))
            .collect(),
    );

    // Filler. Seeded, so the same seed reproduces it exactly (`TST-101`).
    let mut generator = DataGen::new(seed.derive("differential-corpus"));
    for row_index in 0..scale.filler_rows() {
        let mut values = Vec::with_capacity(value_columns.len());
        for (column_index, column) in value_columns.iter().enumerate() {
            // The literal is drawn whether or not it is used, so that adding a null does not shift
            // every later draw and change the whole corpus.
            let literal = generator.literal(column.cql_type())?;
            let is_null = (row_index + column_index) % FILLER_NULL_STRIDE == 0;
            values.push(if is_null { "null".to_owned() } else { literal });
        }
        rows.push(values);
    }

    let names: Vec<&str> = table.columns().iter().map(ColumnSpec::name).collect();
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(index, values)| render_insert(table, &names, index, &values))
        .collect())
}

/// One `INSERT`, with the key columns derived from the row index.
///
/// Keys come from the index rather than from the generator so that a row can be named in a failure
/// message, and so that partition membership is a property of the corpus rather than of the seed.
fn render_insert(table: &TableSpec, names: &[&str], index: usize, values: &[String]) -> String {
    let mut literals = vec![
        format!("'p{:04}'", index / ROWS_PER_PARTITION),
        (index % ROWS_PER_PARTITION).to_string(),
        // Exactly one row carries an empty string in a clustering position; the rest are distinct
        // so that no two rows collide on the primary key.
        if index == 0 {
            "''".to_owned()
        } else {
            format!("'c{index:05}'")
        },
    ];
    literals.extend(values.iter().cloned());

    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table.qualified_name(),
        names.join(", "),
        literals.join(", ")
    )
}

/// The counter deltas, as (`c_hits`, `c_misses`) pairs.
///
/// `i64::MAX` and `i64::MIN + 1` are both here, on rows of their own: a counter is a signed 64-bit
/// accumulator, and applying both to the same cell would overflow rather than test anything. Each
/// row is written exactly once, because a counter write is not idempotent and a retried one is a
/// wrong answer, not a slow one (`CON-012`).
///
/// **`i64::MIN` is not reachable in one statement, and that is a measured gap, not an oversight.**
/// CQL has no negative integer literal in a counter increment — `c = c + -9223372036854775808` is
/// a syntax error — so a negative delta is written `c = c - 9223372036854775808`, and Cassandra
/// then parses the magnitude as a `bigint` and answers
/// `Unable to make long from '9223372036854775808'`, because 2^63 is one past `Long.MAX_VALUE`.
/// Reaching `i64::MIN` needs two updates to the same cell, which would make one row take two
/// statements and quietly break the one-statement-per-row invariant [`CorpusTable::row_count`]
/// depends on. The corpus stops at `i64::MIN + 1`.
const COUNTER_DELTAS: [(i64, i64); 8] = [
    (0, 0),
    (1, -1),
    (-1, 1),
    (i64::MAX, i64::MIN + 1),
    (i64::MIN + 1, i64::MAX),
    (i64::MAX - 1, 1),
    (2, 3),
    (-9_007_199_254_740_993, 9_007_199_254_740_993),
];

/// The counter table's `UPDATE`s (`MIG-030`).
fn counter_rows(table: &TableSpec, scale: CorpusScale) -> Vec<String> {
    let count = match scale {
        CorpusScale::Smoke => 3,
        CorpusScale::Full => COUNTER_DELTAS.len(),
    };
    COUNTER_DELTAS
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, (hits, misses))| {
            format!(
                "UPDATE {} SET c_hits = {}, c_misses = {} WHERE pk = 'p{:04}' AND ck = {}",
                table.qualified_name(),
                counter_increment("c_hits", *hits),
                counter_increment("c_misses", *misses),
                index / ROWS_PER_PARTITION,
                index % ROWS_PER_PARTITION,
            )
        })
        .collect()
}

/// `c = c + n`, or `c = c - |n|` for a negative delta.
///
/// Spelled with a subtraction rather than `+ -9223372036854775808`, which CQL's grammar does not
/// accept, and whose absolute value does not fit in an `i64` either.
fn counter_increment(column: &str, delta: i64) -> String {
    if delta < 0 {
        let magnitude = i128::from(delta).unsigned_abs();
        format!("{column} - {magnitude}")
    } else {
        format!("{column} + {delta}")
    }
}

/// The type-coverage matrix: every type cdm-rs models, and where the corpus puts it.
fn coverage_matrix(
    all_types: &TableSpec,
    counters: &TableSpec,
    capabilities: Capabilities,
) -> Vec<CoverageEntry> {
    let mut present: Vec<(String, String)> = Vec::new();
    for column in all_types.columns() {
        present.push((
            column.cql_type().to_string(),
            format!("{}.{}", all_types.qualified_name(), column.name()),
        ));
    }
    for column in counters.columns() {
        present.push((
            column.cql_type().to_string(),
            format!("{}.{}", counters.qualified_name(), column.name()),
        ));
    }

    let where_covered = |cql_type: &CqlTypeInfo| -> Option<String> {
        let rendered = cql_type.to_string();
        present
            .iter()
            .find(|(name, _)| *name == rendered)
            .map(|(_, column)| column.clone())
    };

    let mut entries: Vec<CoverageEntry> = Vec::new();
    let mut record = |cql_type: &CqlTypeInfo, gap_reason: &str| {
        let (status, detail) = match where_covered(cql_type) {
            Some(column) => (CoverageStatus::Covered, format!("column {column}")),
            None => (CoverageStatus::Gap, gap_reason.to_owned()),
        };
        entries.push(CoverageEntry {
            cql_type: cql_type.to_string(),
            status,
            detail,
        });
    };

    for primitive in &CqlTypeInfo::PRIMITIVES {
        record(
            primitive,
            match primitive {
                CqlTypeInfo::Duration => {
                    "the engine does not implement `duration` (Capabilities::duration)"
                }
                _ => "not generated; this is a bug in the corpus, not a documented gap",
            },
        );
    }
    for dse in &CqlTypeInfo::DSE_TYPES {
        record(
            dse,
            "DSE only: no open-source Cassandra or ScyllaDB image implements it, so a schema \
             containing it does not apply. Build with Capabilities::dse_geometry / date_range.",
        );
    }

    let udts = udt_specs();
    for structural in structural_types(&udts, Capabilities::maximal()) {
        let reason = if matches!(structural, CqlTypeInfo::Vector { .. }) && !capabilities.vectors {
            "vector<T, N> is open-source Cassandra 5.0 and later only (CDC-004). Build with \
             Capabilities::vectors."
        } else {
            "not generated; this is a bug in the corpus, not a documented gap"
        };
        record(&structural, reason);
    }

    // The two values that are not types but that CQL cannot express, and that a reader of this
    // manifest would otherwise assume were covered.
    entries.push(CoverageEntry {
        cql_type: "text (lone surrogate)".to_owned(),
        status: CoverageStatus::Gap,
        detail: "not expressible: a text column is UTF8Type, an unpaired surrogate is not valid \
                 UTF-8, and Cassandra rejects it. Covered instead by an astral character, a \
                 combining mark and a regional-indicator pair."
            .to_owned(),
    });
    entries.push(CoverageEntry {
        cql_type: "counter (delta i64::MIN)".to_owned(),
        status: CoverageStatus::Gap,
        detail: "not expressible in one statement: a negative counter delta is written \
                 `c = c - 9223372036854775808`, and Cassandra rejects that magnitude with \
                 `Unable to make long from '9223372036854775808'`. The corpus stops at \
                 i64::MIN + 1; see COUNTER_DELTAS."
            .to_owned(),
    });

    entries
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
    use crate::differential::corpus_root;
    use std::collections::BTreeSet;

    fn full() -> Corpus {
        Corpus::full(Seed::new(20)).unwrap()
    }

    /// Every literal written into the table under test, across every row.
    fn all_literals(corpus: &Corpus) -> String {
        corpus.insert_statements().join("\n")
    }

    #[test]
    fn tst_020_the_same_seed_produces_a_byte_identical_corpus() {
        assert_eq!(
            Corpus::full(Seed::new(3)).unwrap(),
            Corpus::full(Seed::new(3)).unwrap()
        );
        assert_eq!(
            Corpus::smoke(Seed::new(3)).unwrap(),
            Corpus::smoke(Seed::new(3)).unwrap()
        );
        assert_eq!(
            Corpus::full(Seed::new(3)).unwrap().load_script(),
            Corpus::full(Seed::new(3)).unwrap().load_script()
        );
        assert_ne!(
            Corpus::full(Seed::new(3)).unwrap(),
            Corpus::full(Seed::new(4)).unwrap()
        );
    }

    #[test]
    fn tst_020_only_the_filler_differs_between_seeds() {
        // The claim `Corpus::smoke` makes — "same shape, fewer rows" — and the claim the edge
        // table makes: the interesting rows are fixed, not drawn.
        let a = Corpus::full(Seed::new(1)).unwrap();
        let b = Corpus::full(Seed::new(2)).unwrap();
        assert_eq!(a.schema_statements(), b.schema_statements());
        assert_eq!(a.row_count(), b.row_count());
        let fixed = a.insert_statements().len() - CorpusScale::Full.filler_rows();
        assert_eq!(
            a.insert_statements()[..fixed],
            b.insert_statements()[..fixed]
        );
        assert_ne!(a.insert_statements(), b.insert_statements());

        let smoke = Corpus::smoke(Seed::new(1)).unwrap();
        assert_eq!(smoke.schema_statements(), a.schema_statements());
        assert!(smoke.row_count() < a.row_count());
    }

    #[test]
    fn tst_020_every_cql_type_is_covered_or_named_as_a_gap() {
        let corpus = full();
        let coverage = corpus.coverage();

        for primitive in &CqlTypeInfo::PRIMITIVES {
            let entry = coverage
                .iter()
                .find(|entry| entry.cql_type() == primitive.to_string())
                .unwrap_or_else(|| panic!("{primitive} is missing from the coverage matrix"));
            assert_eq!(
                entry.status(),
                CoverageStatus::Covered,
                "{primitive} is not covered: {}",
                entry.detail()
            );
        }

        // Every gap says why, at length. A one-word reason is not a finding.
        for entry in coverage
            .iter()
            .filter(|entry| entry.status() == CoverageStatus::Gap)
        {
            assert!(
                entry.detail().len() > 40,
                "the gap for `{}` does not explain itself: {}",
                entry.cql_type(),
                entry.detail()
            );
        }

        // And the gaps are exactly the ones the module docs name: the DSE types and vectors, both
        // capability-gated, plus the lone surrogate CQL cannot express.
        let gaps: BTreeSet<&str> = coverage
            .iter()
            .filter(|entry| entry.status() == CoverageStatus::Gap)
            .map(CoverageEntry::cql_type)
            .collect();
        assert_eq!(
            gaps,
            BTreeSet::from([
                "PointType",
                "LineStringType",
                "PolygonType",
                "DateRangeType",
                "vector<float, 3>",
                "text (lone surrogate)",
                "counter (delta i64::MIN)",
            ])
        );
    }

    #[test]
    fn tst_020_the_capability_gated_types_appear_when_the_engine_has_them() {
        let maximal =
            Corpus::with_capabilities(Seed::new(1), CorpusScale::Smoke, Capabilities::maximal())
                .unwrap();
        // With every capability set, the only remaining gaps are the two values CQL itself cannot
        // express — not any type.
        let gaps: BTreeSet<&str> = maximal
            .coverage()
            .iter()
            .filter(|entry| entry.status() == CoverageStatus::Gap)
            .map(CoverageEntry::cql_type)
            .collect();
        assert_eq!(
            gaps,
            BTreeSet::from(["text (lone surrogate)", "counter (delta i64::MIN)"])
        );
        assert!(maximal
            .schema_statements()
            .iter()
            .any(|s| s.contains("vector<float, 3>")));
        assert!(maximal
            .schema_statements()
            .iter()
            .any(|s| s.contains("PointType")));
    }

    #[test]
    fn tst_020_counter_lives_in_its_own_table_and_is_written_with_an_update() {
        let corpus = full();
        let counters = corpus.counter_table().expect("a counter table");

        assert_ne!(counters.qualified_name(), corpus.table());
        assert!(counters.is_counter_table());
        assert!(!corpus.tables().first().unwrap().spec().is_counter_table());

        for statement in counters.write_statements() {
            assert!(
                statement.starts_with("UPDATE cdm_diff.counters SET "),
                "{statement}"
            );
            assert!(!statement.contains("INSERT"), "{statement}");
        }
        // Both ends of the range, each on its own row: the sum of the two would overflow.
        let script = counters.write_statements().join("\n");
        assert!(script.contains("c_hits + 9223372036854775807"), "{script}");
        // `i64::MIN + 1`, not `i64::MIN`: see the note on `COUNTER_DELTAS` for the measured
        // reason, which is that Cassandra rejects the only literal that could express it.
        assert!(script.contains("c_hits - 9223372036854775807"), "{script}");
        assert!(!script.contains("9223372036854775808"), "{script}");
        assert_eq!(
            counters.row_count(),
            u64::try_from(COUNTER_DELTAS.len()).unwrap()
        );
    }

    #[test]
    fn tst_020_the_corpus_nests_to_depth_three_including_a_udt_in_a_collection_in_a_udt() {
        let corpus = full();
        let table = corpus.tables().first().unwrap().spec();
        let deepest = corpus.max_nesting_depth();
        assert!(deepest >= 3, "the corpus only nests to depth {deepest}");

        // The exact shape `TST-020` names.
        assert!(table
            .columns()
            .iter()
            .any(|column| column.cql_type().to_string()
                == "map<text, frozen<list<frozen<set<int>>>>>"));

        // A UDT inside a collection inside a UDT.
        let person = table
            .columns()
            .iter()
            .find(|column| column.cql_type().to_string() == "frozen<diff_person>")
            .expect("the nested UDT column");
        assert!(nesting_depth(person.cql_type()) >= 3);

        assert_eq!(nesting_depth(&CqlTypeInfo::Int), 0);
        assert_eq!(
            nesting_depth(
                &CqlTypeInfo::parse("map<text, frozen<list<frozen<set<int>>>>>").unwrap()
            ),
            3
        );
    }

    #[test]
    fn mig_012_the_corpus_holds_a_null_row_an_empty_row_and_both_frozen_nesses() {
        let corpus = full();
        let table = corpus.tables().first().unwrap().spec();
        let statements = corpus.insert_statements();

        // The null row: every non-key column NULL, and no key column NULL.
        let value_columns = table
            .columns()
            .iter()
            .filter(|c| !c.kind().is_key())
            .count();
        let null_row = statements
            .iter()
            .find(|statement| statement.matches("null").count() >= value_columns)
            .expect("a row of nothing but nulls");
        assert!(!null_row.contains("VALUES (null"), "a key was made null");

        // The empty row: `''`, `0x`, `[]` and `{}` all present in one statement.
        let empty_row = statements
            .iter()
            .find(|statement| statement.contains(", 0x,") && statement.contains(", [],"))
            .expect("a row of empty values");
        assert!(empty_row.contains(", '',"), "{empty_row}");
        assert!(empty_row.contains(", {},"), "{empty_row}");

        // An empty *non-frozen* collection reads back as NULL and an empty *frozen* one does not,
        // so the corpus carries both or it cannot tell the two apart.
        let ddl = table.create_table_statement();
        assert!(ddl.contains("c_list_int list<int>"), "{ddl}");
        assert!(ddl.contains("c_frozen_list_int frozen<list<int>>"), "{ddl}");
    }

    #[test]
    fn tst_020_every_column_is_null_somewhere_and_populated_somewhere() {
        let corpus = full();
        let table = corpus.tables().first().unwrap().spec();
        let statements = corpus.insert_statements();

        // Position within the VALUES list, which is the same as position in the column list.
        for (index, column) in table.columns().iter().enumerate() {
            let values: Vec<&str> = statements
                .iter()
                .filter_map(|statement| split_values(statement).into_iter().nth(index))
                .collect();
            assert_eq!(values.len(), statements.len(), "{}", column.name());
            let nulls = values.iter().filter(|value| **value == "null").count();
            if column.kind().is_key() {
                assert_eq!(nulls, 0, "key column {} was made null", column.name());
            } else {
                assert!(nulls > 0, "{} is never null", column.name());
                assert!(nulls < values.len(), "{} is always null", column.name());
            }
        }
    }

    /// The literals of one `INSERT`, split on the commas that separate them.
    ///
    /// A hand-rolled split rather than a parser: it only has to cope with the statements this
    /// module emits, where the nesting characters are balanced and a comma inside a literal is
    /// either quoted or bracketed.
    fn split_values(statement: &str) -> Vec<&str> {
        let start = match statement.find(") VALUES (") {
            Some(index) => index + ") VALUES (".len(),
            None => return Vec::new(),
        };
        let body = &statement[start..statement.len() - 1];
        let bytes = body.as_bytes();
        let (mut depth, mut quoted, mut begin) = (0_i32, false, 0);
        let mut out = Vec::new();
        for (index, byte) in bytes.iter().enumerate() {
            match byte {
                b'\'' => quoted = !quoted,
                b'[' | b'{' | b'(' if !quoted => depth += 1,
                b']' | b'}' | b')' if !quoted => depth -= 1,
                b',' if !quoted && depth == 0 => {
                    out.push(body[begin..index].trim());
                    begin = index + 1;
                }
                _ => {}
            }
        }
        out.push(body[begin..].trim());
        out
    }

    #[test]
    fn tst_020_the_edge_values_tst_020_names_are_all_present() {
        let corpus = full();
        let literals = all_literals(&corpus);

        for (what, needle) in [
            ("tinyint min", "-128"),
            ("tinyint max", "127"),
            ("smallint min", "-32768"),
            ("smallint max", "32767"),
            ("int min", "-2147483648"),
            ("int max", "2147483647"),
            ("bigint min", "-9223372036854775808"),
            ("bigint max", "9223372036854775807"),
            (
                "varint beyond 64 bits",
                "170141183460469231731687303715884105727",
            ),
            ("epoch", "'1970-01-01'"),
            ("pre-1970 date", "'1969-12-31'"),
            ("Julian/Gregorian cutover", "'1582-10-15'"),
            ("pre-1970 timestamp", "'1969-12-31T23:59:59.999Z'"),
            ("32-bit time_t boundary", "'2038-01-19T03:14:08.000Z'"),
            ("empty string", "''"),
            ("empty blob", "0x"),
            ("multi-byte unicode", "日本語"),
            ("astral unicode", "🦀"),
            ("combining mark", "e\u{301}"),
            ("emoji flag sequence", "🇯🇵"),
            ("positive infinity", "Infinity"),
            ("negative infinity", "-Infinity"),
            ("not a number", "NaN"),
            ("negative zero", "-0.0"),
            (
                "decimal beyond a double",
                "123456789012345678901234567890.098765",
            ),
            ("decimal trailing-zero scale", "1.10"),
            ("ipv6", "'2001:db8::ff00:42:8329'"),
            ("empty collection", "[]"),
            ("empty map", "{}"),
        ] {
            assert!(
                literals.contains(needle),
                "{what} (`{needle}`) is missing from the corpus"
            );
        }
    }

    #[test]
    fn tst_020_every_statement_is_one_line_and_survives_the_cqlsh_loader() {
        // The loader flattens a script line by line and splits it on `;` outside a literal
        // (`crate::sit`), so a statement spanning two lines, or one whose quotes do not balance,
        // is silently cut in half rather than rejected.
        let corpus = full();
        let script = corpus.load_script();
        let flattened = crate::sit::flatten_cql(&script);
        assert_eq!(
            flattened.lines().count(),
            corpus.schema_statements().len()
                + corpus
                    .tables()
                    .iter()
                    .map(|table| table.write_statements().len())
                    .sum::<usize>()
        );
        for line in flattened.lines() {
            assert!(line.ends_with(';'), "{line}");
            assert_eq!(
                line.matches('\'').count() % 2,
                0,
                "unbalanced quotes: {line}"
            );
        }
        // `--` inside a text literal is data, not a comment; the corpus contains one on purpose.
        assert!(flattened.contains("-- not a comment"));
    }

    #[test]
    fn tst_020_the_row_count_is_the_number_of_rows_the_corpus_writes() {
        let corpus = full();
        assert_eq!(
            corpus.row_count(),
            u64::try_from(corpus.insert_statements().len()).unwrap()
        );
        assert!(corpus.row_count() > u64::try_from(CorpusScale::Full.filler_rows()).unwrap());
        assert_eq!(corpus.table(), "cdm_diff.all_types");
        assert_eq!(corpus.keyspace(), "cdm_diff");
        assert_eq!(corpus.seed(), Seed::new(20));
        assert_eq!(corpus.scale(), CorpusScale::Full);
        assert_eq!(corpus.capabilities(), Capabilities::portable());
        assert_eq!(CorpusScale::Smoke.to_string(), "smoke");
        assert_eq!(CoverageStatus::Covered.to_string(), "covered");
        assert_eq!(CoverageStatus::Gap.as_str(), "gap");

        // Every primary key is distinct, or rows would overwrite each other and the count would be
        // a lie — which is exactly the assertion the counter block makes.
        let keys: BTreeSet<Vec<&str>> = corpus
            .insert_statements()
            .iter()
            .map(|statement| split_values(statement).into_iter().take(3).collect())
            .collect();
        assert_eq!(keys.len(), corpus.insert_statements().len());
    }

    #[test]
    fn tst_020_the_corpus_states_its_keys_and_which_columns_carry_a_writetime() {
        // The single integration point with the comparison engine, which builds its `SELECT` from
        // this rather than from `system_schema` (see `CorpusColumn`).
        let corpus = full();
        let table = corpus.tables().first().unwrap();

        let keys: Vec<&str> = table.key_columns().iter().map(|c| c.name()).collect();
        assert_eq!(keys, vec!["pk", "ck", "ck_text"]);
        assert_eq!(
            table.key_columns()[0].kind(),
            ColumnKind::Partition,
            "the partition key comes first"
        );
        assert_eq!(
            table.columns().len(),
            table.key_columns().len() + table.value_columns().len()
        );

        // Cassandra rejects WRITETIME/TTL outright for these three categories, so asking for them
        // does not return null — it fails the whole `SELECT`.
        for column in table.key_columns() {
            assert!(!column.timestamp_eligible(), "{}", column.name());
        }
        for column in table.value_columns() {
            let expected = match column.cql_type() {
                CqlTypeInfo::Counter => false,
                CqlTypeInfo::List { frozen, .. }
                | CqlTypeInfo::Set { frozen, .. }
                | CqlTypeInfo::Map { frozen, .. }
                | CqlTypeInfo::Udt { frozen, .. } => *frozen,
                _ => true,
            };
            assert_eq!(
                column.timestamp_eligible(),
                expected,
                "{} ({})",
                column.name(),
                column.cql_type()
            );
        }

        // The corpus contains a member of every category, or the rule above is untested.
        let value_columns = table.value_columns();
        assert!(value_columns
            .iter()
            .any(|c| !c.timestamp_eligible() && !c.cql_type().is_frozen()));
        assert!(value_columns
            .iter()
            .any(|column| column.timestamp_eligible()));
        let counters = corpus.counter_table().unwrap();
        assert!(counters
            .value_columns()
            .iter()
            .all(|c| !c.timestamp_eligible()));
        assert_eq!(
            counters
                .key_columns()
                .iter()
                .map(|c| c.name())
                .collect::<Vec<_>>(),
            vec!["pk", "ck"]
        );
    }

    #[test]
    fn tst_020_a_type_with_no_literal_syntax_is_an_error_not_a_guess() {
        let err = edge_literals(&CqlTypeInfo::Custom("org.example.Weird".to_owned())).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TypeConversion);

        let err = edge_literals(&CqlTypeInfo::Udt {
            keyspace: None,
            name: "unresolved".to_owned(),
            fields: Vec::new(),
            frozen: true,
        })
        .unwrap_err();
        assert!(err.to_string().contains("CDC-014"), "{err}");
    }

    #[test]
    fn tst_020_the_checked_in_schema_matches_the_generated_one() {
        // `tests/differential/schema.cql` is a rendering, not a second source of truth. It exists
        // so the schema is reviewable in a diff and loadable by a harness with no Rust in it, and
        // this test is what stops the two drifting.
        let path = corpus_root().join(SCHEMA_FILE);
        let recorded = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let generated = full().schema_script();
        assert_eq!(
            recorded,
            generated,
            "{} is stale; replace it with:\n{generated}",
            path.display()
        );
    }

    /// Rewrites `tests/differential/`. Not a test: the regeneration step the two tests above
    /// fail towards.
    ///
    /// Ignored, so `cargo test` never writes to the source tree; run it deliberately with
    /// `cargo test -p cdm-testkit --lib tst_020_rewrite -- --ignored`. It lives here rather than
    /// in `xtask` because the thing it renders is a private function of this module, and a
    /// generator two crates away from its input is a generator that drifts.
    #[test]
    #[ignore = "rewrites tests/differential/; run deliberately after changing the corpus"]
    fn tst_020_rewrite_the_checked_in_renderings() {
        let corpus = full();
        std::fs::write(corpus_root().join(SCHEMA_FILE), corpus.schema_script()).unwrap();
        std::fs::write(
            corpus_root().join(COVERAGE_FILE),
            corpus.coverage_manifest(),
        )
        .unwrap();
    }

    #[test]
    fn tst_020_the_checked_in_coverage_manifest_matches_the_matrix() {
        let path = corpus_root().join(COVERAGE_FILE);
        let recorded = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let generated = full().coverage_manifest();
        assert_eq!(
            recorded,
            generated,
            "{} is stale; replace it with:\n{generated}",
            path.display()
        );
    }
}
