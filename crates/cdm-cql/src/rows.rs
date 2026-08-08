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
//! # A substituted key is substituted on both sides (`MIG-013`)
//!
//! A `null` in a target primary-key column is written as a substitute, so the substitute — not the
//! `null` — is what the target row is keyed by. [`TargetKeyPlan`] therefore carries the
//! [`MissingKeyPolicy`] the write path binds with and applies it as it derives the key, and
//! [`TargetKeyPlan::substituted`] writes the same replacement back into the origin row wherever
//! that is unambiguous, so the comparison sees what the migration wrote rather than the `null` it
//! started from. Without the first, validate looks every substituted row up by a null key and
//! reports it missing; without the second, it finds the row and reports it mismatched.
//!
//! # A corrected row carries the origin's TTL and writetime (`VAL-018`)
//!
//! Autocorrect writes through [`CqlRowSink::write`], and `VAL-018` requires that write to carry the
//! origin row's TTL and writetime exactly as a migrate write does. The two values are resolved by
//! `FEA-040`..`FEA-046`, from `TTL(…)`/`WRITETIME(…)` cells the origin projection selects — which is
//! `cdm-feature`'s work, on the far side of the dependency edge (`ARCHITECTURE.md` §3). So the sink
//! holds a [`RowTimestamps`] rather than the plan itself, and the harness that resolved the plan for
//! the statement's `USING` clause hands the same plan in here. A sink built without one binds
//! `UNSET` for both, which is what an upsert with no `USING` clause has bind markers for anyway.
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
//! - `MIG-013` — the null-key substitution, in the key and in the row
//! - `MIG-030`, `MIG-031` — the counter delta
//! - `VAL-001` — the target lookup by primary key
//! - `VAL-018` — [`RowTimestamps`], bound by [`CqlRowSink::write`]

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
use crate::statement::bind::parse_type;
use crate::statement::{
    BindInputs, Binder, Bound, ColumnMapping, MissingKeyPolicy, OriginRangeSelect,
    TargetSelectByPk, TargetSource, TokenBound,
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

/// What a null in one bound key component is replaced with (`MIG-013`).
#[derive(Debug, Clone)]
struct KeySubstitute {
    /// The replacement, already in the target key column's representation — the very bytes
    /// [`Binder`] would bind for the same column, because both come from
    /// [`MissingKeyPolicy`]'s substitution.
    value: RawCell,
    /// Whether the replacement may also be written back into the origin row's cell.
    ///
    /// Only when the origin column's type is the target key column's type, so the bytes mean the
    /// same thing on both sides, and only when that origin position feeds exactly one target
    /// column, so writing it cannot change what some other column binds. Everything else keeps
    /// the origin cell null and substitutes into the key alone.
    mirror: bool,
}

/// How the bound components of the target primary key are derived, in bind order (`SCH-006`).
///
/// # A null key component is substituted here too (`MIG-013`)
///
/// `MIG-013` is usually described as a property of the write: a key column cannot be `NULL`, so the
/// binder puts the empty string or `transform.missing_key_ts_replace` there instead. But the value
/// the binder chose is what the target row *is* keyed by, so a validate run that derives its lookup
/// key from the raw origin cell derives a key the migration never wrote — a null one, which
/// [`CqlRowSink::fetch`] answers as absent without querying — and reports the row missing forever.
///
/// So the plan carries the policy and substitutes as it builds the key, from the same
/// [`MissingKeyPolicy`] the binder uses. [`TargetKeyPlan::substituted`] additionally
/// writes the replacement back into the origin row where that is unambiguous, which is what makes
/// the *comparison* agree: the target holds the substitute, and comparing it against a null origin
/// cell would turn `MISSING` into `MISMATCH` rather than into `VALID`.
#[derive(Debug, Clone)]
pub struct TargetKeyPlan {
    slots: Vec<TargetKeySlot>,
    /// Per slot, in bind order, the replacement for a null in that component, if it has one.
    substitutes: Vec<Option<KeySubstitute>>,
}

impl TargetKeyPlan {
    /// Resolves the plan from the mapping, the lookup statement built from it, and the null-key
    /// policy the write path binds with (`SCH-006`, `MIG-013`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] when a bound key column is supplied by something `SCH-006`
    /// does not admit as a source for a target primary key: an extracted JSON property, or nothing
    /// at all. An exploded map key or value is *not* such a case — `SCH-006` names it explicitly
    /// and `FEA-022` builds on it — it is carried as a slot that [`TargetKeyPlan::key_of`] fills
    /// from the entry. Also when a column's declared type does not parse, which is a disagreement
    /// with `system_schema` rather than bad data and must stop the run at startup.
    pub fn resolve(
        mapping: &ColumnMapping,
        select: &TargetSelectByPk,
        missing_key: MissingKeyPolicy,
    ) -> Result<Self, CdmError> {
        let mut slots = Vec::with_capacity(select.bound_key_columns().len());
        let mut substitutes = Vec::with_capacity(select.bound_key_columns().len());
        for column in select.bound_key_columns() {
            match mapping.source_of(column) {
                Some(TargetSource::Origin(index)) => {
                    let index = *index;
                    substitutes.push(substitute_for(mapping, column, index, missing_key)?);
                    slots.push(TargetKeySlot::Origin(index));
                }
                Some(TargetSource::ExplodeKey) => {
                    // An exploded entry's key is never null: `FEA-020` produces one target row per
                    // entry, and a map has no null keys.
                    substitutes.push(None);
                    slots.push(TargetKeySlot::ExplodeKey);
                }
                Some(TargetSource::ExplodeValue) => {
                    substitutes.push(None);
                    slots.push(TargetKeySlot::ExplodeValue);
                }
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
        Ok(Self { slots, substitutes })
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
    /// A null component whose column has a `MIG-013` substitute takes the substitute, because that
    /// is the value the migration wrote and therefore the value the target row is keyed by. A null
    /// component with no substitute stays null: `MIG-013` counts that record as an error on the
    /// write side, and on the read side [`CqlRowSink::fetch`] answers it as absent without querying.
    ///
    /// Pass [`ExplodedKeyParts::NONE`] when no explode map is configured. Passing it while
    /// [`TargetKeyPlan::explodes`] is `true` leaves the exploded components null, which
    /// [`CqlRowSink::fetch`] answers as "absent" without issuing a query.
    #[must_use]
    pub fn key_of(&self, row: &Row, entry: ExplodedKeyParts<'_>) -> PrimaryKey {
        PrimaryKey::new(
            self.slots
                .iter()
                .enumerate()
                .map(|(slot_index, slot)| match slot {
                    TargetKeySlot::Origin(index) => {
                        let cell = row.get(*index).cloned().unwrap_or(RawCell::NULL);
                        if cell.is_null() {
                            self.substitute_at(slot_index).unwrap_or(cell)
                        } else {
                            cell
                        }
                    }
                    TargetKeySlot::ExplodeKey => cell_of(entry.key),
                    TargetKeySlot::ExplodeValue => cell_of(entry.value),
                })
                .collect(),
        )
    }

    /// The origin row with every substitutable null key cell replaced (`MIG-013`), or `None` when
    /// the row needs no replacement.
    ///
    /// The key alone is not enough. The migration wrote the substitute into the target's key
    /// column, so a validate run that fetched the right row would still compare that substitute
    /// against a null origin cell and report a mismatch — the record would move from `MISSING` to
    /// `MISMATCH` rather than to `VALID`. Replacing the cell puts both sides in the state the
    /// migration left them in, and it is the same replacement the binder would have made for the
    /// same column, so migrating from the substituted row writes exactly what migrating from the
    /// raw one did.
    ///
    /// `None` — the overwhelmingly common case — means the caller keeps the row it has, so a scan
    /// over rows with no null key components allocates nothing extra.
    #[must_use]
    pub fn substituted(&self, row: &Row) -> Option<Row> {
        let mut cells: Option<Vec<RawCell>> = None;
        for (slot_index, slot) in self.slots.iter().enumerate() {
            let TargetKeySlot::Origin(index) = slot else {
                continue;
            };
            if !row.get(*index).is_none_or(RawCell::is_null) {
                continue;
            }
            let Some(substitute) = self
                .substitutes
                .get(slot_index)
                .and_then(Option::as_ref)
                .filter(|substitute| substitute.mirror)
            else {
                continue;
            };
            let cells = cells.get_or_insert_with(|| row.cells().to_vec());
            if let Some(cell) = cells.get_mut(*index) {
                *cell = substitute.value.clone();
            }
        }
        cells.map(Row::new)
    }

    /// The substitute for the component bound at `slot_index`, if it has one.
    fn substitute_at(&self, slot_index: usize) -> Option<RawCell> {
        self.substitutes
            .get(slot_index)
            .and_then(Option::as_ref)
            .map(|substitute| substitute.value.clone())
    }
}

/// One half of an exploded entry as a cell, null when the caller supplied none.
fn cell_of(bytes: Option<&[u8]>) -> RawCell {
    bytes.map_or(RawCell::NULL, |bytes| RawCell::new(bytes.to_vec()))
}

/// What a null in the key column `column`, sourced from origin position `origin_index`, is
/// replaced with (`MIG-013`).
///
/// # Errors
///
/// [`ErrorKind::SchemaMismatch`] when either side's declared type does not parse.
fn substitute_for(
    mapping: &ColumnMapping,
    column: &str,
    origin_index: usize,
    missing_key: MissingKeyPolicy,
) -> Result<Option<KeySubstitute>, CdmError> {
    let Some(target_column) = mapping
        .target_columns()
        .iter()
        .find(|meta| meta.name == *column)
    else {
        // The bound key columns are taken from this very mapping, so this is unreachable; treating
        // it as "no substitute" keeps the key derivation total rather than inventing an error.
        return Ok(None);
    };
    let target_type = parse_type(target_column, Side::Target)?;
    let Some(value) = missing_key.substitute(&target_type) else {
        return Ok(None);
    };
    // Written back into the origin row only when the bytes mean the same thing there — the
    // conversion the binder would apply is the identity — and only when no other target column
    // reads that origin cell, which would otherwise silently see a value instead of a null and
    // bind it rather than `UNSET` (`MIG-012`).
    let same_type = mapping
        .origin_columns()
        .get(origin_index)
        .map(|meta| parse_type(meta, Side::Origin))
        .transpose()?
        .is_some_and(|origin_type| origin_type == target_type);
    let sole_reader = (0..mapping.target_columns().len())
        .filter(|index| {
            matches!(mapping.source(*index), Some(TargetSource::Origin(other)) if *other == origin_index)
        })
        .count()
        == 1;
    Ok(Some(KeySubstitute {
        value: RawCell::new(value),
        mirror: same_type && sole_reader,
    }))
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
    ///
    /// `missing_key` must be the policy the write path binds with: the key derived here is looked
    /// up against rows the write path wrote, and a source that substituted differently — or not at
    /// all — would report every substituted row missing (`MIG-013`).
    pub async fn prepare(
        session: Arc<Session>,
        select: &OriginRangeSelect,
        mapping: &ColumnMapping,
        target_select: &TargetSelectByPk,
        token_kind: TokenKind,
        missing_key: MissingKeyPolicy,
    ) -> Result<Self, CdmError> {
        let statement = session
            .prepare(select.cql())
            .await
            .map_err(|error| read_error(Side::Origin, select.cql(), &error))?;
        Ok(Self {
            session,
            statement,
            token_kind,
            key_plan: TargetKeyPlan::resolve(mapping, target_select, missing_key)?,
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
                // MIG-013: a null in a target key column was written as a substitute, so the row
                // carries the substitute from here on — both the key it is looked up by and the
                // value it is compared against.
                let origin = self.key_plan.substituted(&origin).unwrap_or(origin);
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

/// The TTL and the writetime one origin row is written with (`FEA-040`..`FEA-046`, `VAL-018`).
///
/// `WritetimeTtlPlan` in `cdm-feature` is the implementation, and it cannot be named here:
/// `cdm-feature` depends on this crate and not the other way round (`ARCHITECTURE.md` §3). The trait
/// is the seam, so that a corrected row is stamped from the same resolved plan a migrated row is —
/// the whole of `VAL-018` is that the two paths must not compute this differently.
///
/// [`CqlRowSink`] calls both methods once per corrected row, and passes the results straight to
/// [`BindInputs::ttl`] and [`BindInputs::writetime`]. `None` means the plan resolves no value, which
/// binds `UNSET` and leaves the server to assign the timestamp (`FEA-046`).
pub trait RowTimestamps: std::fmt::Debug + Send + Sync {
    /// The row's TTL, in seconds.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TypeConversion`] if a selected `TTL(…)` cell is not an `int`.
    fn ttl(&self, row: &Row) -> Result<Option<i32>, CdmError>;

    /// The row's writetime, in microseconds.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TypeConversion`] if a selected `WRITETIME(…)` cell is not a `bigint`.
    fn writetime(&self, row: &Row) -> Result<Option<i64>, CdmError>;
}

/// The target side of a run: the lookup by primary key and the upsert (`PLG-005`, `VAL-001`).
#[derive(Debug)]
pub struct CqlRowSink {
    session: Arc<Session>,
    select: PreparedStatement,
    upsert: PreparedStatement,
    binder: Binder,
    counters: Vec<CounterColumn>,
    timestamps: Option<Arc<dyn RowTimestamps>>,
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
    ///
    /// `timestamps` must be resolved from the same `TTL(…)`/`WRITETIME(…)` projection the origin
    /// scan selects and must be the plan the `binder`'s statement took its `USING` clause from
    /// (`VAL-018`). A parameter rather than a setter because a sink that silently omitted them
    /// still writes the row, still agrees with every counter, and still exits 0 — only the
    /// timestamp is wrong, which is precisely how this was lost once already.
    pub async fn prepare(
        session: Arc<Session>,
        select: &TargetSelectByPk,
        binder: Binder,
        mapping: &ColumnMapping,
        timestamps: Option<Arc<dyn RowTimestamps>>,
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
            timestamps,
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
        // VAL-018: read off the origin row rather than off `source`, because the two differ only in
        // the counter cells `MIG-031` rewrote — and a row whose TTL and writetime are read from a
        // delta is a row whose correction is stamped with an arithmetic artefact.
        let (ttl, writetime) = timestamps_of(self.timestamps.as_deref(), record.origin())?;
        // FEA-020, FEA-022: an exploded record stands for one map entry, and the target's key and
        // value columns are bound from that entry rather than from any origin cell. Without this a
        // correction binds `UNSET` for both, which for a key column the server rejects — so the
        // repair `VAL-003` promises would fail on precisely the rows an explode run produces.
        let entry = record.exploded();
        let bound = self.binder.bind(
            &source,
            BindInputs {
                key: Some(record.key()),
                ttl,
                writetime,
                explode_key: entry.and_then(|entry| entry.key.bytes()).map(|b| &**b),
                explode_value: entry.and_then(|entry| entry.value.bytes()).map(|b| &**b),
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

/// The TTL and writetime to stamp one corrected row with (`VAL-018`).
///
/// `(None, None)` when the sink was built without a plan, and when the plan resolves neither —
/// `FEA-045`'s counter table, or a run that configured no writetime column — both of which the
/// binder turns into `UNSET`, against a statement that has no `USING` clause to bind them into.
///
/// # Errors
///
/// Whatever the plan reports for a cell it cannot decode.
fn timestamps_of(
    plan: Option<&dyn RowTimestamps>,
    row: &Row,
) -> Result<(Option<i32>, Option<i64>), CdmError> {
    let Some(plan) = plan else {
        return Ok((None, None));
    };
    Ok((plan.ttl(row)?, plan.writetime(row)?))
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
        TargetKeyPlan::resolve(&mapping, &select, MissingKeyPolicy::default())
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
        let plan = TargetKeyPlan::resolve(&mapping, &select, MissingKeyPolicy::default()).unwrap();
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

    /// A plan that answers from the last two cells of the row, as `FEA-040`'s does.
    #[derive(Debug)]
    struct LastTwoCells;

    impl RowTimestamps for LastTwoCells {
        fn ttl(&self, row: &Row) -> Result<Option<i32>, CdmError> {
            Ok(row
                .get(row.len() - 2)
                .and_then(RawCell::bytes)
                .and_then(|bytes| <[u8; 4]>::try_from(&**bytes).ok())
                .map(i32::from_be_bytes))
        }

        fn writetime(&self, row: &Row) -> Result<Option<i64>, CdmError> {
            Ok(row
                .get(row.len() - 1)
                .and_then(RawCell::bytes)
                .and_then(|bytes| <[u8; 8]>::try_from(&**bytes).ok())
                .map(i64::from_be_bytes))
        }
    }

    #[test]
    fn val_018_a_correction_takes_its_ttl_and_writetime_from_the_origin_row() {
        // The whole of the defect: the sink bound `BindInputs { key, ..default() }`, so both
        // markers went out as `UNSET` and the coordinator stamped the repaired row with its own
        // wall clock — a timestamp that shadows every later origin write.
        let row = Row::new(vec![
            text("key1"),
            RawCell::new(3600_i32.to_be_bytes().to_vec()),
            RawCell::new(1_087_384_200_000_000_i64.to_be_bytes().to_vec()),
        ]);
        let (ttl, writetime) = timestamps_of(Some(&LastTwoCells), &row).unwrap();
        assert_eq!(ttl, Some(3600));
        assert_eq!(writetime, Some(1_087_384_200_000_000));
    }

    #[test]
    fn val_018_a_sink_with_no_plan_binds_neither_value() {
        // `FEA-045`'s counter table and a run that configured no writetime column both arrive here.
        // `FEA-046` then omits the `USING` clause, and the server assigns the timestamp — which is
        // the one case in which a corrected row may carry one.
        let (ttl, writetime) = timestamps_of(None, &origin_row()).unwrap();
        assert_eq!(ttl, None);
        assert_eq!(writetime, None);
    }

    /// SIT `regression/04_null_ts_in_pk`: `ts` is an ordinary origin column and a target clustering
    /// column, and one origin row leaves it null.
    fn null_ts_origin() -> TableSchema {
        TableSchema {
            keyspace: "origin".to_owned(),
            table: "regression_null_ts_in_pk".to_owned(),
            columns: vec![
                column("key", "text", ColumnKind::PartitionKey, 0),
                column("ts", "timestamp", ColumnKind::Regular, -1),
                column("value", "text", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    fn null_ts_target() -> TableSchema {
        TableSchema {
            keyspace: "target".to_owned(),
            table: "regression_null_ts_in_pk".to_owned(),
            columns: vec![
                column("key", "text", ColumnKind::PartitionKey, 0),
                column("ts", "timestamp", ColumnKind::Clustering, 0),
                column("value", "text", ColumnKind::Regular, -1),
            ],
            is_materialized_view: false,
        }
    }

    fn null_ts_plan(missing_key: MissingKeyPolicy) -> TargetKeyPlan {
        let mapping = ColumnMapping::resolve(
            &null_ts_origin(),
            &null_ts_target(),
            &MappingOptions::default(),
        )
        .unwrap();
        let select = TargetSelectByPk::new(&mapping).unwrap();
        TargetKeyPlan::resolve(&mapping, &select, missing_key).unwrap()
    }

    /// The row whose `ts` is null, in projection order.
    fn null_ts_row() -> Row {
        Row::new(vec![text("key1"), RawCell::NULL, text("valueA")])
    }

    #[test]
    fn mig_013_a_null_timestamp_key_is_looked_up_by_its_replacement() {
        // The migration bound `missing_key_ts_replace` into the target's `ts`, so that — and not
        // null — is what the target row is keyed by.
        let replacement = 1_685_577_600_000_i64;
        let plan = null_ts_plan(MissingKeyPolicy {
            missing_key_ts_replace: Some(replacement),
        });
        let key = plan.key_of(&null_ts_row(), ExplodedKeyParts::NONE);
        assert_eq!(
            key.values(),
            [
                text("key1"),
                RawCell::new(replacement.to_be_bytes().to_vec())
            ],
            "a null timestamp key component takes transform.missing_key_ts_replace (MIG-013)"
        );
    }

    #[test]
    fn mig_013_the_replacement_is_written_into_the_origin_row() {
        // Not only the key: the target holds the replacement, so comparing it against a null
        // origin cell would report a mismatch on a row the migration wrote correctly.
        let replacement = 1_685_577_600_000_i64;
        let plan = null_ts_plan(MissingKeyPolicy {
            missing_key_ts_replace: Some(replacement),
        });
        let substituted = plan
            .substituted(&null_ts_row())
            .expect("the row has a substitutable null key cell");
        assert_eq!(
            substituted.cells(),
            [
                text("key1"),
                RawCell::new(replacement.to_be_bytes().to_vec()),
                text("valueA"),
            ]
        );
    }

    #[test]
    fn mig_013_a_row_with_no_null_key_cell_is_left_exactly_as_it_was() {
        let plan = null_ts_plan(MissingKeyPolicy {
            missing_key_ts_replace: Some(1),
        });
        let row = Row::new(vec![
            text("key2"),
            RawCell::new(7_i64.to_be_bytes().to_vec()),
            text("valueB"),
        ]);
        assert!(
            plan.substituted(&row).is_none(),
            "nothing to substitute means nothing to allocate"
        );
        assert_eq!(
            plan.key_of(&row, ExplodedKeyParts::NONE).values(),
            [text("key2"), RawCell::new(7_i64.to_be_bytes().to_vec())]
        );
    }

    #[test]
    fn mig_013_a_null_timestamp_key_with_no_configured_replacement_stays_null() {
        // `MIG-013` counts that record as an error on the write side; the key stays null here, and
        // `CqlRowSink::fetch` answers it as absent rather than looking up the wrong row.
        let plan = null_ts_plan(MissingKeyPolicy::default());
        assert!(plan.substituted(&null_ts_row()).is_none());
        assert_eq!(
            plan.key_of(&null_ts_row(), ExplodedKeyParts::NONE).values(),
            [text("key1"), RawCell::NULL]
        );
    }

    #[test]
    fn mig_013_a_null_text_key_becomes_the_empty_string_with_no_configuration_at_all() {
        // Java's `defaultForMissingString`, which is not configurable there and is not made
        // configurable here.
        let mut target = null_ts_target();
        target.columns = vec![
            column("key", "text", ColumnKind::PartitionKey, 0),
            column("value", "text", ColumnKind::Clustering, 0),
            column("ts", "timestamp", ColumnKind::Regular, -1),
        ];
        let mapping =
            ColumnMapping::resolve(&null_ts_origin(), &target, &MappingOptions::default()).unwrap();
        let select = TargetSelectByPk::new(&mapping).unwrap();
        let plan = TargetKeyPlan::resolve(&mapping, &select, MissingKeyPolicy::default()).unwrap();
        let row = Row::new(vec![text("key1"), RawCell::NULL, RawCell::NULL]);
        assert_eq!(
            plan.key_of(&row, ExplodedKeyParts::NONE).values(),
            [text("key1"), text("")]
        );
        let substituted = plan
            .substituted(&row)
            .expect("an empty string is written back");
        assert_eq!(substituted.get(2), Some(&text("")));
    }
}
