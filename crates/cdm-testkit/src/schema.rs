//! Table builders and the all-types schema generator (`TST-100`).
//!
//! Java CDM's test fixtures are `CommonMocks`, a single class with forty-five fields that every
//! test configures by mutation and that no test can read in isolation. The replacement is
//! compositional: a [`TableSpec`] is an immutable description of one table, built by naming its
//! columns, and everything else — the DDL, the insert statement, the generated data — is a
//! function of that description.
//!
//! # Why the DDL is a string
//!
//! `cdm-testkit` may not depend on the driver (`ARCHITECTURE.md` §3), so it cannot execute
//! anything. It produces the statements and hands them to a [`TestSession`](crate::TestSession),
//! which is the seam a session implementation fills in. That division has a second benefit: the
//! generated DDL is a value, so a unit test can assert on it without a container, and the same
//! assertions hold for the DDL a containerised test then applies.
//!
//! # Capabilities, not versions
//!
//! [`SchemaGen::all_types`] takes a [`Capabilities`] and emits only what that engine will accept.
//! `vector<T, N>` exists on open-source Cassandra 5.0 and later and nowhere else, so a fixture
//! that always emitted it would fail on three quarters of the supported matrix — which is exactly
//! why `TST-100` asks for a capability query rather than a version check at each call site.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use cdm_codec::{CqlTypeInfo, UdtField};
use cdm_core::{CdmError, ErrorKind};

use crate::containers::Capabilities;

/// The role a column plays in the primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColumnKind {
    /// Part of the partition key.
    Partition,
    /// Part of the clustering key.
    Clustering,
    /// A static column, shared by every row of a partition.
    Static,
    /// An ordinary column.
    Regular,
}

impl ColumnKind {
    /// Whether this column is part of the primary key, and therefore may never be null.
    pub const fn is_key(self) -> bool {
        matches!(self, Self::Partition | Self::Clustering)
    }
}

/// One column of a [`TableSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    name: String,
    cql_type: CqlTypeInfo,
    kind: ColumnKind,
}

impl ColumnSpec {
    /// The column name, as the DDL spells it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The column's type.
    pub const fn cql_type(&self) -> &CqlTypeInfo {
        &self.cql_type
    }

    /// The role the column plays in the primary key.
    pub const fn kind(&self) -> ColumnKind {
        self.kind
    }
}

/// A user-defined type a [`TableSpec`] depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdtSpec {
    name: String,
    fields: Vec<UdtField>,
}

impl UdtSpec {
    /// Declares a UDT.
    pub fn new(name: impl Into<String>, fields: Vec<UdtField>) -> Self {
        Self {
            name: name.into(),
            fields,
        }
    }

    /// The type name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The fields, in declaration order.
    pub fn fields(&self) -> &[UdtField] {
        &self.fields
    }

    /// The `CREATE TYPE` statement.
    pub fn create_statement(&self, keyspace: &str) -> String {
        let fields = self
            .fields
            .iter()
            .map(|field| format!("{} {}", field.name, field.cql_type))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "CREATE TYPE IF NOT EXISTS {keyspace}.{} ({fields})",
            self.name
        )
    }

    /// The type as a column type reference, frozen — which is the only way a UDT may be used in a
    /// collection or a primary key, and is always legal elsewhere.
    pub fn frozen_type(&self) -> CqlTypeInfo {
        CqlTypeInfo::Udt {
            keyspace: None,
            name: self.name.clone(),
            fields: self.fields.clone(),
            frozen: true,
        }
    }
}

/// An immutable description of one table (`TST-100`).
///
/// Build one with [`TableSpec::builder`], or generate one with [`SchemaGen`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSpec {
    keyspace: String,
    table: String,
    udts: Vec<UdtSpec>,
    columns: Vec<ColumnSpec>,
}

impl TableSpec {
    /// Starts building a table.
    pub fn builder(keyspace: impl Into<String>, table: impl Into<String>) -> TableSpecBuilder {
        TableSpecBuilder {
            keyspace: keyspace.into(),
            table: table.into(),
            udts: Vec::new(),
            columns: Vec::new(),
        }
    }

    /// The keyspace.
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// The unqualified table name.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// `keyspace.table`, the form every statement uses.
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.keyspace, self.table)
    }

    /// Every column, in declaration order.
    pub fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }

    /// The user-defined types this table's columns refer to.
    pub fn udts(&self) -> &[UdtSpec] {
        &self.udts
    }

    /// The columns of the given kind, in declaration order.
    pub fn columns_of(&self, kind: ColumnKind) -> Vec<&ColumnSpec> {
        self.columns.iter().filter(|c| c.kind == kind).collect()
    }

    /// Whether any column is a `counter`, which makes this a counter table (`MIG-030`).
    ///
    /// Counter tables are their own world: every non-key column must be a counter, they are
    /// written with `UPDATE ... SET c = c + ?` rather than `INSERT`, and the writes are not
    /// idempotent — which is why `CON-012` forbids retrying them.
    pub fn is_counter_table(&self) -> bool {
        self.columns
            .iter()
            .any(|c| c.cql_type == CqlTypeInfo::Counter)
    }

    /// The `CREATE TYPE` statements this table's columns need, in dependency order.
    pub fn create_type_statements(&self) -> Vec<String> {
        self.udts
            .iter()
            .map(|udt| udt.create_statement(&self.keyspace))
            .collect()
    }

    /// The `CREATE TABLE` statement.
    ///
    /// `IF NOT EXISTS`, because a fixture that is applied twice against a reused container must
    /// be a no-op the second time rather than an error.
    pub fn create_table_statement(&self) -> String {
        let mut ddl = format!("CREATE TABLE IF NOT EXISTS {} (", self.qualified_name());
        for column in &self.columns {
            let static_marker = if column.kind == ColumnKind::Static {
                " static"
            } else {
                ""
            };
            // Writing into a String cannot fail; `write!` is used for its formatting only.
            let _ = write!(ddl, "{} {}{static_marker}, ", column.name, column.cql_type);
        }

        let partition = self
            .columns_of(ColumnKind::Partition)
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let clustering = self
            .columns_of(ColumnKind::Clustering)
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>();

        let _ = write!(ddl, "PRIMARY KEY (({partition})");
        for column in &clustering {
            let _ = write!(ddl, ", {column}");
        }
        ddl.push_str("))");
        ddl
    }

    /// The `DROP TABLE` statement, for a fixture that reuses a container between cases.
    pub fn drop_table_statement(&self) -> String {
        format!("DROP TABLE IF EXISTS {}", self.qualified_name())
    }

    /// Every statement needed to materialise this table, in order.
    pub fn create_statements(&self) -> Vec<String> {
        let mut statements = self.create_type_statements();
        statements.push(self.create_table_statement());
        statements
    }

    /// Looks a column up by name.
    pub fn column(&self, name: &str) -> Option<&ColumnSpec> {
        self.columns.iter().find(|c| c.name == name)
    }
}

/// Builds a [`TableSpec`], validating it once at [`TableSpecBuilder::build`].
#[derive(Debug, Clone)]
pub struct TableSpecBuilder {
    keyspace: String,
    table: String,
    udts: Vec<UdtSpec>,
    columns: Vec<ColumnSpec>,
}

impl TableSpecBuilder {
    /// Adds a partition-key column.
    #[must_use]
    pub fn partition_key(self, name: impl Into<String>, cql_type: CqlTypeInfo) -> Self {
        self.column_of(name, cql_type, ColumnKind::Partition)
    }

    /// Adds a clustering column.
    #[must_use]
    pub fn clustering_key(self, name: impl Into<String>, cql_type: CqlTypeInfo) -> Self {
        self.column_of(name, cql_type, ColumnKind::Clustering)
    }

    /// Adds a static column.
    #[must_use]
    pub fn static_column(self, name: impl Into<String>, cql_type: CqlTypeInfo) -> Self {
        self.column_of(name, cql_type, ColumnKind::Static)
    }

    /// Adds an ordinary column.
    #[must_use]
    pub fn column(self, name: impl Into<String>, cql_type: CqlTypeInfo) -> Self {
        self.column_of(name, cql_type, ColumnKind::Regular)
    }

    /// Adds a column with an explicit kind.
    #[must_use]
    pub fn column_of(
        mut self,
        name: impl Into<String>,
        cql_type: CqlTypeInfo,
        kind: ColumnKind,
    ) -> Self {
        self.columns.push(ColumnSpec {
            name: name.into(),
            cql_type,
            kind,
        });
        self
    }

    /// Declares a UDT the table's columns may refer to.
    #[must_use]
    pub fn udt(mut self, udt: UdtSpec) -> Self {
        self.udts.push(udt);
        self
    }

    /// Validates and freezes the table.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the table has no partition key, repeats a column name, uses an
    /// identifier that would need quoting, mixes counter and non-counter columns, or puts a
    /// counter in the primary key. Every one of these is rejected by a real cluster too; catching
    /// them here turns a container round-trip into an immediate, specific message.
    pub fn build(self) -> Result<TableSpec, CdmError> {
        validate_identifier("keyspace", &self.keyspace)?;
        validate_identifier("table", &self.table)?;

        if self.columns.iter().all(|c| c.kind != ColumnKind::Partition) {
            return Err(CdmError::new(
                ErrorKind::Config,
                format!(
                    "table {}.{} has no partition key",
                    self.keyspace, self.table
                ),
            ));
        }

        let mut seen = BTreeSet::new();
        for column in &self.columns {
            validate_identifier("column", &column.name)?;
            if !seen.insert(column.name.as_str()) {
                return Err(CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "table {}.{} declares column `{}` twice",
                        self.keyspace, self.table, column.name
                    ),
                ));
            }
        }

        let counters = self
            .columns
            .iter()
            .filter(|c| c.cql_type == CqlTypeInfo::Counter)
            .count();
        if counters > 0 {
            for column in &self.columns {
                let is_counter = column.cql_type == CqlTypeInfo::Counter;
                if column.kind.is_key() && is_counter {
                    return Err(CdmError::new(
                        ErrorKind::Config,
                        format!(
                            "counter column `{}` cannot be part of the primary key of {}.{}",
                            column.name, self.keyspace, self.table
                        ),
                    ));
                }
                if !column.kind.is_key() && !is_counter {
                    return Err(CdmError::new(
                        ErrorKind::Config,
                        format!(
                            "counter table {}.{} cannot also hold the non-counter column `{}`",
                            self.keyspace, self.table, column.name
                        ),
                    ));
                }
            }
        }

        Ok(TableSpec {
            keyspace: self.keyspace,
            table: self.table,
            udts: self.udts,
            columns: self.columns,
        })
    }
}

/// Rejects identifiers that a real cluster would need quoted.
///
/// The fixture deliberately does not quote: quoting rules are `SCH-010`'s business in `cdm-cql`,
/// and a test schema that needs them is testing the wrong thing. An unquotable name is therefore
/// a builder error rather than something to paper over.
fn validate_identifier(role: &str, value: &str) -> Result<(), CdmError> {
    let acceptable = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && value.starts_with(|c: char| c.is_ascii_lowercase());
    if acceptable {
        Ok(())
    } else {
        Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "{role} identifier `{value}` must be lowercase ASCII, start with a letter, and \
                 contain only letters, digits and underscores; quoted identifiers are SCH-010's \
                 business, not a fixture's"
            ),
        ))
    }
}

/// The `CREATE KEYSPACE` statement a single-node fixture needs.
///
/// `SimpleStrategy` with `replication_factor = 1`: a fixture is one node, and asking for more
/// replicas than there are nodes makes every quorum write fail in a way that looks like a bug in
/// the code under test.
pub fn create_keyspace_statement(keyspace: &str) -> String {
    format!(
        "CREATE KEYSPACE IF NOT EXISTS {keyspace} WITH replication = \
         {{'class': 'SimpleStrategy', 'replication_factor': 1}}"
    )
}

/// Generates the schemas the test suite needs (`TST-100`).
///
/// A namespace rather than a value: every method is a pure function from a name and a capability
/// set to a [`TableSpec`], so two calls with the same arguments produce byte-identical DDL.
#[derive(Debug, Clone, Copy)]
pub struct SchemaGen;

impl SchemaGen {
    /// The UDT used by the all-types table, and by anything that needs a nested structure.
    pub fn address_udt() -> UdtSpec {
        UdtSpec::new(
            "cdm_address",
            vec![
                UdtField::new("street", CqlTypeInfo::Text),
                UdtField::new("zip", CqlTypeInfo::Int),
            ],
        )
    }

    /// The simplest useful table: a text key and a text value.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the names are not plain identifiers.
    pub fn simple(keyspace: &str, table: &str) -> Result<TableSpec, CdmError> {
        TableSpec::builder(keyspace, table)
            .partition_key("key", CqlTypeInfo::Text)
            .column("value", CqlTypeInfo::Text)
            .build()
    }

    /// A counter table (`MIG-030`): a key and counters, and nothing else.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the names are not plain identifiers.
    pub fn counters(keyspace: &str, table: &str) -> Result<TableSpec, CdmError> {
        TableSpec::builder(keyspace, table)
            .partition_key("key", CqlTypeInfo::Text)
            .clustering_key("bucket", CqlTypeInfo::Int)
            .column("hits", CqlTypeInfo::Counter)
            .column("misses", CqlTypeInfo::Counter)
            .build()
    }

    /// A table with one column of every type the engine supports (`TST-100`, `CDC-001`..`CDC-004`).
    ///
    /// The types that are *not* present, and why:
    ///
    /// * `counter` — a counter column may not coexist with a non-counter one; see
    ///   [`SchemaGen::counters`];
    /// * `vector<float, 3>` — only when [`Capabilities::vectors`] is set;
    /// * the DSE geometry types and `DateRangeType` — only when the corresponding capability is
    ///   set, which no open-source image sets.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the names are not plain identifiers, or if a generated column
    /// name collides — which would be a bug in [`type_slug`].
    pub fn all_types(
        keyspace: &str,
        table: &str,
        capabilities: Capabilities,
    ) -> Result<TableSpec, CdmError> {
        let address = Self::address_udt();
        let mut builder = TableSpec::builder(keyspace, table)
            .udt(address.clone())
            .partition_key("key", CqlTypeInfo::Text)
            .clustering_key("cluster", CqlTypeInfo::Int);

        for cql_type in Self::supported_types(&address, capabilities) {
            builder = builder.column(format!("c_{}", type_slug(&cql_type)), cql_type);
        }

        builder.build()
    }

    /// Every type the all-types table carries, in a stable order.
    ///
    /// Public so a test can enumerate the same set it expects to find in the table, rather than
    /// re-deriving it and drifting.
    pub fn supported_types(address: &UdtSpec, capabilities: Capabilities) -> Vec<CqlTypeInfo> {
        let mut types: Vec<CqlTypeInfo> = CqlTypeInfo::PRIMITIVES
            .iter()
            .filter(|cql_type| match cql_type {
                // A counter cannot share a table with anything else.
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

        types.extend([
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
            address.frozen_type(),
            // A nested structure, because flat collections do not exercise the recursive half of
            // the conversion planner (`CDC-013`).
            CqlTypeInfo::Map {
                key: Box::new(CqlTypeInfo::Text),
                value: Box::new(CqlTypeInfo::List {
                    element: Box::new(address.frozen_type()),
                    frozen: true,
                }),
                frozen: false,
            },
        ]);

        if capabilities.vectors {
            types.push(CqlTypeInfo::Vector {
                element: Box::new(CqlTypeInfo::Float),
                dimensions: 3,
            });
        }

        types
    }
}

/// A column-name fragment derived from a type, e.g. `map_text_frozen_list_int`.
///
/// Derived from the type's own rendering rather than from a hand-written table, so a type added
/// to `cdm-codec` gets a name here without anybody remembering to add one.
pub fn type_slug(cql_type: &CqlTypeInfo) -> String {
    let mut slug = String::new();
    let mut previous_underscore = true; // suppresses a leading underscore
    for ch in cql_type.to_string().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_underscore = false;
        } else if !previous_underscore {
            slug.push('_');
            previous_underscore = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    slug
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
    fn tst_100_a_simple_table_renders_the_ddl_a_cluster_accepts() {
        let table = SchemaGen::simple("cdm_test", "simple").unwrap();
        assert_eq!(table.qualified_name(), "cdm_test.simple");
        assert_eq!(
            table.create_table_statement(),
            "CREATE TABLE IF NOT EXISTS cdm_test.simple (key text, value text, PRIMARY KEY ((key)))"
        );
        assert_eq!(
            table.drop_table_statement(),
            "DROP TABLE IF EXISTS cdm_test.simple"
        );
        assert!(table.create_type_statements().is_empty());
        assert_eq!(table.create_statements().len(), 1);
        assert!(!table.is_counter_table());
    }

    #[test]
    fn tst_100_composite_keys_and_static_columns_render_correctly() {
        let table = TableSpec::builder("ks", "wide")
            .partition_key("pk1", CqlTypeInfo::Text)
            .partition_key("pk2", CqlTypeInfo::Int)
            .clustering_key("ck1", CqlTypeInfo::Int)
            .clustering_key("ck2", CqlTypeInfo::Text)
            .static_column("shared", CqlTypeInfo::Text)
            .column("value", CqlTypeInfo::Text)
            .build()
            .unwrap();

        assert_eq!(
            table.create_table_statement(),
            "CREATE TABLE IF NOT EXISTS ks.wide (pk1 text, pk2 int, ck1 int, ck2 text, \
             shared text static, value text, PRIMARY KEY ((pk1, pk2), ck1, ck2))"
        );
        assert_eq!(table.columns_of(ColumnKind::Partition).len(), 2);
        assert_eq!(table.columns_of(ColumnKind::Clustering).len(), 2);
        assert_eq!(table.columns_of(ColumnKind::Static).len(), 1);
        assert_eq!(table.column("value").unwrap().kind(), ColumnKind::Regular);
        assert!(table.column("absent").is_none());
        assert!(ColumnKind::Partition.is_key());
        assert!(ColumnKind::Clustering.is_key());
        assert!(!ColumnKind::Static.is_key());
        assert!(!ColumnKind::Regular.is_key());
    }

    #[test]
    fn tst_100_a_table_without_a_partition_key_is_rejected_before_the_cluster_sees_it() {
        let err = TableSpec::builder("ks", "bad")
            .column("value", CqlTypeInfo::Text)
            .build()
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("no partition key"), "{err}");
    }

    #[test]
    fn tst_100_duplicate_and_unquotable_identifiers_are_rejected() {
        let err = TableSpec::builder("ks", "dup")
            .partition_key("key", CqlTypeInfo::Text)
            .column("key", CqlTypeInfo::Text)
            .build()
            .unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");

        for (keyspace, table, column) in [
            ("Ks", "t", "c"),
            ("ks", "T", "c"),
            ("ks", "t", "C"),
            ("ks", "t", "9c"),
            ("ks", "t", "with space"),
            ("", "t", "c"),
        ] {
            let err = TableSpec::builder(keyspace, table)
                .partition_key(column, CqlTypeInfo::Text)
                .build()
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Config, "{keyspace}.{table}.{column}");
            assert!(err.to_string().contains("SCH-010"), "{err}");
        }
    }

    #[test]
    fn mig_030_a_counter_table_holds_counters_and_nothing_else() {
        let table = SchemaGen::counters("cdm_test", "hits").unwrap();
        assert!(table.is_counter_table());
        assert_eq!(
            table.create_table_statement(),
            "CREATE TABLE IF NOT EXISTS cdm_test.hits (key text, bucket int, hits counter, \
             misses counter, PRIMARY KEY ((key), bucket))"
        );

        let err = TableSpec::builder("ks", "mixed")
            .partition_key("key", CqlTypeInfo::Text)
            .column("hits", CqlTypeInfo::Counter)
            .column("label", CqlTypeInfo::Text)
            .build()
            .unwrap_err();
        assert!(err.to_string().contains("non-counter column"), "{err}");

        let err = TableSpec::builder("ks", "keyed")
            .partition_key("hits", CqlTypeInfo::Counter)
            .column("other", CqlTypeInfo::Counter)
            .build()
            .unwrap_err();
        assert!(err.to_string().contains("primary key"), "{err}");
    }

    #[test]
    fn tst_100_the_all_types_table_covers_every_primitive_except_counter() {
        let table = SchemaGen::all_types("cdm_test", "all", Capabilities::maximal()).unwrap();
        let types: Vec<&CqlTypeInfo> = table.columns().iter().map(ColumnSpec::cql_type).collect();

        for primitive in &CqlTypeInfo::PRIMITIVES {
            if *primitive == CqlTypeInfo::Counter {
                assert!(!types.contains(&primitive), "counter must not be here");
                continue;
            }
            assert!(types.contains(&primitive), "{primitive} is missing");
        }
        assert!(!table.is_counter_table());
        assert_eq!(table.create_type_statements().len(), 1);
        assert_eq!(
            table.create_type_statements()[0],
            "CREATE TYPE IF NOT EXISTS cdm_test.cdm_address (street text, zip int)"
        );
    }

    #[test]
    fn cdc_004_vectors_appear_only_when_the_engine_has_them() {
        let vector = CqlTypeInfo::Vector {
            element: Box::new(CqlTypeInfo::Float),
            dimensions: 3,
        };

        let with = SchemaGen::all_types("ks", "t", Capabilities::maximal()).unwrap();
        assert!(with.columns().iter().any(|c| *c.cql_type() == vector));

        let without = SchemaGen::all_types("ks", "t", Capabilities::portable()).unwrap();
        assert!(!without.columns().iter().any(|c| *c.cql_type() == vector));
        assert!(without.create_table_statement().contains("cdm_address"));
        assert!(!without.create_table_statement().contains("vector<"));
    }

    #[test]
    fn cdc_003_dse_geometry_appears_only_when_the_engine_has_it() {
        let portable = SchemaGen::all_types("ks", "t", Capabilities::portable()).unwrap();
        for absent in [
            "PointType",
            "LineStringType",
            "PolygonType",
            "DateRangeType",
        ] {
            assert!(
                !portable.create_table_statement().contains(absent),
                "{absent} must not reach an open-source image"
            );
        }
        let maximal = SchemaGen::all_types("ks", "t", Capabilities::maximal()).unwrap();
        assert!(maximal.create_table_statement().contains("PointType"));
    }

    #[test]
    fn tst_100_column_names_are_derived_from_the_type_and_stay_unique() {
        assert_eq!(type_slug(&CqlTypeInfo::Text), "text");
        assert_eq!(type_slug(&CqlTypeInfo::TimeUuid), "timeuuid");
        assert_eq!(
            type_slug(&CqlTypeInfo::Map {
                key: Box::new(CqlTypeInfo::Text),
                value: Box::new(CqlTypeInfo::List {
                    element: Box::new(CqlTypeInfo::Int),
                    frozen: true
                }),
                frozen: false,
            }),
            "map_text_frozen_list_int"
        );

        // Uniqueness is what `build` relies on to accept the generated table at all.
        let table = SchemaGen::all_types("ks", "t", Capabilities::maximal()).unwrap();
        let names: BTreeSet<&str> = table.columns().iter().map(ColumnSpec::name).collect();
        assert_eq!(names.len(), table.columns().len());
    }

    #[test]
    fn tst_100_the_keyspace_statement_asks_for_one_replica() {
        let ddl = create_keyspace_statement("cdm_test");
        assert_eq!(
            ddl,
            "CREATE KEYSPACE IF NOT EXISTS cdm_test WITH replication = \
             {'class': 'SimpleStrategy', 'replication_factor': 1}"
        );
    }

    #[test]
    fn tst_100_generation_is_a_pure_function_of_its_arguments() {
        let first = SchemaGen::all_types("ks", "t", Capabilities::maximal()).unwrap();
        let second = SchemaGen::all_types("ks", "t", Capabilities::maximal()).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.create_table_statement(),
            second.create_table_statement()
        );
        assert_eq!(SchemaGen::address_udt().name(), "cdm_address");
        assert_eq!(SchemaGen::address_udt().fields().len(), 2);
    }
}
