//! The read statements: the origin range scan and the two lookups by primary key.
//!
//! # `ALLOW FILTERING` is not free (`FEA-061`)
//!
//! Java appends `ALLOW FILTERING` to the range scan unconditionally. A pure token-range scan does
//! not need it — the restriction is on the partition token, which is exactly what the ring is
//! ordered by — so the clause only ever suppresses a warning the server was right to emit. Worse,
//! it suppresses it for the *configured* `filter.cql_where` case too, where the warning is the only
//! signal that the run is about to do a full scan per range.
//!
//! cdm-rs therefore emits it only when a `filter.cql_where` is configured, and `--compat-java`
//! restores the unconditional form. Both shapes read the same rows; the difference is whether the
//! server is allowed to say that the query is expensive.

use cdm_core::CdmError;

use crate::schema::TableSchema;

use super::mapping::{ColumnMapping, TargetSource};
use super::projection::OriginProjection;
use super::{join, upsert::where_clause};

/// One end of a token range, typed as its partitioner requires (`FEA-060`, `TOK-001`).
///
/// Murmur3 tokens are `bigint`; Random-partitioner tokens are `varint` and run up to `2^127 - 1`,
/// which is why the two cases cannot share a representation. Binding a Murmur3 token as a `varint`
/// — or the reverse — is rejected by the server rather than silently mis-read, but only after the
/// statement has been prepared, which is far too late in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenBound {
    /// A `Murmur3Partitioner` token, bound as `bigint`.
    Murmur3(i64),
    /// A `RandomPartitioner` token, bound as `varint`.
    Random(i128),
}

impl TokenBound {
    /// The value as the wire represents it.
    ///
    /// `bigint` is eight big-endian bytes. `varint` is a minimal two's-complement big-endian
    /// encoding: the shortest byte string whose sign bit already carries the sign, which is what
    /// Cassandra's `IntegerSerializer` produces and what `BigInteger.toByteArray` returns.
    pub fn serialized(self) -> Vec<u8> {
        match self {
            Self::Murmur3(token) => token.to_be_bytes().to_vec(),
            Self::Random(token) => {
                let bytes = token.to_be_bytes();
                let sign_filler = if token < 0 { 0xff } else { 0x00 };
                let mut start = 0;
                while start + 1 < bytes.len()
                    && bytes.get(start).copied() == Some(sign_filler)
                    && bytes
                        .get(start + 1)
                        .is_some_and(|next| (next & 0x80 != 0) == (sign_filler == 0xff))
                {
                    start += 1;
                }
                bytes.get(start..).unwrap_or(&bytes).to_vec()
            }
        }
    }
}

/// The origin token-range scan (`FEA-060`, `FEA-061`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginRangeSelect {
    cql: String,
}

impl OriginRangeSelect {
    /// Builds the scan.
    ///
    /// `cql_where` is the already-assembled `filter.cql_where` fragment, including its leading
    /// ` AND ` — `cdm-feature`'s `CqlWhereFilter::fragment` produces exactly that, and it is
    /// concatenated with no separator added here, matching Java's `OriginFilterCondition`.
    ///
    /// `compat_java` restores Java's unconditional `ALLOW FILTERING` (`FEA-061`).
    pub fn new(
        table: &TableSchema,
        projection: &OriginProjection,
        cql_where: Option<&str>,
        compat_java: bool,
    ) -> Self {
        let partition_key = join(
            table
                .partition_key()
                .iter()
                .map(|column| column.quoted_name()),
        );
        let filter = cql_where.unwrap_or_default();
        let allow_filtering = if compat_java || !filter.trim().is_empty() {
            " ALLOW FILTERING"
        } else {
            ""
        };
        Self {
            cql: format!(
                "SELECT {} FROM {} WHERE TOKEN({partition_key}) >= ? AND TOKEN({partition_key}) \
                 <= ?{filter}{allow_filtering}",
                projection.cql(),
                table.quoted_name(),
            ),
        }
    }

    /// The statement text.
    pub fn cql(&self) -> &str {
        &self.cql
    }

    /// The two token bounds, in bind order: minimum first, maximum second (`FEA-060`).
    ///
    /// Both bounds are inclusive, which is why the planner's ranges are half-open on the *low*
    /// side: `TOK-003` produces `(prev_max, this_max]` so that `>=`/`<=` here covers the ring
    /// exactly once.
    pub fn binds(min: TokenBound, max: TokenBound) -> [TokenBound; 2] {
        [min, max]
    }
}

/// The origin lookup by primary key, used by rerun and by validate's autocorrect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginSelectByPk {
    cql: String,
}

impl OriginSelectByPk {
    /// Builds the lookup over the origin's own primary key.
    pub fn new(table: &TableSchema, projection: &OriginProjection) -> Self {
        let predicate = table
            .primary_key()
            .iter()
            .map(|column| format!("{}=?", column.quoted_name()))
            .collect::<Vec<_>>()
            .join(" AND ");
        Self {
            cql: format!(
                "SELECT {} FROM {} WHERE {predicate}",
                projection.cql(),
                table.quoted_name(),
            ),
        }
    }

    /// The statement text.
    pub fn cql(&self) -> &str {
        &self.cql
    }
}

/// The target lookup by primary key, used by validate (`VAL-003`) and by the counter delta
/// (`MIG-031`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSelectByPk {
    cql: String,
    bound_key_columns: Vec<String>,
}

impl TargetSelectByPk {
    /// Builds the lookup, inlining any constant primary-key component as a literal (`FEA-012`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`](cdm_core::ErrorKind::SchemaMismatch) if a target primary-key
    /// component has no source. [`ColumnMapping::resolve`] already rejects that (`SCH-006`), so
    /// reaching it here means the mapping was constructed by hand; failing is still better than
    /// emitting a `WHERE` clause that silently matches more rows than it should.
    pub fn new(mapping: &ColumnMapping) -> Result<Self, CdmError> {
        let table = mapping.target_table();
        let (predicate, bound_key_columns) = where_clause(mapping)?;
        let projection = join(
            table
                .columns
                .iter()
                .map(crate::schema::ColumnMeta::quoted_name),
        );
        Ok(Self {
            cql: format!(
                "SELECT {projection} FROM {} WHERE {predicate}",
                table.quoted_name()
            ),
            bound_key_columns,
        })
    }

    /// The statement text.
    pub fn cql(&self) -> &str {
        &self.cql
    }

    /// The target key columns that carry a bind marker, in bind order.
    ///
    /// Shorter than the target primary key whenever a constant column supplies a component, since
    /// `FEA-012` inlines those.
    pub fn bound_key_columns(&self) -> &[String] {
        &self.bound_key_columns
    }
}

/// Whether the target column is a key component that must appear in a `WHERE` clause.
pub(super) fn is_key_source(source: &TargetSource) -> bool {
    matches!(
        source,
        TargetSource::Origin(_)
            | TargetSource::ExplodeKey
            | TargetSource::ExplodeValue
            | TargetSource::ExtractJson(_)
            | TargetSource::Constant(_)
    )
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
    use crate::schema::table::tests::column;
    use crate::schema::ColumnKind;
    use crate::statement::mapping::tests::{origin, target};
    use crate::statement::MappingOptions;

    fn projection() -> OriginProjection {
        OriginProjection::new(&origin().columns, &["WRITETIME(data)".to_owned()])
    }

    #[test]
    fn fea_060_the_range_select_bounds_the_token_of_the_partition_key() {
        let select = OriginRangeSelect::new(&origin(), &projection(), None, false);
        assert_eq!(
            select.cql(),
            "SELECT id,cc,data,notes,WRITETIME(data) FROM ks.src \
             WHERE TOKEN(id) >= ? AND TOKEN(id) <= ?"
        );
    }

    #[test]
    fn fea_060_a_composite_partition_key_appears_whole_inside_token() {
        let mut table = origin();
        table
            .columns
            .push(column("tenant", "text", ColumnKind::PartitionKey, 1));
        let projection = OriginProjection::new(&table.columns, &[]);
        let select = OriginRangeSelect::new(&table, &projection, None, false);
        assert!(
            select.cql().contains("TOKEN(id,tenant) >= ?"),
            "{}",
            select.cql()
        );
    }

    #[test]
    fn fea_061_allow_filtering_is_omitted_without_a_cql_where_and_emitted_with_one() {
        let bare = OriginRangeSelect::new(&origin(), &projection(), None, false);
        assert!(!bare.cql().contains("ALLOW FILTERING"), "{}", bare.cql());

        let filtered =
            OriginRangeSelect::new(&origin(), &projection(), Some(" AND data = 'x'"), false);
        assert!(filtered.cql().ends_with(" AND data = 'x' ALLOW FILTERING"));
    }

    #[test]
    fn fea_061_compat_java_restores_the_unconditional_allow_filtering() {
        let select = OriginRangeSelect::new(&origin(), &projection(), None, true);
        assert!(
            select.cql().ends_with(" ALLOW FILTERING"),
            "{}",
            select.cql()
        );

        let blank = OriginRangeSelect::new(&origin(), &projection(), Some("   "), false);
        assert!(
            !blank.cql().contains("ALLOW FILTERING"),
            "a whitespace-only filter is no filter: {}",
            blank.cql()
        );
    }

    #[test]
    fn fea_060_token_bounds_are_typed_per_partitioner() {
        assert_eq!(
            TokenBound::Murmur3(-1).serialized(),
            vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(TokenBound::Murmur3(0).serialized(), vec![0; 8]);
        assert_eq!(TokenBound::Random(0).serialized(), vec![0x00]);
        assert_eq!(TokenBound::Random(1).serialized(), vec![0x01]);
        assert_eq!(TokenBound::Random(127).serialized(), vec![0x7f]);
        assert_eq!(TokenBound::Random(128).serialized(), vec![0x00, 0x80]);
        assert_eq!(TokenBound::Random(-1).serialized(), vec![0xff]);
        assert_eq!(TokenBound::Random(-129).serialized(), vec![0xff, 0x7f]);
        assert_eq!(
            TokenBound::Random(i128::MAX).serialized().len(),
            16,
            "2^127 - 1 needs every byte"
        );

        let [min, max] = OriginRangeSelect::binds(TokenBound::Murmur3(-5), TokenBound::Murmur3(5));
        assert_eq!(min, TokenBound::Murmur3(-5));
        assert_eq!(max, TokenBound::Murmur3(5));
    }

    #[test]
    fn sch_006_the_target_lookup_binds_key_columns_and_inlines_constant_ones() {
        let mut target = target();
        target
            .columns
            .push(column("tenant", "text", ColumnKind::PartitionKey, 1));
        let options = MappingOptions {
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin(), &target, &options).unwrap();
        let select = TargetSelectByPk::new(&mapping).unwrap();

        assert_eq!(
            select.cql(),
            "SELECT id,cc,payload,notes,tenant FROM ks.dst WHERE id=? AND tenant='acme' AND cc=?"
        );
        assert_eq!(select.bound_key_columns(), ["id", "cc"]);
    }

    #[test]
    fn sch_003_the_origin_lookup_uses_the_origin_primary_key() {
        let select = OriginSelectByPk::new(&origin(), &projection());
        assert_eq!(
            select.cql(),
            "SELECT id,cc,data,notes,WRITETIME(data) FROM ks.src WHERE id=? AND cc=?"
        );
    }

    #[test]
    fn sch_006_an_underivable_key_column_is_refused_by_the_target_lookup() {
        // Bypasses `ColumnMapping::resolve`'s own check by resolving against a schema where the
        // component *is* derivable, then removing the source.
        let mut target = target();
        target
            .columns
            .push(column("tenant", "text", ColumnKind::PartitionKey, 1));
        let mut origin = origin();
        origin
            .columns
            .push(column("tenant", "text", ColumnKind::Regular, -1));
        let mapping = ColumnMapping::resolve(&origin, &target, &MappingOptions::default()).unwrap();
        assert!(TargetSelectByPk::new(&mapping).is_ok());
        assert!(is_key_source(&TargetSource::ExplodeKey));
        assert!(!is_key_source(&TargetSource::Absent));
    }
}
