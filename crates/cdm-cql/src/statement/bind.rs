//! Binding a row into the target statement (`MIG-011`..`MIG-014`, `ERR-005`).
//!
//! # `UNSET`, never `NULL`
//!
//! Binding `NULL` writes a tombstone. Binding `UNSET` writes nothing. For a migration that is not
//! a nuance: a table with twenty nullable columns, migrated with `NULL` binds, gains up to twenty
//! tombstones per row, and the target's read latency degrades until compaction catches up — which,
//! at petabyte scale, is weeks. `MIG-012` therefore requires a `null` value **or an empty
//! collection** to be bound as `UNSET`.
//!
//! The rule is enforced by construction rather than by discipline. [`BoundValue`] has exactly two
//! variants, `Unset` and `Value`; **there is no `Null`**, so no code path in this crate can express
//! a null bind, and the only place the driver's `set_null` could be called is a function that does
//! not exist. A test (`mig_012_no_production_path_can_bind_null`) sweeps the crate's sources to keep
//! it that way.
//!
//! # Zero-copy passthrough (`MIG-040`)
//!
//! [`BoundValue::Value`] holds a `Cow`. When the conversion plan for a column is the identity —
//! which is the common case, since most migrations do not change types — the borrowed frame slice
//! goes straight into the write buffer: no decode, no re-encode, no allocation. A converting plan
//! produces `Cow::Owned`. `mig_040_an_identity_plan_binds_the_frame_slice_itself` asserts the
//! borrow by pointer identity, so the fast path cannot be lost silently.
//!
//! # Counters are structurally excluded from retry
//!
//! A counter update is not idempotent: retrying one double-counts, and no amount of care at the
//! call site makes that safe (`CON-012`, `MIG-032`). So [`Binder::bind`] does not return one type
//! that is sometimes retryable — it returns [`Bound`], whose counter arm carries a
//! [`CounterWrite`]. [`Idempotent`] is a sealed trait implemented only for [`IdempotentWrite`], so
//! a retry helper written as `fn retry<W: Idempotent>(…)` cannot be handed a counter write at all.
//! Deleting a runtime `if is_counter` check is a one-line edit; deleting a trait bound does not
//! compile.

use std::borrow::Cow;
use std::fmt;

use cdm_codec::{ConversionPlan, CqlTypeInfo, Planner};
use cdm_core::{CdmError, ErrorKind, PrimaryKey, RawCell, Row, Side};
use scylla::serialize::row::{RowSerializationContext, SerializeRow};
use scylla::serialize::writers::RowWriter;
use scylla::serialize::SerializationError;

use crate::schema::ColumnMeta;

use super::mapping::{ColumnMapping, TargetSource};
use super::select::TargetSelectByPk;
use super::upsert::{BindSlot, TargetUpsert};

/// One bound parameter: a serialised value, or `UNSET` (`MIG-012`).
///
/// There is deliberately no `Null` variant; see this module's documentation for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundValue<'a> {
    /// The bind marker is left unset, so the write touches the column at all.
    ///
    /// This is what a `null` origin value and an empty collection both become.
    Unset,
    /// The bind marker carries these serialised bytes.
    ///
    /// `Cow::Borrowed` is the zero-copy passthrough of `MIG-040`; `Cow::Owned` is what a
    /// converting plan or a substituted key value produces.
    Value(Cow<'a, [u8]>),
}

impl BoundValue<'_> {
    /// Whether the marker is left unset.
    pub const fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }

    /// The serialised bytes, or `None` when unset.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Unset => None,
            Self::Value(bytes) => Some(bytes),
        }
    }
}

/// A row that binding reads from, whatever produced it.
///
/// Two implementations matter: [`RawRow`](crate::raw::RawRow), straight off the response frame,
/// which is what makes `MIG-040` possible; and [`Row`], which is what a feature that rewrote the
/// row (explode map, extract JSON) hands on. The trait is over `'frame` rather than `&self` so a
/// borrowed cell can outlive the call and end up in a [`BoundValue::Value`] without a copy.
pub trait SourceRow<'frame> {
    /// The cell at `index`: `None` if the row is narrower than that, `Some(None)` if the cell is
    /// CQL `NULL`, `Some(Some(bytes))` otherwise.
    ///
    /// The nesting is deliberate. Collapsing "absent" and "null" is precisely the mistake that
    /// `MIG-012` exists to prevent.
    fn cell(&self, index: usize) -> Option<Option<&'frame [u8]>>;

    /// How many cells the row has.
    fn width(&self) -> usize;
}

impl<'frame> SourceRow<'frame> for crate::raw::RawRow<'frame, '_> {
    fn cell(&self, index: usize) -> Option<Option<&'frame [u8]>> {
        self.cell(index).map(|cell| cell.bytes)
    }

    fn width(&self) -> usize {
        self.len()
    }
}

impl<'frame> SourceRow<'frame> for &'frame Row {
    fn cell(&self, index: usize) -> Option<Option<&'frame [u8]>> {
        self.get(index)
            .map(|cell| cell.bytes().map(|bytes| &**bytes))
    }

    fn width(&self) -> usize {
        self.len()
    }
}

/// What a target primary-key column gets when the origin had no value (`MIG-013`).
///
/// A key column cannot be `NULL` and cannot be `UNSET` — Cassandra rejects both — so a null in a
/// key is either substituted or the record is an error. Java substitutes only two types, and only
/// one of them is configurable; that is reproduced exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MissingKeyPolicy {
    /// `transform.missing_key_ts_replace`, in epoch **milliseconds**.
    ///
    /// Milliseconds, not microseconds, because Java reads the property with `Long` and passes it to
    /// `Instant.ofEpochMilli`. A `timestamp` column is serialised as epoch milliseconds too, so the
    /// value is used unchanged.
    pub missing_key_ts_replace: Option<i64>,
}

impl MissingKeyPolicy {
    /// The substitute for a null in a key column of `cql_type`, or `None` if there is none.
    ///
    /// `text`/`ascii` become the empty string — Java's hard-coded `defaultForMissingString`, which
    /// is not configurable there and is not made configurable here. `timestamp` becomes the
    /// configured replacement. Every other type has no defensible substitute: silently inventing a
    /// UUID or a zero integer would merge distinct origin rows into one target row.
    ///
    /// Visible to the crate because the write path is not the only caller: `TargetKeyPlan` applies
    /// the same substitution when it derives the primary key a validate run looks the target row up
    /// by, and the two must agree byte for byte or validate reports every substituted row missing.
    pub(crate) fn substitute(self, cql_type: &CqlTypeInfo) -> Option<Vec<u8>> {
        match cql_type {
            CqlTypeInfo::Text | CqlTypeInfo::Ascii => Some(Vec::new()),
            CqlTypeInfo::Timestamp => self
                .missing_key_ts_replace
                .map(|millis| millis.to_be_bytes().to_vec()),
            _ => None,
        }
    }
}

/// The per-row inputs binding needs that do not come from the origin row itself.
///
/// All of them are `Option`, and `None` uniformly means "this run does not produce one", which
/// binds `UNSET` rather than `NULL` (`MIG-012`).
#[derive(Debug, Clone, Copy, Default)]
pub struct BindInputs<'a> {
    /// The row's TTL in seconds (`FEA-040`).
    pub ttl: Option<i32>,
    /// The row's writetime in microseconds (`FEA-040`).
    pub writetime: Option<i64>,
    /// The key of the exploded map entry this record represents (`FEA-020`).
    pub explode_key: Option<&'a [u8]>,
    /// The value of the exploded map entry this record represents (`FEA-020`).
    pub explode_value: Option<&'a [u8]>,
    /// The property extracted from a JSON document (`FEA-030`).
    pub extracted_json: Option<&'a [u8]>,
    /// The record's primary key, carried only so that a failure can identify the row without
    /// naming its contents (`SEC-002`).
    pub key: Option<&'a PrimaryKey>,
}

/// One target column, resolved for binding.
#[derive(Debug, Clone)]
struct ColumnBind {
    name: String,
    cql_type: CqlTypeInfo,
    declared_type: String,
    plan: ConversionPlan,
    source: TargetSource,
    is_key: bool,
    unset_when_empty: bool,
    is_map: bool,
}

/// The resolved binding plan for one target statement (`MIG-011`).
#[derive(Debug, Clone)]
pub struct Binder {
    statement: TargetUpsert,
    columns: Vec<ColumnBind>,
    missing_key: MissingKeyPolicy,
    map_remove_null_value: bool,
}

impl Binder {
    /// Resolves the plan: one conversion per mapped column pair, computed once (`CDC-010`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] when a column's declared type does not parse. That is a
    /// disagreement between cdm-rs and `system_schema`, not bad data, and it must stop the run
    /// before any row is written.
    pub fn new(
        mapping: &ColumnMapping,
        statement: TargetUpsert,
        planner: &Planner,
        missing_key: MissingKeyPolicy,
        map_remove_null_value: bool,
    ) -> Result<Self, CdmError> {
        let mut columns = Vec::with_capacity(mapping.target_columns().len());
        for (index, column) in mapping.target_columns().iter().enumerate() {
            let cql_type = parse_type(column, Side::Target)?;
            let source = mapping
                .source(index)
                .cloned()
                .unwrap_or(TargetSource::Absent);
            let plan = match &source {
                TargetSource::Origin(origin_index) => {
                    let origin_column = mapping
                        .origin_columns()
                        .get(*origin_index)
                        .ok_or_else(|| out_of_range(*origin_index, mapping))?;
                    let origin_type = parse_type(origin_column, Side::Origin)?;
                    planner.plan_types(&origin_type, &cql_type)
                }
                // Everything else already arrives in the target's representation: a constant is
                // parsed against the target type at startup, an exploded entry is converted by
                // `ExplodePlan`, an extracted property is encoded against the target type.
                _ => ConversionPlan::Passthrough,
            };
            columns.push(ColumnBind {
                name: column.name.clone(),
                declared_type: column.cql_type.clone(),
                unset_when_empty: is_java_collection(&cql_type),
                is_map: matches!(cql_type, CqlTypeInfo::Map { .. }),
                cql_type,
                plan,
                source,
                is_key: column.kind.is_key(),
            });
        }
        Ok(Self {
            statement,
            columns,
            missing_key,
            map_remove_null_value,
        })
    }

    /// The statement this binder binds into.
    pub const fn statement(&self) -> &TargetUpsert {
        &self.statement
    }

    /// Binds one origin row (`MIG-011`..`MIG-014`).
    ///
    /// The returned value is a [`Bound`], not a `BoundWrite`: whether the write may be retried is
    /// a property of the statement, and expressing it in the type is what keeps a counter write out
    /// of a retry loop (`CON-012`).
    ///
    /// # Errors
    ///
    /// A [`BindFailure`], which carries everything `ERR-005` requires and nothing `SEC-002`
    /// forbids. Record-level: the engine counts `ERROR` and carries on.
    pub fn bind<'frame, R>(
        &self,
        row: &R,
        inputs: BindInputs<'frame>,
    ) -> Result<Bound<'frame>, BindFailure>
    where
        R: SourceRow<'frame>,
    {
        let mut values = Vec::with_capacity(self.statement.bind_count());
        for (bind_index, slot) in self.statement.slots().iter().enumerate() {
            let value = match *slot {
                BindSlot::Column(index) | BindSlot::KeyColumn(index) => {
                    let column = self.columns.get(index).ok_or_else(|| {
                        self.failure(bind_index, index, "<unknown>", "internal", &inputs, None)
                    })?;
                    self.bind_column(column, row, &inputs).map_err(|cause| {
                        self.column_failure(bind_index, index, column, &inputs, cause)
                    })?
                }
                BindSlot::Ttl => inputs
                    .ttl
                    .map_or(BoundValue::Unset, |ttl| owned(ttl.to_be_bytes().to_vec())),
                BindSlot::Writetime => inputs.writetime.map_or(BoundValue::Unset, |writetime| {
                    owned(writetime.to_be_bytes().to_vec())
                }),
            };
            values.push(value);
        }

        let write = BoundWrite { values };
        Ok(if self.statement.is_counter() {
            Bound::Counter(CounterWrite(write))
        } else {
            Bound::Idempotent(IdempotentWrite(write))
        })
    }

    /// Resolves the target key columns [`TargetSelectByPk`] binds, once (`MIG-031`, `VAL-001`).
    ///
    /// The lookup is by name, which is why it happens here and not per row: the select statement
    /// reports the columns it left bind markers for — a constant key component is inlined, so it
    /// is *not* among them (`FEA-012`) — and those names have to be turned into positions in this
    /// binder's plan exactly once.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if a key column the statement binds is not a target column of this
    /// binder, which can only happen if the two were built from different mappings.
    pub fn key_binding(&self, select: &TargetSelectByPk) -> Result<KeyBinding, CdmError> {
        let mut indices = Vec::with_capacity(select.bound_key_columns().len());
        for name in select.bound_key_columns() {
            let index = self
                .columns
                .iter()
                .position(|column| &column.name == name)
                .ok_or_else(|| {
                    CdmError::new(
                        ErrorKind::Internal,
                        format!(
                            "the target lookup binds key column `{name}`, which is not a target \
                             column of this binder; the statement and the binder were built from \
                             different mappings"
                        ),
                    )
                })?;
            indices.push(index);
        }
        Ok(KeyBinding { indices })
    }

    /// Binds one row's primary key into [`TargetSelectByPk`] (`MIG-031`, `VAL-001`).
    ///
    /// Shares [`Binder::bind`]'s conversion plans and missing-key policy, so the key the lookup
    /// searches for is byte-identical to the key the write would use — which is the whole point:
    /// a counter delta read against a differently-encoded key silently reads zero and doubles the
    /// counter.
    ///
    /// # Errors
    ///
    /// A [`BindFailure`], on the same terms as [`Binder::bind`].
    pub fn bind_key<'frame, R>(
        &self,
        binding: &KeyBinding,
        row: &R,
        inputs: BindInputs<'frame>,
    ) -> Result<BoundWrite<'frame>, BindFailure>
    where
        R: SourceRow<'frame>,
    {
        let mut values = Vec::with_capacity(binding.indices.len());
        for (bind_index, &index) in binding.indices.iter().enumerate() {
            let column = self.columns.get(index).ok_or_else(|| {
                self.failure(bind_index, index, "<unknown>", "internal", &inputs, None)
            })?;
            let value = self
                .bind_column(column, row, &inputs)
                .map_err(|cause| self.column_failure(bind_index, index, column, &inputs, cause))?;
            values.push(value);
        }
        Ok(BoundWrite { values })
    }

    /// One column's value, before it becomes a [`BoundValue`].
    fn bind_column<'frame, R>(
        &self,
        column: &ColumnBind,
        row: &R,
        inputs: &BindInputs<'frame>,
    ) -> Result<BoundValue<'frame>, Option<CdmError>>
    where
        R: SourceRow<'frame>,
    {
        let raw: Option<Cow<'frame, [u8]>> = match &column.source {
            TargetSource::Origin(index) => {
                let cell = row.cell(*index).ok_or(None)?;
                match cell {
                    None => None,
                    Some(bytes) => Some(Self::convert(column, bytes).map_err(Some)?),
                }
            }
            TargetSource::ExplodeKey => inputs.explode_key.map(Cow::Borrowed),
            TargetSource::ExplodeValue => inputs.explode_value.map(Cow::Borrowed),
            TargetSource::ExtractJson(_) => inputs.extracted_json.map(Cow::Borrowed),
            // A constant never reaches the binder: `MIG-010` inlines it in the statement text, so
            // it has no bind marker at all.
            TargetSource::Constant(_) | TargetSource::Absent => None,
        };

        let Some(bytes) = raw else {
            return self.absent_value(column).map_err(Some);
        };

        // MIG-014, applied before the emptiness test: a map whose every value was null becomes an
        // empty map, and an empty map is UNSET. Java reaches the same place by a different route.
        let bytes = if self.map_remove_null_value && column.is_map {
            strip_null_map_values(bytes).map_err(Some)?
        } else {
            bytes
        };

        if column.unset_when_empty && is_empty_collection(&bytes) {
            if column.is_key {
                // A key column that is an empty collection cannot be unset; the server rejects it.
                return Err(Some(key_cannot_be_unset(column)));
            }
            return Ok(BoundValue::Unset);
        }
        Ok(BoundValue::Value(bytes))
    }

    /// Applies the column's conversion plan, preserving the frame borrow when it is the identity.
    fn convert<'frame>(
        column: &ColumnBind,
        bytes: &'frame [u8],
    ) -> Result<Cow<'frame, [u8]>, CdmError> {
        if column.plan.is_identity() {
            // MIG-040: the whole point. No decode, no re-encode, no allocation.
            return Ok(Cow::Borrowed(bytes));
        }
        let converted = column.plan.apply(&RawCell::new(bytes.to_vec()))?;
        Ok(Cow::Owned(
            converted.bytes().map(|b| b.to_vec()).unwrap_or_default(),
        ))
    }

    /// What a column with no value binds (`MIG-012`, `MIG-013`).
    fn absent_value<'frame>(&self, column: &ColumnBind) -> Result<BoundValue<'frame>, CdmError> {
        if !column.is_key {
            return Ok(BoundValue::Unset);
        }
        self.missing_key
            .substitute(&column.cql_type)
            .map(|bytes| BoundValue::Value(Cow::Owned(bytes)))
            .ok_or_else(|| missing_key_error(column))
    }

    fn column_failure(
        &self,
        bind_index: usize,
        column_index: usize,
        column: &ColumnBind,
        inputs: &BindInputs<'_>,
        cause: Option<CdmError>,
    ) -> BindFailure {
        self.failure(
            bind_index,
            column_index,
            &column.name,
            &column.declared_type,
            inputs,
            cause,
        )
    }

    fn failure(
        &self,
        bind_index: usize,
        column_index: usize,
        column: &str,
        cql_type: &str,
        inputs: &BindInputs<'_>,
        cause: Option<CdmError>,
    ) -> BindFailure {
        BindFailure(Box::new(BindFailureDetail {
            column: column.to_owned(),
            cql_type: cql_type.to_owned(),
            bind_index,
            column_index,
            statement: self.statement.cql().to_owned(),
            key: inputs.key.cloned(),
            cause,
        }))
    }
}

/// The target key columns [`TargetSelectByPk`] binds, resolved to binder positions.
///
/// Held rather than recomputed because `MIG-031` performs one target lookup *per row* on a counter
/// table: a name lookup there would be a string comparison per key column per row, which
/// `ARCHITECTURE.md` §5.5 exists to keep off the hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    indices: Vec<usize>,
}

impl KeyBinding {
    /// How many key columns are bound. Shorter than the target primary key whenever a constant
    /// column supplies a component (`FEA-012`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether every key component is inlined, so the lookup binds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// The outcome of binding: a write, typed by whether it may be retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound<'a> {
    /// An ordinary write. Safe to batch and to retry (`CON-011`).
    Idempotent(IdempotentWrite<'a>),
    /// A counter write. Never batched, never retried (`CON-012`, `MIG-032`).
    Counter(CounterWrite<'a>),
}

impl<'a> Bound<'a> {
    /// The bound values, whichever arm this is.
    pub const fn values(&self) -> &BoundWrite<'a> {
        match self {
            Self::Idempotent(write) => &write.0,
            Self::Counter(write) => &write.0,
        }
    }
}

/// A bound write that may be retried and batched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotentWrite<'a>(BoundWrite<'a>);

/// A bound counter write.
///
/// Deliberately **not** [`Idempotent`]: re-executing it adds the delta a second time, so a generic
/// retry or batch helper cannot accept one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterWrite<'a>(BoundWrite<'a>);

impl<'a> IdempotentWrite<'a> {
    /// The bound values.
    pub const fn values(&self) -> &BoundWrite<'a> {
        &self.0
    }

    /// Copies every borrowed value so the write no longer depends on the frame it was read from.
    ///
    /// The zero-copy path of `MIG-040` is the default and must stay so; this is the escape hatch
    /// for the case where it cannot hold — a feature that rewrites the row produces values owned by
    /// a per-row buffer, and a write that outlives that buffer has to own them. Calling it on a
    /// passthrough write would silently undo `MIG-040`, so it is called in exactly one place, from
    /// the migrate job's feature path, and never from its fast path.
    #[must_use]
    pub fn into_owned(self) -> IdempotentWrite<'static> {
        IdempotentWrite(self.0.into_owned())
    }
}

impl<'a> CounterWrite<'a> {
    /// The bound values.
    pub const fn values(&self) -> &BoundWrite<'a> {
        &self.0
    }

    /// Copies every borrowed value; see [`IdempotentWrite::into_owned`].
    #[must_use]
    pub fn into_owned(self) -> CounterWrite<'static> {
        CounterWrite(self.0.into_owned())
    }
}

mod sealed {
    /// Keeps [`Idempotent`](super::Idempotent) closed: a downstream crate cannot declare a counter
    /// write retryable by implementing the trait for it.
    pub trait Sealed {}
    impl Sealed for super::IdempotentWrite<'_> {}
}

/// A write that may be re-executed without changing the result.
///
/// Sealed, and implemented only for [`IdempotentWrite`]. Write a retry or batch helper as
/// `fn f<W: Idempotent>(w: W)` and a counter write is excluded at compile time (`CON-012`).
pub trait Idempotent: sealed::Sealed {}

impl Idempotent for IdempotentWrite<'_> {}

/// The bound parameters of one statement, in bind order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundWrite<'a> {
    values: Vec<BoundValue<'a>>,
}

impl<'a> BoundWrite<'a> {
    /// The bound parameters, in bind order.
    pub fn values(&self) -> &[BoundValue<'a>] {
        &self.values
    }

    /// How many parameters were bound.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Copies every borrowed value, detaching the write from the frame it was read from.
    ///
    /// A `Cow::Owned` value is moved rather than copied, so a write that was already owned costs
    /// nothing here.
    #[must_use]
    pub fn into_owned(self) -> BoundWrite<'static> {
        BoundWrite {
            values: self
                .values
                .into_iter()
                .map(|value| match value {
                    BoundValue::Unset => BoundValue::Unset,
                    BoundValue::Value(bytes) => BoundValue::Value(Cow::Owned(bytes.into_owned())),
                })
                .collect(),
        }
    }

    /// Whether nothing was bound.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl SerializeRow for BoundWrite<'_> {
    /// Writes the parameters straight into the frame.
    ///
    /// `set_unset` for [`BoundValue::Unset`] and `set_value` for a value. `set_null` is never
    /// called — it is not reachable from this crate at all, which is the whole of `MIG-012`.
    fn serialize(
        &self,
        ctx: &RowSerializationContext<'_>,
        writer: &mut RowWriter<'_>,
    ) -> Result<(), SerializationError> {
        let expected = ctx.columns().len();
        if expected != self.values.len() {
            return Err(SerializationError::new(ArityError {
                expected,
                actual: self.values.len(),
            }));
        }
        for value in &self.values {
            let cell = writer.make_cell_writer();
            match value {
                BoundValue::Unset => {
                    cell.set_unset();
                }
                BoundValue::Value(bytes) => {
                    cell.set_value(bytes).map_err(SerializationError::new)?;
                }
            }
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// The prepared statement wants a different number of parameters than were bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArityError {
    expected: usize,
    actual: usize,
}

impl fmt::Display for ArityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the prepared statement has {} bind markers but {} values were bound; the statement \
             and the binding plan disagree (MIG-011)",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for ArityError {}

/// A bind failure, with everything needed to diagnose it and nothing that leaks row data
/// (`ERR-005`, `SEC-002`).
///
/// # The tension between `ERR-005` and `SEC-002`
///
/// `ERR-005` asks for "the value, its type, the column name, the CQL type, the bind index and the
/// statement CQL", matching Java's diagnostics. `SEC-002` says row values must not be logged by
/// default. Both cannot hold, and `SEC-002` wins: the value is the one field that identifies a
/// customer's data, and every other field is enough to reproduce the failure. So the failure
/// carries the column, its CQL type, the bind index, the column index and the statement, plus the
/// **primary key** rather than the value — which is what `ARCHITECTURE.md` §13 asks a failing row
/// to be identified by, and which is strictly more useful for finding the row again.
/// The failure is boxed because it travels in the `Err` arm of the per-row hot path: a fat error
/// type would widen every `Result` the migrate loop moves, for a case that must be rare.
#[derive(Debug)]
pub struct BindFailure(Box<BindFailureDetail>);

#[derive(Debug)]
struct BindFailureDetail {
    column: String,
    cql_type: String,
    bind_index: usize,
    column_index: usize,
    statement: String,
    key: Option<PrimaryKey>,
    cause: Option<CdmError>,
}

impl BindFailure {
    /// The target column that failed to bind.
    pub fn column(&self) -> &str {
        &self.0.column
    }

    /// Its CQL type, as `system_schema` spells it.
    pub fn cql_type(&self) -> &str {
        &self.0.cql_type
    }

    /// The position of the bind marker in the statement.
    pub const fn bind_index(&self) -> usize {
        self.0.bind_index
    }

    /// The position of the column in the target column list.
    pub const fn column_index(&self) -> usize {
        self.0.column_index
    }

    /// The statement being bound.
    pub fn statement(&self) -> &str {
        &self.0.statement
    }

    /// The primary key of the offending row, when the caller supplied one.
    pub const fn key(&self) -> Option<&PrimaryKey> {
        self.0.key.as_ref()
    }

    /// The underlying conversion or substitution error, if there was one.
    pub const fn cause(&self) -> Option<&CdmError> {
        self.0.cause.as_ref()
    }

    /// Logs the failure at `ERROR`, once (`ERR-005`).
    pub fn log(&self) {
        tracing::error!(
            column = %self.0.column,
            cql_type = %self.0.cql_type,
            column_index = self.0.column_index,
            bind_index = self.0.bind_index,
            statement = %self.0.statement,
            primary_key = %self.0.key.as_ref().map_or_else(|| "<unknown>".to_owned(), ToString::to_string),
            "failed to bind a value (ERR-005)"
        );
    }
}

impl fmt::Display for BindFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to bind column `{}` of type {} at column index {} and bind index {} of \
             statement: {}",
            self.0.column,
            self.0.cql_type,
            self.0.column_index,
            self.0.bind_index,
            self.0.statement
        )?;
        if let Some(key) = &self.0.key {
            write!(f, " (primary key {key})")?;
        }
        if let Some(cause) = &self.0.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl From<BindFailure> for CdmError {
    /// A bind failure is a `Write` error: the row was read fine, the statement is fine, and the
    /// value could not be placed into it.
    fn from(failure: BindFailure) -> Self {
        let key = failure.key().cloned();
        let column = failure.column().to_owned();
        Self::new(ErrorKind::Write, failure.to_string()).with_context(|c| {
            let c = c.with_side(Side::Target).with_column(column);
            match key {
                Some(key) => c.with_primary_key(key),
                None => c,
            }
        })
    }
}

/// Whether a value of this type is unset when it is empty (`MIG-012`).
///
/// Exactly Java's `CqlData.isEmptyCollection`: the driver hands back a `java.util.Collection` for
/// `list` and `set` and a `java.util.Map` for `map`, and nothing else. A `tuple`, a UDT and a
/// `vector` are *not* collections there — they come back as `TupleValue`, `UdtValue` and
/// `CqlVector` — so an empty one of those is written, not unset. `cdm-codec`'s
/// `CqlTypeInfo::is_collection` is broader than that and must not be used here.
fn is_java_collection(cql_type: &CqlTypeInfo) -> bool {
    matches!(
        cql_type,
        CqlTypeInfo::List { .. } | CqlTypeInfo::Set { .. } | CqlTypeInfo::Map { .. }
    )
}

/// Whether the serialised collection has no elements.
///
/// A CQL collection is `[i32 element count][elements…]`. A shorter buffer than that cannot be a
/// well-formed collection; treating it as empty is the conservative reading, since the alternative
/// is binding a malformed value the server would reject.
fn is_empty_collection(bytes: &[u8]) -> bool {
    match bytes.get(..4) {
        Some(count) => {
            let count: [u8; 4] = count.try_into().unwrap_or([0; 4]);
            i32::from_be_bytes(count) == 0
        }
        None => true,
    }
}

/// Removes map entries whose value is null (`MIG-014`).
///
/// The serialised form is `[i32 count]([i32 klen] k [i32 vlen] v)*`, where a length of `-1` is a
/// null. Walking the bytes rather than decoding into typed values keeps the passthrough spirit:
/// the surviving keys and values are copied verbatim, never re-encoded.
fn strip_null_map_values(bytes: Cow<'_, [u8]>) -> Result<Cow<'_, [u8]>, CdmError> {
    let count = read_i32(&bytes, 0)?;
    let mut offset = 4usize;
    let mut kept: Vec<(usize, usize)> = Vec::new();
    let mut dropped = 0i32;

    for _ in 0..count.max(0) {
        let entry_start = offset;
        offset = skip_element(&bytes, offset)?.0;
        let (after_value, value_len) = skip_element(&bytes, offset)?;
        offset = after_value;
        if value_len < 0 {
            dropped += 1;
        } else {
            kept.push((entry_start, offset));
        }
    }

    if dropped == 0 {
        // Nothing to strip: the value keeps whatever borrow it arrived with, so a passthrough
        // column stays a passthrough column (`MIG-040`).
        return Ok(bytes);
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&count.saturating_sub(dropped).to_be_bytes());
    for (start, end) in kept {
        out.extend_from_slice(bytes.get(start..end).unwrap_or_default());
    }
    Ok(Cow::Owned(out))
}

/// Advances past one length-prefixed element, returning the new offset and the declared length.
fn skip_element(bytes: &[u8], offset: usize) -> Result<(usize, i32), CdmError> {
    let len = read_i32(bytes, offset)?;
    let next = offset.checked_add(4).ok_or_else(truncated)?;
    if len < 0 {
        return Ok((next, len));
    }
    let width = usize::try_from(len).map_err(|_| truncated())?;
    let end = next.checked_add(width).ok_or_else(truncated)?;
    if end > bytes.len() {
        return Err(truncated());
    }
    Ok((end, len))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, CdmError> {
    let end = offset.checked_add(4).ok_or_else(truncated)?;
    let slice = bytes.get(offset..end).ok_or_else(truncated)?;
    let array: [u8; 4] = slice.try_into().map_err(|_| truncated())?;
    Ok(i32::from_be_bytes(array))
}

fn truncated() -> CdmError {
    CdmError::new(
        ErrorKind::TypeConversion,
        "the serialised map is truncated: an element length runs past the end of the value \
         (MIG-014)",
    )
}

fn owned<'a>(bytes: Vec<u8>) -> BoundValue<'a> {
    BoundValue::Value(Cow::Owned(bytes))
}

pub(crate) fn parse_type(column: &ColumnMeta, side: Side) -> Result<CqlTypeInfo, CdmError> {
    CqlTypeInfo::parse(&column.cql_type)
        .map_err(|e| e.with_context(|c| c.with_side(side).with_column(column.name.clone())))
}

fn out_of_range(index: usize, mapping: &ColumnMapping) -> CdmError {
    CdmError::new(
        ErrorKind::Internal,
        format!(
            "the column mapping refers to origin column {index}, but the projection has {}",
            mapping.origin_columns().len()
        ),
    )
}

fn missing_key_error(column: &ColumnBind) -> CdmError {
    CdmError::new(
        ErrorKind::Write,
        format!(
            "target primary-key column `{}` of type {} is null on the origin and has no \
             substitute: only text and ascii keys default to the empty string, and a timestamp \
             key needs transform.missing_key_ts_replace (MIG-013).",
            column.name, column.cql_type
        ),
    )
    .with_context(|c| c.with_side(Side::Target).with_column(column.name.clone()))
}

fn key_cannot_be_unset(column: &ColumnBind) -> CdmError {
    CdmError::new(
        ErrorKind::Write,
        format!(
            "target primary-key column `{}` is an empty collection, which cannot be bound as \
             UNSET because a key must be present (MIG-012, MIG-013).",
            column.name
        ),
    )
    .with_context(|c| c.with_side(Side::Target).with_column(column.name.clone()))
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
    use cdm_codec::{CodecRegistry, PlannerOptions};
    use cdm_core::TableRef;

    use crate::schema::table::tests::column;
    use crate::schema::{ColumnKind, TableSchema};
    use crate::statement::upsert::tests::{counter_origin, counter_target};
    use crate::statement::{MappingOptions, StatementOptions, UsingClause};

    fn planner() -> Planner {
        Planner::new(
            CodecRegistry::with_builtins(&[], None).unwrap(),
            PlannerOptions::default(),
        )
    }

    fn table(name: &str, columns: Vec<ColumnMeta>) -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: name.to_owned(),
            columns,
            is_materialized_view: false,
        }
    }

    fn schema() -> TableSchema {
        table(
            "t",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("data", "text", ColumnKind::Regular, -1),
                column("tags", "set<text>", ColumnKind::Regular, -1),
                column("props", "map<text, text>", ColumnKind::Regular, -1),
            ],
        )
    }

    fn binder(missing_key: MissingKeyPolicy, map_remove_null_value: bool) -> Binder {
        let schema = schema();
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        Binder::new(
            &mapping,
            statement,
            &planner(),
            missing_key,
            map_remove_null_value,
        )
        .unwrap()
    }

    fn row(cells: Vec<Option<Vec<u8>>>) -> Row {
        Row::new(
            cells
                .into_iter()
                .map(|cell| cell.map_or(RawCell::NULL, RawCell::new))
                .collect(),
        )
    }

    /// `[count][len]bytes…` — a serialised list/set of text values.
    fn collection(elements: &[&str]) -> Vec<u8> {
        let mut out = i32::try_from(elements.len())
            .unwrap()
            .to_be_bytes()
            .to_vec();
        for element in elements {
            out.extend_from_slice(&i32::try_from(element.len()).unwrap().to_be_bytes());
            out.extend_from_slice(element.as_bytes());
        }
        out
    }

    /// `[count]([klen]k[vlen]v)*` — a serialised map, where `None` is a null value.
    fn map(entries: &[(&str, Option<&str>)]) -> Vec<u8> {
        let mut out = i32::try_from(entries.len()).unwrap().to_be_bytes().to_vec();
        for (key, value) in entries {
            out.extend_from_slice(&i32::try_from(key.len()).unwrap().to_be_bytes());
            out.extend_from_slice(key.as_bytes());
            match value {
                None => out.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(value) => {
                    out.extend_from_slice(&i32::try_from(value.len()).unwrap().to_be_bytes());
                    out.extend_from_slice(value.as_bytes());
                }
            }
        }
        out
    }

    #[test]
    fn mig_012_a_null_value_is_unset_never_null() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let source = row(vec![Some(7i32.to_be_bytes().to_vec()), None, None, None]);
        let bound = binder.bind(&&source, BindInputs::default()).unwrap();
        let values = bound.values().values();

        assert_eq!(values.len(), 4);
        assert_eq!(values[0], BoundValue::Value(Cow::Borrowed(&[0, 0, 0, 7])));
        assert!(values[1].is_unset(), "a null text column must be UNSET");
        assert!(values[2].is_unset());
        assert!(values[3].is_unset());
        assert_eq!(values[1].bytes(), None);
    }

    #[test]
    fn mig_012_an_empty_collection_is_unset_but_an_empty_string_is_not() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let source = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            Some(Vec::new()),
            Some(collection(&[])),
            Some(map(&[])),
        ]);
        let bound = binder.bind(&&source, BindInputs::default()).unwrap();
        let values = bound.values().values();

        assert_eq!(
            values[1],
            BoundValue::Value(Cow::Borrowed(&[][..])),
            "an empty text is a value, not an absence"
        );
        assert!(values[2].is_unset(), "an empty set writes no tombstone");
        assert!(values[3].is_unset(), "an empty map writes no tombstone");

        let populated = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            Some(b"v".to_vec()),
            Some(collection(&["a"])),
            Some(map(&[("k", Some("v"))])),
        ]);
        let bound = binder.bind(&&populated, BindInputs::default()).unwrap();
        assert!(bound.values().values().iter().all(|v| !v.is_unset()));
    }

    #[test]
    fn mig_012_a_tuple_or_vector_is_not_a_collection_for_the_unset_rule() {
        for declared in ["tuple<int, text>", "vector<float, 3>", "frozen<address>"] {
            let parsed = CqlTypeInfo::parse(declared).unwrap();
            assert!(
                !is_java_collection(&parsed),
                "{declared} must not be treated as a collection"
            );
            assert!(parsed.is_collection() || declared.starts_with("frozen"));
        }
        for declared in [
            "list<int>",
            "set<text>",
            "map<text, int>",
            "frozen<list<int>>",
        ] {
            assert!(is_java_collection(&CqlTypeInfo::parse(declared).unwrap()));
        }
    }

    #[test]
    fn mig_012_no_production_path_can_bind_null() {
        // `set_null` is the driver call that writes a tombstone. It must appear nowhere outside a
        // test, and `BoundValue` must have no `Null` variant to route into it.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut offenders = Vec::new();
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    stack.push(entry.unwrap().path());
                }
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            if production.contains("set_null(") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "set_null is reachable from {offenders:?}"
        );
    }

    /// One origin cell, as the generator produces it.
    ///
    /// The four states a migration actually meets. Collapsing any two of them is the mistake
    /// `MIG-012` exists to prevent: *absent* and *null* mean different things to the projection,
    /// and *empty* and *null* mean different things to the storage engine.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CellState {
        /// The row is narrower than the projection.
        Absent,
        /// CQL `NULL`.
        Null,
        /// An empty value: `""` for text, a zero-element collection otherwise.
        Empty,
        /// A value with content.
        Populated,
    }

    impl CellState {
        /// The bytes this state contributes at `index` of [`schema`], or `None` for `Absent`.
        ///
        /// The nested `Option` is the point rather than an accident: the outer one is "the row is
        /// this narrow", the inner one is CQL `NULL`, and `MIG-012` exists because those two are
        /// not the same thing. `SourceRow::cell` has the same shape for the same reason.
        #[allow(clippy::option_option)]
        fn cell(self, index: usize) -> Option<Option<Vec<u8>>> {
            let populated = match index {
                0 => 7i32.to_be_bytes().to_vec(),
                1 => b"payload".to_vec(),
                2 => collection(&["a", "b"]),
                _ => map(&[("k", Some("v"))]),
            };
            let empty = match index {
                0 | 1 => Vec::new(),
                2 => collection(&[]),
                _ => map(&[]),
            };
            match self {
                Self::Absent => None,
                Self::Null => Some(None),
                Self::Empty => Some(Some(empty)),
                Self::Populated => Some(Some(populated)),
            }
        }

        /// Whether `MIG-012` says the bind marker for column `index` must be left `UNSET`.
        ///
        /// The rule, written out rather than asked of the code under test: a null is `UNSET`, an
        /// empty *collection* is `UNSET`, and an empty *string* is a value. `Absent` never reaches
        /// this: a row narrower than the projection is a bind failure, not an unset column, which
        /// is `MIG-011`'s rule and is asserted separately below.
        fn must_be_unset(self, index: usize) -> bool {
            let is_collection = index >= 2;
            match self {
                Self::Absent | Self::Null => true,
                Self::Empty => is_collection,
                Self::Populated => false,
            }
        }
    }

    /// The wire form of a bound row: `-2` for unset, `-1` for null, a length otherwise.
    ///
    /// This is where the structural claim of `MIG-012` becomes an observable one. [`BoundValue`]
    /// has no `Null` variant, so a null bind should be unrepresentable — but "should be" is a
    /// claim about the source, and what reaches the server is bytes.
    fn wire_lengths(write: &BoundWrite<'_>) -> Vec<i32> {
        let mut buffer = Vec::new();
        let mut writer = RowWriter::new(&mut buffer);
        for value in write.values() {
            let cell = writer.make_cell_writer();
            match value {
                BoundValue::Unset => {
                    cell.set_unset();
                }
                BoundValue::Value(bytes) => {
                    cell.set_value(bytes).unwrap();
                }
            }
        }

        let mut lengths = Vec::new();
        let mut offset = 0usize;
        while offset + 4 <= buffer.len() {
            let length = i32::from_be_bytes(buffer[offset..offset + 4].try_into().unwrap());
            lengths.push(length);
            offset += 4;
            if length > 0 {
                offset += usize::try_from(length).unwrap_or(0);
            }
        }
        lengths
    }

    use proptest::prelude::*;

    proptest! {
        /// `TST-010`, `MIG-012`: whatever shape the origin row is, the write that leaves the
        /// binder contains no null bind — at the wire level, not merely in the type.
        ///
        /// The module claims a null bind is structurally impossible because [`BoundValue`] has no
        /// `Null` variant. That is a claim about the *source*, and a source-level claim is exactly
        /// the kind that survives a refactor while quietly ceasing to be true. So this asserts the
        /// bytes: every marker is `-2` (unset) or a length `>= 0`, and `-1` — CQL's null — never
        /// appears, over every row shape the four cell states produce across the four columns.
        #[test]
        fn tst_010_mig_012_no_generated_row_can_produce_a_null_bind(
            states in proptest::collection::vec(
                proptest::sample::select(
                    &[
                        CellState::Absent,
                        CellState::Null,
                        CellState::Empty,
                        CellState::Populated,
                    ][..],
                ),
                4..=4,
            ),
        ) {
            let binder = binder(MissingKeyPolicy::default(), false);
            // A row is a prefix of the projection, so `Absent` truncates: everything from the
            // first absent cell onwards is absent too, whatever the generator drew.
            let mut cells = Vec::new();
            for (index, state) in states.iter().enumerate() {
                match state.cell(index) {
                    Some(cell) => cells.push(cell),
                    None => break,
                }
            }
            let truncated = cells.len() < states.len();
            let source = row(cells);

            // Exactly two shapes may be refused, and both are refusals rather than silent
            // nulls: a row narrower than the projection (`MIG-011` — the failure mode that
            // would otherwise write the right data into the wrong column), and an `int`
            // primary key with no value and no substitute (`MIG-013`).
            let expected_refusal = truncated || states[0] == CellState::Null;
            let Ok(bound) = binder.bind(&&source, BindInputs::default()) else {
                prop_assert!(
                    expected_refusal,
                    "a bind must only fail for a short row or a missing key; states = {:?}",
                    states,
                );
                return Ok(());
            };
            prop_assert!(
                !expected_refusal,
                "a short row or a null key must be refused, not bound; states = {:?}",
                states,
            );

            let write = bound.values();
            prop_assert_eq!(write.len(), 4, "every target column gets a marker (MIG-011)");

            for (index, length) in wire_lengths(write).into_iter().enumerate() {
                prop_assert_ne!(
                    length,
                    -1,
                    "column {} was bound as NULL, which writes a tombstone (MIG-012)",
                    index,
                );
                prop_assert_eq!(
                    length == -2,
                    states[index].must_be_unset(index),
                    "column {} in state {:?} bound the wrong way",
                    index,
                    states[index],
                );
            }
        }

        /// `TST-010`, `MIG-012`: an empty *string* is a value and an empty *collection* is not.
        ///
        /// The pair a single "is it empty?" check would conflate, over generated content.
        #[test]
        fn tst_010_mig_012_emptiness_means_different_things_to_text_and_to_collections(
            text in ".*",
            elements in proptest::collection::vec("[a-z]{1,8}", 0..6),
        ) {
            let binder = binder(MissingKeyPolicy::default(), false);
            let borrowed: Vec<&str> = elements.iter().map(String::as_str).collect();
            let source = row(vec![
                Some(1i32.to_be_bytes().to_vec()),
                Some(text.as_bytes().to_vec()),
                Some(collection(&borrowed)),
                Some(map(&[])),
            ]);
            let bound = binder.bind(&&source, BindInputs::default()).unwrap();
            let values = bound.values().values();

            prop_assert!(
                !values[1].is_unset(),
                "a text value is never unset, however short"
            );
            prop_assert_eq!(values[1].bytes(), Some(text.as_bytes()));
            prop_assert_eq!(values[2].is_unset(), elements.is_empty());
            prop_assert!(values[3].is_unset(), "an empty map is always unset");
        }
    }

    #[test]
    fn mig_040_an_identity_plan_binds_the_frame_slice_itself() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let payload = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            Some(b"the quick brown fox".to_vec()),
            None,
            None,
        ]);
        let bound = binder.bind(&&payload, BindInputs::default()).unwrap();
        let values = bound.values().values();

        let source = payload.get(1).unwrap().bytes().unwrap().as_ptr();
        match &values[1] {
            BoundValue::Value(Cow::Borrowed(bytes)) => {
                assert_eq!(
                    bytes.as_ptr(),
                    source,
                    "the bound value must be the row's own bytes, not a copy"
                );
            }
            other => panic!("passthrough was lost; got {other:?}"),
        }
    }

    #[test]
    fn mig_040_a_converting_plan_owns_its_bytes() {
        let origin = table(
            "o",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("data", "int", ColumnKind::Regular, -1),
            ],
        );
        let target = table(
            "t",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("data", "text", ColumnKind::Regular, -1),
            ],
        );
        let mapping = ColumnMapping::resolve(&origin, &target, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let registry =
            CodecRegistry::with_builtins(&[cdm_codec::Codecset::IntString], None).unwrap();
        let planner = Planner::new(registry, PlannerOptions::default());
        let binder = Binder::new(
            &mapping,
            statement,
            &planner,
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let source = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            Some(42i32.to_be_bytes().to_vec()),
        ]);
        let bound = binder.bind(&&source, BindInputs::default()).unwrap();
        assert_eq!(
            bound.values().values()[1],
            BoundValue::Value(Cow::Owned(b"42".to_vec()))
        );
    }

    #[test]
    fn mig_013_a_null_text_key_becomes_the_empty_string() {
        let schema = table(
            "t",
            vec![
                column("id", "text", ColumnKind::PartitionKey, 0),
                column("data", "text", ColumnKind::Regular, -1),
            ],
        );
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let source = row(vec![None, Some(b"v".to_vec())]);
        let bound = binder.bind(&&source, BindInputs::default()).unwrap();
        assert_eq!(
            bound.values().values()[0],
            BoundValue::Value(Cow::Owned(Vec::new())),
            "a text key must be the empty string, never UNSET"
        );
    }

    #[test]
    fn mig_013_a_null_timestamp_key_uses_the_configured_replacement_or_fails() {
        let schema = table(
            "t",
            vec![
                column("ts", "timestamp", ColumnKind::PartitionKey, 0),
                column("data", "text", ColumnKind::Regular, -1),
            ],
        );
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();

        let unset = Binder::new(
            &mapping,
            statement.clone(),
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();
        let source = row(vec![None, None]);
        let failure = unset.bind(&&source, BindInputs::default()).unwrap_err();
        assert!(
            failure
                .cause()
                .unwrap()
                .message()
                .contains("transform.missing_key_ts_replace"),
            "{failure}"
        );

        let configured = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy {
                missing_key_ts_replace: Some(1_700_000_000_000),
            },
            false,
        )
        .unwrap();
        let bound = configured.bind(&&source, BindInputs::default()).unwrap();
        assert_eq!(
            bound.values().values()[0].bytes(),
            Some(&1_700_000_000_000i64.to_be_bytes()[..])
        );
    }

    #[test]
    fn mig_013_a_null_key_of_any_other_type_is_a_record_error() {
        let schema = table(
            "t",
            vec![
                column("id", "uuid", ColumnKind::PartitionKey, 0),
                column("data", "text", ColumnKind::Regular, -1),
            ],
        );
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy {
                missing_key_ts_replace: Some(1),
            },
            false,
        )
        .unwrap();

        let key = PrimaryKey::new(vec![RawCell::NULL]);
        let source = row(vec![None, None]);
        let failure = binder
            .bind(
                &&source,
                BindInputs {
                    key: Some(&key),
                    ..BindInputs::default()
                },
            )
            .unwrap_err();
        assert_eq!(failure.column(), "id");
        assert_eq!(failure.key(), Some(&key));
    }

    #[test]
    fn mig_014_map_entries_with_null_values_are_stripped_before_binding() {
        let binder = binder(MissingKeyPolicy::default(), true);
        let source = map(&[("a", Some("1")), ("b", None), ("c", Some("3"))]);
        let expected = map(&[("a", Some("1")), ("c", Some("3"))]);
        let input = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            None,
            None,
            Some(source),
        ]);
        let bound = binder.bind(&&input, BindInputs::default()).unwrap();
        assert_eq!(bound.values().values()[3].bytes(), Some(&expected[..]));
    }

    #[test]
    fn mig_014_a_map_whose_every_value_is_null_becomes_unset_not_an_empty_map() {
        let binder = binder(MissingKeyPolicy::default(), true);
        let input = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            None,
            None,
            Some(map(&[("a", None), ("b", None)])),
        ]);
        let bound = binder.bind(&&input, BindInputs::default()).unwrap();
        assert!(
            bound.values().values()[3].is_unset(),
            "stripping every entry leaves an empty map, and MIG-012 unsets that"
        );
    }

    #[test]
    fn mig_014_the_transform_is_off_by_default_and_leaves_the_map_alone() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let source = map(&[("a", Some("1")), ("b", None)]);
        let input = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            None,
            None,
            Some(source.clone()),
        ]);
        let bound = binder.bind(&&input, BindInputs::default()).unwrap();
        assert_eq!(bound.values().values()[3].bytes(), Some(&source[..]));
    }

    #[test]
    fn mig_014_a_truncated_map_is_a_record_error_not_a_panic() {
        let binder = binder(MissingKeyPolicy::default(), true);
        let input = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            None,
            None,
            Some(vec![0, 0, 0, 1, 0, 0, 0, 9, b'a']),
        ]);
        let failure = binder.bind(&&input, BindInputs::default()).unwrap_err();
        assert_eq!(failure.cause().unwrap().kind(), ErrorKind::TypeConversion);
    }

    #[test]
    fn mig_011_ttl_and_writetime_bind_after_the_columns_and_unset_when_absent() {
        let schema = schema();
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(
            &mapping,
            StatementOptions {
                using: UsingClause {
                    ttl: true,
                    timestamp: true,
                },
            },
        )
        .unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let source = row(vec![Some(1i32.to_be_bytes().to_vec()), None, None, None]);
        let bound = binder
            .bind(
                &&source,
                BindInputs {
                    ttl: Some(3600),
                    writetime: Some(1_700_000_000_000_000),
                    ..BindInputs::default()
                },
            )
            .unwrap();
        let values = bound.values().values();
        assert_eq!(values.len(), 6);
        assert_eq!(values[4].bytes(), Some(&3600i32.to_be_bytes()[..]));
        assert_eq!(
            values[5].bytes(),
            Some(&1_700_000_000_000_000i64.to_be_bytes()[..])
        );

        let without = binder.bind(&&source, BindInputs::default()).unwrap();
        assert!(without.values().values()[4].is_unset());
        assert!(without.values().values()[5].is_unset());
    }

    #[test]
    fn mig_011_an_exploded_entry_supplies_the_key_and_value_columns() {
        let origin = table(
            "o",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("m", "map<text, int>", ColumnKind::Regular, -1),
            ],
        );
        let target = table(
            "t",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("k", "text", ColumnKind::Clustering, 0),
                column("v", "int", ColumnKind::Regular, -1),
            ],
        );
        let options = MappingOptions {
            explode_map: Some(("m".to_owned(), "k".to_owned(), "v".to_owned())),
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin, &target, &options).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let source = row(vec![Some(1i32.to_be_bytes().to_vec()), None]);
        let value = 9i32.to_be_bytes();
        let bound = binder
            .bind(
                &&source,
                BindInputs {
                    explode_key: Some(b"alpha"),
                    explode_value: Some(&value),
                    ..BindInputs::default()
                },
            )
            .unwrap();
        let values = bound.values().values();
        assert_eq!(values[1].bytes(), Some(&b"alpha"[..]));
        assert_eq!(values[2].bytes(), Some(&value[..]));
    }

    #[test]
    fn mig_012_a_counter_write_is_typed_so_that_it_cannot_be_retried() {
        fn only_idempotent<W: Idempotent>(_write: W) {}

        let mapping = ColumnMapping::resolve(
            &counter_origin(),
            &counter_target(),
            &MappingOptions::default(),
        )
        .unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let source = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            Some(b"cc".to_vec()),
            Some(5i64.to_be_bytes().to_vec()),
        ]);
        let bound = binder.bind(&&source, BindInputs::default()).unwrap();
        match bound {
            Bound::Counter(write) => {
                // `only_idempotent(write)` here does not compile, which is the point.
                assert_eq!(write.values().len(), 3);
            }
            Bound::Idempotent(_) => panic!("a counter table must not produce an idempotent write"),
        }

        let plain = schema();
        let mapping = ColumnMapping::resolve(&plain, &plain, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();
        let plain_row = row(vec![Some(1i32.to_be_bytes().to_vec()), None, None, None]);
        let bound = binder.bind(&&plain_row, BindInputs::default()).unwrap();
        match bound {
            Bound::Idempotent(write) => only_idempotent(write),
            Bound::Counter(_) => panic!("a plain table must not produce a counter write"),
        }
    }

    #[test]
    fn mig_031_the_key_binding_binds_the_target_lookups_key_columns_in_its_order() {
        use crate::statement::TargetSelectByPk;

        let mapping = ColumnMapping::resolve(
            &counter_origin(),
            &counter_target(),
            &MappingOptions::default(),
        )
        .unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();
        let select = TargetSelectByPk::new(&mapping).unwrap();
        let binding = binder.key_binding(&select).unwrap();
        assert_eq!(binding.len(), 2);
        assert!(!binding.is_empty());

        let source = row(vec![
            Some(7i32.to_be_bytes().to_vec()),
            Some(b"cc".to_vec()),
            Some(5i64.to_be_bytes().to_vec()),
        ]);
        let key = binder
            .bind_key(&binding, &&source, BindInputs::default())
            .unwrap();
        assert_eq!(key.len(), 2, "the counter column is not part of the key");
        assert_eq!(key.values()[0].bytes(), Some(&7i32.to_be_bytes()[..]));
        assert_eq!(key.values()[1].bytes(), Some(&b"cc"[..]));
    }

    #[test]
    fn mig_031_a_constant_key_component_is_inlined_and_so_is_not_bound() {
        use crate::statement::TargetSelectByPk;

        let mut target = counter_target();
        target
            .columns
            .push(column("tenant", "text", ColumnKind::PartitionKey, 1));
        let options = MappingOptions {
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&counter_origin(), &target, &options).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();
        let binding = binder
            .key_binding(&TargetSelectByPk::new(&mapping).unwrap())
            .unwrap();
        assert_eq!(binding.len(), 2, "`tenant` is a literal, not a bind marker");
    }

    #[test]
    fn mig_040_into_owned_detaches_a_write_from_the_frame_it_borrowed() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let payload = row(vec![
            Some(1i32.to_be_bytes().to_vec()),
            Some(b"borrowed".to_vec()),
            None,
            None,
        ]);
        let Bound::Idempotent(write) = binder.bind(&&payload, BindInputs::default()).unwrap()
        else {
            panic!("a plain table binds an idempotent write")
        };
        let borrowed = write.values().values()[1].bytes().unwrap().as_ptr();
        let owned = write.into_owned();
        assert_eq!(owned.values().values()[1].bytes(), Some(&b"borrowed"[..]));
        assert_ne!(
            owned.values().values()[1].bytes().unwrap().as_ptr(),
            borrowed,
            "into_owned must copy, or the write still depends on the frame"
        );
        assert!(owned.values().values()[2].is_unset(), "UNSET survives");
    }

    #[test]
    fn err_005_a_bind_failure_names_the_column_type_indices_and_statement() {
        let schema = table(
            "t",
            vec![
                column("id", "uuid", ColumnKind::PartitionKey, 0),
                column("data", "text", ColumnKind::Regular, -1),
            ],
        );
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let key = PrimaryKey::new(vec![RawCell::new(vec![1, 2])]);
        let source = row(vec![None, None]);
        let failure = binder
            .bind(
                &&source,
                BindInputs {
                    key: Some(&key),
                    ..BindInputs::default()
                },
            )
            .unwrap_err();

        assert_eq!(failure.column(), "id");
        assert_eq!(failure.cql_type(), "uuid");
        assert_eq!(failure.bind_index(), 0);
        assert_eq!(failure.column_index(), 0);
        assert!(failure.statement().starts_with("INSERT INTO ks.t"));
        failure.log();

        let rendered = failure.to_string();
        assert!(rendered.contains("`id`"), "{rendered}");
        assert!(rendered.contains("uuid"), "{rendered}");
        assert!(rendered.contains("bind index 0"), "{rendered}");
        assert!(rendered.contains("INSERT INTO ks.t"), "{rendered}");
        assert!(rendered.contains("(0x0102)"), "{rendered}");

        let error: CdmError = failure.into();
        assert_eq!(error.kind(), ErrorKind::Write);
        assert_eq!(error.context().side, Some(Side::Target));
        assert_eq!(error.context().primary_key, Some(key));
    }

    #[test]
    fn sec_002_a_bind_failure_never_renders_the_value() {
        let schema = table(
            "t",
            vec![
                column("id", "uuid", ColumnKind::PartitionKey, 0),
                column("secret", "text", ColumnKind::Regular, -1),
            ],
        );
        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let binder = Binder::new(
            &mapping,
            statement,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();
        let source = row(vec![None, Some(b"hunter2".to_vec())]);
        let failure = binder.bind(&&source, BindInputs::default()).unwrap_err();
        assert!(
            !failure.to_string().contains("hunter2"),
            "row values must not reach a log line: {failure}"
        );
    }

    #[test]
    fn mig_011_a_row_shorter_than_the_projection_is_an_error_not_a_panic() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let source = row(vec![Some(vec![0, 0, 0, 1])]);
        let failure = binder.bind(&&source, BindInputs::default()).unwrap_err();
        assert_eq!(failure.column(), "data");
        assert!(failure.cause().is_none());
    }

    #[test]
    fn mig_012_the_raw_row_source_reports_null_and_absent_separately() {
        let source = row(vec![Some(b"v".to_vec()), None]);
        let view: &Row = &source;
        assert_eq!(SourceRow::cell(&view, 0), Some(Some(&b"v"[..])));
        assert_eq!(SourceRow::cell(&view, 1), Some(None));
        assert_eq!(SourceRow::cell(&view, 2), None);
        assert_eq!(SourceRow::width(&view), 2);
    }

    #[test]
    fn mig_011_the_serialised_row_is_unset_where_the_binding_is() {
        let binder = binder(MissingKeyPolicy::default(), false);
        let source = row(vec![Some(vec![0, 0, 0, 1]), None, None, None]);
        let bound = binder.bind(&&source, BindInputs::default()).unwrap();
        let write = bound.values();
        assert!(!SerializeRow::is_empty(write));
        assert_eq!(write.len(), 4);
        assert!(!write.is_empty());

        // The wire form: a four-byte value, then three `-2` (unset) markers.
        let mut buffer = Vec::new();
        let mut writer = RowWriter::new(&mut buffer);
        for value in write.values() {
            let cell = writer.make_cell_writer();
            match value {
                BoundValue::Unset => {
                    cell.set_unset();
                }
                BoundValue::Value(bytes) => {
                    cell.set_value(bytes).unwrap();
                }
            }
        }
        assert_eq!(&buffer[0..4], &4i32.to_be_bytes());
        assert_eq!(&buffer[8..12], &(-2i32).to_be_bytes());
        assert_eq!(&buffer[12..16], &(-2i32).to_be_bytes());
        assert_eq!(&buffer[16..20], &(-2i32).to_be_bytes());
    }

    #[test]
    fn err_005_an_arity_mismatch_explains_which_side_disagrees() {
        let error = ArityError {
            expected: 3,
            actual: 4,
        };
        let rendered = error.to_string();
        assert!(rendered.contains("3 bind markers"), "{rendered}");
        assert!(rendered.contains("4 values"), "{rendered}");
    }

    #[test]
    fn mig_014_an_empty_or_truncated_collection_reads_as_empty() {
        assert!(is_empty_collection(&[]));
        assert!(is_empty_collection(&[0, 0]));
        assert!(is_empty_collection(&0i32.to_be_bytes()));
        assert!(!is_empty_collection(&collection(&["a"])));
        assert_eq!(
            TableRef::new("ks", "t").to_string(),
            "ks.t",
            "the fixture's table reference is the one the mapping carries"
        );
    }
}
