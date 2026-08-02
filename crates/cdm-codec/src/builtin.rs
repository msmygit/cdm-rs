//! The built-in codec set, with Java-identical semantics (`CDC-020`, `CDC-021`).
//!
//! Every codec here is a [`CodecPlugin`] registered through the ordinary public path
//! (`CDC-030`, `PLG-010`) — there is no privileged internal route a third-party crate could not
//! take. The names are Java's `Codecset` enum constants verbatim, because they are what
//! `transform.codecs` accepts in a configuration file that may have been written for Java CDM.
//!
//! # Where Java's behaviour is surprising
//!
//! Three places, each reproduced deliberately and marked in the code:
//!
//! * `DOUBLE_STRING` formats with `RoundingMode.FLOOR`, not the usual half-even, and truncates at
//!   nine fraction digits (`DOUBLE_StringCodec`);
//! * `BIGINT_BIGINTEGER` encodes a big integer through `BigInteger.longValue()`, which **silently
//!   truncates to the low 64 bits** rather than failing (`BIGINT_BigIntegerCodec.encode`);
//! * `TIMESTAMP_STRING_FORMAT` resolves its zone to a **fixed offset taken at startup**
//!   (`ZoneId.of(zone).getRules().getOffset(Instant.now())`), so a value whose own instant falls on
//!   the other side of a daylight-saving boundary is converted with today's offset, not its own
//!   (`TIMESTAMP_StringFormatCodec`).

use std::str::FromStr as _;
use std::sync::Arc;

use bigdecimal::BigDecimal;
use cdm_core::{CdmError, CodecPlugin, ErrorKind, Plugin, RawCell, Registry, TypePair};
use chrono::{DateTime, FixedOffset, Offset as _, TimeZone as _, Utc};
use num_bigint::{BigInt, Sign};
use serde::{Deserialize, Serialize};

use crate::codec::{Converter, FnConverter};
use crate::format::{format_double_java, parse_double_java, JavaDateFormat};
use crate::geo::{DateRange, Geometry};
use crate::wire::{
    conversion_error, read_ascii, read_decimal, read_f64, read_i32, read_i64, read_text,
    read_varint, write_decimal, write_f64, write_i32, write_i64, write_varint,
};

/// The provider name every built-in codec reports.
pub const BUILTIN_PROVIDER: &str = "cdm-codec";

/// The named codec set of `CDC-020`, spelled as Java's `Codecset` enum spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Codecset {
    /// `int` ↔ `text`.
    IntString,
    /// `double` ↔ `text`, formatted `0.#########`.
    DoubleString,
    /// `bigint` ↔ `text`.
    BigintString,
    /// `bigint` ↔ arbitrary-precision integer. Always registered.
    BigintBiginteger,
    /// `decimal` ↔ `text`.
    DecimalString,
    /// `timestamp` ↔ epoch-millis `text`.
    TimestampStringMillis,
    /// `timestamp` ↔ `text` via `transform.codecs.timestamp_format`.
    TimestampStringFormat,
    /// DSE `PointType` ↔ `text` (well-known text).
    PointType,
    /// DSE `PolygonType` ↔ `text` (well-known text).
    PolygonType,
    /// DSE `DateRangeType` ↔ `text`.
    DateRange,
    /// DSE `LineStringType` ↔ `text` (well-known text).
    LineString,
    /// `text` ↔ `blob`.
    StringBlob,
    /// `ascii` ↔ `blob`.
    AsciiBlob,
}

impl Codecset {
    /// Every codec, in the order Java's `Codecset` declares them.
    pub const ALL: [Self; 13] = [
        Self::IntString,
        Self::DoubleString,
        Self::BigintString,
        Self::BigintBiginteger,
        Self::DecimalString,
        Self::TimestampStringMillis,
        Self::TimestampStringFormat,
        Self::PointType,
        Self::PolygonType,
        Self::DateRange,
        Self::LineString,
        Self::StringBlob,
        Self::AsciiBlob,
    ];

    /// The configuration name, e.g. `INT_STRING`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::IntString => "INT_STRING",
            Self::DoubleString => "DOUBLE_STRING",
            Self::BigintString => "BIGINT_STRING",
            Self::BigintBiginteger => "BIGINT_BIGINTEGER",
            Self::DecimalString => "DECIMAL_STRING",
            Self::TimestampStringMillis => "TIMESTAMP_STRING_MILLIS",
            Self::TimestampStringFormat => "TIMESTAMP_STRING_FORMAT",
            Self::PointType => "POINT_TYPE",
            Self::PolygonType => "POLYGON_TYPE",
            Self::DateRange => "DATE_RANGE",
            Self::LineString => "LINE_STRING",
            Self::StringBlob => "STRING_BLOB",
            Self::AsciiBlob => "ASCII_BLOB",
        }
    }

    /// Resolves a configured name.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] — a Tier-1 error — naming the unknown codec and listing the
    /// available ones.
    pub fn parse(name: &str) -> Result<Self, CdmError> {
        let wanted = name.trim().to_ascii_uppercase();
        Self::ALL
            .into_iter()
            .find(|codec| codec.name() == wanted)
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "unknown codec `{name}`; available codecs are {}",
                        Self::ALL.map(Self::name).join(", ")
                    ),
                )
                .with_context(|c| c.with_config_key("transform.codecs"))
            })
    }
}

/// The `timestamp_format` / `timestamp_format_zone` settings of `TIMESTAMP_STRING_FORMAT`
/// (`CDC-021`, `CDC-022`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampFormat {
    format: JavaDateFormat,
    zone: String,
    offset: FixedOffset,
}

impl TimestampFormat {
    /// Resolves the settings, taking the zone's offset as of now.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] — a Tier-1 error (`CDC-021`) — when the format is empty or
    /// uses an unsupported pattern letter (`CDC-022`), or when the zone is not a known IANA zone
    /// identifier.
    pub fn new(format: &str, zone: &str) -> Result<Self, CdmError> {
        Self::with_reference(format, zone, Utc::now())
    }

    /// Resolves the settings against an explicit reference instant.
    ///
    /// Java captures `ZoneId.of(zone).getRules().getOffset(Instant.now())` in the codec's
    /// constructor and uses that **fixed** offset for every value thereafter
    /// (`TIMESTAMP_StringFormatCodec`). That is surprising — a summer run and a winter run of the
    /// same migration disagree by an hour for a zone that observes daylight saving — but it is the
    /// behaviour data already in target clusters was written with, so cdm-rs reproduces it. This
    /// constructor makes the reference instant explicit so the behaviour is testable.
    ///
    /// # Errors
    ///
    /// As [`TimestampFormat::new`].
    pub fn with_reference(
        format: &str,
        zone: &str,
        reference: DateTime<Utc>,
    ) -> Result<Self, CdmError> {
        let parsed = JavaDateFormat::parse(format)?;
        if zone.is_empty() {
            return Err(zone_error(zone));
        }
        let tz: chrono_tz::Tz = zone.parse().map_err(|_| zone_error(zone))?;
        let offset = tz.offset_from_utc_datetime(&reference.naive_utc()).fix();
        Ok(Self {
            format: parsed,
            zone: zone.to_owned(),
            offset,
        })
    }

    /// The Java pattern, as configured.
    pub fn pattern(&self) -> &str {
        self.format.pattern()
    }

    /// The configured zone identifier.
    pub fn zone(&self) -> &str {
        &self.zone
    }

    /// The fixed offset resolved at construction.
    pub const fn offset(&self) -> FixedOffset {
        self.offset
    }

    /// Renders epoch milliseconds through the configured pattern and offset.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] when the value is not a representable instant.
    pub fn format_millis(&self, millis: i64) -> Result<String, CdmError> {
        let instant = DateTime::<Utc>::from_timestamp_millis(millis)
            .ok_or_else(|| conversion_error(format!("{millis} is not a representable instant")))?;
        let local = instant.with_timezone(&self.offset).naive_local();
        Ok(self.format.format_datetime(&local))
    }

    /// Parses text through the configured pattern and offset into epoch milliseconds.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] when the text does not match the pattern.
    pub fn parse_millis(&self, text: &str) -> Result<i64, CdmError> {
        let local = self.format.parse_datetime(text)?;
        self.offset
            .from_local_datetime(&local)
            .single()
            .map(|value| value.timestamp_millis())
            .ok_or_else(|| conversion_error(format!("`{text}` is not a representable instant")))
    }
}

fn zone_error(zone: &str) -> CdmError {
    CdmError::new(
        ErrorKind::Config,
        format!(
            "transform.codecs.timestamp_format_zone `{zone}` is not a valid zone identifier; it \
             is required and must name an IANA zone such as `Europe/Dublin`"
        ),
    )
    .with_context(|c| c.with_config_key("transform.codecs.timestamp_format_zone"))
}

/// A converter that needs the `TIMESTAMP_STRING_FORMAT` settings.
#[derive(Debug)]
struct FormatConverter {
    settings: Arc<TimestampFormat>,
    to_text: bool,
}

impl Converter for FormatConverter {
    fn name(&self) -> &'static str {
        Codecset::TimestampStringFormat.name()
    }

    fn convert(&self, value: &RawCell) -> Result<RawCell, CdmError> {
        let Some(bytes) = value.bytes() else {
            return Ok(RawCell::NULL);
        };
        if self.to_text {
            let text = self.settings.format_millis(read_i64(bytes)?)?;
            Ok(RawCell::new(text.into_bytes()))
        } else {
            let millis = self.settings.parse_millis(read_text(bytes)?)?;
            Ok(RawCell::new(write_i64(millis)))
        }
    }
}

/// One built-in codec, registered like any other plugin.
#[derive(Debug)]
pub(crate) struct BuiltinCodec {
    codec: Codecset,
    conversions: Vec<(TypePair, Arc<dyn Converter>)>,
}

impl Plugin for BuiltinCodec {
    fn name(&self) -> &'static str {
        self.codec.name()
    }

    fn provider(&self) -> &'static str {
        BUILTIN_PROVIDER
    }
}

impl CodecPlugin for BuiltinCodec {
    fn conversions(&self) -> Vec<TypePair> {
        self.conversions
            .iter()
            .map(|(pair, _)| pair.clone())
            .collect()
    }

    fn convert(&self, pair: &TypePair, value: &RawCell) -> Result<RawCell, CdmError> {
        self.conversions
            .iter()
            .find(|(candidate, _)| candidate == pair)
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "codec `{}` was asked for the conversion {} -> {}, which it does not serve",
                        self.codec.name(),
                        pair.origin,
                        pair.target
                    ),
                )
            })
            .and_then(|(_, converter)| converter.convert(value))
    }
}

fn simple(
    codec: Codecset,
    origin: &str,
    target: &str,
    convert: fn(&[u8]) -> Result<Vec<u8>, CdmError>,
) -> (TypePair, Arc<dyn Converter>) {
    (
        TypePair::new(origin, target),
        Arc::new(FnConverter {
            codec: codec.name(),
            convert,
        }),
    )
}

/// Builds the plugin for one codec.
///
/// # Errors
///
/// Returns [`ErrorKind::Config`] when `TIMESTAMP_STRING_FORMAT` is requested with no settings
/// (`CDC-021`).
pub(crate) fn plugin(
    codec: Codecset,
    timestamp_format: Option<&Arc<TimestampFormat>>,
) -> Result<Arc<dyn CodecPlugin>, CdmError> {
    let conversions: Vec<(TypePair, Arc<dyn Converter>)> = match codec {
        Codecset::IntString => vec![
            simple(codec, "int", "text", int_to_text),
            simple(codec, "text", "int", text_to_int),
        ],
        Codecset::DoubleString => vec![
            simple(codec, "double", "text", double_to_text),
            simple(codec, "text", "double", text_to_double),
        ],
        Codecset::BigintString => vec![
            simple(codec, "bigint", "text", bigint_to_text),
            simple(codec, "text", "bigint", text_to_bigint),
        ],
        Codecset::DecimalString => vec![
            simple(codec, "decimal", "text", decimal_to_text),
            simple(codec, "text", "decimal", text_to_decimal),
        ],
        Codecset::BigintBiginteger => vec![
            simple(codec, "bigint", "varint", bigint_to_varint),
            simple(codec, "varint", "bigint", varint_to_bigint),
        ],
        Codecset::StringBlob => vec![
            simple(codec, "text", "blob", identity_bytes),
            simple(codec, "blob", "text", blob_to_text),
        ],
        Codecset::AsciiBlob => vec![
            simple(codec, "ascii", "blob", identity_bytes),
            simple(codec, "blob", "ascii", blob_to_ascii),
        ],
        Codecset::TimestampStringMillis => vec![
            simple(codec, "timestamp", "text", timestamp_to_millis_text),
            simple(codec, "text", "timestamp", millis_text_to_timestamp),
        ],
        Codecset::TimestampStringFormat => {
            let settings = timestamp_format.ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Config,
                    "codec TIMESTAMP_STRING_FORMAT requires transform.codecs.timestamp_format and \
                     transform.codecs.timestamp_format_zone",
                )
                .with_context(|c| c.with_config_key("transform.codecs.timestamp_format"))
            })?;
            vec![
                (
                    TypePair::new("timestamp", "text"),
                    Arc::new(FormatConverter {
                        settings: Arc::clone(settings),
                        to_text: true,
                    }) as Arc<dyn Converter>,
                ),
                (
                    TypePair::new("text", "timestamp"),
                    Arc::new(FormatConverter {
                        settings: Arc::clone(settings),
                        to_text: false,
                    }),
                ),
            ]
        }
        Codecset::PointType => vec![
            simple(codec, "PointType", "text", wkb_to_wkt),
            simple(codec, "text", "PointType", wkt_to_wkb),
        ],
        Codecset::LineString => vec![
            simple(codec, "LineStringType", "text", wkb_to_wkt),
            simple(codec, "text", "LineStringType", wkt_to_wkb),
        ],
        Codecset::PolygonType => vec![
            simple(codec, "PolygonType", "text", wkb_to_wkt),
            simple(codec, "text", "PolygonType", wkt_to_wkb),
        ],
        Codecset::DateRange => vec![
            simple(codec, "DateRangeType", "text", date_range_to_text),
            simple(codec, "text", "DateRangeType", text_to_date_range),
        ],
    };
    Ok(Arc::new(BuiltinCodec { codec, conversions }))
}

/// Registers the requested built-in codecs into a fresh `cdm-core` [`Registry`] (`CDC-030`).
///
/// `BIGINT_BIGINTEGER` is registered whether or not it appears in `enabled`: Java registers it
/// unconditionally because reading collection writetimes needs it (`CDC-020`).
///
/// # Errors
///
/// Returns [`ErrorKind::Config`] when `TIMESTAMP_STRING_FORMAT` is requested without settings, or
/// when two registrations collide.
pub(crate) fn registry_with_builtins(
    enabled: &[Codecset],
    timestamp_format: Option<TimestampFormat>,
) -> Result<Registry, CdmError> {
    let settings = timestamp_format.map(Arc::new);
    let mut wanted: Vec<Codecset> = vec![Codecset::BigintBiginteger];
    for codec in enabled {
        if !wanted.contains(codec) {
            wanted.push(*codec);
        }
    }
    let mut builder = Registry::builder();
    for codec in wanted {
        builder = builder.register_codec(plugin(codec, settings.as_ref())?);
    }
    builder.build()
}

// ---------------------------------------------------------------------------------------------
// The conversions themselves. Each cites the Java class it reproduces.
// ---------------------------------------------------------------------------------------------

/// `INT_StringCodec.decode`: `TypeCodecs.INT.decode(...).toString()`.
fn int_to_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(read_i32(bytes)?.to_string().into_bytes())
}

/// `TEXT_IntegerCodec.decode`: `Integer.parseInt(text)`. No trimming, as in Java.
fn text_to_int(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let text = read_text(bytes)?;
    let value: i32 = text
        .parse()
        .map_err(|e| conversion_error(format!("`{text}` is not an int: {e}")))?;
    Ok(write_i32(value))
}

/// `BIGINT_StringCodec.decode`: `Long.toString(...)`.
fn bigint_to_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(read_i64(bytes)?.to_string().into_bytes())
}

/// `TEXT_LongCodec.decode`: `Long.parseLong(text)`.
fn text_to_bigint(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let text = read_text(bytes)?;
    let value: i64 = text
        .parse()
        .map_err(|e| conversion_error(format!("`{text}` is not a bigint: {e}")))?;
    Ok(write_i64(value))
}

/// `DOUBLE_StringCodec.decode`: `new DecimalFormat("0.#########")` with grouping off and
/// `RoundingMode.FLOOR`.
fn double_to_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(format_double_java(read_f64(bytes)?).into_bytes())
}

/// `TEXT_DoubleCodec.decode`: `Double.valueOf(text)`.
fn text_to_double(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(write_f64(parse_double_java(read_text(bytes)?)?))
}

/// `DECIMAL_StringCodec.decode`: `BigDecimal.toString()`.
fn decimal_to_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(read_decimal(bytes)?.to_string().into_bytes())
}

/// `TEXT_BigDecimalCodec.decode`: `new BigDecimal(text)`.
fn text_to_decimal(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let text = read_text(bytes)?;
    let value = BigDecimal::from_str(text)
        .map_err(|e| conversion_error(format!("`{text}` is not a decimal: {e}")))?;
    write_decimal(&value)
}

/// `BIGINT_BigIntegerCodec.decode`: `BigInteger.valueOf(long)`.
fn bigint_to_varint(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(write_varint(&BigInt::from(read_i64(bytes)?)))
}

/// `BIGINT_BigIntegerCodec.encode`: `TypeCodecs.BIGINT.encode(value.longValue())`.
///
/// `BigInteger.longValue()` keeps the **low 64 bits** of an over-wide value and discards the rest
/// without complaint, so `2^64 + 1` becomes `1`. cdm-rs reproduces that rather than failing,
/// because a target cluster written by Java CDM already contains those truncated values.
fn varint_to_bigint(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let value = read_varint(bytes)?;
    Ok(write_i64(low_64_bits(&value)))
}

fn low_64_bits(value: &BigInt) -> i64 {
    let bytes = value.to_signed_bytes_le();
    let fill = if value.sign() == Sign::Minus { 0xff } else { 0 };
    let mut buffer = [fill; 8];
    for (slot, byte) in buffer.iter_mut().zip(bytes.iter()) {
        *slot = *byte;
    }
    i64::from_le_bytes(buffer)
}

/// `TEXT_BLOBCodec` / `ASCII_BLOBCodec`: text and blob share a byte representation, so this
/// direction is a pure copy.
// The signature is fixed by `FnConverter`, so this one conversion cannot drop its `Result`.
#[allow(clippy::unnecessary_wraps)]
fn identity_bytes(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(bytes.to_vec())
}

/// `BLOB_TEXTCodec.decode`: `TypeCodecs.TEXT.decode(bytes)`.
///
/// Java's `String` construction replaces malformed UTF-8 with U+FFFD and writes the replacement
/// through to the target, silently corrupting the value. cdm-rs validates instead and reports a
/// record-level `TypeConversion` error, which the engine counts as `ERROR`: a loud failure on one
/// row is strictly better than a quiet corruption of it.
fn blob_to_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    read_text(bytes)?;
    Ok(bytes.to_vec())
}

/// `BLOB_ASCIICodec.decode`: `TypeCodecs.ASCII.decode(bytes)`, with the same strictness as
/// [`blob_to_text`].
fn blob_to_ascii(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    read_ascii(bytes)?;
    Ok(bytes.to_vec())
}

/// `TIMESTAMP_StringMillisCodec.decode`.
///
/// The length test is Java's, verbatim: an 8-byte buffer is a `timestamp` and becomes an
/// epoch-milli string; anything else is already UTF-8 text arriving from a `text` column through
/// `CqlConversion`, and passes through untouched.
fn timestamp_to_millis_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    if bytes.len() == 8 {
        Ok(read_i64(bytes)?.to_string().into_bytes())
    } else {
        Ok(bytes.to_vec())
    }
}

/// `TEXTMillis_InstantCodec.decode`: `Instant.ofEpochMilli(Long.parseLong(text))`.
fn millis_text_to_timestamp(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    let text = read_text(bytes)?;
    let millis: i64 = text
        .parse()
        .map_err(|e| conversion_error(format!("`{text}` is not an epoch-millis timestamp: {e}")))?;
    Ok(write_i64(millis))
}

/// `PointCodec` / `LineStringCodec` / `PolygonCodec`: `Geometry.asWellKnownText()`.
fn wkb_to_wkt(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(Geometry::from_wkb(bytes)?.to_wkt().into_bytes())
}

/// The inverse: well-known text to well-known binary.
fn wkt_to_wkb(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Geometry::from_wkt(read_text(bytes)?)?.to_wkb()
}

/// `DateRangeCodec.format`, minus the CQL quoting the driver adds.
fn date_range_to_text(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(DateRange::from_bytes(bytes)?.to_text().into_bytes())
}

/// `DateRangeCodec.parse`.
fn text_to_date_range(bytes: &[u8]) -> Result<Vec<u8>, CdmError> {
    Ok(DateRange::parse(read_text(bytes)?)?.to_bytes())
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
    use crate::codec::CodecRegistry;
    use crate::types::CqlTypeInfo;

    fn registry(codecs: &[Codecset]) -> CodecRegistry {
        CodecRegistry::with_builtins(codecs, None).unwrap()
    }

    fn convert(registry: &CodecRegistry, from: &str, to: &str, value: &[u8]) -> Vec<u8> {
        let origin = CqlTypeInfo::parse(from).unwrap();
        let target = CqlTypeInfo::parse(to).unwrap();
        let converted = registry
            .converter(&origin, &target)
            .unwrap_or_else(|| panic!("no converter for {from} -> {to}"))
            .convert(&RawCell::new(value.to_vec()))
            .unwrap();
        converted.bytes().unwrap().to_vec()
    }

    #[test]
    fn cdc_020_the_named_codec_set_is_exactly_javas() {
        assert_eq!(
            Codecset::ALL.map(Codecset::name),
            [
                "INT_STRING",
                "DOUBLE_STRING",
                "BIGINT_STRING",
                "BIGINT_BIGINTEGER",
                "DECIMAL_STRING",
                "TIMESTAMP_STRING_MILLIS",
                "TIMESTAMP_STRING_FORMAT",
                "POINT_TYPE",
                "POLYGON_TYPE",
                "DATE_RANGE",
                "LINE_STRING",
                "STRING_BLOB",
                "ASCII_BLOB",
            ]
        );
        for codec in Codecset::ALL {
            assert_eq!(Codecset::parse(codec.name()).unwrap(), codec);
            assert_eq!(
                Codecset::parse(&codec.name().to_lowercase()).unwrap(),
                codec
            );
        }
        let error = Codecset::parse("NO_SUCH_CODEC").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("INT_STRING"), "{error}");
    }

    #[test]
    fn cdc_020_bigint_biginteger_is_always_registered() {
        // Java registers it unconditionally because reading collection writetimes needs it.
        let implicit = registry(&[]);
        assert!(implicit
            .converter(&CqlTypeInfo::BigInt, &CqlTypeInfo::VarInt)
            .is_some());
        assert!(implicit
            .converter(&CqlTypeInfo::VarInt, &CqlTypeInfo::BigInt)
            .is_some());
        // Naming it explicitly does not register it twice.
        let explicit = registry(&[Codecset::BigintBiginteger]);
        assert_eq!(implicit.len(), explicit.len());
    }

    #[test]
    fn cdc_020_int_string_matches_the_java_known_vectors() {
        // INT_StringCodecTest: "10" and 101.
        let registry = registry(&[Codecset::IntString]);
        assert_eq!(convert(&registry, "int", "text", &[0, 0, 0, 10]), b"10");
        assert_eq!(convert(&registry, "text", "int", b"10"), vec![0, 0, 0, 10]);
        assert_eq!(convert(&registry, "int", "text", &[0, 0, 0, 101]), b"101");
    }

    #[test]
    fn cdc_020_bigint_string_matches_the_java_known_vectors() {
        // BIGINT_StringCodecTest / TEXT_LongCodecTest: Long.MAX_VALUE.
        let registry = registry(&[Codecset::BigintString]);
        let max = i64::MAX.to_be_bytes();
        assert_eq!(
            convert(&registry, "bigint", "text", &max),
            b"9223372036854775807"
        );
        assert_eq!(
            convert(&registry, "text", "bigint", b"9223372036854775807"),
            max.to_vec()
        );
    }

    #[test]
    fn cdc_020_double_string_matches_the_java_known_vector() {
        // DOUBLE_StringCodecTest / TEXT_DoubleCodecTest: 21474836470.7.
        let registry = registry(&[Codecset::DoubleString]);
        let bits = 21_474_836_470.7_f64.to_be_bytes();
        assert_eq!(
            convert(&registry, "double", "text", &bits),
            b"21474836470.7"
        );
        assert_eq!(
            convert(&registry, "text", "double", b"21474836470.7"),
            bits.to_vec()
        );
    }

    #[test]
    fn cdc_020_decimal_string_matches_the_java_known_vector() {
        // DECIMAL_StringCodecTest / TEXT_BigDecimalCodecTest: 123.456 and 12345.6789.
        let registry = registry(&[Codecset::DecimalString]);
        let encoded = convert(&registry, "text", "decimal", b"123.456");
        assert_eq!(&encoded[..4], &[0, 0, 0, 3], "scale 3");
        assert_eq!(convert(&registry, "decimal", "text", &encoded), b"123.456");
        let encoded = convert(&registry, "text", "decimal", b"12345.6789");
        assert_eq!(
            convert(&registry, "decimal", "text", &encoded),
            b"12345.6789"
        );
    }

    #[test]
    fn cdc_020_bigint_biginteger_truncates_to_the_low_64_bits_as_java_does() {
        // BigInteger.longValue() silently keeps the low 64 bits.
        let registry = registry(&[]);
        let hundred_and_one = 101_i64.to_be_bytes();
        assert_eq!(
            convert(&registry, "bigint", "varint", &hundred_and_one),
            vec![101]
        );
        assert_eq!(
            convert(&registry, "varint", "bigint", &[101]),
            hundred_and_one.to_vec()
        );
        // 2^64 + 1 has the same low 64 bits as 1.
        let wide = BigInt::from(1_u8) << 64_u32;
        let wide = wide + BigInt::from(1_u8);
        assert_eq!(
            convert(&registry, "varint", "bigint", &write_varint(&wide)),
            1_i64.to_be_bytes().to_vec()
        );
        assert_eq!(low_64_bits(&BigInt::from(-1)), -1);
    }

    #[test]
    fn cdc_020_string_blob_and_ascii_blob_share_a_byte_representation() {
        // TEXT_BLOBCodecTest / ASCII_BLOBCodecTest both use this string.
        let input = b"Encode this Text string to Blob";
        let registry = registry(&[Codecset::StringBlob, Codecset::AsciiBlob]);
        assert_eq!(convert(&registry, "text", "blob", input), input.to_vec());
        assert_eq!(convert(&registry, "blob", "text", input), input.to_vec());
        assert_eq!(convert(&registry, "ascii", "blob", input), input.to_vec());
        assert_eq!(convert(&registry, "blob", "ascii", input), input.to_vec());
    }

    #[test]
    fn cdc_020_blob_to_text_rejects_bytes_java_would_silently_replace() {
        let registry = registry(&[Codecset::StringBlob, Codecset::AsciiBlob]);
        let converter = registry
            .converter(&CqlTypeInfo::Blob, &CqlTypeInfo::Text)
            .unwrap();
        let error = converter
            .convert(&RawCell::new(vec![0xff, 0xfe]))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TypeConversion);
        let converter = registry
            .converter(&CqlTypeInfo::Blob, &CqlTypeInfo::Ascii)
            .unwrap();
        assert!(converter.convert(&RawCell::new(vec![0xc3, 0xa9])).is_err());
    }

    #[test]
    fn cdc_020_timestamp_string_millis_matches_the_java_known_vector() {
        // TIMESTAMP_StringMillisCodecTest / TEXTMillis_InstantCodecTest: 1681333035000.
        let registry = registry(&[Codecset::TimestampStringMillis]);
        let millis = 1_681_333_035_000_i64.to_be_bytes();
        assert_eq!(
            convert(&registry, "timestamp", "text", &millis),
            b"1681333035000"
        );
        assert_eq!(
            convert(&registry, "text", "timestamp", b"1681333035000"),
            millis.to_vec()
        );
    }

    #[test]
    fn cdc_020_timestamp_string_millis_disambiguates_on_buffer_length() {
        // Java's TIMESTAMP_StringMillisCodec.decode: 8 bytes is an Instant, anything else is
        // UTF-8 text arriving from a TEXT column and passes through untouched.
        let registry = registry(&[Codecset::TimestampStringMillis]);
        assert_eq!(
            convert(&registry, "timestamp", "text", b"1681333035000"),
            b"1681333035000".to_vec()
        );
        assert_eq!(convert(&registry, "timestamp", "text", b"abc"), b"abc");
        let converter = registry
            .converter(&CqlTypeInfo::Timestamp, &CqlTypeInfo::Text)
            .unwrap();
        assert_eq!(converter.convert(&RawCell::NULL).unwrap(), RawCell::NULL);
    }

    #[test]
    fn cdc_021_timestamp_string_format_requires_its_settings() {
        let error =
            CodecRegistry::with_builtins(&[Codecset::TimestampStringFormat], None).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("timestamp_format"), "{error}");
    }

    #[test]
    fn cdc_021_an_empty_format_or_unparseable_zone_is_a_tier_1_error() {
        assert_eq!(
            TimestampFormat::new("", "Europe/Dublin")
                .unwrap_err()
                .kind(),
            ErrorKind::Config
        );
        let error = TimestampFormat::new("yyMMddHHmmss", "INVALID_TIMEZONE").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("INVALID_TIMEZONE"), "{error}");
        assert_eq!(
            TimestampFormat::new("yyMMddHHmmss", "").unwrap_err().kind(),
            ErrorKind::Config
        );
        assert_eq!(
            TimestampFormat::new("INVALID_FORMAT", "Europe/Dublin")
                .unwrap_err()
                .kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn cdc_020_timestamp_string_format_matches_the_java_known_vector() {
        // TIMESTAMP_StringFormatCodecTest: format yyMMddHHmmss, zone Europe/Dublin, "220412215715".
        // Java resolves the zone to a *fixed* offset at construction time, so the expected instant
        // depends on the reference instant, not on the value's own date. Pinning the reference to
        // a July instant pins the offset to Dublin's summer +01:00.
        let reference = DateTime::<Utc>::from_timestamp_millis(1_688_000_000_000).unwrap();
        let settings =
            TimestampFormat::with_reference("yyMMddHHmmss", "Europe/Dublin", reference).unwrap();
        assert_eq!(settings.pattern(), "yyMMddHHmmss");
        assert_eq!(settings.zone(), "Europe/Dublin");
        assert_eq!(settings.offset().local_minus_utc(), 3600);

        let millis = settings.parse_millis("220412215715").unwrap();
        // 2022-04-12T21:57:15+01:00 is 2022-04-12T20:57:15Z.
        assert_eq!(millis, 1_649_797_035_000);
        assert_eq!(settings.format_millis(millis).unwrap(), "220412215715");

        let registry =
            CodecRegistry::with_builtins(&[Codecset::TimestampStringFormat], Some(settings))
                .unwrap();
        assert_eq!(
            convert(&registry, "timestamp", "text", &millis.to_be_bytes()),
            b"220412215715"
        );
        assert_eq!(
            convert(&registry, "text", "timestamp", b"220412215715"),
            millis.to_be_bytes().to_vec()
        );
    }

    #[test]
    fn cdc_020_timestamp_string_format_uses_one_fixed_offset_for_every_value() {
        // The surprising part of TIMESTAMP_StringFormatCodec: a January value is converted with
        // the offset that was in force when the codec was constructed, not with January's.
        let summer = DateTime::<Utc>::from_timestamp_millis(1_688_000_000_000).unwrap();
        let winter = DateTime::<Utc>::from_timestamp_millis(1_704_067_200_000).unwrap();
        let in_summer =
            TimestampFormat::with_reference("yyMMddHHmmss", "Europe/Dublin", summer).unwrap();
        let in_winter =
            TimestampFormat::with_reference("yyMMddHHmmss", "Europe/Dublin", winter).unwrap();
        assert_eq!(in_summer.offset().local_minus_utc(), 3600);
        assert_eq!(in_winter.offset().local_minus_utc(), 0);
        assert_ne!(
            in_summer.parse_millis("220112215715").unwrap(),
            in_winter.parse_millis("220112215715").unwrap()
        );
    }

    #[test]
    fn cdc_020_dse_geometry_codecs_use_the_java_well_known_text_fixtures() {
        let registry = registry(&[
            Codecset::PointType,
            Codecset::LineString,
            Codecset::PolygonType,
        ]);
        for (cql_type, wkt) in [
            ("PointType", "POINT (30 10)"),
            ("LineStringType", "LINESTRING (30 10, 10 30, 40 40)"),
            (
                "PolygonType",
                "POLYGON ((30 10, 40 40, 20 40, 10 20, 30 10))",
            ),
        ] {
            let wkb = convert(&registry, "text", cql_type, wkt.as_bytes());
            assert_eq!(convert(&registry, cql_type, "text", &wkb), wkt.as_bytes());
        }
    }

    #[test]
    fn cdc_020_date_range_codec_round_trips_the_java_fixture() {
        let registry = registry(&[Codecset::DateRange]);
        let bytes = convert(&registry, "text", "DateRangeType", b"2001-01-01");
        assert_eq!(
            convert(&registry, "DateRangeType", "text", &bytes),
            b"2001-01-01"
        );
    }

    #[test]
    fn cdc_020_every_codec_reports_errors_rather_than_guessing() {
        let registry = registry(&Codecset::ALL[..6]);
        for (from, to, bad) in [
            ("text", "int", &b"not a number"[..]),
            ("text", "bigint", b"not a number"),
            ("text", "double", b"not a number"),
            ("text", "decimal", b"not a number"),
            ("text", "timestamp", b"not a millis timestamp"),
            ("int", "text", b"xx"),
            ("bigint", "text", b"xx"),
            ("double", "text", b"xx"),
        ] {
            let origin = CqlTypeInfo::parse(from).unwrap();
            let target = CqlTypeInfo::parse(to).unwrap();
            let error = registry
                .converter(&origin, &target)
                .unwrap_or_else(|| panic!("no converter for {from} -> {to}"))
                .convert(&RawCell::new(bad.to_vec()))
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::TypeConversion, "{from} -> {to}");
        }
    }

    /// Every codec except `TIMESTAMP_STRING_FORMAT`, which claims the same `timestamp` <-> `text`
    /// pair as `TIMESTAMP_STRING_MILLIS` and so cannot be enabled alongside it.
    fn all_but_timestamp_format() -> Vec<Codecset> {
        Codecset::ALL
            .into_iter()
            .filter(|codec| *codec != Codecset::TimestampStringFormat)
            .collect()
    }

    #[test]
    fn cdc_030_the_two_timestamp_codecs_claim_the_same_pair_and_cannot_both_be_enabled() {
        // Both TIMESTAMP_StringMillisCodec and TIMESTAMP_StringFormatCodec are (TIMESTAMP, String)
        // codecs in Java too, where the later registration silently wins. cdm-rs refuses the
        // ambiguity at startup instead, naming both codecs.
        let settings = TimestampFormat::new("yyyyMMddHHmmss", "UTC").unwrap();
        let error = CodecRegistry::with_builtins(
            &[
                Codecset::TimestampStringMillis,
                Codecset::TimestampStringFormat,
            ],
            Some(settings),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(
            error.to_string().contains("TIMESTAMP_STRING_MILLIS"),
            "{error}"
        );
        assert!(
            error.to_string().contains("TIMESTAMP_STRING_FORMAT"),
            "{error}"
        );
    }

    #[test]
    fn cdc_020_every_codec_maps_null_to_null() {
        // Java's codecs all return null for null; MIG-012 turns that into an UNSET binding.
        let settings = TimestampFormat::new("yyyyMMddHHmmss", "UTC").unwrap();
        let registry = CodecRegistry::with_builtins(&all_but_timestamp_format(), None).unwrap();
        let format_only =
            CodecRegistry::with_builtins(&[Codecset::TimestampStringFormat], Some(settings))
                .unwrap();
        for entry in registry.entries().iter().chain(format_only.entries()) {
            assert_eq!(
                entry.converter().convert(&RawCell::NULL).unwrap(),
                RawCell::NULL,
                "{} {} -> {}",
                entry.codec(),
                entry.origin(),
                entry.target()
            );
        }
    }

    #[test]
    fn cdc_030_a_builtin_codec_asked_for_a_pair_it_does_not_serve_reports_an_internal_error() {
        let codec = plugin(Codecset::IntString, None).unwrap();
        let error = codec
            .convert(&TypePair::new("blob", "blob"), &RawCell::NULL)
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Internal);
    }

    #[test]
    fn cdc_031_the_builtin_registry_describes_every_pair_it_serves() {
        let registry = CodecRegistry::with_builtins(&all_but_timestamp_format(), None).unwrap();
        let descriptions = registry.descriptions();
        assert_eq!(descriptions.len(), (Codecset::ALL.len() - 1) * 2);
        assert!(descriptions.iter().all(|d| d.provider == BUILTIN_PROVIDER));
        assert!(descriptions
            .iter()
            .any(|d| d.codec == "DOUBLE_STRING" && d.from == "double" && d.to == "text"));
        assert!(descriptions
            .iter()
            .any(|d| d.codec == "POINT_TYPE" && d.from == "PointType" && d.to == "text"));
    }
}
