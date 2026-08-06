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

/// Which origin projection positions make up the target primary key, in bind order.
#[derive(Debug, Clone)]
struct KeyPlan {
    origin_indices: Vec<usize>,
}

impl KeyPlan {
    /// Resolves the plan from the mapping and the lookup statement built from it.
    fn resolve(mapping: &ColumnMapping, select: &TargetSelectByPk) -> Result<Self, CdmError> {
        let mut origin_indices = Vec::with_capacity(select.bound_key_columns().len());
        for column in select.bound_key_columns() {
            match mapping.source_of(column) {
                Some(TargetSource::Origin(index)) => origin_indices.push(*index),
                other => {
                    return Err(CdmError::new(
                        ErrorKind::SchemaMismatch,
                        format!(
                            "the target primary-key column `{column}` is supplied by {} rather \
                             than by an origin column, so a row's key cannot be derived from the \
                             origin row alone (SCH-006).",
                            match other {
                                Some(TargetSource::ExplodeKey | TargetSource::ExplodeValue) =>
                                    "an exploded map entry",
                                Some(TargetSource::ExtractJson(_)) => "an extracted JSON property",
                                Some(TargetSource::Constant(_)) => "a constant",
                                _ => "nothing",
                            }
                        ),
                    )
                    .with_context(|c| c.with_side(Side::Target).with_column(column.clone())));
                }
            }
        }
        Ok(Self { origin_indices })
    }

    /// The key of one origin row.
    fn key_of(&self, row: &Row) -> PrimaryKey {
        PrimaryKey::new(
            self.origin_indices
                .iter()
                .map(|index| row.get(*index).cloned().unwrap_or(RawCell::NULL))
                .collect(),
        )
    }
}

/// The origin side of a run: a paged token-range scan (`PLG-005`, `FEA-060`).
#[derive(Debug)]
pub struct CqlRowSource {
    session: Arc<Session>,
    statement: PreparedStatement,
    token_kind: TokenKind,
    key_plan: KeyPlan,
}

impl CqlRowSource {
    /// Prepares the range scan.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] if the statement does not prepare, and [`ErrorKind::SchemaMismatch`] if
    /// the target primary key cannot be derived from an origin row. Both are startup failures: a
    /// statement that will never prepare, or a key that can never be built, must not be discovered
    /// on the first row.
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
            key_plan: KeyPlan::resolve(mapping, target_select)?,
        })
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
    key_plan: KeyPlan,
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
                let key = self.key_plan.key_of(&origin);
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
