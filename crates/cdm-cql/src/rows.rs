//! The Cassandra-backed [`RowSource`] and [`RowSink`] (`PLG-005`).
//!
//! `cdm-core` declares the two seams a job reads and writes through, and says where their Cassandra
//! implementation belongs: here, in the one crate allowed to depend on the driver
//! (`ARCHITECTURE.md` §3). Everything above them — the scheduler, the jobs — is written against the
//! traits and can be exercised with in-memory doubles, which is what makes a validate run testable
//! without a cluster at all.
//!
//! ```text
//!   RowSource::open(range)  ──►  paged origin scan  ──►  Record { key, origin row }
//!   RowSink::fetch(key)     ──►  target SELECT by PK ──►  Record { key, target row }
//!   RowSink::write(record)  ──►  target upsert       ──►  ()
//! ```
//!
//! # The primary key is the *bound* key, not the whole key
//!
//! A [`PrimaryKey`] here holds exactly the values that carry a bind marker in
//! [`TargetSelectByPk`], in bind order. That is shorter than the target's declared primary key
//! whenever a constant column supplies a component, because `FEA-012` inlines those as literals.
//! Both the source and the sink derive it the same way, from the same [`ColumnMapping`], so a key
//! built while reading always binds against the statement built for looking it up.
//!
//! # An exploded map entry is a permitted key source (`SCH-006`, `FEA-022`)
//!
//! `SCH-006` admits three sources for a target primary-key component: a mapped origin column, a
//! constant column, and the key or value of an exploded map entry. [`TargetKeyPlan`] therefore
//! carries a slot per bound component rather than an origin position per component, and the
//! exploded slots are filled from the entry the key is being derived *for*.
//!
//! That distinction is the whole of `FEA-020`: one origin row stands for one target row per map
//! entry, so there is no single key an origin row has. [`CqlRowSource`] cannot fill those slots —
//! exploding a map means converting its elements, which lives in `cdm-feature` on the far side of
//! the dependency edge — so the records it emits carry a key whose exploded components are null,
//! and the job that explodes the row completes it with [`TargetKeyPlan::key_of`], once per entry.
//! [`TargetKeyPlan::explodes`] is how a caller knows it must. A null key component is never
//! looked up: [`CqlRowSink::fetch`] answers "absent" for it without issuing a query, so a caller
//! that forgets reports every row missing rather than validating against the wrong target row.
//!
//! # Counter corrections are converged, not re-added
//!
//! `MIG-030` writes a counter as `SET c = c + ?`, so the bound value is a *delta*. When validate
//! corrects a mismatched counter row it already holds the target's current value, and
//! [`CqlRowSink::write`] subtracts it before binding (`MIG-031`) — the correction converges the
//! counter on the origin's value instead of adding to it a second time. When the target row is
//! absent there is nothing to subtract and the origin's value is the delta, which is why
//! `VAL-004` guards that case behind an explicit opt-in: a counter row that was *deleted* rather
//! than never written will come back doubled, and no amount of arithmetic here can tell those two
//! situations apart.
//!
//! # Specification
//!
//! - `PLG-005` — [`CqlRowSource`], [`CqlRowSink`]
//! - `FEA-060` — the paged origin range scan
//! - `MIG-030`, `MIG-031` — the counter delta
//! - `VAL-001` — the target lookup by primary key

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{
    CdmError, ErrorKind, Plugin, PrimaryKey, RawCell, Record, Row, RowSink, RowSource, RowStream,
    Side, TokenRange,
};
use scylla::client::session::Session;
use scylla::response::PagingState;
use scylla::serialize::row::{RowSerializationContext, SerializeRow};
use scylla::serialize::writers::RowWriter;
use scylla::serialize::SerializationError;
use scylla::statement::prepared::PreparedStatement;

use crate::raw::RawRow;
use crate::statement::{
    BindInputs, Binder, Bound, ColumnMapping, OriginRangeSelect, TargetSelectByPk, TargetSource,
    TokenBound,
};

/// Which partitioner's token type the range scan binds (`TOK-001`).
///
/// A `Murmur3Partitioner` token is a `bigint`; a `RandomPartitioner` token is a `varint` running to
/// `2^127 - 1`. Binding one as the other is rejected by the server, but only after the statement has
/// prepared, which in a run is far too late.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// `org.apache.cassandra.dht.Murmur3Partitioner`.
    Murmur3,
    /// `org.apache.cassandra.dht.RandomPartitioner`.
    Random,
}

impl TokenKind {
    /// The bound form of one end of a range.
    fn bound(self, token: i128) -> TokenBound {
        match self {
            Self::Murmur3 => TokenBound::Murmur3(i64::try_from(token).unwrap_or(i64::MIN)),
            Self::Random => TokenBound::Random(token),
        }
    }
}

/// Where one bound component of the target primary key takes its value from (`SCH-006`).
///
/// `SCH-006` admits three sources for a target primary-key component: a mapped origin column, a
/// constant column and the key or value of an exploded map entry. A constant takes no slot here —
/// `FEA-012` inlines it into the statement as a literal, so it carries no bind marker — which
/// leaves the three variants below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKeySlot {
    /// The origin projection position that supplies the component.
    Origin(usize),
    /// The key of the exploded map entry this target row was produced from (`FEA-020`, `FEA-022`).
    ExplodeKey,
    /// The value of the exploded map entry this target row was produced from (`FEA-020`,
    /// `FEA-022`).
    ExplodeValue,
}

/// The exploded map entry a key is being derived for (`FEA-020`, `FEA-022`).
///
/// The two halves arrive as already-converted wire bytes, which is what
/// [`ExplodePlan::explode`](https://docs.rs/cdm-feature) produces — `cdm-feature` depends on this
/// crate and not the other way round (`ARCHITECTURE.md` §3), so the entry crosses the seam as
/// plain data, exactly as [`BindInputs::explode_key`] does for the write path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExplodedKeyParts<'a> {
    /// The entry's key, converted to the target key column's type (`FEA-021`).
    pub key: Option<&'a [u8]>,
    /// The entry's value, converted to the target value column's type (`FEA-021`).
    pub value: Option<&'a [u8]>,
}

impl ExplodedKeyParts<'_> {
    /// No entry: the caller is deriving a key for an origin row that is not being exploded.
    pub const NONE: Self = Self {
        key: None,
        value: None,
    };
}

/// How the bound components of the target primary key are derived, in bind order (`SCH-006`).
#[derive(Debug, Clone)]
pub struct TargetKeyPlan {
    slots: Vec<TargetKeySlot>,
}

impl TargetKeyPlan {
    /// Resolves the plan from the mapping and the lookup statement built from it.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] when a bound key column is supplied by something `SCH-006`
    /// does not admit as a source for a target primary key: an extracted JSON property, or nothing
    /// at all. An exploded map key or value is *not* such a case — `SCH-006` names it explicitly
    /// and `FEA-022` builds on it — it is carried as a slot that [`TargetKeyPlan::key_of`] fills
    /// from the entry.
    pub fn resolve(mapping: &ColumnMapping, select: &TargetSelectByPk) -> Result<Self, CdmError> {
        let mut slots = Vec::with_capacity(select.bound_key_columns().len());
        for column in select.bound_key_columns() {
            match mapping.source_of(column) {
                Some(TargetSource::Origin(index)) => slots.push(TargetKeySlot::Origin(*index)),
                Some(TargetSource::ExplodeKey) => slots.push(TargetKeySlot::ExplodeKey),
                Some(TargetSource::ExplodeValue) => slots.push(TargetKeySlot::ExplodeValue),
                other => {
                    return Err(CdmError::new(
                        ErrorKind::SchemaMismatch,
                        format!(
                            "the target primary-key column `{column}` is supplied by {}, which \
                             `SCH-006` does not admit as a source for a target primary key: a \
                             component must come from a mapped origin column, a constant column or \
                             an exploded map entry (SCH-006).",
                            match other {
                                Some(TargetSource::ExtractJson(_)) => "an extracted JSON property",
                                Some(TargetSource::Constant(_)) =>
                                    "a constant, which carries no bind marker",
                                _ => "nothing",
                            }
                        ),
                    )
                    .with_context(|c| c.with_side(Side::Target).with_column(column.clone())));
                }
            }
        }
        Ok(Self { slots })
    }

    /// The slots, in the order [`TargetSelectByPk`] binds them.
    #[must_use]
    pub fn slots(&self) -> &[TargetKeySlot] {
        &self.slots
    }

    /// Whether any component comes from an exploded map entry (`FEA-022`).
    ///
    /// `true` means one origin row stands for as many target rows as the map has entries, so a key
    /// derived with [`ExplodedKeyParts::NONE`] identifies none of them. The caller must explode the
    /// row and call [`TargetKeyPlan::key_of`] once per entry.
    #[must_use]
    pub fn explodes(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| !matches!(slot, TargetKeySlot::Origin(_)))
    }

    /// The key of one origin row, for one exploded map entry.
    ///
    /// Pass [`ExplodedKeyParts::NONE`] when no explode map is configured. Passing it while
    /// [`TargetKeyPlan::explodes`] is `true` leaves the exploded components null, which
    /// [`CqlRowSink::fetch`] answers as "absent" without issuing a query.
    #[must_use]
    pub fn key_of(&self, row: &Row, entry: ExplodedKeyParts<'_>) -> PrimaryKey {
        PrimaryKey::new(
            self.slots
                .iter()
                .map(|slot| match slot {
                    TargetKeySlot::Origin(index) => {
                        row.get(*index).cloned().unwrap_or(RawCell::NULL)
                    }
                    TargetKeySlot::ExplodeKey => cell_of(entry.key),
                    TargetKeySlot::ExplodeValue => cell_of(entry.value),
                })
                .collect(),
        )
    }
}

/// One half of an exploded entry as a cell, null when the caller supplied none.
fn cell_of(bytes: Option<&[u8]>) -> RawCell {
    bytes.map_or(RawCell::NULL, |bytes| RawCell::new(bytes.to_vec()))
}

/// The origin side of a run: a paged token-range scan (`PLG-005`, `FEA-060`).
#[derive(Debug)]
pub struct CqlRowSource {
    session: Arc<Session>,
    statement: PreparedStatement,
    token_kind: TokenKind,
    key_plan: TargetKeyPlan,
}

impl CqlRowSource {
    /// Prepares the range scan.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] if the statement does not prepare, and [`ErrorKind::SchemaMismatch`] if
    /// a target primary-key component comes from a source `SCH-006` does not admit. Both are
    /// startup failures: a statement that will never prepare, or a key that can never be built,
    /// must not be discovered on the first row.
    pub async fn prepare(
        session: Arc<Session>,
        select: &OriginRangeSelect,
        mapping: &ColumnMapping,
        target_select: &TargetSelectByPk,
        token_kind: TokenKind,
    ) -> Result<Self, CdmError> {
        let statement = session
            .prepare(select.cql())
            .await
            .map_err(|error| read_error(Side::Origin, select.cql(), &error))?;
        Ok(Self {
            session,
            statement,
            token_kind,
            key_plan: TargetKeyPlan::resolve(mapping, target_select)?,
        })
    }

    /// How this source derives a record's target primary key (`SCH-006`).
    ///
    /// Exposed because a run with an explode map must complete the key itself: the records this
    /// source emits leave the exploded components null, and the job holding the [`ExplodePlan`]
    /// calls [`TargetKeyPlan::key_of`] once per entry (`FEA-022`).
    ///
    /// [`ExplodePlan`]: https://docs.rs/cdm-feature
    #[must_use]
    pub const fn key_plan(&self) -> &TargetKeyPlan {
        &self.key_plan
    }
}

impl Plugin for CqlRowSource {
    fn name(&self) -> &'static str {
        "cassandra-origin"
    }

    fn provider(&self) -> &'static str {
        "cdm-cql"
    }
}

#[async_trait]
impl RowSource for CqlRowSource {
    async fn open(&self, range: TokenRange) -> Result<Box<dyn RowStream>, CdmError> {
        Ok(Box::new(RangePager {
            session: Arc::clone(&self.session),
            statement: self.statement.clone(),
            bounds: RawBinds::of([
                self.token_kind.bound(range.min()).serialized(),
                self.token_kind.bound(range.max()).serialized(),
            ]),
            paging: Some(PagingState::start()),
            buffer: VecDeque::new(),
            key_plan: self.key_plan.clone(),
        }))
    }
}

/// One range's paged scan.
///
/// Rows are converted to owned [`Record`]s a page at a time and the frame is released, so the
/// stream holds one page rather than one range — which is what `NFR-003`'s memory envelope is
/// computed against.
struct RangePager {
    session: Arc<Session>,
    statement: PreparedStatement,
    bounds: RawBinds,
    paging: Option<PagingState>,
    buffer: VecDeque<Record>,
    key_plan: TargetKeyPlan,
}

#[async_trait]
impl RowStream for RangePager {
    async fn next_record(&mut self) -> Result<Option<Record>, CdmError> {
        loop {
            if let Some(record) = self.buffer.pop_front() {
                return Ok(Some(record));
            }
            let Some(paging) = self.paging.take() else {
                return Ok(None);
            };
            let (result, response) = self
                .session
                .execute_single_page(&self.statement, &self.bounds, paging)
                .await
                .map_err(|error| {
                    read_error(Side::Origin, self.statement.get_statement(), &error)
                })?;
            let rows = result.into_rows_result().map_err(|error| {
                read_error(Side::Origin, self.statement.get_statement(), &error)
            })?;
            for row in rows
                .rows::<RawRow<'_, '_>>()
                .map_err(|error| read_error(Side::Origin, self.statement.get_statement(), &error))?
            {
                let row = row.map_err(|error| {
                    read_error(Side::Origin, self.statement.get_statement(), &error)
                })?;
                let origin = own(&row);
                // The exploded components, if any, stay null here: which map entry the key is for
                // is not a question this side of the seam can answer (see the module docs).
                let key = self.key_plan.key_of(&origin, ExplodedKeyParts::NONE);
                self.buffer.push_back(Record::new(key, origin));
            }
            self.paging = match response.into_paging_control_flow() {
                std::ops::ControlFlow::Continue(state) => Some(state),
                std::ops::ControlFlow::Break(()) => None,
            };
        }
    }
}

/// The target side of a run: the lookup by primary key and the upsert (`PLG-005`, `VAL-001`).
#[derive(Debug)]
pub struct CqlRowSink {
    session: Arc<Session>,
    select: PreparedStatement,
    upsert: PreparedStatement,
    binder: Binder,
    counters: Vec<CounterColumn>,
}

/// A counter column, and where its two operands live (`MIG-031`).
#[derive(Debug, Clone, Copy)]
struct CounterColumn {
    origin_index: usize,
    target_index: usize,
}

impl CqlRowSink {
    /// Prepares both statements.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] or [`ErrorKind::Write`] if a statement does not prepare.
    pub async fn prepare(
        session: Arc<Session>,
        select: &TargetSelectByPk,
        binder: Binder,
        mapping: &ColumnMapping,
    ) -> Result<Self, CdmError> {
        let prepared_select = session
            .prepare(select.cql())
            .await
            .map_err(|error| read_error(Side::Target, select.cql(), &error))?;
        let upsert_cql = binder.statement().cql().to_owned();
        let prepared_upsert = session
            .prepare(upsert_cql.as_str())
            .await
            .map_err(|error| {
                CdmError::new(
                    ErrorKind::Write,
                    format!("cannot prepare the target upsert `{upsert_cql}`: {error}"),
                )
                .with_context(|c| c.with_side(Side::Target))
            })?;
        let counters = if binder.statement().is_counter() {
            mapping
                .target_columns()
                .iter()
                .enumerate()
                .filter(|(_, column)| column.cql_type.eq_ignore_ascii_case("counter"))
                .filter_map(
                    |(target_index, column)| match mapping.source_of(&column.name) {
                        Some(TargetSource::Origin(origin_index)) => Some(CounterColumn {
                            origin_index: *origin_index,
                            target_index,
                        }),
                        _ => None,
                    },
                )
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            session,
            select: prepared_select,
            upsert: prepared_upsert,
            binder,
            counters,
        })
    }

    /// The origin row a counter write should bind, with each counter turned into a delta
    /// (`MIG-031`).
    ///
    /// Returns the row unchanged when the table has no counters or the target row is absent, which
    /// is the missing-row case where the origin's value *is* the delta.
    fn counter_deltas(&self, record: &Record) -> Option<Row> {
        let target = record.target()?;
        if self.counters.is_empty() {
            return None;
        }
        let mut cells: Vec<RawCell> = record.origin().cells().to_vec();
        for counter in &self.counters {
            let origin_value = cells
                .get(counter.origin_index)
                .and_then(counter_value)
                .unwrap_or(0);
            let target_value = target
                .get(counter.target_index)
                .and_then(counter_value)
                .unwrap_or(0);
            if let Some(cell) = cells.get_mut(counter.origin_index) {
                *cell = RawCell::new(
                    origin_value
                        .saturating_sub(target_value)
                        .to_be_bytes()
                        .to_vec(),
                );
            }
        }
        Some(Row::new(cells))
    }
}

impl Plugin for CqlRowSink {
    fn name(&self) -> &'static str {
        "cassandra-target"
    }

    fn provider(&self) -> &'static str {
        "cdm-cql"
    }
}

#[async_trait]
impl RowSink for CqlRowSink {
    async fn write(&self, record: &Record) -> Result<(), CdmError> {
        let adjusted = self.counter_deltas(record);
        // `&Row` is what implements `SourceRow`, not `Row`: the trait is over the frame lifetime so
        // that a borrowed cell can outlive the call, which is what `MIG-040` is built on.
        let source: &Row = adjusted.as_ref().unwrap_or_else(|| record.origin());
        let bound = self.binder.bind(
            &source,
            BindInputs {
                key: Some(record.key()),
                ..BindInputs::default()
            },
        )?;
        // Both arms execute identically; the distinction is a type-level one that keeps a counter
        // write out of any generic retry helper (`CON-012`). There is no retry here at all.
        let values = match &bound {
            Bound::Idempotent(write) => write.values(),
            Bound::Counter(write) => write.values(),
        };
        self.session
            .execute_unpaged(&self.upsert, values)
            .await
            .map_err(|error| {
                CdmError::new(
                    ErrorKind::Write,
                    format!("the target upsert failed: {error}"),
                )
                .with_context(|c| {
                    c.with_side(Side::Target)
                        .with_primary_key(record.key().clone())
                })
            })?;
        Ok(())
    }

    async fn flush(&self) -> Result<(), CdmError> {
        // Nothing is buffered: validate's corrections are issued one at a time and synchronously,
        // because a batched counter correction is exactly what `MIG-032` forbids.
        Ok(())
    }

    async fn fetch(&self, key: &PrimaryKey) -> Result<Option<Record>, CdmError> {
        // A `NULL` cannot appear in a Cassandra primary key, so a key component that is null
        // describes a row the target cannot hold: the answer is "absent", not a query. Returning it
        // here rather than binding a null also keeps `set_null` out of this crate entirely, which
        // is what `mig_012_no_production_path_can_bind_null` sweeps for.
        let mut binds = Vec::with_capacity(key.len());
        for cell in key.values() {
            let Some(bytes) = cell.bytes() else {
                return Ok(None);
            };
            binds.push(bytes.to_vec());
        }
        let binds = RawBinds::of(binds);
        let result = self
            .session
            .execute_unpaged(&self.select, &binds)
            .await
            .map_err(|error| read_error(Side::Target, self.select.get_statement(), &error))?;
        let rows = result
            .into_rows_result()
            .map_err(|error| read_error(Side::Target, self.select.get_statement(), &error))?;
        let mut iter = rows
            .rows::<RawRow<'_, '_>>()
            .map_err(|error| read_error(Side::Target, self.select.get_statement(), &error))?;
        match iter.next() {
            None => Ok(None),
            Some(row) => {
                let row = row.map_err(|error| {
                    read_error(Side::Target, self.select.get_statement(), &error)
                })?;
                Ok(Some(Record::new(key.clone(), own(&row))))
            }
        }
    }
}

/// A failed read, naming the side and the statement but never a value (`ERR-001`, `SEC-002`).
fn read_error(side: Side, statement: &str, error: &dyn std::fmt::Display) -> CdmError {
    CdmError::new(
        ErrorKind::Read,
        format!("the {side} read failed: {error} (statement: {statement})"),
    )
    .with_context(|c| c.with_side(side))
}

/// A `bigint`-shaped counter value.
fn counter_value(cell: &RawCell) -> Option<i64> {
    let bytes = cell.bytes()?;
    let array: [u8; 8] = bytes.as_ref().try_into().ok()?;
    Some(i64::from_be_bytes(array))
}

/// Copies a frame row into owned cells.
fn own(row: &RawRow<'_, '_>) -> Row {
    Row::new(
        row.cells()
            .iter()
            .map(|cell| {
                cell.bytes
                    .map_or(RawCell::NULL, |bytes| RawCell::new(bytes.to_vec()))
            })
            .collect(),
    )
}

/// Bind values as raw serialised bytes.
///
/// The read statements bind key components and token bounds, both of which cdm-rs already holds in
/// wire form; decoding them into typed values in order to re-encode them would be pure loss. There
/// is no null arm, for the same structural reason [`BoundValue`](crate::statement::BoundValue) has
/// none: nothing in this crate may reach the driver's `set_null` (`MIG-012`).
#[derive(Debug, Clone)]
struct RawBinds {
    values: Vec<Vec<u8>>,
}

impl RawBinds {
    fn of(values: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }
}

impl SerializeRow for RawBinds {
    fn serialize(
        &self,
        ctx: &RowSerializationContext<'_>,
        writer: &mut RowWriter<'_>,
    ) -> Result<(), SerializationError> {
        let expected = ctx.columns().len();
        if expected != self.values.len() {
            return Err(SerializationError::new(std::io::Error::other(format!(
                "the prepared statement has {expected} bind markers but {} values were bound",
                self.values.len()
            ))));
        }
        for value in &self.values {
            writer
                .make_cell_writer()
                .set_value(value)
                .map_err(SerializationError::new)?;
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
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
    use crate::schema::table::tests::column;
    use crate::schema::{ColumnKind, TableSchema};
    use crate::statement::MappingOptions;

    fn origin() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "src".to_owned(),
            columns: vec![
                column("id", "text", ColumnKind::PartitionKey, 0),
                column("value", "text", ColumnKind::Regular, -1),
                column("fruits", "map<text, int>", ColumnKind::Regular, -1),
                column("doc", "text", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    /// The target of SIT `features/02_explode_map`: the exploded key is a clustering column.
    fn target() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "dst".to_owned(),
            columns: vec![
                column("id", "text", ColumnKind::PartitionKey, 0),
                column("fruit", "text", ColumnKind::Clustering, 0),
                column("value", "text", ColumnKind::Regular, -1),
                column("fruit_qty", "int", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    fn explode_options() -> MappingOptions {
        MappingOptions {
            explode_map: Some((
                "fruits".to_owned(),
                "fruit".to_owned(),
                "fruit_qty".to_owned(),
            )),
            ..MappingOptions::default()
        }
    }

    fn plan_for(options: &MappingOptions) -> Result<TargetKeyPlan, CdmError> {
        let mapping = ColumnMapping::resolve(&origin(), &target(), options)?;
        let select = TargetSelectByPk::new(&mapping)?;
        TargetKeyPlan::resolve(&mapping, &select)
    }

    fn text(value: &str) -> RawCell {
        RawCell::new(value.as_bytes().to_vec())
    }

    fn origin_row() -> Row {
        Row::new(vec![
            text("key1"),
            text("valueA"),
            RawCell::NULL,
            RawCell::NULL,
        ])
    }

    #[test]
    fn sch_006_an_exploded_map_key_column_is_a_permitted_primary_key_source() {
        let plan = plan_for(&explode_options())
            .expect("SCH-006 admits an explode-map key column as a primary-key source");
        assert_eq!(
            plan.slots(),
            [TargetKeySlot::Origin(0), TargetKeySlot::ExplodeKey],
            "the key binds the partition key from the origin row and the clustering column from \
             the exploded entry"
        );
        assert!(plan.explodes());
    }

    #[test]
    fn fea_022_the_exploded_key_fills_its_own_key_slot() {
        let plan = plan_for(&explode_options()).unwrap();
        let key = plan.key_of(
            &origin_row(),
            ExplodedKeyParts {
                key: Some(b"apples"),
                value: None,
            },
        );
        assert_eq!(key.values(), [text("key1"), text("apples")]);

        // FEA-020: the same origin row, a different entry, a different target row.
        let other = plan.key_of(
            &origin_row(),
            ExplodedKeyParts {
                key: Some(b"oranges"),
                value: None,
            },
        );
        assert_ne!(key.values(), other.values());
    }

    #[test]
    fn fea_022_the_exploded_value_fills_a_key_slot_when_it_is_a_key_column() {
        let mut target = target();
        // The value half of the entry as the clustering column instead of the key half.
        target.columns = vec![
            column("id", "text", ColumnKind::PartitionKey, 0),
            column("fruit_qty", "int", ColumnKind::Clustering, 0),
            column("fruit", "text", ColumnKind::Regular, -1),
        ];
        let mapping = ColumnMapping::resolve(&origin(), &target, &explode_options()).unwrap();
        let select = TargetSelectByPk::new(&mapping).unwrap();
        let plan = TargetKeyPlan::resolve(&mapping, &select).unwrap();
        assert_eq!(
            plan.slots(),
            [TargetKeySlot::Origin(0), TargetKeySlot::ExplodeValue]
        );
        let qty = 3_i32.to_be_bytes();
        let key = plan.key_of(
            &origin_row(),
            ExplodedKeyParts {
                key: Some(b"apples"),
                value: Some(&qty),
            },
        );
        assert_eq!(key.values(), [text("key1"), RawCell::new(qty.to_vec())]);
    }

    #[test]
    fn fea_022_an_unexploded_key_leaves_the_exploded_component_null() {
        // What `CqlRowSource` produces before the row is exploded: a key that identifies no target
        // row, which `CqlRowSink::fetch` answers as absent rather than looking up the wrong one.
        let plan = plan_for(&explode_options()).unwrap();
        let key = plan.key_of(&origin_row(), ExplodedKeyParts::NONE);
        assert_eq!(key.values(), [text("key1"), RawCell::NULL]);
    }

    #[test]
    fn sch_006_a_constant_primary_key_component_takes_no_slot() {
        // FEA-012 inlines a constant as a literal, so it has no bind marker and nothing to derive.
        let options = MappingOptions {
            constants: vec![("fruit".to_owned(), "'apples'".to_owned())],
            ..MappingOptions::default()
        };
        let plan = plan_for(&options).expect("SCH-006 admits a constant key component");
        assert_eq!(plan.slots(), [TargetKeySlot::Origin(0)]);
        assert!(!plan.explodes());
        assert_eq!(
            plan.key_of(&origin_row(), ExplodedKeyParts::NONE).values(),
            [text("key1")]
        );
    }

    #[test]
    fn sch_006_an_extracted_json_property_is_still_not_a_permitted_key_source() {
        // The narrowing, not the removal: `SCH-006` names three sources, and extract-JSON
        // (`FEA-030`) is not one of them.
        let options = MappingOptions {
            extract_json: Some(("doc".to_owned(), "fruit".to_owned())),
            ..MappingOptions::default()
        };
        let error = plan_for(&options).expect_err("an extracted property cannot supply a key");
        assert_eq!(error.kind(), ErrorKind::SchemaMismatch);
        assert!(
            error.to_string().contains("an extracted JSON property"),
            "{error}"
        );
        assert!(error.to_string().contains("SCH-006"), "{error}");
    }
}
