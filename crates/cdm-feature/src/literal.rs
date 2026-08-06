//! CQL literals, parsed and type-checked against a target column type (`FEA-011`).
//!
//! # Why this exists here
//!
//! Java parses a constant-column value with `codecRegistry.codecFor(dataType).parse(value)`, i.e.
//! with the driver's CQL-literal parser. cdm-rs has no driver in this crate by design
//! (`ARCHITECTURE.md` §3.2) and `cdm-codec` converts *between wire representations* rather than from
//! CQL source, so the literal parser lives here — next to its only two callers, constant columns
//! (`FEA-011`) and the JSON extractor (`FEA-031`), which share every code path below.
//!
//! # The syntax is CQL's, not Rust's
//!
//! A `text` constant is quoted (`'abc'`, with `''` for an embedded quote) and a numeric one is not,
//! exactly as in a `.properties` file written for Java. That distinction is load-bearing: it is what
//! lets `FEA-012` splice a constant straight into a `WHERE` clause as source text while `FEA-010`
//! binds the same value as bytes, and it is why a constant is type-checked at validation time rather
//! than discovered to be unparsable on the first row.

use bigdecimal::BigDecimal;
use cdm_codec::CqlTypeInfo;
use cdm_core::{CdmError, ErrorKind, RawCell};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use num_bigint::BigInt;
use std::net::IpAddr;
use std::str::FromStr;
use uuid::Uuid;

/// Days from `0000-01-01`-style epoch handling: CQL `date` is days since the Unix epoch, stored
/// biased by 2^31 so that the unsigned wire value is ordered.
const DATE_EPOCH_BIAS: i64 = 1 << 31;

fn invalid(literal: &str, cql_type: &CqlTypeInfo, detail: impl AsRef<str>) -> CdmError {
    let detail = detail.as_ref();
    CdmError::new(
        ErrorKind::TypeConversion,
        format!("`{literal}` is not a valid {cql_type} literal: {detail}"),
    )
}

/// Whether `text` is a single-quoted CQL string literal.
fn is_quoted(text: &str) -> bool {
    text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'')
}

/// Removes the surrounding quotes of a CQL string literal and unescapes `''`.
fn unquote(text: &str) -> Option<String> {
    if !is_quoted(text) {
        return None;
    }
    let inner = text.get(1..text.len() - 1)?;
    Some(inner.replace("''", "'"))
}

/// Parses a CQL literal for `cql_type` into its serialised form.
///
/// String-like types require quotes, as CQL and Java's `TypeCodec::parse` do; every other type
/// accepts its bare form and, where CQL allows one, its quoted form too.
///
/// # Errors
///
/// Returns [`ErrorKind::TypeConversion`] naming the literal and the type. Callers surface it as a
/// Tier-2 configuration diagnostic (`FEA-011`) or as a record-level error (`FEA-031`), which is why
/// it is an error value and not a panic even though the input is usually static configuration.
pub fn parse_literal(literal: &str, cql_type: &CqlTypeInfo) -> Result<RawCell, CdmError> {
    let text = literal.trim();
    if text.eq_ignore_ascii_case("null") {
        return Ok(RawCell::NULL);
    }
    match cql_type {
        CqlTypeInfo::Text | CqlTypeInfo::Ascii | CqlTypeInfo::Inet => {
            // A `blob` is the one string-ish type CQL writes unquoted, as `0x…`, so it is handled
            // by the permissive arm below rather than here.
            let value = unquote(text).ok_or_else(|| {
                invalid(
                    literal,
                    cql_type,
                    "a string literal must be enclosed in single quotes",
                )
            })?;
            encode_scalar(&value, cql_type, literal)
        }
        CqlTypeInfo::List { .. }
        | CqlTypeInfo::Set { .. }
        | CqlTypeInfo::Map { .. }
        | CqlTypeInfo::Tuple { .. } => parse_collection(text, cql_type, literal),
        _ => {
            let value = unquote(text).unwrap_or_else(|| text.to_owned());
            encode_scalar(&value, cql_type, literal)
        }
    }
}

/// Serialises an already-unquoted scalar value.
fn encode_scalar(value: &str, cql_type: &CqlTypeInfo, literal: &str) -> Result<RawCell, CdmError> {
    let bytes: Vec<u8> =
        match cql_type {
            CqlTypeInfo::Text => value.as_bytes().to_vec(),
            CqlTypeInfo::Ascii => {
                if !value.is_ascii() {
                    return Err(invalid(literal, cql_type, "value contains non-ASCII bytes"));
                }
                value.as_bytes().to_vec()
            }
            CqlTypeInfo::Boolean => match value.to_ascii_lowercase().as_str() {
                "true" => vec![1],
                "false" => vec![0],
                _ => return Err(invalid(literal, cql_type, "expected `true` or `false`")),
            },
            CqlTypeInfo::TinyInt => parse_number::<i8>(value, cql_type, literal)?
                .to_be_bytes()
                .to_vec(),
            CqlTypeInfo::SmallInt => parse_number::<i16>(value, cql_type, literal)?
                .to_be_bytes()
                .to_vec(),
            CqlTypeInfo::Int => parse_number::<i32>(value, cql_type, literal)?
                .to_be_bytes()
                .to_vec(),
            CqlTypeInfo::BigInt | CqlTypeInfo::Counter => {
                parse_number::<i64>(value, cql_type, literal)?
                    .to_be_bytes()
                    .to_vec()
            }
            CqlTypeInfo::Float => parse_number::<f32>(value, cql_type, literal)?
                .to_be_bytes()
                .to_vec(),
            CqlTypeInfo::Double => parse_number::<f64>(value, cql_type, literal)?
                .to_be_bytes()
                .to_vec(),
            CqlTypeInfo::VarInt => {
                let value = BigInt::from_str(value)
                    .map_err(|e| invalid(literal, cql_type, e.to_string()))?;
                if value == BigInt::from(0) {
                    vec![0]
                } else {
                    value.to_signed_bytes_be()
                }
            }
            CqlTypeInfo::Decimal => {
                let decimal = BigDecimal::from_str(value)
                    .map_err(|e| invalid(literal, cql_type, e.to_string()))?;
                let (unscaled, scale) = decimal.as_bigint_and_exponent();
                let scale = i32::try_from(scale)
                    .map_err(|_| invalid(literal, cql_type, "scale does not fit in 32 bits"))?;
                let mut bytes = scale.to_be_bytes().to_vec();
                if unscaled == BigInt::from(0) {
                    bytes.push(0);
                } else {
                    bytes.extend_from_slice(&unscaled.to_signed_bytes_be());
                }
                bytes
            }
            CqlTypeInfo::Uuid | CqlTypeInfo::TimeUuid => Uuid::parse_str(value)
                .map_err(|e| invalid(literal, cql_type, e.to_string()))?
                .as_bytes()
                .to_vec(),
            CqlTypeInfo::Inet => match IpAddr::from_str(value)
                .map_err(|e| invalid(literal, cql_type, e.to_string()))?
            {
                IpAddr::V4(v4) => v4.octets().to_vec(),
                IpAddr::V6(v6) => v6.octets().to_vec(),
            },
            CqlTypeInfo::Blob => parse_blob(value, cql_type, literal)?,
            CqlTypeInfo::Timestamp => parse_timestamp(value, cql_type, literal)?
                .to_be_bytes()
                .to_vec(),
            CqlTypeInfo::Date => parse_date(value, cql_type, literal)?.to_be_bytes().to_vec(),
            CqlTypeInfo::Time => parse_time(value, cql_type, literal)?.to_be_bytes().to_vec(),
            other => return Err(invalid(
                literal,
                other,
                "cdm-rs cannot express a constant of this type; supply it from the origin instead",
            )),
        };
    Ok(RawCell::new(bytes))
}

fn parse_number<T: FromStr>(
    value: &str,
    cql_type: &CqlTypeInfo,
    literal: &str,
) -> Result<T, CdmError>
where
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|e| invalid(literal, cql_type, e.to_string()))
}

fn parse_blob(value: &str, cql_type: &CqlTypeInfo, literal: &str) -> Result<Vec<u8>, CdmError> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if !hex.len().is_multiple_of(2) {
        return Err(invalid(literal, cql_type, "hex string has an odd length"));
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let digits: Vec<char> = hex.chars().collect();
    for pair in digits.chunks(2) {
        let (Some(high), Some(low)) = (pair.first(), pair.get(1)) else {
            return Err(invalid(literal, cql_type, "hex string has an odd length"));
        };
        let byte = format!("{high}{low}");
        bytes.push(
            u8::from_str_radix(&byte, 16).map_err(|e| invalid(literal, cql_type, e.to_string()))?,
        );
    }
    Ok(bytes)
}

/// CQL `timestamp` accepts epoch milliseconds or a date-time string. The accepted string forms are
/// the ones CQL itself documents; a zone-less form is read as UTC, as Cassandra does when the client
/// supplies no default zone.
fn parse_timestamp(value: &str, cql_type: &CqlTypeInfo, literal: &str) -> Result<i64, CdmError> {
    if let Ok(millis) = value.parse::<i64>() {
        return Ok(millis);
    }
    if let Ok(offset) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(offset.timestamp_millis());
    }
    for pattern in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, pattern) {
            return Ok(naive.and_utc().timestamp_millis());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date.and_time(NaiveTime::MIN).and_utc().timestamp_millis());
    }
    Err(invalid(
        literal,
        cql_type,
        "expected epoch milliseconds or an ISO-8601 date-time",
    ))
}

/// CQL `date` on the wire is an unsigned day count biased by 2^31.
fn parse_date(value: &str, cql_type: &CqlTypeInfo, literal: &str) -> Result<u32, CdmError> {
    let days = if let Ok(days) = value.parse::<i64>() {
        days
    } else {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|e| invalid(literal, cql_type, e.to_string()))?;
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)
            .ok_or_else(|| invalid(literal, cql_type, "unrepresentable epoch"))?;
        date.signed_duration_since(epoch).num_days()
    };
    u32::try_from(days + DATE_EPOCH_BIAS)
        .map_err(|_| invalid(literal, cql_type, "date is outside the representable range"))
}

/// CQL `time` is nanoseconds since midnight.
fn parse_time(value: &str, cql_type: &CqlTypeInfo, literal: &str) -> Result<i64, CdmError> {
    if let Ok(nanos) = value.parse::<i64>() {
        return Ok(nanos);
    }
    for pattern in ["%H:%M:%S%.f", "%H:%M"] {
        if let Ok(time) = NaiveTime::parse_from_str(value, pattern) {
            if let Some(nanos) = time.signed_duration_since(NaiveTime::MIN).num_nanoseconds() {
                return Ok(nanos);
            }
        }
    }
    Err(invalid(
        literal,
        cql_type,
        "expected nanoseconds since midnight or `HH:MM:SS`",
    ))
}

/// Parses `[a, b]`, `{a, b}`, `{k: v}` and `(a, b)` into native-protocol framing.
fn parse_collection(
    text: &str,
    cql_type: &CqlTypeInfo,
    literal: &str,
) -> Result<RawCell, CdmError> {
    let (open, close) = match cql_type {
        CqlTypeInfo::List { .. } => ('[', ']'),
        CqlTypeInfo::Set { .. } | CqlTypeInfo::Map { .. } => ('{', '}'),
        _ => ('(', ')'),
    };
    let inner = text
        .strip_prefix(open)
        .and_then(|rest| rest.strip_suffix(close))
        .ok_or_else(|| invalid(literal, cql_type, format!("expected `{open}…{close}`")))?;
    let elements = split_top_level(inner, ',');

    let mut out = Vec::new();
    match cql_type {
        CqlTypeInfo::Tuple { elements: types } => {
            if elements.len() != types.len() {
                return Err(invalid(
                    literal,
                    cql_type,
                    format!(
                        "expected {} components, got {}",
                        types.len(),
                        elements.len()
                    ),
                ));
            }
            for (element, element_type) in elements.iter().zip(types) {
                write_element(&mut out, &parse_literal(element, element_type)?)?;
            }
        }
        CqlTypeInfo::Map { key, value, .. } => {
            write_count(&mut out, elements.len(), cql_type, literal)?;
            for element in &elements {
                let mut pair = split_top_level(element, ':');
                if pair.len() != 2 {
                    return Err(invalid(literal, cql_type, "expected `key: value` entries"));
                }
                let entry_value = pair.remove(1);
                let entry_key = pair.remove(0);
                write_element(&mut out, &parse_literal(&entry_key, key)?)?;
                write_element(&mut out, &parse_literal(&entry_value, value)?)?;
            }
        }
        CqlTypeInfo::List { element, .. } | CqlTypeInfo::Set { element, .. } => {
            let elements: Vec<&String> = elements.iter().filter(|e| !e.is_empty()).collect();
            write_count(&mut out, elements.len(), cql_type, literal)?;
            for value in elements {
                write_element(&mut out, &parse_literal(value, element)?)?;
            }
        }
        other => return Err(invalid(literal, other, "not a collection type")),
    }
    Ok(RawCell::new(out))
}

fn write_count(
    out: &mut Vec<u8>,
    count: usize,
    cql_type: &CqlTypeInfo,
    literal: &str,
) -> Result<(), CdmError> {
    let count = i32::try_from(count)
        .map_err(|_| invalid(literal, cql_type, "collection has too many elements"))?;
    out.extend_from_slice(&count.to_be_bytes());
    Ok(())
}

fn write_element(out: &mut Vec<u8>, cell: &RawCell) -> Result<(), CdmError> {
    match cell.bytes() {
        None => out.extend_from_slice(&(-1_i32).to_be_bytes()),
        Some(bytes) => {
            let length = i32::try_from(bytes.len()).map_err(|_| {
                CdmError::new(
                    ErrorKind::TypeConversion,
                    "serialised collection element exceeds 2 GiB",
                )
            })?;
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(bytes);
        }
    }
    Ok(())
}

/// Splits on `separator`, ignoring separators nested inside brackets or quotes.
fn split_top_level(text: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0_usize;
    let mut quoted = false;
    for character in text.chars() {
        match character {
            '\'' => {
                quoted = !quoted;
                current.push(character);
            }
            '[' | '{' | '(' | '<' if !quoted => {
                depth += 1;
                current.push(character);
            }
            ']' | '}' | ')' | '>' if !quoted => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            c if c == separator && depth == 0 && !quoted => {
                parts.push(current.trim().to_owned());
                current = String::new();
            }
            _ => current.push(character),
        }
    }
    parts.push(current.trim().to_owned());
    if parts.len() == 1 && parts.first().is_some_and(String::is_empty) {
        return vec![String::new()];
    }
    parts
}

/// Serialises a JSON value into a target column type (`FEA-031`).
///
/// JSON has four scalar shapes and CQL has twenty, so the mapping is deliberately literal-minded: a
/// JSON string is treated as the *unquoted* form of a CQL literal, which is what makes
/// `{"when":"2024-01-01"}` land in a `date` column; a number or boolean is rendered and parsed the
/// same way; an object or array is only representable in a string-like column, where it is stored as
/// compact JSON. Anything else is a record-level conversion error rather than a silent null.
///
/// # Errors
///
/// Returns [`ErrorKind::TypeConversion`], which the engine counts as `ERROR` for that record.
pub fn encode_json(value: &serde_json::Value, cql_type: &CqlTypeInfo) -> Result<RawCell, CdmError> {
    match value {
        serde_json::Value::Null => Ok(RawCell::NULL),
        serde_json::Value::String(text) => encode_scalar(text, cql_type, text),
        serde_json::Value::Bool(flag) => {
            let rendered = flag.to_string();
            encode_scalar(&rendered, cql_type, &rendered)
        }
        serde_json::Value::Number(number) => {
            let rendered = number.to_string();
            encode_scalar(&rendered, cql_type, &rendered)
        }
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            let rendered = value.to_string();
            match cql_type {
                CqlTypeInfo::Text | CqlTypeInfo::Ascii => {
                    encode_scalar(&rendered, cql_type, &rendered)
                }
                other => Err(invalid(
                    &rendered,
                    other,
                    "a JSON object or array can only be extracted into a text column",
                )),
            }
        }
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

    fn bytes(literal: &str, cql_type: &CqlTypeInfo) -> Vec<u8> {
        parse_literal(literal, cql_type)
            .unwrap()
            .bytes()
            .unwrap()
            .to_vec()
    }

    #[test]
    fn fea_011_numeric_literals_serialise_big_endian() {
        assert_eq!(bytes("1234", &CqlTypeInfo::Int), vec![0, 0, 4, 210]);
        assert_eq!(bytes("-1", &CqlTypeInfo::TinyInt), vec![0xff]);
        assert_eq!(bytes("-1", &CqlTypeInfo::SmallInt), vec![0xff, 0xff]);
        assert_eq!(
            bytes("1", &CqlTypeInfo::BigInt),
            vec![0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(bytes("0", &CqlTypeInfo::VarInt), vec![0]);
        assert_eq!(bytes("1.5", &CqlTypeInfo::Double).len(), 8);
        assert_eq!(bytes("1.5", &CqlTypeInfo::Float).len(), 4);
        assert_eq!(&bytes("1.50", &CqlTypeInfo::Decimal)[..4], &[0, 0, 0, 2]);
        assert_eq!(bytes("0", &CqlTypeInfo::Decimal), vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn fea_011_string_literals_must_be_quoted_as_cql_requires() {
        assert_eq!(bytes("'abcd'", &CqlTypeInfo::Text), b"abcd".to_vec());
        assert_eq!(bytes("'it''s'", &CqlTypeInfo::Text), b"it's".to_vec());
        assert_eq!(bytes("'abcd'", &CqlTypeInfo::Ascii), b"abcd".to_vec());

        let unquoted = parse_literal("abcd", &CqlTypeInfo::Text).unwrap_err();
        assert_eq!(unquoted.kind(), ErrorKind::TypeConversion);
        assert!(unquoted.message().contains("single quotes"));
        assert!(parse_literal("'é'", &CqlTypeInfo::Ascii).is_err());
    }

    #[test]
    fn fea_011_temporal_literals_accept_both_the_numeric_and_the_iso_form() {
        assert_eq!(
            bytes("1700000000000", &CqlTypeInfo::Timestamp),
            1_700_000_000_000_i64.to_be_bytes().to_vec()
        );
        assert_eq!(
            bytes("'2023-11-14T22:13:20Z'", &CqlTypeInfo::Timestamp),
            1_700_000_000_000_i64.to_be_bytes().to_vec()
        );
        assert_eq!(
            bytes("'2023-11-14 22:13:20'", &CqlTypeInfo::Timestamp),
            1_700_000_000_000_i64.to_be_bytes().to_vec()
        );
        assert_eq!(
            bytes("'1970-01-02'", &CqlTypeInfo::Date),
            u32::try_from(DATE_EPOCH_BIAS + 1)
                .unwrap()
                .to_be_bytes()
                .to_vec()
        );
        assert_eq!(
            bytes("'00:00:01'", &CqlTypeInfo::Time),
            1_000_000_000_i64.to_be_bytes().to_vec()
        );
        assert!(parse_literal("'not a time'", &CqlTypeInfo::Time).is_err());
        assert!(parse_literal("'yesterday'", &CqlTypeInfo::Timestamp).is_err());
        assert!(parse_literal("'not-a-date'", &CqlTypeInfo::Date).is_err());
    }

    #[test]
    fn fea_011_uuid_inet_blob_and_boolean_literals_round_trip() {
        assert_eq!(
            bytes("1b4e28ba-2fa1-11d2-883f-0016d3cca427", &CqlTypeInfo::Uuid).len(),
            16
        );
        assert_eq!(bytes("'127.0.0.1'", &CqlTypeInfo::Inet), vec![127, 0, 0, 1]);
        assert_eq!(bytes("'::1'", &CqlTypeInfo::Inet).len(), 16);
        assert_eq!(bytes("'0x00ff'", &CqlTypeInfo::Blob), vec![0, 255]);
        assert_eq!(bytes("true", &CqlTypeInfo::Boolean), vec![1]);
        assert_eq!(bytes("FALSE", &CqlTypeInfo::Boolean), vec![0]);
        assert!(parse_literal("maybe", &CqlTypeInfo::Boolean).is_err());
        assert!(parse_literal("'0xfff'", &CqlTypeInfo::Blob).is_err());
        assert!(parse_literal("nope", &CqlTypeInfo::Uuid).is_err());
        assert!(parse_literal("'999.1.1.1'", &CqlTypeInfo::Inet).is_err());
    }

    #[test]
    fn fea_011_null_is_accepted_for_every_type() {
        assert!(parse_literal("null", &CqlTypeInfo::Int).unwrap().is_null());
        assert!(parse_literal("NULL", &CqlTypeInfo::Text).unwrap().is_null());
    }

    #[test]
    fn fea_011_collection_literals_use_native_protocol_framing() {
        let list = CqlTypeInfo::parse("list<int>").unwrap();
        assert_eq!(
            bytes("[1, 2]", &list),
            vec![0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 2]
        );
        assert_eq!(bytes("[]", &list), vec![0, 0, 0, 0]);

        let map = CqlTypeInfo::parse("map<text, int>").unwrap();
        assert_eq!(
            bytes("{'a': 1}", &map),
            vec![0, 0, 0, 1, 0, 0, 0, 1, b'a', 0, 0, 0, 4, 0, 0, 0, 1]
        );

        let tuple = CqlTypeInfo::parse("tuple<int, text>").unwrap();
        assert_eq!(
            bytes("(1, 'a')", &tuple),
            vec![0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, 1, b'a']
        );

        assert!(parse_literal("1, 2", &list).is_err());
        assert!(parse_literal("{'a'}", &map).is_err());
        assert!(parse_literal("(1)", &tuple).is_err());
    }

    #[test]
    fn fea_011_a_type_with_no_literal_form_is_rejected_rather_than_guessed() {
        let error = parse_literal("x", &CqlTypeInfo::Duration).unwrap_err();
        assert!(error.message().contains("cannot express a constant"));
        assert!(parse_literal("x", &CqlTypeInfo::Point).is_err());
    }

    #[test]
    fn fea_031_json_scalars_are_encoded_as_unquoted_literals() {
        use serde_json::json;
        assert_eq!(
            encode_json(&json!("abcd"), &CqlTypeInfo::Text)
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec(),
            b"abcd".to_vec()
        );
        assert_eq!(
            encode_json(&json!(7), &CqlTypeInfo::Int)
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec(),
            vec![0, 0, 0, 7]
        );
        assert_eq!(
            encode_json(&json!("2023-11-14T22:13:20Z"), &CqlTypeInfo::Timestamp)
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec(),
            1_700_000_000_000_i64.to_be_bytes().to_vec()
        );
        assert!(encode_json(&json!(true), &CqlTypeInfo::Boolean).is_ok());
        assert!(encode_json(&json!(null), &CqlTypeInfo::Int)
            .unwrap()
            .is_null());
    }

    #[test]
    fn fea_031_a_nested_json_value_only_fits_a_text_column() {
        use serde_json::json;
        let nested = json!({"a": [1, 2]});
        assert_eq!(
            encode_json(&nested, &CqlTypeInfo::Text)
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec(),
            br#"{"a":[1,2]}"#.to_vec()
        );
        assert!(encode_json(&nested, &CqlTypeInfo::Int).is_err());
        assert!(encode_json(&json!("x"), &CqlTypeInfo::Int).is_err());
    }
}
