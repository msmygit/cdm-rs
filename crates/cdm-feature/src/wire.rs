//! The sliver of native-protocol framing the features need.
//!
//! `cdm-codec` owns CQL serialisation, but keeps its reader private: it exists to serve the
//! conversion planner, and widening it into a public byte-level API for one consumer would make
//! every future change to it a breaking change for this crate. What `cdm-feature` needs is
//! genuinely small — the element framing of a `map` (`FEA-020`) and of a writetime `list`
//! (`FEA-043`) — so it is decoded here, against the same protocol v4+ layout `cdm-codec` documents:
//! an `i32` element count followed by `i32`-length-prefixed elements, where `-1` denotes `null`.

use cdm_core::{CdmError, ErrorKind, RawCell};

fn truncated(what: &str) -> CdmError {
    CdmError::new(
        ErrorKind::TypeConversion,
        format!("truncated serialised {what}"),
    )
}

fn take<'a>(
    bytes: &'a [u8],
    at: &mut usize,
    count: usize,
    what: &str,
) -> Result<&'a [u8], CdmError> {
    let end = at.checked_add(count).ok_or_else(|| truncated(what))?;
    let slice = bytes.get(*at..end).ok_or_else(|| truncated(what))?;
    *at = end;
    Ok(slice)
}

fn take_i32(bytes: &[u8], at: &mut usize, what: &str) -> Result<i32, CdmError> {
    let slice = take(bytes, at, 4, what)?;
    let array = <[u8; 4]>::try_from(slice).map_err(|_| truncated(what))?;
    Ok(i32::from_be_bytes(array))
}

/// One `i32`-length-prefixed element, as a cell so that the `-1` null marker keeps its meaning.
fn take_element(bytes: &[u8], at: &mut usize, what: &str) -> Result<RawCell, CdmError> {
    let length = take_i32(bytes, at, what)?;
    if length < 0 {
        return Ok(RawCell::NULL);
    }
    let length = usize::try_from(length).map_err(|_| truncated(what))?;
    Ok(RawCell::new(take(bytes, at, length, what)?.to_vec()))
}

/// The entries of a serialised `map`, in wire order — which Cassandra emits sorted by key, so the
/// explosion of a given row is deterministic across runs.
///
/// # Errors
///
/// Returns [`ErrorKind::TypeConversion`] if the buffer is not a well-formed map, which is a
/// record-level failure rather than a fatal one (`ARCHITECTURE.md` §13).
pub(crate) fn map_entries(bytes: &[u8]) -> Result<Vec<(RawCell, RawCell)>, CdmError> {
    let mut at = 0_usize;
    let count = take_i32(bytes, &mut at, "map")?;
    let count = usize::try_from(count).map_err(|_| {
        CdmError::new(
            ErrorKind::TypeConversion,
            "map has a negative element count",
        )
    })?;
    let mut entries = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let key = take_element(bytes, &mut at, "map")?;
        let value = take_element(bytes, &mut at, "map")?;
        entries.push((key, value));
    }
    Ok(entries)
}

/// The elements of a serialised `list`, which is what `WRITETIME(collection)` and
/// `TTL(collection)` return (`FEA-043`).
///
/// # Errors
///
/// As [`map_entries`].
pub(crate) fn list_elements(bytes: &[u8]) -> Result<Vec<RawCell>, CdmError> {
    let mut at = 0_usize;
    let count = take_i32(bytes, &mut at, "list")?;
    let count = usize::try_from(count).map_err(|_| {
        CdmError::new(
            ErrorKind::TypeConversion,
            "list has a negative element count",
        )
    })?;
    let mut elements = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        elements.push(take_element(bytes, &mut at, "list")?);
    }
    Ok(elements)
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

    fn map_bytes(entries: &[(&[u8], Option<&[u8]>)]) -> Vec<u8> {
        let mut out = i32::try_from(entries.len()).unwrap().to_be_bytes().to_vec();
        for (key, value) in entries {
            out.extend_from_slice(&i32::try_from(key.len()).unwrap().to_be_bytes());
            out.extend_from_slice(key);
            match value {
                None => out.extend_from_slice(&(-1_i32).to_be_bytes()),
                Some(value) => {
                    out.extend_from_slice(&i32::try_from(value.len()).unwrap().to_be_bytes());
                    out.extend_from_slice(value);
                }
            }
        }
        out
    }

    #[test]
    fn fea_020_map_framing_decodes_entries_in_wire_order() {
        let bytes = map_bytes(&[(b"a", Some(&[0, 0, 0, 1])), (b"b", None)]);
        let entries = map_entries(&bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, RawCell::new(b"a".to_vec()));
        assert_eq!(entries[0].1, RawCell::new(vec![0, 0, 0, 1]));
        assert_eq!(entries[1].1, RawCell::NULL);
    }

    #[test]
    fn fea_023_an_empty_map_decodes_to_no_entries() {
        assert!(map_entries(&[0, 0, 0, 0]).unwrap().is_empty());
    }

    #[test]
    fn fea_020_a_truncated_map_is_a_record_level_conversion_error() {
        assert_eq!(
            map_entries(&[0, 0, 0]).unwrap_err().kind(),
            ErrorKind::TypeConversion
        );
        assert!(map_entries(&[0, 0, 0, 1, 0, 0, 0, 8, 1]).is_err());
        assert!(map_entries(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn fea_043_list_framing_decodes_writetime_elements() {
        let mut bytes = 2_i32.to_be_bytes().to_vec();
        for value in [7_i64, 9_i64] {
            bytes.extend_from_slice(&8_i32.to_be_bytes());
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        let elements = list_elements(&bytes).unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[1], RawCell::new(9_i64.to_be_bytes().to_vec()));
        assert!(list_elements(&[0, 0, 0, 1]).is_err());
        assert!(list_elements(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }
}
