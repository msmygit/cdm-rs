//! DSE geometry and `DateRangeType` encodings (`CDC-003`).
//!
//! DSE stores these four types as opaque custom columns whose bytes the driver simply carries.
//! Java CDM registers the DSE driver's own codecs for them (`CodecFactory`, `POINT_TYPE`,
//! `LINE_STRING`, `POLYGON_TYPE`, `DATE_RANGE`); cdm-rs has no JVM, so the encodings are
//! implemented here, over raw bytes, and exposed as ordinary codecs (`ARCHITECTURE.md` §6.1).
//!
//! * geometry uses **OGC well-known binary**: a byte-order flag, a 32-bit geometry type, then
//!   IEEE-754 coordinates. The textual form is OGC well-known text, which is what
//!   `Geometry.asWellKnownText()` produces and what the Java tests assert against.
//! * `DateRangeType` uses the DSE date-range encoding: a one-byte range kind followed by zero,
//!   one or two bounds, each an 8-byte epoch-milli value and a one-byte precision.

use cdm_core::CdmError;
use chrono::{DateTime, Datelike as _, NaiveDate, TimeZone as _, Timelike as _, Utc};

use crate::wire::conversion_error;

/// WKB byte-order marker for little-endian, which is what DSE writes.
const WKB_LITTLE_ENDIAN: u8 = 1;
/// WKB geometry type codes.
const WKB_POINT: u32 = 1;
/// WKB geometry type code for a line string.
const WKB_LINE_STRING: u32 = 2;
/// WKB geometry type code for a polygon.
const WKB_POLYGON: u32 = 3;

/// A planar geometry, in the three shapes DSE supports (`CDC-003`).
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// A single coordinate.
    Point(f64, f64),
    /// An ordered sequence of coordinates.
    LineString(Vec<(f64, f64)>),
    /// An exterior ring followed by zero or more interior rings.
    Polygon(Vec<Vec<(f64, f64)>>),
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    little_endian: bool,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, CdmError> {
        let flag = *bytes
            .first()
            .ok_or_else(|| conversion_error("empty well-known binary value"))?;
        Ok(Self {
            bytes,
            pos: 1,
            little_endian: flag == WKB_LITTLE_ENDIAN,
        })
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], CdmError> {
        let end = self.pos + count;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| conversion_error("truncated well-known binary value"))?;
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, CdmError> {
        let raw = <[u8; 4]>::try_from(self.take(4)?)
            .map_err(|_| conversion_error("truncated well-known binary value"))?;
        Ok(if self.little_endian {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    }

    fn f64(&mut self) -> Result<f64, CdmError> {
        let raw = <[u8; 8]>::try_from(self.take(8)?)
            .map_err(|_| conversion_error("truncated well-known binary value"))?;
        Ok(if self.little_endian {
            f64::from_le_bytes(raw)
        } else {
            f64::from_be_bytes(raw)
        })
    }

    fn point(&mut self) -> Result<(f64, f64), CdmError> {
        Ok((self.f64()?, self.f64()?))
    }

    fn ring(&mut self) -> Result<Vec<(f64, f64)>, CdmError> {
        let count = self.u32()? as usize;
        let mut points = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            points.push(self.point()?);
        }
        Ok(points)
    }
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_point(out: &mut Vec<u8>, point: (f64, f64)) {
    out.extend_from_slice(&point.0.to_le_bytes());
    out.extend_from_slice(&point.1.to_le_bytes());
}

fn push_ring(out: &mut Vec<u8>, ring: &[(f64, f64)]) -> Result<(), CdmError> {
    push_u32(
        out,
        u32::try_from(ring.len()).map_err(|_| conversion_error("geometry has too many points"))?,
    );
    for point in ring {
        push_point(out, *point);
    }
    Ok(())
}

impl Geometry {
    /// Decodes well-known binary.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`](cdm_core::ErrorKind::TypeConversion) when the buffer
    /// is truncated or names a geometry type DSE does not have.
    pub fn from_wkb(bytes: &[u8]) -> Result<Self, CdmError> {
        let mut cursor = Cursor::new(bytes)?;
        let kind = cursor.u32()?;
        match kind {
            WKB_POINT => {
                let (x, y) = cursor.point()?;
                Ok(Self::Point(x, y))
            }
            WKB_LINE_STRING => Ok(Self::LineString(cursor.ring()?)),
            WKB_POLYGON => {
                let rings = cursor.u32()? as usize;
                let mut out = Vec::with_capacity(rings.min(64));
                for _ in 0..rings {
                    out.push(cursor.ring()?);
                }
                Ok(Self::Polygon(out))
            }
            other => Err(conversion_error(format!(
                "unknown well-known-binary geometry type {other}"
            ))),
        }
    }

    /// Encodes well-known binary, little-endian as DSE writes it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`](cdm_core::ErrorKind::TypeConversion) for a geometry
    /// with more points than a 32-bit count can express.
    pub fn to_wkb(&self) -> Result<Vec<u8>, CdmError> {
        let mut out = vec![WKB_LITTLE_ENDIAN];
        match self {
            Self::Point(x, y) => {
                push_u32(&mut out, WKB_POINT);
                push_point(&mut out, (*x, *y));
            }
            Self::LineString(points) => {
                push_u32(&mut out, WKB_LINE_STRING);
                push_ring(&mut out, points)?;
            }
            Self::Polygon(rings) => {
                push_u32(&mut out, WKB_POLYGON);
                push_u32(
                    &mut out,
                    u32::try_from(rings.len())
                        .map_err(|_| conversion_error("polygon has too many rings"))?,
                );
                for ring in rings {
                    push_ring(&mut out, ring)?;
                }
            }
        }
        Ok(out)
    }

    /// Renders OGC well-known text, matching `Geometry.asWellKnownText()`: `POINT (30 10)`,
    /// `LINESTRING (30 10, 10 30, 40 40)`, `POLYGON ((30 10, 40 40, 20 40, 10 20, 30 10))`.
    pub fn to_wkt(&self) -> String {
        fn points(ring: &[(f64, f64)]) -> String {
            ring.iter()
                .map(|(x, y)| format!("{x} {y}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
        match self {
            Self::Point(x, y) => format!("POINT ({x} {y})"),
            Self::LineString(ring) => format!("LINESTRING ({})", points(ring)),
            Self::Polygon(rings) => {
                let inner = rings
                    .iter()
                    .map(|ring| format!("({})", points(ring)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("POLYGON ({inner})")
            }
        }
    }

    /// Parses OGC well-known text.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`](cdm_core::ErrorKind::TypeConversion) when the text is
    /// not a `POINT`, `LINESTRING` or `POLYGON` literal.
    pub fn from_wkt(text: &str) -> Result<Self, CdmError> {
        let trimmed = text.trim().trim_matches('\'').trim();
        let upper = trimmed.to_ascii_uppercase();
        let (_keyword, body) = trimmed
            .split_once('(')
            .ok_or_else(|| conversion_error(format!("`{text}` is not well-known text")))?;
        let body = body
            .strip_suffix(')')
            .ok_or_else(|| conversion_error(format!("`{text}` is not well-known text")))?;

        if upper.starts_with("POINT") {
            let coords = parse_points(body)?;
            let point = coords
                .first()
                .ok_or_else(|| conversion_error("POINT requires one coordinate"))?;
            return Ok(Self::Point(point.0, point.1));
        }
        if upper.starts_with("LINESTRING") {
            return Ok(Self::LineString(parse_points(body)?));
        }
        if upper.starts_with("POLYGON") {
            let mut rings = Vec::new();
            let mut rest = body.trim();
            while let Some(start) = rest.find('(') {
                let after = rest.get(start + 1..).unwrap_or_default();
                let end = after
                    .find(')')
                    .ok_or_else(|| conversion_error(format!("`{text}` has an unclosed ring")))?;
                rings.push(parse_points(after.get(..end).unwrap_or_default())?);
                rest = after.get(end + 1..).unwrap_or_default();
            }
            if rings.is_empty() {
                return Err(conversion_error("POLYGON requires at least one ring"));
            }
            return Ok(Self::Polygon(rings));
        }
        Err(conversion_error(format!(
            "`{text}` is not a POINT, LINESTRING or POLYGON literal"
        )))
    }
}

fn parse_points(body: &str) -> Result<Vec<(f64, f64)>, CdmError> {
    body.split(',')
        .map(|pair| {
            let mut parts = pair.split_whitespace();
            let x = parts.next().ok_or_else(|| conversion_error("missing x"))?;
            let y = parts.next().ok_or_else(|| conversion_error("missing y"))?;
            let x: f64 = x
                .parse()
                .map_err(|_| conversion_error(format!("`{x}` is not a coordinate")))?;
            let y: f64 = y
                .parse()
                .map_err(|_| conversion_error(format!("`{y}` is not a coordinate")))?;
            Ok((x, y))
        })
        .collect()
}

/// The precision of one date-range bound, as DSE encodes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// `2001`
    Year,
    /// `2001-01`
    Month,
    /// `2001-01-01`
    Day,
    /// `2001-01-01T10Z`
    Hour,
    /// `2001-01-01T10:15Z`
    Minute,
    /// `2001-01-01T10:15:30Z`
    Second,
    /// `2001-01-01T10:15:30.123Z`
    Millisecond,
}

impl Precision {
    const fn code(self) -> u8 {
        match self {
            Self::Year => 0,
            Self::Month => 1,
            Self::Day => 2,
            Self::Hour => 3,
            Self::Minute => 4,
            Self::Second => 5,
            Self::Millisecond => 6,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Year,
            1 => Self::Month,
            2 => Self::Day,
            3 => Self::Hour,
            4 => Self::Minute,
            5 => Self::Second,
            6 => Self::Millisecond,
            _ => return None,
        })
    }
}

/// One bound of a date range: an instant and the precision it was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateRangeBound {
    /// Epoch milliseconds.
    pub millis: i64,
    /// How much of the instant is significant.
    pub precision: Precision,
}

/// A DSE `DateRangeType` value (`CDC-003`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRange {
    /// `2001-01-01` — a single date at some precision.
    Single(DateRangeBound),
    /// `[2001-01-01 TO 2001-01-02]`
    Closed(DateRangeBound, DateRangeBound),
    /// `[2001-01-01 TO *]`
    OpenUpper(DateRangeBound),
    /// `[* TO 2001-01-02]`
    OpenLower(DateRangeBound),
    /// `[* TO *]`
    BothOpen,
    /// `*`
    SingleOpen,
}

impl DateRange {
    /// Decodes the DSE date-range encoding.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`](cdm_core::ErrorKind::TypeConversion) when the buffer
    /// is truncated or the range kind or precision code is unknown.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CdmError> {
        let kind = *bytes
            .first()
            .ok_or_else(|| conversion_error("empty date-range value"))?;
        let mut rest = bytes.get(1..).unwrap_or_default();
        let mut bound = || -> Result<DateRangeBound, CdmError> {
            let raw = rest
                .get(..9)
                .ok_or_else(|| conversion_error("truncated date-range bound"))?;
            let millis = i64::from_be_bytes(
                <[u8; 8]>::try_from(raw.get(..8).unwrap_or_default())
                    .map_err(|_| conversion_error("truncated date-range bound"))?,
            );
            let code = *raw.get(8).unwrap_or(&0xff);
            let precision = Precision::from_code(code).ok_or_else(|| {
                conversion_error(format!("unknown date-range precision code {code}"))
            })?;
            rest = rest.get(9..).unwrap_or_default();
            Ok(DateRangeBound { millis, precision })
        };
        match kind {
            0 => Ok(Self::Single(bound()?)),
            1 => Ok(Self::Closed(bound()?, bound()?)),
            2 => Ok(Self::OpenUpper(bound()?)),
            3 => Ok(Self::OpenLower(bound()?)),
            4 => Ok(Self::BothOpen),
            5 => Ok(Self::SingleOpen),
            other => Err(conversion_error(format!("unknown date-range kind {other}"))),
        }
    }

    /// Encodes the DSE date-range encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        fn push(out: &mut Vec<u8>, bound: DateRangeBound) {
            out.extend_from_slice(&bound.millis.to_be_bytes());
            out.push(bound.precision.code());
        }
        let mut out = Vec::with_capacity(19);
        match self {
            Self::Single(bound) => {
                out.push(0);
                push(&mut out, *bound);
            }
            Self::Closed(lower, upper) => {
                out.push(1);
                push(&mut out, *lower);
                push(&mut out, *upper);
            }
            Self::OpenUpper(lower) => {
                out.push(2);
                push(&mut out, *lower);
            }
            Self::OpenLower(upper) => {
                out.push(3);
                push(&mut out, *upper);
            }
            Self::BothOpen => out.push(4),
            Self::SingleOpen => out.push(5),
        }
        out
    }

    /// Renders the DSE textual form, which is what `DateRange.toString()` produces.
    pub fn to_text(&self) -> String {
        match self {
            Self::Single(bound) => format_bound(*bound),
            Self::Closed(lower, upper) => {
                format!("[{} TO {}]", format_bound(*lower), format_bound(*upper))
            }
            Self::OpenUpper(lower) => format!("[{} TO *]", format_bound(*lower)),
            Self::OpenLower(upper) => format!("[* TO {}]", format_bound(*upper)),
            Self::BothOpen => "[* TO *]".to_owned(),
            Self::SingleOpen => "*".to_owned(),
        }
    }

    /// Parses the DSE textual form, as `DateRange.parse` does.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`](cdm_core::ErrorKind::TypeConversion) when the text is
    /// not a date-range literal.
    pub fn parse(text: &str) -> Result<Self, CdmError> {
        let trimmed = text.trim().trim_matches('\'').trim();
        if trimmed == "*" {
            return Ok(Self::SingleOpen);
        }
        if let Some(inner) = trimmed.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            let (lower, upper) = inner
                .split_once(" TO ")
                .ok_or_else(|| conversion_error(format!("`{text}` is not a date range")))?;
            let (lower, upper) = (lower.trim(), upper.trim());
            return Ok(match (lower == "*", upper == "*") {
                (true, true) => Self::BothOpen,
                (true, false) => Self::OpenLower(parse_bound(upper, true)?),
                (false, true) => Self::OpenUpper(parse_bound(lower, false)?),
                (false, false) => {
                    Self::Closed(parse_bound(lower, false)?, parse_bound(upper, true)?)
                }
            });
        }
        Ok(Self::Single(parse_bound(trimmed, false)?))
    }
}

fn format_bound(bound: DateRangeBound) -> String {
    let Some(value) = DateTime::<Utc>::from_timestamp_millis(bound.millis) else {
        return bound.millis.to_string();
    };
    match bound.precision {
        Precision::Year => format!("{:04}", value.year()),
        Precision::Month => format!("{:04}-{:02}", value.year(), value.month()),
        Precision::Day => format!(
            "{:04}-{:02}-{:02}",
            value.year(),
            value.month(),
            value.day()
        ),
        Precision::Hour => format!("{}T{:02}Z", format_date(value), value.hour()),
        Precision::Minute => format!(
            "{}T{:02}:{:02}Z",
            format_date(value),
            value.hour(),
            value.minute()
        ),
        Precision::Second => format!(
            "{}T{:02}:{:02}:{:02}Z",
            format_date(value),
            value.hour(),
            value.minute(),
            value.second()
        ),
        Precision::Millisecond => format!(
            "{}T{:02}:{:02}:{:02}.{:03}Z",
            format_date(value),
            value.hour(),
            value.minute(),
            value.second(),
            value.timestamp_subsec_millis()
        ),
    }
}

fn format_date(value: DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        value.year(),
        value.month(),
        value.day()
    )
}

/// Parses one bound. An upper bound is rounded **up** to the end of its precision unit, which is
/// what makes `[2001-01-01 TO 2001-01-01]` cover the whole day, as DSE defines it.
fn parse_bound(text: &str, upper: bool) -> Result<DateRangeBound, CdmError> {
    let text = text.trim().trim_end_matches('Z');
    let (date_part, time_part) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let date_fields: Vec<&str> = date_part.split('-').collect();
    let number = |value: Option<&&str>, what: &str| -> Result<u32, CdmError> {
        value
            .ok_or_else(|| conversion_error(format!("date range is missing its {what}")))?
            .parse::<u32>()
            .map_err(|_| conversion_error(format!("date range has a non-numeric {what}")))
    };

    let year = i32::try_from(number(date_fields.first(), "year")?)
        .map_err(|_| conversion_error("date-range year out of range"))?;
    let month = if date_fields.len() > 1 {
        number(date_fields.get(1), "month")?
    } else {
        1
    };
    let day = if date_fields.len() > 2 {
        number(date_fields.get(2), "day")?
    } else {
        1
    };

    let time_fields: Vec<&str> = time_part
        .map(|t| t.split(':').collect())
        .unwrap_or_default();
    let hour = if time_fields.is_empty() {
        0
    } else {
        number(time_fields.first(), "hour")?
    };
    let minute = if time_fields.len() > 1 {
        number(time_fields.get(1), "minute")?
    } else {
        0
    };
    let (second, millis) = match time_fields.get(2) {
        None => (0, 0),
        Some(field) => match field.split_once('.') {
            None => (number(Some(field), "second")?, 0),
            Some((sec, frac)) => {
                let sec = number(Some(&sec), "second")?;
                let mut frac = frac.to_owned();
                frac.truncate(3);
                while frac.len() < 3 {
                    frac.push('0');
                }
                (
                    sec,
                    frac.parse::<u32>()
                        .map_err(|_| conversion_error("date range has a non-numeric fraction"))?,
                )
            }
        },
    };

    let precision = match (date_fields.len(), time_fields.len()) {
        (1, _) => Precision::Year,
        (2, _) => Precision::Month,
        (_, 0) => Precision::Day,
        (_, 1) => Precision::Hour,
        (_, 2) => Precision::Minute,
        _ => {
            if time_fields.get(2).is_some_and(|f| f.contains('.')) {
                Precision::Millisecond
            } else {
                Precision::Second
            }
        }
    };

    let start = Utc
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .ok_or_else(|| conversion_error(format!("`{text}` is not a valid instant")))?
        .timestamp_millis()
        + i64::from(millis);

    let millis = if upper {
        end_of_unit(year, month, day, hour, minute, second, precision)?
    } else {
        start
    };
    Ok(DateRangeBound { millis, precision })
}

/// The last millisecond of the precision unit containing the given fields.
fn end_of_unit(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    precision: Precision,
) -> Result<i64, CdmError> {
    let invalid = || conversion_error("date range does not denote a valid instant");
    let next = match precision {
        Precision::Year => Utc
            .with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0)
            .single()
            .ok_or_else(invalid)?,
        Precision::Month => {
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            Utc.with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
                .single()
                .ok_or_else(invalid)?
        }
        Precision::Day => {
            let date = NaiveDate::from_ymd_opt(year, month, day)
                .and_then(|d| d.succ_opt())
                .ok_or_else(invalid)?;
            date.and_hms_opt(0, 0, 0).ok_or_else(invalid)?.and_utc()
        }
        Precision::Hour => {
            Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
                .single()
                .ok_or_else(invalid)?
                + chrono::Duration::hours(1)
        }
        Precision::Minute => {
            Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
                .single()
                .ok_or_else(invalid)?
                + chrono::Duration::minutes(1)
        }
        Precision::Second | Precision::Millisecond => {
            Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
                .single()
                .ok_or_else(invalid)?
                + chrono::Duration::seconds(1)
        }
    };
    Ok(next.timestamp_millis() - 1)
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

    // The three well-known-text fixtures below are lifted verbatim from the Java tests
    // POINTTYPE_CodecTest, LINESTRINGTYPE_CodecTest and POLYGONTYPE_CodecTest.
    const POINT: &str = "POINT (30 10)";
    const LINE_STRING: &str = "LINESTRING (30 10, 10 30, 40 40)";
    const POLYGON: &str = "POLYGON ((30 10, 40 40, 20 40, 10 20, 30 10))";

    #[test]
    fn cdc_003_geometry_well_known_text_matches_the_java_fixtures() {
        for wkt in [POINT, LINE_STRING, POLYGON] {
            let geometry = Geometry::from_wkt(wkt).unwrap();
            assert_eq!(geometry.to_wkt(), wkt);
            let wkb = geometry.to_wkb().unwrap();
            assert_eq!(Geometry::from_wkb(&wkb).unwrap(), geometry);
        }
    }

    #[test]
    fn cdc_003_point_well_known_binary_is_little_endian_type_1() {
        let wkb = Geometry::Point(30.0, 10.0).to_wkb().unwrap();
        assert_eq!(wkb.len(), 21);
        assert_eq!(&wkb[..5], &[1, 1, 0, 0, 0]);
        assert_eq!(&wkb[5..13], &30.0_f64.to_le_bytes());
        assert_eq!(&wkb[13..], &10.0_f64.to_le_bytes());
        // A big-endian buffer decodes just as well, because the flag byte says so.
        let mut be = vec![0_u8];
        be.extend_from_slice(&WKB_POINT.to_be_bytes());
        be.extend_from_slice(&30.0_f64.to_be_bytes());
        be.extend_from_slice(&10.0_f64.to_be_bytes());
        assert_eq!(
            Geometry::from_wkb(&be).unwrap(),
            Geometry::Point(30.0, 10.0)
        );
    }

    #[test]
    fn cdc_003_malformed_geometry_is_a_conversion_error() {
        assert!(Geometry::from_wkb(&[]).is_err());
        assert!(Geometry::from_wkb(&[1, 9, 0, 0, 0]).is_err());
        assert!(Geometry::from_wkb(&[1, 1, 0, 0, 0, 1, 2]).is_err());
        assert!(Geometry::from_wkt("CIRCLE (1 1)").is_err());
        assert!(Geometry::from_wkt("POINT 30 10").is_err());
        assert!(Geometry::from_wkt("POINT (a b)").is_err());
        assert!(Geometry::from_wkt("POLYGON ((1 1)").is_err());
    }

    #[test]
    fn cdc_003_date_range_single_date_round_trips_the_java_fixture() {
        // DATERANGETYPE_CodecTest parses exactly this string.
        let range = DateRange::parse("2001-01-01").unwrap();
        assert_eq!(range.to_text(), "2001-01-01");
        let bytes = range.to_bytes();
        assert_eq!(bytes.len(), 10);
        assert_eq!(bytes[0], 0, "SINGLE_DATE");
        assert_eq!(bytes[9], 2, "DAY precision");
        assert_eq!(DateRange::from_bytes(&bytes).unwrap(), range);
    }

    #[test]
    fn cdc_003_date_range_covers_every_kind_and_precision() {
        for text in [
            "2001",
            "2001-01",
            "2001-01-01",
            "2001-01-01T10Z",
            "2001-01-01T10:15Z",
            "2001-01-01T10:15:30Z",
            "2001-01-01T10:15:30.123Z",
            "[2001-01-01 TO 2001-01-02]",
            "[2001-01-01 TO *]",
            "[* TO 2001-01-02]",
            "[* TO *]",
            "*",
        ] {
            let range = DateRange::parse(text).unwrap();
            assert_eq!(range.to_text(), text, "text form of {text}");
            assert_eq!(DateRange::from_bytes(&range.to_bytes()).unwrap(), range);
        }
    }

    #[test]
    fn cdc_003_an_upper_bound_is_rounded_up_to_the_end_of_its_unit() {
        let DateRange::Closed(lower, upper) =
            DateRange::parse("[2001-01-01 TO 2001-01-01]").unwrap()
        else {
            panic!("expected a closed range");
        };
        assert_eq!(upper.millis - lower.millis, 86_400_000 - 1);
    }

    #[test]
    fn cdc_003_malformed_date_ranges_are_conversion_errors() {
        assert!(DateRange::from_bytes(&[]).is_err());
        assert!(DateRange::from_bytes(&[9]).is_err());
        assert!(DateRange::from_bytes(&[0, 0, 0]).is_err());
        assert!(DateRange::from_bytes(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 99]).is_err());
        assert!(DateRange::parse("[2001 2002]").is_err());
        assert!(DateRange::parse("20x1").is_err());
        assert!(DateRange::parse("2001-13-01").is_err());
    }
}
