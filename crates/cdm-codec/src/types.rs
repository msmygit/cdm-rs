//! The driver-independent CQL type taxonomy (`CDC-001`..`CDC-004`).
//!
//! [`CqlTypeInfo`] is the shape of a CQL type as `system_schema.columns.type` spells it, parsed
//! into a tree. It is deliberately independent of any driver: `cdm-codec` may not depend on
//! `scylla` or on `cdm-cql` (`ARCHITECTURE.md` §3.2), so everything downstream — the conversion
//! planner, the codec registry, the built-in codecs — speaks in terms of this enum and raw byte
//! buffers.
//!
//! # What is covered
//!
//! * every CQL primitive of `CDC-001`;
//! * `list`/`set`/`map`/`tuple`/UDT and `vector<T, N>` of `CDC-002`, nested arbitrarily, with
//!   frozen-ness recorded;
//! * the DSE geometry types and `DateRangeType` of `CDC-003`;
//! * anything else the schema reports, as [`CqlTypeInfo::Custom`], so an unknown type is carried
//!   rather than rejected.
//!
//! # Frozen-ness
//!
//! Frozen-ness is *recorded* but is not part of type identity for conversion purposes. This
//! matches the Java driver, whose `DefaultListType.equals` and friends compare element types only,
//! and it matches the wire: a frozen and a non-frozen collection of the same element type
//! serialise identically. Use [`CqlTypeInfo::same_type`] for the conversion-planning comparison
//! and `==` for exact structural equality including frozen-ness.

use std::fmt;

use cdm_core::CdmError;
use serde::{Deserialize, Serialize};

/// One field of a user-defined type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UdtField {
    /// The field name, as the schema spells it.
    pub name: String,
    /// The field's type.
    pub cql_type: CqlTypeInfo,
}

impl UdtField {
    /// Creates a field.
    pub fn new(name: impl Into<String>, cql_type: CqlTypeInfo) -> Self {
        Self {
            name: name.into(),
            cql_type,
        }
    }
}

/// A CQL type, driver-independently (`CDC-001`..`CDC-004`).
///
/// ```
/// use cdm_codec::CqlTypeInfo;
///
/// let t = CqlTypeInfo::parse("map<text, frozen<list<int>>>")?;
/// assert_eq!(t.to_string(), "map<text, frozen<list<int>>>");
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CqlTypeInfo {
    /// `ascii` — US-ASCII text.
    Ascii,
    /// `bigint` — 64-bit signed integer.
    BigInt,
    /// `blob` — arbitrary bytes.
    Blob,
    /// `boolean`.
    Boolean,
    /// `counter` — 64-bit signed counter (`MIG-030`).
    Counter,
    /// `date` — days since epoch, biased by 2^31.
    Date,
    /// `decimal` — arbitrary-precision signed decimal.
    Decimal,
    /// `double` — IEEE-754 binary64.
    Double,
    /// `duration` — months/days/nanoseconds.
    Duration,
    /// `float` — IEEE-754 binary32.
    Float,
    /// `inet` — IPv4 or IPv6 address.
    Inet,
    /// `int` — 32-bit signed integer.
    Int,
    /// `smallint` — 16-bit signed integer.
    SmallInt,
    /// `text` (and its alias `varchar`) — UTF-8 text.
    Text,
    /// `time` — nanoseconds since midnight.
    Time,
    /// `timestamp` — milliseconds since the Unix epoch.
    Timestamp,
    /// `timeuuid` — version-1 UUID.
    TimeUuid,
    /// `tinyint` — 8-bit signed integer.
    TinyInt,
    /// `uuid`.
    Uuid,
    /// `varint` — arbitrary-precision signed integer.
    VarInt,
    /// DSE `PointType` (`CDC-003`), WKB-encoded.
    Point,
    /// DSE `LineStringType` (`CDC-003`), WKB-encoded.
    LineString,
    /// DSE `PolygonType` (`CDC-003`), WKB-encoded.
    Polygon,
    /// DSE `DateRangeType` (`CDC-003`).
    DateRange,
    /// `list<T>`, optionally frozen.
    List {
        /// The element type.
        element: Box<CqlTypeInfo>,
        /// Whether the collection is frozen.
        frozen: bool,
    },
    /// `set<T>`, optionally frozen.
    Set {
        /// The element type.
        element: Box<CqlTypeInfo>,
        /// Whether the collection is frozen.
        frozen: bool,
    },
    /// `map<K, V>`, optionally frozen.
    Map {
        /// The key type.
        key: Box<CqlTypeInfo>,
        /// The value type.
        value: Box<CqlTypeInfo>,
        /// Whether the collection is frozen.
        frozen: bool,
    },
    /// `tuple<...>`. Tuples are always frozen in CQL.
    Tuple {
        /// The component types, in declaration order.
        elements: Vec<CqlTypeInfo>,
    },
    /// A user-defined type.
    Udt {
        /// The keyspace the type is declared in, when the schema qualified it.
        keyspace: Option<String>,
        /// The type name.
        name: String,
        /// The fields, in declaration order. Empty when the type was parsed from a bare name and
        /// no [`UdtResolver`] supplied the definition.
        fields: Vec<UdtField>,
        /// Whether the UDT is frozen.
        frozen: bool,
    },
    /// `vector<T, N>` (`CDC-004`) — a collection with a fixed number of dimensions.
    Vector {
        /// The element type.
        element: Box<CqlTypeInfo>,
        /// The number of elements. Part of the type: `vector<float, 3>` and `vector<float, 4>`
        /// are different types.
        dimensions: usize,
    },
    /// Anything the schema reports that cdm-rs does not model, carried verbatim so that a column
    /// of an unknown type still round-trips as raw bytes.
    Custom(String),
}

/// Supplies UDT definitions to [`CqlTypeInfo::parse_with`].
///
/// `system_schema.columns.type` names a UDT by name only, so resolving its fields needs the
/// keyspace's type definitions — which `cdm-cql` reads (`SCH-001`) and this crate must not.
pub trait UdtResolver {
    /// The fields of the named UDT, or `None` if the name is not a UDT in scope.
    fn resolve(&self, keyspace: Option<&str>, name: &str) -> Option<Vec<UdtField>>;
}

impl UdtResolver for () {
    fn resolve(&self, _keyspace: Option<&str>, _name: &str) -> Option<Vec<UdtField>> {
        None
    }
}

impl<F> UdtResolver for F
where
    F: Fn(Option<&str>, &str) -> Option<Vec<UdtField>>,
{
    fn resolve(&self, keyspace: Option<&str>, name: &str) -> Option<Vec<UdtField>> {
        self(keyspace, name)
    }
}

impl CqlTypeInfo {
    /// Every primitive type of `CDC-001`, in specification order.
    pub const PRIMITIVES: [Self; 20] = [
        Self::Ascii,
        Self::BigInt,
        Self::Blob,
        Self::Boolean,
        Self::Counter,
        Self::Date,
        Self::Decimal,
        Self::Double,
        Self::Duration,
        Self::Float,
        Self::Inet,
        Self::Int,
        Self::SmallInt,
        Self::Text,
        Self::Time,
        Self::Timestamp,
        Self::TimeUuid,
        Self::TinyInt,
        Self::Uuid,
        Self::VarInt,
    ];

    /// The four DSE custom types of `CDC-003`.
    pub const DSE_TYPES: [Self; 4] = [
        Self::Point,
        Self::LineString,
        Self::Polygon,
        Self::DateRange,
    ];

    /// Parses a type as `system_schema.columns.type` spells it, leaving UDT fields unresolved.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::SchemaMismatch`](cdm_core::ErrorKind::SchemaMismatch) when the type expression is malformed.
    pub fn parse(text: &str) -> Result<Self, CdmError> {
        Self::parse_with(text, &())
    }

    /// Parses a type, resolving UDT names through `resolver`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::SchemaMismatch`](cdm_core::ErrorKind::SchemaMismatch) when the type expression is malformed.
    pub fn parse_with(text: &str, resolver: &dyn UdtResolver) -> Result<Self, CdmError> {
        let mut parser = parse::Parser::new(text, resolver);
        let parsed = parser.parse_type()?;
        parser.finish()?;
        Ok(parsed)
    }

    /// Whether this is one of the primitive types of `CDC-001` or a DSE custom type, i.e. a type
    /// with no element types to recurse into.
    ///
    /// Java's `CqlData.isPrimitive` answers the same question, and treats the DSE geometry types
    /// and `DateRangeType` as primitives too, because its
    /// `primitiveDataTypeToJavaClassMap` contains them.
    pub const fn is_primitive(&self) -> bool {
        !matches!(
            self,
            Self::List { .. }
                | Self::Set { .. }
                | Self::Map { .. }
                | Self::Tuple { .. }
                | Self::Udt { .. }
                | Self::Vector { .. }
                | Self::Custom(_)
        )
    }

    /// Whether this is a collection in Java's sense: a list, set, map, tuple, UDT or vector.
    /// *(Java `CqlData.isCollection`.)*
    pub const fn is_collection(&self) -> bool {
        matches!(
            self,
            Self::List { .. }
                | Self::Set { .. }
                | Self::Map { .. }
                | Self::Tuple { .. }
                | Self::Udt { .. }
                | Self::Vector { .. }
        )
    }

    /// Whether the type is frozen. Primitives, tuples and vectors are never reported as frozen:
    /// tuples are implicitly frozen and `vector` has no frozen form.
    /// *(Java `CqlData.isFrozen`.)*
    pub const fn is_frozen(&self) -> bool {
        match self {
            Self::List { frozen, .. } | Self::Set { frozen, .. } | Self::Map { frozen, .. } => {
                *frozen
            }
            Self::Udt { frozen, .. } => *frozen,
            _ => false,
        }
    }

    /// The element types to recurse into: the element of a list/set/vector, the key and value of a
    /// map, the components of a tuple, the field types of a UDT.
    /// *(Java `CqlData.extractDataTypesFromCollection`.)*
    pub fn element_types(&self) -> Vec<&Self> {
        match self {
            Self::List { element, .. }
            | Self::Set { element, .. }
            | Self::Vector { element, .. } => vec![element],
            Self::Map { key, value, .. } => vec![key, value],
            Self::Tuple { elements } => elements.iter().collect(),
            Self::Udt { fields, .. } => fields.iter().map(|f| &f.cql_type).collect(),
            _ => Vec::new(),
        }
    }

    /// Structural equality *ignoring frozen-ness*, which is the comparison the conversion planner
    /// uses (`CDC-011`).
    ///
    /// The Java driver's `DataType.equals` also ignores the frozen flag, and so does the wire
    /// format: a `frozen<list<int>>` and a `list<int>` serialise identically, so a column pair
    /// that differs only in frozen-ness is a legitimate `Passthrough`.
    pub fn same_type(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::List { element: a, .. }, Self::List { element: b, .. })
            | (Self::Set { element: a, .. }, Self::Set { element: b, .. }) => a.same_type(b),
            (
                Self::Map {
                    key: ka, value: va, ..
                },
                Self::Map {
                    key: kb, value: vb, ..
                },
            ) => ka.same_type(kb) && va.same_type(vb),
            (Self::Tuple { elements: a }, Self::Tuple { elements: b }) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same_type(y))
            }
            (
                Self::Udt {
                    name: na,
                    fields: fa,
                    ..
                },
                Self::Udt {
                    name: nb,
                    fields: fb,
                    ..
                },
            ) => {
                na == nb
                    && fa.len() == fb.len()
                    && fa
                        .iter()
                        .zip(fb)
                        .all(|(x, y)| x.name == y.name && x.cql_type.same_type(&y.cql_type))
            }
            (
                Self::Vector {
                    element: a,
                    dimensions: da,
                },
                Self::Vector {
                    element: b,
                    dimensions: db,
                },
            ) => da == db && a.same_type(b),
            _ => self == other,
        }
    }

    /// The fixed serialised width of a value of this type, when it has one.
    ///
    /// Used by the `vector<T, N>` serialisation, whose elements are a contiguous array when the
    /// element type is fixed-width (`ARCHITECTURE.md` §6.1).
    pub const fn fixed_width(&self) -> Option<usize> {
        match self {
            Self::Boolean | Self::TinyInt => Some(1),
            Self::SmallInt => Some(2),
            Self::Int | Self::Date | Self::Float => Some(4),
            Self::BigInt | Self::Counter | Self::Double | Self::Time | Self::Timestamp => Some(8),
            Self::Uuid | Self::TimeUuid => Some(16),
            _ => None,
        }
    }
}

impl fmt::Display for CqlTypeInfo {
    /// Renders the type as CQL spells it, which is what diagnostics and `cdm codecs list`
    /// (`CDC-031`) print.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ascii => f.write_str("ascii"),
            Self::BigInt => f.write_str("bigint"),
            Self::Blob => f.write_str("blob"),
            Self::Boolean => f.write_str("boolean"),
            Self::Counter => f.write_str("counter"),
            Self::Date => f.write_str("date"),
            Self::Decimal => f.write_str("decimal"),
            Self::Double => f.write_str("double"),
            Self::Duration => f.write_str("duration"),
            Self::Float => f.write_str("float"),
            Self::Inet => f.write_str("inet"),
            Self::Int => f.write_str("int"),
            Self::SmallInt => f.write_str("smallint"),
            Self::Text => f.write_str("text"),
            Self::Time => f.write_str("time"),
            Self::Timestamp => f.write_str("timestamp"),
            Self::TimeUuid => f.write_str("timeuuid"),
            Self::TinyInt => f.write_str("tinyint"),
            Self::Uuid => f.write_str("uuid"),
            Self::VarInt => f.write_str("varint"),
            Self::Point => f.write_str("PointType"),
            Self::LineString => f.write_str("LineStringType"),
            Self::Polygon => f.write_str("PolygonType"),
            Self::DateRange => f.write_str("DateRangeType"),
            Self::List { element, frozen } => write_wrapped(f, *frozen, "list", &[element]),
            Self::Set { element, frozen } => write_wrapped(f, *frozen, "set", &[element]),
            Self::Map { key, value, frozen } => write_wrapped(f, *frozen, "map", &[key, value]),
            Self::Tuple { elements } => {
                let refs: Vec<&Self> = elements.iter().collect();
                write_wrapped(f, false, "tuple", &refs)
            }
            Self::Udt {
                keyspace,
                name,
                frozen,
                ..
            } => {
                if *frozen {
                    f.write_str("frozen<")?;
                }
                if let Some(keyspace) = keyspace {
                    write!(f, "{keyspace}.")?;
                }
                f.write_str(name)?;
                if *frozen {
                    f.write_str(">")?;
                }
                Ok(())
            }
            Self::Vector {
                element,
                dimensions,
            } => write!(f, "vector<{element}, {dimensions}>"),
            Self::Custom(name) => f.write_str(name),
        }
    }
}

fn write_wrapped(
    f: &mut fmt::Formatter<'_>,
    frozen: bool,
    keyword: &str,
    args: &[&CqlTypeInfo],
) -> fmt::Result {
    if frozen {
        f.write_str("frozen<")?;
    }
    write!(f, "{keyword}<")?;
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{arg}")?;
    }
    f.write_str(">")?;
    if frozen {
        f.write_str(">")?;
    }
    Ok(())
}

mod parse {
    //! A recursive-descent parser for `system_schema.columns.type` expressions.

    use super::{CqlTypeInfo, UdtField, UdtResolver};
    use cdm_core::{CdmError, ErrorKind};

    /// The fully qualified names DSE reports for its custom types, and the short forms a
    /// configuration file may use.
    const DSE_TYPES: [(&str, CqlTypeInfo); 8] = [
        (
            "org.apache.cassandra.db.marshal.PointType",
            CqlTypeInfo::Point,
        ),
        (
            "org.apache.cassandra.db.marshal.LineStringType",
            CqlTypeInfo::LineString,
        ),
        (
            "org.apache.cassandra.db.marshal.PolygonType",
            CqlTypeInfo::Polygon,
        ),
        (
            "org.apache.cassandra.db.marshal.DateRangeType",
            CqlTypeInfo::DateRange,
        ),
        ("pointtype", CqlTypeInfo::Point),
        ("linestringtype", CqlTypeInfo::LineString),
        ("polygontype", CqlTypeInfo::Polygon),
        ("daterangetype", CqlTypeInfo::DateRange),
    ];

    pub(super) struct Parser<'a> {
        text: &'a str,
        pos: usize,
        resolver: &'a dyn UdtResolver,
    }

    impl<'a> Parser<'a> {
        pub(super) fn new(text: &'a str, resolver: &'a dyn UdtResolver) -> Self {
            Self {
                text,
                pos: 0,
                resolver,
            }
        }

        pub(super) fn finish(&mut self) -> Result<(), CdmError> {
            self.skip_whitespace();
            if self.pos < self.text.len() {
                return Err(self.error("unexpected trailing input"));
            }
            Ok(())
        }

        fn error(&self, message: &str) -> CdmError {
            CdmError::new(
                ErrorKind::SchemaMismatch,
                format!(
                    "cannot parse CQL type `{}` at offset {}: {message}",
                    self.text, self.pos
                ),
            )
        }

        fn rest(&self) -> &'a str {
            self.text.get(self.pos..).unwrap_or_default()
        }

        fn skip_whitespace(&mut self) {
            let trimmed = self.rest().trim_start();
            self.pos = self.text.len() - trimmed.len();
        }

        fn peek(&self) -> Option<char> {
            self.rest().chars().next()
        }

        fn eat(&mut self, expected: char) -> Result<(), CdmError> {
            self.skip_whitespace();
            if self.peek() == Some(expected) {
                self.pos += expected.len_utf8();
                Ok(())
            } else {
                Err(self.error(&format!("expected `{expected}`")))
            }
        }

        /// An identifier, possibly dotted (`ks.udt`) and possibly a fully qualified Java class
        /// name. Single-quoted class names are unquoted first.
        fn identifier(&mut self) -> Result<String, CdmError> {
            self.skip_whitespace();
            if self.peek() == Some('\'') {
                self.pos += 1;
                let rest = self.rest();
                let end = rest
                    .find('\'')
                    .ok_or_else(|| self.error("unterminated quoted type name"))?;
                let name = rest.get(..end).unwrap_or_default().to_owned();
                self.pos += end + 1;
                return Ok(name);
            }
            if self.peek() == Some('"') {
                self.pos += 1;
                let rest = self.rest();
                let end = rest
                    .find('"')
                    .ok_or_else(|| self.error("unterminated quoted identifier"))?;
                let name = rest.get(..end).unwrap_or_default().to_owned();
                self.pos += end + 1;
                return Ok(name);
            }
            let rest = self.rest();
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '$'))
                .unwrap_or(rest.len());
            if end == 0 {
                return Err(self.error("expected a type name"));
            }
            let name = rest.get(..end).unwrap_or_default().to_owned();
            self.pos += end;
            Ok(name)
        }

        fn dimensions(&mut self) -> Result<usize, CdmError> {
            self.skip_whitespace();
            let rest = self.rest();
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let digits = rest.get(..end).unwrap_or_default();
            let value: usize = digits
                .parse()
                .map_err(|_| self.error("expected a vector dimension count"))?;
            self.pos += end;
            Ok(value)
        }

        pub(super) fn parse_type(&mut self) -> Result<CqlTypeInfo, CdmError> {
            self.parse_type_inner(false)
        }

        fn parse_type_inner(&mut self, frozen: bool) -> Result<CqlTypeInfo, CdmError> {
            let name = self.identifier()?;
            let lower = name.to_ascii_lowercase();
            match lower.as_str() {
                "frozen" => {
                    self.eat('<')?;
                    let inner = self.parse_type_inner(true)?;
                    self.eat('>')?;
                    Ok(inner)
                }
                "list" | "set" => {
                    self.eat('<')?;
                    let element = Box::new(self.parse_type_inner(false)?);
                    self.eat('>')?;
                    Ok(if lower == "list" {
                        CqlTypeInfo::List { element, frozen }
                    } else {
                        CqlTypeInfo::Set { element, frozen }
                    })
                }
                "map" => {
                    self.eat('<')?;
                    let key = Box::new(self.parse_type_inner(false)?);
                    self.eat(',')?;
                    let value = Box::new(self.parse_type_inner(false)?);
                    self.eat('>')?;
                    Ok(CqlTypeInfo::Map { key, value, frozen })
                }
                "tuple" => {
                    self.eat('<')?;
                    let mut elements = vec![self.parse_type_inner(false)?];
                    loop {
                        self.skip_whitespace();
                        match self.peek() {
                            Some(',') => {
                                self.pos += 1;
                                elements.push(self.parse_type_inner(false)?);
                            }
                            _ => break,
                        }
                    }
                    self.eat('>')?;
                    Ok(CqlTypeInfo::Tuple { elements })
                }
                "vector" => {
                    self.eat('<')?;
                    let element = Box::new(self.parse_type_inner(false)?);
                    self.eat(',')?;
                    let dimensions = self.dimensions()?;
                    self.eat('>')?;
                    Ok(CqlTypeInfo::Vector {
                        element,
                        dimensions,
                    })
                }
                _ => Ok(self.leaf(&name, &lower, frozen)),
            }
        }

        fn leaf(&self, name: &str, lower: &str, frozen: bool) -> CqlTypeInfo {
            if let Some(primitive) = primitive(lower) {
                return primitive;
            }
            for (candidate, dse) in DSE_TYPES {
                if candidate == name || candidate == lower {
                    return dse;
                }
            }
            // A dotted name is `keyspace.type`; a Java class name would have matched above.
            let (keyspace, simple) = match name.rsplit_once('.') {
                Some((ks, simple)) => (Some(ks.to_owned()), simple.to_owned()),
                None => (None, name.to_owned()),
            };
            let fields: Vec<UdtField> = self
                .resolver
                .resolve(keyspace.as_deref(), &simple)
                .unwrap_or_default();
            CqlTypeInfo::Udt {
                keyspace,
                name: simple,
                fields,
                frozen,
            }
        }
    }

    fn primitive(lower: &str) -> Option<CqlTypeInfo> {
        Some(match lower {
            "ascii" => CqlTypeInfo::Ascii,
            "bigint" => CqlTypeInfo::BigInt,
            "blob" => CqlTypeInfo::Blob,
            "boolean" => CqlTypeInfo::Boolean,
            "counter" => CqlTypeInfo::Counter,
            "date" => CqlTypeInfo::Date,
            "decimal" => CqlTypeInfo::Decimal,
            "double" => CqlTypeInfo::Double,
            "duration" => CqlTypeInfo::Duration,
            "float" => CqlTypeInfo::Float,
            "inet" => CqlTypeInfo::Inet,
            "int" => CqlTypeInfo::Int,
            "smallint" => CqlTypeInfo::SmallInt,
            // `varchar` is an alias of `text`, not a distinct type.
            "text" | "varchar" => CqlTypeInfo::Text,
            "time" => CqlTypeInfo::Time,
            "timestamp" => CqlTypeInfo::Timestamp,
            "timeuuid" => CqlTypeInfo::TimeUuid,
            "tinyint" => CqlTypeInfo::TinyInt,
            "uuid" => CqlTypeInfo::Uuid,
            "varint" => CqlTypeInfo::VarInt,
            _ => return None,
        })
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
    use cdm_core::ErrorKind;

    #[test]
    fn cdc_001_every_primitive_type_round_trips_through_its_cql_spelling() {
        for primitive in CqlTypeInfo::PRIMITIVES {
            let text = primitive.to_string();
            assert_eq!(CqlTypeInfo::parse(&text).unwrap(), primitive, "{text}");
            assert!(primitive.is_primitive());
            assert!(!primitive.is_collection());
            assert!(!primitive.is_frozen());
        }
        // The specification lists 21 names for 20 types: `varchar` is an alias of `text`.
        assert_eq!(CqlTypeInfo::parse("varchar").unwrap(), CqlTypeInfo::Text);
    }

    #[test]
    fn cdc_001_an_unknown_type_name_without_a_resolver_is_a_udt() {
        let parsed = CqlTypeInfo::parse("ks.address").unwrap();
        assert_eq!(
            parsed,
            CqlTypeInfo::Udt {
                keyspace: Some("ks".to_owned()),
                name: "address".to_owned(),
                fields: Vec::new(),
                frozen: false,
            }
        );
        assert_eq!(parsed.to_string(), "ks.address");
    }

    #[test]
    fn cdc_002_collections_nest_arbitrarily_and_record_frozen_ness() {
        let parsed = CqlTypeInfo::parse("map<text, frozen<list<set<int>>>>").unwrap();
        assert_eq!(parsed.to_string(), "map<text, frozen<list<set<int>>>>");
        assert!(!parsed.is_frozen());
        assert!(parsed.is_collection());
        let value = parsed.element_types()[1];
        assert!(value.is_frozen());
        assert_eq!(value.element_types()[0].to_string(), "set<int>");
    }

    #[test]
    fn cdc_002_tuples_and_udts_expose_their_components() {
        let tuple = CqlTypeInfo::parse("tuple<int, text, blob>").unwrap();
        assert_eq!(tuple.element_types().len(), 3);
        assert_eq!(tuple.to_string(), "tuple<int, text, blob>");
        // Tuples are implicitly frozen, so CQL never reports them as such.
        assert!(!CqlTypeInfo::parse("frozen<tuple<int>>")
            .unwrap()
            .is_frozen());

        let udt = CqlTypeInfo::Udt {
            keyspace: None,
            name: "address".to_owned(),
            fields: vec![
                UdtField::new("street", CqlTypeInfo::Text),
                UdtField::new("zip", CqlTypeInfo::Int),
            ],
            frozen: true,
        };
        assert_eq!(udt.to_string(), "frozen<address>");
        assert_eq!(udt.element_types().len(), 2);
        assert!(udt.is_frozen());
    }

    #[test]
    fn cdc_002_a_udt_resolver_supplies_field_definitions() {
        let resolver = |keyspace: Option<&str>, name: &str| {
            (keyspace == Some("ks") && name == "address")
                .then(|| vec![UdtField::new("street", CqlTypeInfo::Text)])
        };
        let parsed = CqlTypeInfo::parse_with("list<frozen<ks.address>>", &resolver).unwrap();
        assert_eq!(parsed.element_types()[0].element_types().len(), 1);
    }

    #[test]
    fn cdc_003_dse_custom_types_parse_from_class_names_and_short_names() {
        assert_eq!(
            CqlTypeInfo::parse("org.apache.cassandra.db.marshal.PointType").unwrap(),
            CqlTypeInfo::Point
        );
        assert_eq!(
            CqlTypeInfo::parse("'org.apache.cassandra.db.marshal.DateRangeType'").unwrap(),
            CqlTypeInfo::DateRange
        );
        assert_eq!(
            CqlTypeInfo::parse("LineStringType").unwrap(),
            CqlTypeInfo::LineString
        );
        assert_eq!(
            CqlTypeInfo::parse("polygontype").unwrap(),
            CqlTypeInfo::Polygon
        );
        for dse in CqlTypeInfo::DSE_TYPES {
            assert!(dse.is_primitive(), "{dse} should behave as a primitive");
        }
    }

    #[test]
    fn cdc_004_vectors_carry_their_dimensions_as_part_of_the_type() {
        let parsed = CqlTypeInfo::parse("vector<float, 3>").unwrap();
        assert_eq!(parsed.to_string(), "vector<float, 3>");
        assert!(parsed.is_collection());
        assert!(!parsed.same_type(&CqlTypeInfo::parse("vector<float, 4>").unwrap()));
        assert!(parsed.same_type(&CqlTypeInfo::parse("vector<float, 3>").unwrap()));
        assert_eq!(CqlTypeInfo::Float.fixed_width(), Some(4));
        assert_eq!(CqlTypeInfo::Text.fixed_width(), None);
    }

    #[test]
    fn cdc_011_frozen_ness_is_not_part_of_conversion_identity() {
        let frozen = CqlTypeInfo::parse("frozen<list<int>>").unwrap();
        let plain = CqlTypeInfo::parse("list<int>").unwrap();
        assert_ne!(frozen, plain, "structural equality keeps frozen-ness");
        assert!(frozen.same_type(&plain), "conversion identity ignores it");
        assert!(!frozen.same_type(&CqlTypeInfo::parse("list<text>").unwrap()));
    }

    #[test]
    fn cdc_001_malformed_type_expressions_are_schema_mismatches() {
        for bad in ["list<int", "map<int>", "vector<float>", "list<int>>", ""] {
            let error = CqlTypeInfo::parse(bad).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::SchemaMismatch, "{bad}");
        }
    }

    #[test]
    fn cdc_001_quoted_identifiers_and_custom_types_are_carried_verbatim() {
        assert_eq!(
            CqlTypeInfo::parse("\"MyUdt\"").unwrap(),
            CqlTypeInfo::Udt {
                keyspace: None,
                name: "MyUdt".to_owned(),
                fields: Vec::new(),
                frozen: false,
            }
        );
        assert_eq!(
            CqlTypeInfo::Custom("com.example.Thing".to_owned()).to_string(),
            "com.example.Thing"
        );
        assert!(!CqlTypeInfo::Custom("x".to_owned()).is_primitive());
        assert!(!CqlTypeInfo::Custom("x".to_owned()).is_collection());
    }

    #[test]
    fn cdc_002_udt_identity_compares_names_and_fields_but_not_frozen_ness() {
        let a = CqlTypeInfo::Udt {
            keyspace: Some("origin_ks".to_owned()),
            name: "address".to_owned(),
            fields: vec![UdtField::new("street", CqlTypeInfo::Text)],
            frozen: true,
        };
        let b = CqlTypeInfo::Udt {
            keyspace: Some("target_ks".to_owned()),
            name: "address".to_owned(),
            fields: vec![UdtField::new("street", CqlTypeInfo::Text)],
            frozen: false,
        };
        assert!(a.same_type(&b), "keyspace and frozen-ness are not identity");
        let c = CqlTypeInfo::Udt {
            keyspace: None,
            name: "address".to_owned(),
            fields: vec![UdtField::new("street", CqlTypeInfo::Ascii)],
            frozen: false,
        };
        assert!(!a.same_type(&c));
    }
}
