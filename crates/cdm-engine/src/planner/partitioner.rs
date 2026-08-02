//! Partitioner detection and the default token bounds (`TOK-001`, `TOK-002`).

use std::fmt;

use cdm_core::{CdmError, ErrorKind, TokenRange};
use serde::{Deserialize, Serialize};

/// The partitioner the origin cluster uses to map a partition key onto the ring (`TOK-001`).
///
/// Java CDM reads `system.local.partitioner` and only ever splits a numeric ring; cdm-rs
/// recognises the same three partitioners Cassandra ships and rejects anything else with a
/// message that names what it found and what it supports, rather than planning a ring that does
/// not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Partitioner {
    /// `Murmur3Partitioner` — 64-bit signed tokens, the default since Cassandra 1.2.
    Murmur3,
    /// `RandomPartitioner` — 127-bit unsigned MD5 tokens, from the pre-1.2 era.
    Random,
    /// `ByteOrderedPartitioner` — tokens are the raw key bytes, so the ring has no numeric
    /// geometry to split. Recognised, but a plan needs explicit bounds.
    ByteOrdered,
}

impl Partitioner {
    /// Every partitioner cdm-rs recognises, in declaration order.
    pub const ALL: [Self; 3] = [Self::Murmur3, Self::Random, Self::ByteOrdered];

    /// The fully qualified Java class name, exactly as `system.local.partitioner` reports it.
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Murmur3 => "org.apache.cassandra.dht.Murmur3Partitioner",
            Self::Random => "org.apache.cassandra.dht.RandomPartitioner",
            Self::ByteOrdered => "org.apache.cassandra.dht.ByteOrderedPartitioner",
        }
    }

    /// The class name without its package, as used in configuration and log lines.
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Murmur3 => "Murmur3Partitioner",
            Self::Random => "RandomPartitioner",
            Self::ByteOrdered => "ByteOrderedPartitioner",
        }
    }

    /// Detects the partitioner from the value of `system.local.partitioner` (`TOK-001`).
    ///
    /// Both the fully qualified class name and the bare class name are accepted, in any case and
    /// with surrounding whitespace, because operators paste this value from `nodetool describe
    /// cluster`, from `cqlsh` and from configuration files, which spell it three different ways.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] naming the unrecognised partitioner and listing the
    /// supported ones. cdm-rs never guesses: an unknown partitioner means the token geometry is
    /// unknown, and planning a ring on a guess reads the wrong rows.
    pub fn detect(reported: &str) -> Result<Self, CdmError> {
        let trimmed = reported.trim();
        // Compare on the lowercased class name with the `Partitioner` suffix removed, so that
        // `Murmur3Partitioner`, `murmur3`, and the fully qualified name all land on the same
        // variant.
        let folded = trimmed
            .rsplit('.')
            .next()
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        let bare = folded.trim_end_matches("partitioner");
        for candidate in Self::ALL {
            let expected = candidate.short_name().to_ascii_lowercase();
            if bare == expected.trim_end_matches("partitioner") {
                return Ok(candidate);
            }
        }
        let supported = Self::ALL
            .iter()
            .map(|p| p.class_name())
            .collect::<Vec<_>>()
            .join(", ");
        Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "unsupported origin partitioner `{trimmed}`; cdm-rs supports {supported}. \
                 Set `filter.token.min` and `filter.token.max` explicitly only if you are certain \
                 the ring is numeric."
            ),
        ))
    }

    /// The full token ring of this partitioner (`TOK-002`).
    ///
    /// `Murmur3Partitioner` spans `[i64::MIN, i64::MAX]` and `RandomPartitioner` spans
    /// `[0, 2^127 - 1]`. `ByteOrderedPartitioner` has no numeric ring, so it has no default.
    pub const fn full_ring(self) -> Option<TokenRange> {
        match self {
            Self::Murmur3 => Some(TokenRange::MURMUR3_FULL),
            Self::Random => Some(TokenRange::RANDOM_FULL),
            Self::ByteOrdered => None,
        }
    }

    /// Resolves the segment of the ring to plan, applying `filter.token.min` / `.max` over the
    /// partitioner defaults (`TOK-002`).
    ///
    /// A bound the operator supplies wins over the default, which is exactly Java's precedence.
    /// Unlike Java, a bound outside the partitioner's ring is rejected rather than silently
    /// planned: tokens outside the ring match no row, so such a run would silently migrate less
    /// data than the operator asked for.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if `ByteOrderedPartitioner` is used without both bounds, if
    /// `min > max`, or if either bound falls outside this partitioner's ring.
    pub fn resolve_bounds(
        self,
        min: Option<i128>,
        max: Option<i128>,
    ) -> Result<TokenRange, CdmError> {
        let Some(ring) = self.full_ring() else {
            let (Some(min), Some(max)) = (min, max) else {
                return Err(CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "{} has no numeric token ring, so `filter.token.min` and \
                         `filter.token.max` must both be set to plan a run",
                        self.short_name()
                    ),
                ));
            };
            return TokenRange::new(min, max).map_err(|e| bounds_error(e.message()));
        };

        let min = min.unwrap_or_else(|| ring.min());
        let max = max.unwrap_or_else(|| ring.max());
        if !ring.contains(min) || !ring.contains(max) {
            return Err(bounds_error(&format!(
                "token bounds [{min}, {max}] fall outside the {} ring {ring}",
                self.short_name()
            )));
        }
        TokenRange::new(min, max).map_err(|e| bounds_error(e.message()))
    }
}

/// Wraps a bounds failure in the config key the operator has to change.
fn bounds_error(message: &str) -> CdmError {
    CdmError::new(ErrorKind::Config, message.to_owned())
        .with_context(|ctx| ctx.with_config_key("filter.token"))
}

impl fmt::Display for Partitioner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.short_name())
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

    #[test]
    fn tok_001_every_supported_partitioner_is_detected_by_class_and_short_name() {
        for partitioner in Partitioner::ALL {
            assert_eq!(
                Partitioner::detect(partitioner.class_name()).unwrap(),
                partitioner
            );
            assert_eq!(
                Partitioner::detect(partitioner.short_name()).unwrap(),
                partitioner
            );
        }
        assert_eq!(
            Partitioner::detect("  murmur3partitioner  ").unwrap(),
            Partitioner::Murmur3
        );
        assert_eq!(
            Partitioner::detect("org.apache.cassandra.dht.RandomPartitioner").unwrap(),
            Partitioner::Random
        );
        assert!(Partitioner::Murmur3
            .class_name()
            .starts_with("org.apache.cassandra.dht."));
    }

    #[test]
    fn tok_001_an_unknown_partitioner_is_a_clear_configuration_error() {
        let err = Partitioner::detect("com.example.MyPartitioner").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("com.example.MyPartitioner"));
        assert!(err.message().contains("Murmur3Partitioner"));
        assert!(Partitioner::detect("").is_err());
    }

    #[test]
    fn tok_002_default_bounds_are_the_full_ring_of_the_partitioner() {
        assert_eq!(
            Partitioner::Murmur3.resolve_bounds(None, None).unwrap(),
            TokenRange::MURMUR3_FULL
        );
        assert_eq!(
            Partitioner::Murmur3
                .resolve_bounds(None, None)
                .unwrap()
                .min(),
            i128::from(i64::MIN)
        );
        assert_eq!(
            Partitioner::Random.resolve_bounds(None, None).unwrap(),
            TokenRange::RANDOM_FULL
        );
        assert_eq!(
            Partitioner::Random
                .resolve_bounds(None, None)
                .unwrap()
                .max(),
            i128::MAX
        );
    }

    #[test]
    fn tok_002_filter_bounds_override_the_defaults_one_side_at_a_time() {
        let only_min = Partitioner::Murmur3.resolve_bounds(Some(0), None).unwrap();
        assert_eq!(only_min.min(), 0);
        assert_eq!(only_min.max(), i128::from(i64::MAX));

        let only_max = Partitioner::Murmur3.resolve_bounds(None, Some(0)).unwrap();
        assert_eq!(only_max.min(), i128::from(i64::MIN));
        assert_eq!(only_max.max(), 0);

        let both = Partitioner::Murmur3
            .resolve_bounds(Some(-10), Some(10))
            .unwrap();
        assert_eq!(both, TokenRange::new(-10, 10).unwrap());
    }

    #[test]
    fn tok_002_bounds_outside_the_ring_or_inverted_are_rejected() {
        let outside = Partitioner::Murmur3
            .resolve_bounds(None, Some(i128::from(i64::MAX) + 1))
            .unwrap_err();
        assert_eq!(outside.kind(), ErrorKind::Config);
        assert!(outside.message().contains("outside"));

        let negative_random = Partitioner::Random
            .resolve_bounds(Some(-1), None)
            .unwrap_err();
        assert_eq!(negative_random.kind(), ErrorKind::Config);

        let inverted = Partitioner::Murmur3
            .resolve_bounds(Some(10), Some(9))
            .unwrap_err();
        assert!(inverted.message().contains("inverted"));
        assert_eq!(
            inverted.context().config_key.as_deref(),
            Some("filter.token")
        );
    }

    #[test]
    fn tok_002_byte_ordered_has_no_default_ring_and_demands_explicit_bounds() {
        assert!(Partitioner::ByteOrdered.full_ring().is_none());
        let err = Partitioner::ByteOrdered
            .resolve_bounds(None, None)
            .unwrap_err();
        assert!(err.message().contains("no numeric token ring"));
        assert!(Partitioner::ByteOrdered
            .resolve_bounds(Some(0), None)
            .is_err());
        assert_eq!(
            Partitioner::ByteOrdered
                .resolve_bounds(Some(0), Some(99))
                .unwrap(),
            TokenRange::new(0, 99).unwrap()
        );
    }

    #[test]
    fn tok_001_partitioner_renders_and_round_trips_through_serde() {
        assert_eq!(Partitioner::Murmur3.to_string(), "Murmur3Partitioner");
        let json = serde_json::to_string(&Partitioner::ByteOrdered).unwrap();
        assert_eq!(json, "\"byte_ordered\"");
        assert_eq!(
            serde_json::from_str::<Partitioner>(&json).unwrap(),
            Partitioner::ByteOrdered
        );
    }
}
