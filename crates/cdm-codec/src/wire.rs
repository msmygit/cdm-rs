//! Byte-level CQL serialisation, for the conversions that cannot avoid decoding.
//!
//! A conversion plan that is [`Passthrough`](crate::ConversionPlan::Passthrough) never touches
//! these functions — that is the whole point of `MIG-040`. They exist for the conversions that
//! must actually reinterpret a value: the built-in codecs of `CDC-020` and the collection, tuple,
//! UDT and vector framing the recursive plans of `CDC-012`..`CDC-015` re-emit.
//!
//! Everything here speaks native protocol v4+ framing, which is what the driver hands us and what
//! it expects back:
//!
//! * primitives are big-endian, fixed width where the type has one;
//! * a collection is an `i32` element count followed by `i32`-length-prefixed elements, where
//!   `-1` denotes `null`;
//! * a tuple or UDT is `i32`-length-prefixed fields with no count, one per declared component;
//! * a `vector<T, N>` of a fixed-width `T` is a contiguous array with no framing at all.

use bigdecimal::BigDecimal;
use cdm_core::{CdmError, ErrorKind};
use num_bigint::{BigInt, Sign};

/// A type-conversion failure. Record-level: the engine counts `ERROR` and continues
/// (`ARCHITECTURE.md` §13).
pub(crate) fn conversion_error(message: impl Into<String>) -> CdmError {
    CdmError::new(ErrorKind::TypeConversion, message)
}

/// A sequential reader over a serialised value.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) const fn is_exhausted(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], CdmError> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or_else(|| conversion_error("serialised value length overflowed"))?;
        let slice = self.bytes.get(self.pos..end).ok_or_else(|| {
            conversion_error(format!(
                "truncated serialised value: wanted {count} more bytes, {} remain",
                self.remaining()
            ))
        })?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn take_i32(&mut self) -> Result<i32, CdmError> {
        read_i32(self.take(4)?)
    }

    /// One `i32`-length-prefixed element; `None` for the `-1` null marker.
    pub(crate) fn take_element(&mut self) -> Result<Option<&'a [u8]>, CdmError> {
        let length = self.take_i32()?;
        if length < 0 {
            return Ok(None);
        }
        let length = usize::try_from(length)
            .map_err(|_| conversion_error("negative element length in serialised collection"))?;
        Ok(Some(self.take(length)?))
    }
}

/// Appends one `i32`-length-prefixed element, writing the `-1` null marker for `None`.
pub(crate) fn write_element(out: &mut Vec<u8>, element: Option<&[u8]>) -> Result<(), CdmError> {
    match element {
        None => out.extend_from_slice(&(-1_i32).to_be_bytes()),
        Some(bytes) => {
            let length = i32::try_from(bytes.len())
                .map_err(|_| conversion_error("serialised element exceeds 2 GiB"))?;
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(bytes);
        }
    }
    Ok(())
}

fn fixed<const N: usize>(bytes: &[u8], what: &str) -> Result<[u8; N], CdmError> {
    <[u8; N]>::try_from(bytes).map_err(|_| {
        conversion_error(format!(
            "expected {N} bytes for a {what} value, got {}",
            bytes.len()
        ))
    })
}

/// Reads a 4-byte big-endian `int`.
pub(crate) fn read_i32(bytes: &[u8]) -> Result<i32, CdmError> {
    Ok(i32::from_be_bytes(fixed::<4>(bytes, "int")?))
}

/// Reads an 8-byte big-endian `bigint`, `counter` or `timestamp`.
pub(crate) fn read_i64(bytes: &[u8]) -> Result<i64, CdmError> {
    Ok(i64::from_be_bytes(fixed::<8>(bytes, "bigint")?))
}

/// Reads an 8-byte IEEE-754 `double`.
pub(crate) fn read_f64(bytes: &[u8]) -> Result<f64, CdmError> {
    Ok(f64::from_be_bytes(fixed::<8>(bytes, "double")?))
}

/// Serialises an `int`.
pub(crate) fn write_i32(value: i32) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// Serialises a `bigint`, `counter` or `timestamp`.
pub(crate) fn write_i64(value: i64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// Serialises a `double`.
pub(crate) fn write_f64(value: f64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// Reads `text`/`varchar`, rejecting invalid UTF-8.
pub(crate) fn read_text(bytes: &[u8]) -> Result<&str, CdmError> {
    std::str::from_utf8(bytes)
        .map_err(|e| conversion_error(format!("value is not valid UTF-8 text: {e}")))
}

/// Reads `ascii`, rejecting any byte above 0x7F.
pub(crate) fn read_ascii(bytes: &[u8]) -> Result<&str, CdmError> {
    if !bytes.is_ascii() {
        return Err(conversion_error(
            "value is not valid US-ASCII: it contains a byte above 0x7F",
        ));
    }
    read_text(bytes)
}

/// Reads a `varint`: a big-endian two's-complement integer of any length.
pub(crate) fn read_varint(bytes: &[u8]) -> Result<BigInt, CdmError> {
    if bytes.is_empty() {
        return Err(conversion_error("empty varint value"));
    }
    Ok(BigInt::from_signed_bytes_be(bytes))
}

/// Serialises a `varint`. Zero is one `0x00` byte, as Cassandra encodes it.
pub(crate) fn write_varint(value: &BigInt) -> Vec<u8> {
    if value.sign() == Sign::NoSign {
        return vec![0];
    }
    value.to_signed_bytes_be()
}

/// Reads a `decimal`: a 4-byte big-endian scale followed by a `varint` unscaled value.
pub(crate) fn read_decimal(bytes: &[u8]) -> Result<BigDecimal, CdmError> {
    let mut reader = Reader::new(bytes);
    let scale = reader.take_i32()?;
    let unscaled = read_varint(reader.take(reader.remaining())?)?;
    Ok(BigDecimal::new(unscaled, i64::from(scale)))
}

/// Serialises a `decimal`.
pub(crate) fn write_decimal(value: &BigDecimal) -> Result<Vec<u8>, CdmError> {
    let (unscaled, scale) = value.as_bigint_and_exponent();
    let scale = i32::try_from(scale)
        .map_err(|_| conversion_error(format!("decimal scale {scale} does not fit in 32 bits")))?;
    let mut out = scale.to_be_bytes().to_vec();
    out.extend_from_slice(&write_varint(&unscaled));
    Ok(out)
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
    use std::str::FromStr;

    #[test]
    fn cdc_001_primitive_encodings_are_big_endian_and_fixed_width() {
        assert_eq!(write_i32(10), vec![0, 0, 0, 10]);
        assert_eq!(read_i32(&[0, 0, 0, 10]).unwrap(), 10);
        assert_eq!(
            write_i64(9_223_372_036_854_775_807),
            vec![0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            read_i64(&[0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(),
            9_223_372_036_854_775_807
        );
        let bits = write_f64(21_474_836_470.7);
        assert!((read_f64(&bits).unwrap() - 21_474_836_470.7).abs() < f64::EPSILON);
    }

    #[test]
    fn cdc_001_truncated_buffers_are_type_conversion_errors() {
        assert_eq!(
            read_i32(&[0, 0]).unwrap_err().kind(),
            ErrorKind::TypeConversion
        );
        assert_eq!(
            read_i64(&[0, 0]).unwrap_err().kind(),
            ErrorKind::TypeConversion
        );
        assert_eq!(
            read_f64(&[0, 0]).unwrap_err().kind(),
            ErrorKind::TypeConversion
        );
        assert!(read_varint(&[]).is_err());
        assert!(read_text(&[0xff, 0xfe]).is_err());
        assert!(read_ascii(&[0xc3, 0xa9]).is_err());
        assert_eq!(read_ascii(b"abc").unwrap(), "abc");
    }

    #[test]
    fn cdc_001_varint_and_decimal_use_cassandra_framing() {
        assert_eq!(write_varint(&BigInt::from(0)), vec![0]);
        assert_eq!(write_varint(&BigInt::from(-1)), vec![0xff]);
        assert_eq!(read_varint(&[0xff]).unwrap(), BigInt::from(-1));

        // 123.456 = unscaled 123456, scale 3.
        let decimal = BigDecimal::from_str("123.456").unwrap();
        let encoded = write_decimal(&decimal).unwrap();
        assert_eq!(&encoded[..4], &[0, 0, 0, 3]);
        assert_eq!(read_decimal(&encoded).unwrap(), decimal);
    }

    #[test]
    fn cdc_012_collection_framing_round_trips_including_nulls() {
        let mut out = Vec::new();
        out.extend_from_slice(&2_i32.to_be_bytes());
        write_element(&mut out, Some(&write_i32(7))).unwrap();
        write_element(&mut out, None).unwrap();

        let mut reader = Reader::new(&out);
        assert_eq!(reader.take_i32().unwrap(), 2);
        assert_eq!(reader.take_element().unwrap(), Some(&[0, 0, 0, 7][..]));
        assert_eq!(reader.take_element().unwrap(), None);
        assert!(reader.is_exhausted());
    }

    #[test]
    fn cdc_012_a_truncated_collection_is_reported_not_silently_accepted() {
        let mut reader = Reader::new(&[0, 0, 0, 1, 0, 0, 0, 8, 1, 2]);
        assert_eq!(reader.take_i32().unwrap(), 1);
        assert!(reader.take_element().is_err());
    }
}
