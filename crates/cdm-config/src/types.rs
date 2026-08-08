//! Leaf value types used by the configuration model.
//!
//! Each one exists because the property registry needs a [`PropertyKind`] richer than "string":
//! a duration, a token, a UUID or a closed enumeration. Keeping them here means the model file
//! reads as a list of properties rather than a list of type definitions.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::meta::{PropertyKind, PropertyValue};

/// Declares a closed enumeration together with the variant spellings the registry advertises.
macro_rules! config_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[doc = $doc:literal])+
                $variant:ident => $wire:literal,
            )+
        }
        default = $default:ident;
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize, schemars::JsonSchema,
        )]
        pub enum $name {
            $(
                $(#[doc = $doc])+
                #[serde(rename = $wire)]
                $variant,
            )+
        }

        impl $name {
            /// Every accepted spelling, in declaration order.
            pub const VARIANTS: &'static [&'static str] = &[$($wire,)+];

            /// The wire spelling of this variant.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl PropertyValue for $name {
            fn kind() -> PropertyKind {
                PropertyKind::Enum(Self::VARIANTS)
            }
        }
    };
}

config_enum! {
    /// A CQL consistency level (`CFG-160`).
    ///
    /// Unlike Java CDM, an unrecognised value is a Tier-1 error rather than a silent coercion to
    /// `LOCAL_QUORUM` (`CFG-161`); `--compat-java` restores the coercion.
    pub enum ConsistencyLevel {
        /// Any node, including a hinted handoff target. Writes only.
        Any => "ANY",
        /// One replica.
        One => "ONE",
        /// Two replicas.
        Two => "TWO",
        /// Three replicas.
        Three => "THREE",
        /// A quorum of all replicas.
        Quorum => "QUORUM",
        /// One replica in the local datacentre.
        LocalOne => "LOCAL_ONE",
        /// A quorum of the local datacentre's replicas. The default.
        LocalQuorum => "LOCAL_QUORUM",
        /// A quorum in every datacentre.
        EachQuorum => "EACH_QUORUM",
        /// Linearisable consistency, cluster-wide.
        Serial => "SERIAL",
        /// Linearisable consistency within the local datacentre.
        LocalSerial => "LOCAL_SERIAL",
        /// Every replica.
        All => "ALL",
    }
    default = LocalQuorum;
}

config_enum! {
    /// Which secure-connect-bundle the Astra DevOps API should hand back (`CFG-110`).
    pub enum ScbType {
        /// The bundle for the database's primary region.
        Default => "default",
        /// A bundle for a custom domain.
        Custom => "custom",
    }
    default = Default;
}

config_enum! {
    /// How to route CQL traffic to Astra (`CON-022`, `CON-026`).
    pub enum AstraMode {
        /// SNI proxy routing, as the bundle's `config.json` describes it.
        Sni => "sni",
        /// A single endpoint, for deployments that terminate SNI upstream.
        SingleEndpoint => "single_endpoint",
    }
    default = Sni;
}

config_enum! {
    /// The on-disk format of a trust store (`CFG-120`).
    pub enum TrustStoreType {
        /// A Java key store.
        Jks => "JKS",
        /// A PKCS#12 store.
        Pkcs12 => "PKCS12",
        /// PEM-encoded certificates.
        Pem => "PEM",
    }
    default = Jks;
}

config_enum! {
    /// How the HTTP control plane authenticates callers (`SEC-010`).
    pub enum AuthMode {
        /// No authentication. Only safe on a loopback bind.
        None => "none",
        /// A bearer token in the `Authorization` header.
        Bearer => "bearer",
        /// Mutual TLS.
        Mtls => "mtls",
    }
    default = None;
}

config_enum! {
    /// What an inline guardrail violation does to the row that caused it (`GRD-004`).
    ///
    /// The column-size guardrail reports; it does not filter. So the default changes nothing about
    /// what a run writes: `check` counts the row `LARGE` and carries on, which is the only
    /// behaviour Java has — its guardrail is a job of its own and never runs alongside a migration.
    /// `warn` is the same judgement said more quietly, for a run that wants the finding in the log
    /// without an `ERROR` line per row. `block` is the one mode that changes the outcome: the row
    /// is counted `LARGE` and *not* written, which is what an operator wants when the target
    /// enforces a column-size limit and a migration that skips the handful of rows over it is far
    /// more useful than one that fails on them.
    pub enum GuardrailMode {
        /// Count the row `LARGE`, report it at `ERROR`, and process it as normal. Java's behaviour.
        Check => "check",
        /// As `check`, but the finding is reported at `WARN`.
        Warn => "warn",
        /// Count the row `LARGE` and skip it: nothing is written for it (`GRD-004`).
        Block => "block",
    }
    default = Check;
}

config_enum! {
    /// Where run events are written (`MET-030`).
    pub enum EventSink {
        /// Events are not emitted.
        None => "none",
        /// One JSON object per line on standard output.
        StdoutJson => "stdout_json",
        /// One JSON object per line in a file.
        File => "file",
    }
    default = None;
}

config_enum! {
    /// The shape of the validate discrepancy report (`VAL-013`).
    ///
    /// Three shapes for three readers. `ndjson` is the one to reach for: it is written a record at
    /// a time, so a run that is killed leaves a file that is still readable up to the last complete
    /// line, and every log pipeline already parses it. `json` is one array — friendlier to a
    /// small consumer that wants `serde_json::from_reader`, at the price of being unreadable if
    /// the run does not reach the closing bracket. `csv` is for a spreadsheet, which is what a
    /// discrepancy report is opened in more often than anyone likes to admit.
    ///
    /// Parquet is named by `docs/SPEC.md` and is deliberately not offered; the reasoning is
    /// recorded there, under `VAL-013`.
    pub enum ReportFormat {
        /// No report is written. The default: a report is an export, and an export happens when
        /// somebody asks for it.
        None => "none",
        /// One JSON array of discrepancy records.
        Json => "json",
        /// One JSON discrepancy record per line.
        Ndjson => "ndjson",
        /// A header row and one row per differing column.
        Csv => "csv",
    }
    default = None;
}

config_enum! {
    /// How rows are grouped into a write batch (`MIG-022`).
    ///
    /// A Cassandra `UNLOGGED` batch that spans partitions is not an optimisation: the coordinator
    /// fans it out to every replica set involved and the batch finishes no sooner than its slowest
    /// participant. The only batch that is faster than the individual writes it replaces is a
    /// single-partition one, which the coordinator applies as a single mutation.
    pub enum BatchGrouping {
        /// Rows belonging to different partitions are never batched together.
        Strict => "strict",
        /// Java's behaviour: rows are appended in the order they are read, whatever partition they
        /// belong to, and the batch is sent once it reaches `perfops.batch_size`.
        Legacy => "legacy",
    }
    default = Strict;
}

config_enum! {
    /// How the token ring is divided into ranges (`TOK-003`, `TOK-008`, `TOK-010`).
    ///
    /// The strategy lives here rather than in `cdm-engine` because it is a *configuration* value:
    /// the JSON Schema, `docs/generated/PROPERTIES.md` and the config-builder form are all
    /// projections of this registry (`CFG-001`), and a second copy of the spellings in the
    /// planner would be a second place for them to drift. `cdm-engine::planner` re-exports this
    /// type and adds the planning behaviour.
    pub enum PlanStrategy {
        /// Java CDM's splitter, reproduced exactly. The default, and the only `[P]` strategy.
        Fixed => "fixed",
        /// Split along ring-ownership boundaries, so every range maps to a single replica set and
        /// the reads for it can be routed with no coordinator hop (`TOK-008`).
        RingAware => "ring_aware",
        /// Start from `fixed` and subdivide any range whose estimated row count exceeds
        /// `plan.max_rows_per_range`, so a hot range does not become the straggler that sets the
        /// wall clock (`TOK-010`).
        Adaptive => "adaptive",
    }
    default = Fixed;
}

impl PlanStrategy {
    /// Every strategy, in declaration order.
    ///
    /// [`PlanStrategy::VARIANTS`] gives the same list as wire spellings; this one gives the
    /// values, which is what a parser and an exhaustiveness test need.
    pub const ALL: [Self; 3] = [Self::Fixed, Self::RingAware, Self::Adaptive];

    /// Whether this strategy needs origin cluster metadata before it can plan.
    ///
    /// `fixed` is pure geometry and plans with nothing at all; the other two consult the ring
    /// (`TOK-008`) or `system.size_estimates` (`TOK-010`).
    #[must_use]
    pub const fn needs_topology(self) -> bool {
        !matches!(self, Self::Fixed)
    }
}

impl std::str::FromStr for PlanStrategy {
    type Err = cdm_core::CdmError;

    /// Parses a strategy, accepting `-` for `_` and any case, as the CLI's `--set` does.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalised = value.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == normalised)
            .ok_or_else(|| {
                cdm_core::CdmError::new(
                    cdm_core::ErrorKind::Config,
                    format!(
                        "unknown plan strategy `{value}`; expected one of fixed, ring_aware, \
                         adaptive"
                    ),
                )
                .with_context(|ctx| ctx.with_config_key("plan.strategy"))
            })
    }
}

config_enum! {
    /// The shape of log records.
    pub enum LogFormat {
        /// Human-readable, multi-line, coloured when the terminal supports it.
        Pretty => "pretty",
        /// One JSON object per record.
        Json => "json",
        /// Human-readable, one line per record.
        Compact => "compact",
    }
    default = Pretty;
}

/// A duration written the way an operator says it: `100ms`, `30s`, `5m`, `1h30m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationSetting(#[serde(with = "humantime_serde")] Duration);

impl DurationSetting {
    /// Builds a setting from a whole number of seconds.
    pub const fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs))
    }

    /// Builds a setting from a whole number of milliseconds.
    pub const fn from_millis(millis: u64) -> Self {
        Self(Duration::from_millis(millis))
    }

    /// The underlying duration.
    pub const fn get(self) -> Duration {
        self.0
    }
}

impl From<DurationSetting> for Duration {
    fn from(value: DurationSetting) -> Self {
        value.0
    }
}

impl fmt::Display for DurationSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Round-trips through the same formatter `humantime_serde` uses, so the documented
        // default and the accepted input spelling are the same string.
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(text)) => f.write_str(&text),
            _ => write!(f, "{:?}", self.0),
        }
    }
}

impl schemars::JsonSchema for DurationSetting {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Duration".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "duration",
            "description": "A duration such as `100ms`, `30s`, `5m` or `1h30m`.",
            "examples": ["30s", "5m"],
        })
    }
}

impl PropertyValue for DurationSetting {
    fn kind() -> PropertyKind {
        PropertyKind::Duration
    }
}

/// An Astra database identifier (`CFG-110`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AstraDatabaseId(uuid::Uuid);

impl AstraDatabaseId {
    /// Wraps a UUID.
    pub const fn new(id: uuid::Uuid) -> Self {
        Self(id)
    }

    /// The underlying UUID.
    pub const fn get(self) -> uuid::Uuid {
        self.0
    }
}

impl fmt::Display for AstraDatabaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl schemars::JsonSchema for AstraDatabaseId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AstraDatabaseId".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "uuid",
            "description": "The Astra database UUID, as shown in the Astra console.",
        })
    }
}

impl PropertyValue for AstraDatabaseId {
    fn kind() -> PropertyKind {
        PropertyKind::Uuid
    }
}

/// A partitioner token bound (`CFG-180`).
///
/// Tokens are 128-bit because `RandomPartitioner` produces values up to 2¹²⁷, which JSON cannot
/// represent as a number. The value therefore travels as a decimal *string* in JSON and in the
/// generated schema, while the Rust type stays a real `i128`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenBound(i128);

impl TokenBound {
    /// Wraps a token value.
    pub const fn new(value: i128) -> Self {
        Self(value)
    }

    /// The token value.
    pub const fn get(self) -> i128 {
        self.0
    }
}

impl fmt::Display for TokenBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for TokenBound {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for TokenBound {
    /// Accepts a JSON string or, for the convenience of hand-written TOML and YAML that stays
    /// inside 64 bits, a JSON number.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Number(i64),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Text(text) => text
                .trim()
                .parse::<i128>()
                .map(Self)
                .map_err(|_| serde::de::Error::custom(format!("`{text}` is not a token value"))),
            Raw::Number(n) => Ok(Self(i128::from(n))),
        }
    }
}

impl schemars::JsonSchema for TokenBound {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TokenBound".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^-?[0-9]+$",
            "description":
                "A partitioner token, as a decimal string. Murmur3 tokens span \
                 -2^63..2^63-1; RandomPartitioner tokens span 0..2^127.",
        })
    }
}

impl PropertyValue for TokenBound {
    fn kind() -> PropertyKind {
        PropertyKind::BigInteger
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
    fn cfg_161_every_consistency_level_of_the_spec_is_accepted_case_insensitively() {
        let expected = [
            "ANY",
            "ONE",
            "TWO",
            "THREE",
            "QUORUM",
            "LOCAL_ONE",
            "LOCAL_QUORUM",
            "EACH_QUORUM",
            "SERIAL",
            "LOCAL_SERIAL",
            "ALL",
        ];
        let mut actual = ConsistencyLevel::VARIANTS.to_vec();
        actual.sort_unstable();
        let mut expected_sorted = expected.to_vec();
        expected_sorted.sort_unstable();
        assert_eq!(actual, expected_sorted);
        assert_eq!(ConsistencyLevel::default(), ConsistencyLevel::LocalQuorum);

        let kind = <ConsistencyLevel as PropertyValue>::kind();
        assert_eq!(
            kind.coerce("local_quorum").unwrap(),
            serde_json::json!("LOCAL_QUORUM")
        );
        assert!(kind.coerce("LOCAL_QUOROM").is_err());
    }

    #[test]
    fn cfg_200_durations_round_trip_through_their_operator_spelling() {
        let five_minutes = DurationSetting::from_secs(300);
        assert_eq!(five_minutes.to_string(), "5m");
        assert_eq!(DurationSetting::from_millis(100).to_string(), "100ms");
        assert_eq!(five_minutes.get(), Duration::from_secs(300));
        assert_eq!(Duration::from(five_minutes), Duration::from_secs(300));
        let parsed: DurationSetting = serde_json::from_str("\"5m\"").unwrap();
        assert_eq!(parsed, five_minutes);
    }

    #[test]
    fn cfg_180_token_bounds_survive_values_that_json_numbers_cannot_hold() {
        let big = TokenBound::new(170_141_183_460_469_231_731_687_303_715_884_105_727_i128);
        let json = serde_json::to_string(&big).unwrap();
        assert_eq!(json, "\"170141183460469231731687303715884105727\"");
        assert_eq!(serde_json::from_str::<TokenBound>(&json).unwrap(), big);
        // A plain JSON number is still accepted for hand-written files.
        assert_eq!(
            serde_json::from_str::<TokenBound>("-9223372036854775808")
                .unwrap()
                .get(),
            i128::from(i64::MIN)
        );
        assert!(serde_json::from_str::<TokenBound>("\"x\"").is_err());
        assert_eq!(big.to_string().len(), 39);
    }

    #[test]
    fn cfg_110_astra_identifiers_are_uuids() {
        let id: AstraDatabaseId =
            serde_json::from_str("\"1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d\"").unwrap();
        assert_eq!(id.to_string(), "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d");
        assert_eq!(id.get().to_string(), id.to_string());
        assert!(serde_json::from_str::<AstraDatabaseId>("\"not-a-uuid\"").is_err());
        assert_eq!(
            <AstraDatabaseId as PropertyValue>::kind(),
            PropertyKind::Uuid
        );
    }

    #[test]
    fn cfg_200_the_remaining_enumerations_default_as_the_spec_says() {
        assert_eq!(ScbType::default().as_str(), "default");
        assert_eq!(AstraMode::default().as_str(), "sni");
        assert_eq!(TrustStoreType::default().as_str(), "JKS");
        assert_eq!(AuthMode::default().as_str(), "none");
        assert_eq!(EventSink::default().as_str(), "none");
        assert_eq!(LogFormat::default().as_str(), "pretty");
        assert_eq!(LogFormat::Compact.to_string(), "compact");
        assert_eq!(EventSink::VARIANTS, ["none", "stdout_json", "file"]);
    }
}
