//! The seeded data generator (`TST-100`, `TST-101`).
//!
//! Produces CQL *literals*, not driver values, for the reason given in [`crate::schema`]: this
//! crate cannot depend on a driver, so the thing it can produce and a
//! [`TestSession`](crate::TestSession) can execute is a statement. That constraint turns out to
//! be a feature — a literal is a value a unit test can assert on, snapshot, and compare between
//! the origin it wrote and the target it expects, with no cluster in the loop.
//!
//! # Determinism
//!
//! Every value is drawn from a [`Seed`]-derived generator, so a `DataGen` built from the same
//! seed emits the same rows in the same order (`TST-101`). Nothing here consults the clock, the
//! environment, a hash map's iteration order, or the platform's integer widths.
//!
//! # What a generated value is *for*
//!
//! Wide, awkward and boring in equal measure: text with quotes and non-ASCII in it, `blob`s
//! including the empty one, collections that are never empty (an empty collection and a null are
//! the same thing on the wire, which `MIG-012` turns on), and numbers at the edges of their
//! ranges. Values are not, however, adversarial for its own sake — `TST-010`'s property tests are
//! where the pathological cases belong.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use cdm_codec::{CqlTypeInfo, UdtField};
use cdm_core::{CdmError, ErrorKind};
use rand::rngs::StdRng;
use rand::RngExt;

use crate::schema::{ColumnKind, ColumnSpec, TableSpec};
use crate::seed::{choose, Seed};

/// How adventurous a [`DataGen`] should be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DataGenOptions {
    null_probability: f64,
    min_collection_len: usize,
    max_collection_len: usize,
    max_text_len: usize,
}

impl Default for DataGenOptions {
    fn default() -> Self {
        Self {
            // No nulls by default. A null is not a value but the absence of one, and a fixture
            // that sprinkles them by default makes every "did this column round-trip?" assertion
            // vacuously true some fraction of the time.
            null_probability: 0.0,
            // Never empty: an empty collection and a null collection are indistinguishable on the
            // wire, which is the whole subject of `MIG-012`. A fixture that emits empty
            // collections by accident tests that requirement by accident too.
            min_collection_len: 1,
            max_collection_len: 3,
            max_text_len: 12,
        }
    }
}

impl DataGenOptions {
    /// The chance that a nullable column is generated as `NULL`, in `0.0..=1.0`.
    ///
    /// Primary-key columns are never null regardless: a cluster rejects that outright.
    #[must_use]
    pub fn with_null_probability(mut self, probability: f64) -> Self {
        self.null_probability = probability.clamp(0.0, 1.0);
        self
    }

    /// The inclusive bounds on generated collection lengths. A minimum of zero opts in to empty
    /// collections, with the `MIG-012` caveat above.
    #[must_use]
    pub fn with_collection_len(mut self, min: usize, max: usize) -> Self {
        self.min_collection_len = min.min(max);
        self.max_collection_len = max.max(min);
        self
    }

    /// The maximum number of characters in a generated `text` or `ascii` value.
    #[must_use]
    pub fn with_max_text_len(mut self, max: usize) -> Self {
        self.max_text_len = max.max(1);
        self
    }
}

/// One generated row: a literal per column, in the table's declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRow {
    values: Vec<(String, String)>,
}

impl GeneratedRow {
    /// The literal generated for a column, if the row has one.
    pub fn literal(&self, column: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, literal)| literal.as_str())
    }

    /// Every column and its literal, in declaration order.
    pub fn values(&self) -> &[(String, String)] {
        &self.values
    }

    /// The statement that writes this row: an `INSERT`, or an `UPDATE` for a counter table.
    ///
    /// Counter tables get an `UPDATE ... SET c = c + ?` because that is the only way CQL allows a
    /// counter to be written (`MIG-030`), and the reason counter writes are not idempotent and
    /// must never be retried (`CON-012`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the row does not carry a literal for every column of `table` —
    /// which can only happen if the row was generated from a different table.
    pub fn write_statement(&self, table: &TableSpec) -> Result<String, CdmError> {
        for column in table.columns() {
            if self.literal(column.name()).is_none() {
                return Err(CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "row has no value for column `{}` of {}",
                        column.name(),
                        table.qualified_name()
                    ),
                ));
            }
        }

        if table.is_counter_table() {
            self.counter_update(table)
        } else {
            Ok(self.insert(table))
        }
    }

    /// `INSERT INTO ks.t (a, b) VALUES (…, …)`.
    fn insert(&self, table: &TableSpec) -> String {
        let names = self
            .values
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let literals = self
            .values
            .iter()
            .map(|(_, literal)| literal.clone())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "INSERT INTO {} ({names}) VALUES ({literals})",
            table.qualified_name()
        )
    }

    /// `UPDATE ks.t SET c = c + n WHERE k = …`.
    fn counter_update(&self, table: &TableSpec) -> Result<String, CdmError> {
        let mut assignments = Vec::new();
        let mut predicates = Vec::new();
        for column in table.columns() {
            let literal = self.literal(column.name()).unwrap_or("null");
            if column.kind().is_key() {
                predicates.push(format!("{} = {literal}", column.name()));
            } else {
                assignments.push(format!("{name} = {name} + {literal}", name = column.name()));
            }
        }
        if assignments.is_empty() {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "counter table {} has no counter columns to increment",
                    table.qualified_name()
                ),
            ));
        }
        Ok(format!(
            "UPDATE {} SET {} WHERE {}",
            table.qualified_name(),
            assignments.join(", "),
            predicates.join(" AND ")
        ))
    }

    /// The `WHERE` clause that selects exactly this row, for a read-back assertion.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if a key column has no literal in this row.
    pub fn primary_key_predicate(&self, table: &TableSpec) -> Result<String, CdmError> {
        let mut predicates = Vec::new();
        for column in table.columns().iter().filter(|c| c.kind().is_key()) {
            let literal = self.literal(column.name()).ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!("row has no value for key column `{}`", column.name()),
                )
            })?;
            predicates.push(format!("{} = {literal}", column.name()));
        }
        Ok(predicates.join(" AND "))
    }
}

/// Generates deterministic values for CQL types (`TST-100`, `TST-101`).
///
/// ```
/// use cdm_testkit::{DataGen, SchemaGen, Seed};
///
/// let table = SchemaGen::simple("cdm_test", "kv")?;
/// let row = DataGen::new(Seed::new(7)).row(&table)?;
/// assert_eq!(row, DataGen::new(Seed::new(7)).row(&table)?);
/// assert!(row.write_statement(&table)?.starts_with("INSERT INTO cdm_test.kv"));
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug)]
pub struct DataGen {
    rng: StdRng,
    seed: Seed,
    options: DataGenOptions,
}

impl DataGen {
    /// A generator seeded from `seed`, with the default options.
    pub fn new(seed: Seed) -> Self {
        Self::with_options(seed, DataGenOptions::default())
    }

    /// A generator seeded from `seed`, with explicit options.
    pub fn with_options(seed: Seed, options: DataGenOptions) -> Self {
        Self {
            rng: seed.rng(),
            seed,
            options,
        }
    }

    /// The seed this generator was built from, for a failure message (`TST-101`).
    pub const fn seed(&self) -> Seed {
        self.seed
    }

    /// A literal for one type.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TypeConversion`] for a type no literal syntax exists for:
    /// [`CqlTypeInfo::Custom`], and a UDT whose fields are unknown. Both are cases where a
    /// generated literal would be a guess, and a guess that a cluster rejects is worse than an
    /// error that says why.
    // One arm per CQL type, and the whole point is that the list is exhaustive and readable in
    // one place. Splitting it to satisfy a line count would scatter the type taxonomy.
    #[allow(clippy::too_many_lines)]
    pub fn literal(&mut self, cql_type: &CqlTypeInfo) -> Result<String, CdmError> {
        match cql_type {
            CqlTypeInfo::Ascii => Ok(quote(&self.ascii_text())),
            CqlTypeInfo::Text => Ok(quote(&self.unicode_text())),
            CqlTypeInfo::Boolean => Ok(self.rng.random::<bool>().to_string()),
            CqlTypeInfo::TinyInt => Ok(self.rng.random::<i8>().to_string()),
            CqlTypeInfo::SmallInt => Ok(self.rng.random::<i16>().to_string()),
            CqlTypeInfo::Int => Ok(self.rng.random::<i32>().to_string()),
            // A counter literal is the *delta* an UPDATE adds, never an absolute value.
            CqlTypeInfo::BigInt | CqlTypeInfo::Counter => Ok(self.rng.random::<i64>().to_string()),
            CqlTypeInfo::VarInt => Ok(self.rng.random::<i128>().to_string()),
            CqlTypeInfo::Float => {
                let value: f32 = self.rng.random_range(-1.0e6..1.0e6);
                Ok(format!("{value:?}"))
            }
            CqlTypeInfo::Double => {
                let value: f64 = self.rng.random_range(-1.0e9..1.0e9);
                Ok(format!("{value:?}"))
            }
            CqlTypeInfo::Decimal => {
                let units: i64 = self.rng.random_range(-1_000_000_000..1_000_000_000);
                let fraction: u32 = self.rng.random_range(0..1_000_000);
                Ok(format!("{units}.{fraction:06}"))
            }
            CqlTypeInfo::Blob => Ok(self.blob()),
            CqlTypeInfo::Inet => Ok(quote(&self.ipv4())),
            CqlTypeInfo::Uuid => Ok(self.uuid(4)),
            CqlTypeInfo::TimeUuid => Ok(self.uuid(1)),
            CqlTypeInfo::Date => self.date(),
            CqlTypeInfo::Time => Ok(self.time()),
            CqlTypeInfo::Timestamp => self.timestamp(),
            CqlTypeInfo::Duration => Ok(self.duration()),
            CqlTypeInfo::Point => Ok(quote(&format!(
                "POINT ({} {})",
                self.coordinate(),
                self.coordinate()
            ))),
            CqlTypeInfo::LineString => Ok(quote(&format!(
                "LINESTRING ({} {}, {} {})",
                self.coordinate(),
                self.coordinate(),
                self.coordinate(),
                self.coordinate()
            ))),
            // A polygon's ring must close, so the first point is repeated verbatim.
            CqlTypeInfo::Polygon => {
                let first = format!("{} {}", self.coordinate(), self.coordinate());
                Ok(quote(&format!(
                    "POLYGON (({first}, {} {}, {} {}, {first}))",
                    self.coordinate(),
                    self.coordinate(),
                    self.coordinate(),
                    self.coordinate()
                )))
            }
            CqlTypeInfo::DateRange => {
                let year: i32 = self.rng.random_range(1900..2100);
                Ok(quote(&format!("[{year}-01-01 TO {year}-12-31]")))
            }
            CqlTypeInfo::List { element, .. } => {
                let items = self.elements(element)?;
                Ok(format!("[{}]", items.join(", ")))
            }
            CqlTypeInfo::Set { element, .. } => {
                // A CQL set literal with a repeated element is legal but collapses, which would
                // make a round-trip assertion on the generated literal fail for a reason that has
                // nothing to do with the code under test.
                let items: BTreeSet<String> = self.elements(element)?.into_iter().collect();
                Ok(format!(
                    "{{{}}}",
                    items.into_iter().collect::<Vec<_>>().join(", ")
                ))
            }
            CqlTypeInfo::Map { key, value, .. } => {
                let len = self.collection_len();
                let mut entries: Vec<(String, String)> = Vec::new();
                let mut keys = BTreeSet::new();
                for _ in 0..len {
                    let key_literal = self.literal(key)?;
                    let value_literal = self.literal(value)?;
                    if keys.insert(key_literal.clone()) {
                        entries.push((key_literal, value_literal));
                    }
                }
                entries.sort();
                let rendered = entries
                    .into_iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Ok(format!("{{{rendered}}}"))
            }
            CqlTypeInfo::Tuple { elements } => {
                let mut items = Vec::with_capacity(elements.len());
                for element in elements {
                    items.push(self.literal(element)?);
                }
                Ok(format!("({})", items.join(", ")))
            }
            CqlTypeInfo::Udt { name, fields, .. } => {
                if fields.is_empty() {
                    return Err(CdmError::new(
                        ErrorKind::TypeConversion,
                        format!(
                            "cannot generate a literal for UDT `{name}`: its fields are unknown, \
                             so resolve it with a UdtResolver first (CDC-014)"
                        ),
                    ));
                }
                self.udt_literal(fields)
            }
            CqlTypeInfo::Vector {
                element,
                dimensions,
            } => {
                let mut items = Vec::with_capacity(*dimensions);
                for _ in 0..*dimensions {
                    items.push(self.literal(element)?);
                }
                Ok(format!("[{}]", items.join(", ")))
            }
            CqlTypeInfo::Custom(name) => Err(CdmError::new(
                ErrorKind::TypeConversion,
                format!("cannot generate a literal for the custom type `{name}`"),
            )),
            // `CqlTypeInfo` is `#[non_exhaustive]`: a type added to `cdm-codec` must fail loudly
            // here rather than silently never appear in generated data.
            other => Err(CdmError::new(
                ErrorKind::TypeConversion,
                format!(
                    "cdm-testkit has no literal syntax for `{other}`; teach DataGen::literal \
                     about it (TST-100)"
                ),
            )),
        }
    }

    /// One row for `table`.
    ///
    /// Key columns are never null; other columns are null with probability
    /// [`DataGenOptions::with_null_probability`].
    ///
    /// # Errors
    ///
    /// As [`DataGen::literal`], for the first column whose type has no literal syntax.
    pub fn row(&mut self, table: &TableSpec) -> Result<GeneratedRow, CdmError> {
        let mut values = Vec::with_capacity(table.columns().len());
        for column in table.columns() {
            values.push((column.name().to_owned(), self.column_literal(column)?));
        }
        Ok(GeneratedRow { values })
    }

    /// `count` rows for `table`, all distinct in their partition key by construction of the
    /// generator rather than by rejection sampling — a collision is astronomically unlikely for
    /// the key types the generators produce, and asserting on it would make the row count
    /// nondeterministic.
    ///
    /// # Errors
    ///
    /// As [`DataGen::row`].
    pub fn rows(&mut self, table: &TableSpec, count: usize) -> Result<Vec<GeneratedRow>, CdmError> {
        (0..count).map(|_| self.row(table)).collect()
    }

    /// A literal for one column, honouring the null rules.
    fn column_literal(&mut self, column: &ColumnSpec) -> Result<String, CdmError> {
        let nullable = column.kind() == ColumnKind::Regular || column.kind() == ColumnKind::Static;
        if nullable
            && self.options.null_probability > 0.0
            && self.rng.random::<f64>() < self.options.null_probability
        {
            return Ok("null".to_owned());
        }
        self.literal(column.cql_type())
    }

    /// Between [`DataGenOptions::min_collection_len`] and `max_collection_len` elements of the
    /// given type.
    fn elements(&mut self, element: &CqlTypeInfo) -> Result<Vec<String>, CdmError> {
        let len = self.collection_len();
        (0..len).map(|_| self.literal(element)).collect()
    }

    fn collection_len(&mut self) -> usize {
        let min = self.options.min_collection_len;
        let max = self.options.max_collection_len;
        if min >= max {
            min
        } else {
            self.rng.random_range(min..=max)
        }
    }

    /// A UDT literal, `{field: value, …}`.
    fn udt_literal(&mut self, fields: &[UdtField]) -> Result<String, CdmError> {
        let mut rendered = String::from("{");
        for (index, field) in fields.iter().enumerate() {
            let value = self.literal(&field.cql_type)?;
            if index > 0 {
                rendered.push_str(", ");
            }
            // Writing into a String cannot fail.
            let _ = write!(rendered, "{}: {value}", field.name);
        }
        rendered.push('}');
        Ok(rendered)
    }

    /// Printable ASCII, excluding the quote — which the `text` generator deliberately includes.
    fn ascii_text(&mut self) -> String {
        const ALPHABET: &[char] = &[
            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q',
            'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3', '4', '5', '6', '7',
            '8', '9', '-', '_', ' ', '.',
        ];
        self.text_from(ALPHABET)
    }

    /// Text with the awkward characters in it: a quote, which must be doubled to survive a
    /// literal, and non-ASCII, which must survive UTF-8 round-tripping.
    fn unicode_text(&mut self) -> String {
        const ALPHABET: &[char] = &[
            'a', 'z', '0', '9', ' ', '\'', '"', '\\', '%', '✓', 'é', 'ß', '日', '本', '語', '🦀',
        ];
        self.text_from(ALPHABET)
    }

    fn text_from(&mut self, alphabet: &[char]) -> String {
        let len = self.rng.random_range(1..=self.options.max_text_len);
        (0..len)
            .filter_map(|_| choose(&mut self.rng, alphabet.len()).and_then(|i| alphabet.get(i)))
            .collect()
    }

    /// `0x…`, sometimes empty — an empty blob is a value, and is not a null (`MIG-012`).
    fn blob(&mut self) -> String {
        let len = self.rng.random_range(0..8);
        let mut hex = String::from("0x");
        for _ in 0..len {
            let byte: u8 = self.rng.random();
            // Writing into a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    fn ipv4(&mut self) -> String {
        let octets: [u8; 4] = self.rng.random();
        octets
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }

    /// A UUID of the given version, in the canonical hyphenated form CQL expects.
    ///
    /// The version and variant bits are set explicitly because Cassandra validates them: a
    /// `timeuuid` column rejects anything that is not version 1, so a generator that emitted
    /// random bytes would fail on a schema rather than on a bug.
    fn uuid(&mut self, version: u8) -> String {
        let mut bytes: [u8; 16] = self.rng.random();
        if let Some(byte) = bytes.get_mut(6) {
            *byte = (*byte & 0x0f) | (version << 4);
        }
        if let Some(byte) = bytes.get_mut(8) {
            *byte = (*byte & 0x3f) | 0x80;
        }
        uuid::Uuid::from_bytes(bytes).to_string()
    }

    /// `'yyyy-mm-dd'`.
    fn date(&mut self) -> Result<String, CdmError> {
        let days: i64 = self.rng.random_range(-25_000..25_000);
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
            .ok_or_else(|| CdmError::new(ErrorKind::Internal, "1970-01-01 is not a valid date"))?;
        let date = epoch
            .checked_add_signed(chrono::TimeDelta::days(days))
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!("{days} days from the epoch overflows a date"),
                )
            })?;
        Ok(quote(&date.format("%Y-%m-%d").to_string()))
    }

    /// `'hh:mm:ss.fffffffff'` — nanosecond precision, because `time` has it and a generator that
    /// stopped at milliseconds would never exercise the truncation path.
    fn time(&mut self) -> String {
        let nanos: u64 = self.rng.random_range(0..86_400_000_000_000);
        let seconds = nanos / 1_000_000_000;
        let remainder = nanos % 1_000_000_000;
        quote(&format!(
            "{:02}:{:02}:{:02}.{remainder:09}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        ))
    }

    /// `'yyyy-mm-ddThh:mm:ss.sssZ'` — millisecond precision, which is all `timestamp` has.
    fn timestamp(&mut self) -> Result<String, CdmError> {
        let millis: i64 = self.rng.random_range(0..4_102_444_800_000);
        let moment = chrono::DateTime::from_timestamp_millis(millis).ok_or_else(|| {
            CdmError::new(
                ErrorKind::Internal,
                format!("{millis} ms is not a representable timestamp"),
            )
        })?;
        Ok(quote(&moment.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()))
    }

    /// `12mo3d456ns` — months, days and nanoseconds, the three independent components a CQL
    /// `duration` actually has.
    fn duration(&mut self) -> String {
        let months: u32 = self.rng.random_range(0..24);
        let days: u32 = self.rng.random_range(0..31);
        let nanos: u64 = self.rng.random_range(0..1_000_000_000);
        format!("{months}mo{days}d{nanos}ns")
    }

    /// A well-known-text coordinate, rendered so it always has a decimal point.
    fn coordinate(&mut self) -> String {
        let value: f64 = self.rng.random_range(-180.0..180.0);
        format!("{value:.4}")
    }
}

/// Wraps a string in single quotes, doubling any it contains — CQL's only escape.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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
    use crate::containers::Capabilities;
    use crate::schema::SchemaGen;

    #[test]
    fn tst_101_the_same_seed_generates_the_same_rows() {
        let table = SchemaGen::all_types("ks", "t", Capabilities::maximal()).unwrap();
        let first = DataGen::new(Seed::new(11)).rows(&table, 5).unwrap();
        let second = DataGen::new(Seed::new(11)).rows(&table, 5).unwrap();
        assert_eq!(first, second);

        let different = DataGen::new(Seed::new(12)).rows(&table, 5).unwrap();
        assert_ne!(first, different);
    }

    #[test]
    fn tst_101_the_generator_remembers_its_seed_so_a_failure_can_name_it() {
        let generator = DataGen::new(Seed::new(4242));
        assert_eq!(generator.seed(), Seed::new(4242));
        assert!(generator.seed().banner().contains("4242"));
    }

    #[test]
    fn tst_100_every_supported_type_has_a_literal() {
        let address = SchemaGen::address_udt();
        let mut generator = DataGen::new(Seed::new(1));
        for cql_type in SchemaGen::supported_types(&address, Capabilities::maximal()) {
            let literal = generator.literal(&cql_type).unwrap();
            assert!(!literal.is_empty(), "{cql_type} generated nothing");
        }
        // Counter is excluded from the all-types table but still has a literal: the delta an
        // UPDATE adds.
        assert!(generator.literal(&CqlTypeInfo::Counter).is_ok());
    }

    #[test]
    fn tst_100_a_type_with_no_literal_syntax_is_an_error_not_a_guess() {
        let mut generator = DataGen::new(Seed::new(1));

        let err = generator
            .literal(&CqlTypeInfo::Custom("org.example.Weird".to_owned()))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TypeConversion);
        assert!(err.to_string().contains("org.example.Weird"), "{err}");

        let err = generator
            .literal(&CqlTypeInfo::Udt {
                keyspace: None,
                name: "unresolved".to_owned(),
                fields: Vec::new(),
                frozen: true,
            })
            .unwrap_err();
        assert!(err.to_string().contains("CDC-014"), "{err}");
    }

    #[test]
    fn tst_100_text_literals_escape_the_quote_they_contain() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it''s'");
        assert_eq!(quote("''"), "''''''");

        // The text alphabet contains a quote on purpose, so this is reachable rather than
        // theoretical: over many draws, at least one literal must contain a doubled quote, and
        // every literal must be balanced.
        let mut generator = DataGen::new(Seed::new(3));
        let mut saw_escape = false;
        for _ in 0..200 {
            let literal = generator.literal(&CqlTypeInfo::Text).unwrap();
            assert!(
                literal.starts_with('\'') && literal.ends_with('\''),
                "{literal}"
            );
            let inner = &literal[1..literal.len() - 1];
            saw_escape |= inner.contains("''");
            // Every quote inside must be part of a doubled pair.
            assert_eq!(inner.matches('\'').count() % 2, 0, "{literal}");
        }
        assert!(saw_escape, "the text generator never produced a quote");
    }

    #[test]
    fn tst_100_uuid_literals_carry_the_version_the_column_type_demands() {
        let mut generator = DataGen::new(Seed::new(5));
        for _ in 0..50 {
            let timeuuid = generator.literal(&CqlTypeInfo::TimeUuid).unwrap();
            let parsed = uuid::Uuid::parse_str(&timeuuid).unwrap();
            assert_eq!(parsed.get_version_num(), 1, "timeuuid must be version 1");

            let uuid = generator.literal(&CqlTypeInfo::Uuid).unwrap();
            let parsed = uuid::Uuid::parse_str(&uuid).unwrap();
            assert_eq!(parsed.get_version_num(), 4);
            assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
        }
    }

    #[test]
    fn tst_100_temporal_literals_are_in_the_form_cql_parses() {
        let mut generator = DataGen::new(Seed::new(6));
        for _ in 0..50 {
            let date = generator.literal(&CqlTypeInfo::Date).unwrap();
            assert_eq!(date.len(), 12, "{date}"); // 'yyyy-mm-dd'
            let time = generator.literal(&CqlTypeInfo::Time).unwrap();
            assert_eq!(time.len(), 20, "{time}"); // 'hh:mm:ss.fffffffff'
            let timestamp = generator.literal(&CqlTypeInfo::Timestamp).unwrap();
            assert!(timestamp.ends_with("Z'"), "{timestamp}");
            let duration = generator.literal(&CqlTypeInfo::Duration).unwrap();
            assert!(
                duration.contains("mo") && duration.ends_with("ns"),
                "{duration}"
            );
        }
    }

    #[test]
    fn mig_012_collections_are_never_empty_by_default() {
        let mut generator = DataGen::new(Seed::new(8));
        let list = CqlTypeInfo::List {
            element: Box::new(CqlTypeInfo::Int),
            frozen: false,
        };
        for _ in 0..50 {
            let literal = generator.literal(&list).unwrap();
            assert_ne!(literal, "[]", "an empty collection is a null on the wire");
        }

        // Opting in is explicit.
        let mut permissive = DataGen::with_options(
            Seed::new(8),
            DataGenOptions::default().with_collection_len(0, 0),
        );
        assert_eq!(permissive.literal(&list).unwrap(), "[]");
    }

    #[test]
    fn tst_100_set_and_map_literals_carry_no_duplicate_keys() {
        // A tinyint has only 256 values, so a three-element set of them collides often — which
        // is exactly the case the de-duplication exists for.
        let mut generator = DataGen::with_options(
            Seed::new(9),
            DataGenOptions::default().with_collection_len(8, 8),
        );
        let set = CqlTypeInfo::Set {
            element: Box::new(CqlTypeInfo::Boolean),
            frozen: false,
        };
        for _ in 0..50 {
            let literal = generator.literal(&set).unwrap();
            let items: Vec<&str> = literal
                .trim_matches(|c| c == '{' || c == '}')
                .split(", ")
                .collect();
            let unique: BTreeSet<&&str> = items.iter().collect();
            assert_eq!(items.len(), unique.len(), "{literal}");
        }

        let map = CqlTypeInfo::Map {
            key: Box::new(CqlTypeInfo::Boolean),
            value: Box::new(CqlTypeInfo::Int),
            frozen: false,
        };
        for _ in 0..50 {
            let literal = generator.literal(&map).unwrap();
            let keys: Vec<&str> = literal
                .trim_matches(|c| c == '{' || c == '}')
                .split(", ")
                .filter_map(|entry| entry.split(':').next())
                .collect();
            let unique: BTreeSet<&&str> = keys.iter().collect();
            assert_eq!(keys.len(), unique.len(), "{literal}");
        }
    }

    #[test]
    fn tst_100_a_row_renders_an_insert_naming_every_column() {
        let table = SchemaGen::simple("cdm_test", "kv").unwrap();
        let row = DataGen::new(Seed::new(2)).row(&table).unwrap();
        let statement = row.write_statement(&table).unwrap();

        assert!(statement.starts_with("INSERT INTO cdm_test.kv (key, value) VALUES ("));
        assert!(statement.ends_with(')'));
        assert_eq!(row.values().len(), 2);
        assert!(row.literal("key").is_some());
        assert!(row.literal("absent").is_none());
        assert!(row
            .primary_key_predicate(&table)
            .unwrap()
            .starts_with("key = '"));
    }

    #[test]
    fn mig_030_a_counter_row_renders_an_update_not_an_insert() {
        let table = SchemaGen::counters("cdm_test", "hits").unwrap();
        let row = DataGen::new(Seed::new(2)).row(&table).unwrap();
        let statement = row.write_statement(&table).unwrap();

        assert!(
            statement.starts_with("UPDATE cdm_test.hits SET "),
            "{statement}"
        );
        assert!(statement.contains("hits = hits + "), "{statement}");
        assert!(statement.contains("misses = misses + "), "{statement}");
        assert!(statement.contains(" WHERE key = "), "{statement}");
        assert!(statement.contains(" AND bucket = "), "{statement}");
    }

    #[test]
    fn tst_100_a_row_from_the_wrong_table_is_an_error_not_a_malformed_statement() {
        let kv = SchemaGen::simple("ks", "kv").unwrap();
        let other = SchemaGen::simple("ks", "other").unwrap();
        let mismatched = TableSpec::builder("ks", "third")
            .partition_key("id", CqlTypeInfo::Int)
            .build()
            .unwrap();

        let row = DataGen::new(Seed::new(1)).row(&kv).unwrap();
        // Same shape, different name: the statement is still well formed.
        assert!(row.write_statement(&other).is_ok());

        let err = row.write_statement(&mismatched).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert!(err.to_string().contains("`id`"), "{err}");
        assert!(row.primary_key_predicate(&mismatched).is_err());
    }

    #[test]
    fn tst_100_nulls_are_opt_in_and_never_land_on_a_key() {
        let table = SchemaGen::simple("ks", "kv").unwrap();

        let mut never = DataGen::new(Seed::new(1));
        for _ in 0..50 {
            let row = never.row(&table).unwrap();
            assert_ne!(row.literal("value"), Some("null"));
        }

        let mut always = DataGen::with_options(
            Seed::new(1),
            DataGenOptions::default().with_null_probability(1.0),
        );
        for _ in 0..50 {
            let row = always.row(&table).unwrap();
            assert_eq!(row.literal("value"), Some("null"));
            assert_ne!(row.literal("key"), Some("null"), "a key is never null");
        }
    }

    #[test]
    fn tst_100_options_clamp_nonsense_rather_than_producing_it() {
        let options = DataGenOptions::default()
            .with_null_probability(5.0)
            .with_collection_len(9, 2)
            .with_max_text_len(0);
        assert!((options.null_probability - 1.0).abs() < f64::EPSILON);
        assert_eq!(options.min_collection_len, 2);
        assert_eq!(options.max_collection_len, 9);
        assert_eq!(options.max_text_len, 1);
        assert!(
            (DataGenOptions::default()
                .with_null_probability(-1.0)
                .null_probability)
                .abs()
                < f64::EPSILON
        );
    }
}
