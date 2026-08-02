//! Round-trip property tests for every built-in codec (`CDC-032`, `TST-031`), and the proof that
//! a `Passthrough` plan is lossless (`CDC-011`; the `MIG-040`/`TST-030` fast path builds on it).
//!
//! The known-vector and error-case halves of `TST-031` live beside the implementations, in the
//! unit-test modules of `src/builtin.rs`, `src/format.rs` and `src/geo.rs`, where they can assert
//! against the exact fixtures the Java tests use. What is here is the part that needs generated
//! input: every codec, over thousands of random values.

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cdm_codec::{
    format_double_java, CodecRegistry, Codecset, ConversionPlan, CqlTypeInfo, DateRange, Geometry,
    Planner, PlannerOptions, TimestampFormat, UdtField,
};
use cdm_core::RawCell;
use proptest::prelude::*;

/// Converts through the registered codec for `origin -> target`, which must exist.
fn convert(registry: &CodecRegistry, origin: &str, target: &str, value: Vec<u8>) -> Vec<u8> {
    let origin = CqlTypeInfo::parse(origin).unwrap();
    let target = CqlTypeInfo::parse(target).unwrap();
    registry
        .converter(&origin, &target)
        .expect("codec is registered")
        .convert(&RawCell::new(value))
        .expect("conversion succeeds")
        .bytes()
        .expect("a non-null value converts to a non-null value")
        .to_vec()
}

fn registry(codecs: &[Codecset]) -> CodecRegistry {
    CodecRegistry::with_builtins(codecs, None).unwrap()
}

/// A `decimal` buffer: a four-byte scale followed by the two's-complement unscaled value.
fn decimal_bytes(unscaled: i64, scale: i32) -> Vec<u8> {
    let mut out = scale.to_be_bytes().to_vec();
    out.extend_from_slice(&minimal_signed_be(unscaled));
    out
}

/// The shortest big-endian two's-complement encoding of a signed integer, as `varint` uses.
fn minimal_signed_be(value: i64) -> Vec<u8> {
    let full = value.to_be_bytes();
    let mut start = 0;
    while start < 7 {
        let head = full[start];
        let next = full[start + 1];
        let redundant = (head == 0 && next & 0x80 == 0) || (head == 0xff && next & 0x80 != 0);
        if !redundant {
            break;
        }
        start += 1;
    }
    full[start..].to_vec()
}

/// One `i32`-length-prefixed element, `-1` for null.
fn element(bytes: Option<&[u8]>) -> Vec<u8> {
    match bytes {
        None => (-1_i32).to_be_bytes().to_vec(),
        Some(bytes) => {
            let mut out = i32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
            out.extend_from_slice(bytes);
            out
        }
    }
}

/// Native-protocol collection framing: a count, then length-prefixed elements.
fn collection(elements: &[Option<Vec<u8>>]) -> Vec<u8> {
    let mut out = i32::try_from(elements.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    for value in elements {
        out.extend_from_slice(&element(value.as_deref()));
    }
    out
}

/// Native-protocol map framing: an entry count, then alternating keys and values.
fn map(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut out = i32::try_from(entries.len()).unwrap().to_be_bytes().to_vec();
    for (key, value) in entries {
        out.extend_from_slice(&element(Some(key)));
        out.extend_from_slice(&element(Some(value)));
    }
    out
}

/// Tuple and UDT framing: length-prefixed fields, with no count.
fn fields(values: &[Option<Vec<u8>>]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&element(value.as_deref()));
    }
    out
}

fn udt(name: &str, definition: &[(&str, &str)]) -> CqlTypeInfo {
    CqlTypeInfo::Udt {
        keyspace: None,
        name: name.to_owned(),
        fields: definition
            .iter()
            .map(|(field, cql_type)| UdtField::new(*field, CqlTypeInfo::parse(cql_type).unwrap()))
            .collect(),
        frozen: true,
    }
}

fn planner() -> Planner {
    Planner::new(
        registry(&[
            Codecset::IntString,
            Codecset::BigintString,
            Codecset::DecimalString,
            Codecset::StringBlob,
            Codecset::TimestampStringMillis,
        ]),
        PlannerOptions::default(),
    )
}

proptest! {
    #[test]
    fn cdc_032_int_string_round_trips(value in any::<i32>()) {
        let registry = registry(&[Codecset::IntString]);
        let text = convert(&registry, "int", "text", value.to_be_bytes().to_vec());
        prop_assert_eq!(String::from_utf8(text.clone()).unwrap(), value.to_string());
        let back = convert(&registry, "text", "int", text);
        prop_assert_eq!(back, value.to_be_bytes().to_vec());
    }

    #[test]
    fn cdc_032_bigint_string_round_trips(value in any::<i64>()) {
        let registry = registry(&[Codecset::BigintString]);
        let text = convert(&registry, "bigint", "text", value.to_be_bytes().to_vec());
        let back = convert(&registry, "text", "bigint", text);
        prop_assert_eq!(back, value.to_be_bytes().to_vec());
    }

    #[test]
    fn cdc_032_bigint_biginteger_round_trips(value in any::<i64>()) {
        // Always registered, whatever `transform.codecs` says.
        let registry = registry(&[]);
        let varint = convert(&registry, "bigint", "varint", value.to_be_bytes().to_vec());
        let back = convert(&registry, "varint", "bigint", varint);
        prop_assert_eq!(back, value.to_be_bytes().to_vec());
    }

    #[test]
    fn cdc_032_double_string_is_idempotent_and_never_scientific(value in any::<f64>()) {
        prop_assume!(value.is_finite());
        let registry = registry(&[Codecset::DoubleString]);
        let text = convert(&registry, "double", "text", value.to_be_bytes().to_vec());
        let rendered = String::from_utf8(text.clone()).unwrap();
        prop_assert!(!rendered.contains('E') && !rendered.contains('e'), "{rendered}");
        prop_assert!(rendered.split('.').nth(1).is_none_or(|f| f.len() <= 9), "{rendered}");

        // The conversion is lossy by construction — nine fraction digits, rounded down — so the
        // property is idempotence: re-rendering the parsed value reproduces the same text.
        let back = convert(&registry, "text", "double", text);
        let again = convert(&registry, "double", "text", back);
        prop_assert_eq!(String::from_utf8(again).unwrap(), rendered);
    }

    #[test]
    fn cdc_032_double_string_formatting_agrees_with_the_pattern(value in any::<f64>()) {
        prop_assume!(value.is_finite());
        let rendered = format_double_java(value);
        prop_assert_eq!(rendered.matches('.').count() <= 1, true);
        prop_assert!(!rendered.starts_with('.'), "{rendered}");
        prop_assert!(!rendered.starts_with("-."), "{rendered}");
    }

    #[test]
    fn cdc_032_decimal_string_round_trips(unscaled in any::<i64>(), scale in 0_i32..12) {
        let registry = registry(&[Codecset::DecimalString]);
        let encoded = decimal_bytes(unscaled, scale);
        let text = convert(&registry, "decimal", "text", encoded.clone());
        let back = convert(&registry, "text", "decimal", text);
        // Re-rendering is the fixed point: `BigDecimal` normalises `0E-3` style spellings.
        let text_again = convert(&registry, "decimal", "text", back.clone());
        prop_assert_eq!(
            convert(&registry, "text", "decimal", text_again),
            back.clone()
        );
        prop_assert_eq!(
            convert(&registry, "decimal", "text", encoded.clone()),
            convert(&registry, "decimal", "text", back)
        );
    }

    #[test]
    fn cdc_032_string_blob_round_trips(value in ".*") {
        let registry = registry(&[Codecset::StringBlob]);
        let blob = convert(&registry, "text", "blob", value.clone().into_bytes());
        let back = convert(&registry, "blob", "text", blob);
        prop_assert_eq!(String::from_utf8(back).unwrap(), value);
    }

    #[test]
    fn cdc_032_ascii_blob_round_trips(value in "[ -~]*") {
        let registry = registry(&[Codecset::AsciiBlob]);
        let blob = convert(&registry, "ascii", "blob", value.clone().into_bytes());
        let back = convert(&registry, "blob", "ascii", blob);
        prop_assert_eq!(String::from_utf8(back).unwrap(), value);
    }

    #[test]
    fn cdc_032_timestamp_string_millis_round_trips(millis in any::<i64>()) {
        let registry = registry(&[Codecset::TimestampStringMillis]);
        let text = convert(&registry, "timestamp", "text", millis.to_be_bytes().to_vec());
        prop_assert_eq!(String::from_utf8(text.clone()).unwrap(), millis.to_string());
        let back = convert(&registry, "text", "timestamp", text);
        prop_assert_eq!(back, millis.to_be_bytes().to_vec());
    }

    #[test]
    fn cdc_032_timestamp_string_format_round_trips(seconds in -2_000_000_000_i64..2_000_000_000) {
        let settings = TimestampFormat::new("yyyyMMddHHmmss", "Europe/Dublin").unwrap();
        let registry =
            CodecRegistry::with_builtins(&[Codecset::TimestampStringFormat], Some(settings))
                .unwrap();
        // The pattern has second resolution, so only whole seconds can round-trip.
        let millis = seconds * 1000;
        let text = convert(&registry, "timestamp", "text", millis.to_be_bytes().to_vec());
        prop_assert_eq!(text.len(), 14);
        let back = convert(&registry, "text", "timestamp", text);
        prop_assert_eq!(back, millis.to_be_bytes().to_vec());
    }

    #[test]
    fn cdc_032_geometry_codecs_round_trip(
        xs in proptest::collection::vec(-180.0_f64..180.0, 2..8),
        ys in proptest::collection::vec(-90.0_f64..90.0, 2..8),
    ) {
        let registry = registry(&[
            Codecset::PointType,
            Codecset::LineString,
            Codecset::PolygonType,
        ]);
        let points: Vec<(f64, f64)> = xs.iter().copied().zip(ys.iter().copied()).collect();

        let cases = [
            ("PointType", Geometry::Point(points[0].0, points[0].1)),
            ("LineStringType", Geometry::LineString(points.clone())),
            ("PolygonType", Geometry::Polygon(vec![points])),
        ];
        for (cql_type, geometry) in cases {
            let wkb = geometry.to_wkb().unwrap();
            let wkt = convert(&registry, cql_type, "text", wkb.clone());
            let back = convert(&registry, "text", cql_type, wkt);
            prop_assert_eq!(&back, &wkb);
            prop_assert_eq!(Geometry::from_wkb(&back).unwrap(), geometry);
        }
    }

    #[test]
    fn cdc_032_date_range_codec_round_trips(
        year in 1600_i32..2400,
        month in 1_u32..13,
        day in 1_u32..29,
        hour in 0_u32..24,
    ) {
        let registry = registry(&[Codecset::DateRange]);
        for text in [
            format!("{year:04}"),
            format!("{year:04}-{month:02}"),
            format!("{year:04}-{month:02}-{day:02}"),
            format!("{year:04}-{month:02}-{day:02}T{hour:02}Z"),
            format!("[{year:04}-{month:02}-{day:02} TO *]"),
            format!("[* TO {year:04}-{month:02}-{day:02}]"),
        ] {
            let encoded = convert(&registry, "text", "DateRangeType", text.clone().into_bytes());
            let back = convert(&registry, "DateRangeType", "text", encoded.clone());
            prop_assert_eq!(String::from_utf8(back).unwrap(), text.clone());
            prop_assert_eq!(DateRange::parse(&text).unwrap().to_bytes(), encoded);
        }
    }

    /// `CDC-011`: a `Passthrough` plan moves the raw bytes, and doing so is indistinguishable from
    /// a full decode-and-re-encode. The comparison is made by converting out to `text` and back
    /// with registered codecs, which is a genuine deserialise/serialise cycle, and asserting the
    /// result is the very bytes passthrough would have moved. This is the property the zero-copy
    /// fast path of `MIG-040` (`TST-030`) rests on.
    #[test]
    fn cdc_011_passthrough_is_indistinguishable_from_a_full_decode_and_reencode(
        ints in proptest::collection::vec(any::<i32>(), 0..6),
        longs in proptest::collection::vec(any::<i64>(), 1..4),
        text in "[a-z]{0,12}",
        millis in any::<i64>(),
    ) {
        let planner = planner();
        let int_values: Vec<Option<Vec<u8>>> =
            ints.iter().map(|v| Some(v.to_be_bytes().to_vec())).collect();

        let cases: Vec<(CqlTypeInfo, CqlTypeInfo, Vec<u8>)> = vec![
            (
                CqlTypeInfo::parse("list<int>").unwrap(),
                CqlTypeInfo::parse("list<text>").unwrap(),
                collection(&int_values),
            ),
            (
                CqlTypeInfo::parse("map<bigint, int>").unwrap(),
                CqlTypeInfo::parse("map<text, text>").unwrap(),
                map(&[(
                    longs[0].to_be_bytes().to_vec(),
                    ints.first().copied().unwrap_or(5).to_be_bytes().to_vec(),
                )]),
            ),
            (
                CqlTypeInfo::parse("tuple<int, bigint, timestamp>").unwrap(),
                CqlTypeInfo::parse("tuple<text, text, text>").unwrap(),
                fields(&[
                    Some(ints.first().copied().unwrap_or(7).to_be_bytes().to_vec()),
                    Some(longs[0].to_be_bytes().to_vec()),
                    Some(millis.to_be_bytes().to_vec()),
                ]),
            ),
            (
                udt("origin", &[("a", "int"), ("b", "bigint")]),
                udt("target", &[("a", "text"), ("b", "text")]),
                fields(&[
                    Some(ints.first().copied().unwrap_or(-3).to_be_bytes().to_vec()),
                    Some(longs[0].to_be_bytes().to_vec()),
                ]),
            ),
            (
                CqlTypeInfo::parse("text").unwrap(),
                CqlTypeInfo::parse("blob").unwrap(),
                text.clone().into_bytes(),
            ),
            (
                CqlTypeInfo::parse("vector<float, 2>").unwrap(),
                CqlTypeInfo::parse("vector<float, 2>").unwrap(),
                [1.5_f32.to_be_bytes(), (-0.25_f32).to_be_bytes()].concat(),
            ),
        ];

        for (origin, target, value) in cases {
            let cell = RawCell::new(value.clone());

            // Passthrough is byte-identical, for every type, including the composite ones.
            let passthrough = planner.plan_types(&origin, &origin);
            prop_assert!(
                matches!(passthrough, ConversionPlan::Passthrough),
                "{origin} -> {origin} was {}",
                passthrough.kind()
            );
            prop_assert_eq!(passthrough.apply(&cell).unwrap(), cell.clone());

            // And a full decode/re-encode through registered codecs lands on the same bytes.
            let out = planner.plan_types(&origin, &target);
            let back = planner.plan_types(&target, &origin);
            let converted = out.apply(&cell).unwrap();
            prop_assert_eq!(
                back.apply(&converted).unwrap(),
                cell,
                "{} -> {} -> {}",
                origin,
                target,
                origin
            );
        }
    }
}
