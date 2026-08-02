//! The one typed configuration model (`CFG-001`, `CFG-100`..`CFG-200`).
//!
//! This file **is** the property registry. Every legacy `spark.cdm.*` name cdm-rs accepts, every
//! default it applies, every unit, every secret and every line of property documentation is
//! written here once and nowhere else; the JSON Schema, `docs/generated/PROPERTIES.md`, the
//! `.properties` alias table and the config-builder UI form are all projections of it
//! (`CFG-001`).
//!
//! The section headings below map one-to-one onto `docs/SPEC.md` §3.5, and the requirement ID in
//! each struct's documentation is the row of the parity table it implements.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use crate::secret::Secret;
use crate::types::{
    AstraDatabaseId, AstraMode, AuthMode, ConsistencyLevel, DurationSetting, EventSink, LogFormat,
    ScbType, TokenBound, TrustStoreType,
};

cdm_properties! {
    /// The complete configuration of a cdm-rs run (`CFG-001`).
    ///
    /// Construct one with [`ConfigLoader`](crate::ConfigLoader) rather than by hand: only the
    /// loader applies the layered precedence of `CFG-010`, resolves secret indirection
    /// (`CFG-012`) and reports the diagnostics of `CFG-021`.
    pub struct CdmConfig {
        sections {
            /// How to reach the origin and target clusters (`CFG-100`).
            #[cdm()]
            pub connect: Connect,

            /// Which tables and columns take part in the run (`CFG-130`).
            #[cdm()]
            pub schema: SchemaSettings,

            /// What the validate job repairs when it finds a difference (`CFG-140`).
            #[cdm()]
            pub autocorrect: Autocorrect,

            /// Run tracking, resume and rerun (`CFG-150`).
            #[cdm()]
            pub track_run: TrackRun,

            /// Throughput, concurrency and reliability tuning (`CFG-160`).
            #[cdm()]
            pub perfops: PerfOps,

            /// Changes applied to values on their way to the target (`CFG-170`).
            #[cdm()]
            pub transform: Transform,

            /// Which rows take part in the run (`CFG-180`).
            #[cdm()]
            pub filter: Filter,

            /// Optional behaviours (`CFG-190`).
            #[cdm()]
            pub feature: Feature,

            /// The HTTP control plane (`CFG-200`).
            #[cdm()]
            pub server: Server,

            /// Metrics and event export (`CFG-200`).
            #[cdm()]
            pub metrics: Metrics,

            /// Distributed mode (`CFG-200`).
            #[cdm()]
            pub cluster: Cluster,

            /// Logging (`CFG-200`).
            #[cdm()]
            pub logging: Logging
        }
    }
}

// =================================================================================================
// §3.5.1 Connection — CFG-100, and §3.5.2/§3.5.3 which hang off each side
// =================================================================================================

cdm_properties! {
    /// The origin and target connections (`CFG-100`).
    pub struct Connect {
        sections {
            /// The cluster data is read from.
            #[cdm(side = "origin")]
            pub origin: SideConnect,

            /// The cluster data is written to.
            #[cdm(side = "target")]
            pub target: SideConnect
        }
    }
}

cdm_properties! {
    /// One side's connection (`CFG-100`).
    ///
    /// Declared once and instantiated twice; the `{side}` placeholder in each legacy alias is
    /// substituted with `origin` or `target` when the registry is built.
    pub struct SideConnect {
        fields {
            /// Contact point host name or IP address.
            ///
            /// This is the normal way to reach Apache Cassandra, DSE, HCD and ScyllaDB. Either
            /// this or [`scb`](SideConnect::scb) must be set for the side, and setting both is an
            /// error (`CFG-024`, `CFG-041`).
            #[cdm(legacy = ["spark.cdm.connect.{side}.host"], example = "10.0.0.1")]
            pub host: String = "localhost".to_owned(),

            /// Native transport port.
            #[cdm(legacy = ["spark.cdm.connect.{side}.port"])]
            pub port: u16 = 9042,

            /// Path to an Astra DB secure-connect-bundle zip.
            ///
            /// **Astra DB only.** Self-managed Apache Cassandra, DSE, HCD and ScyllaDB use
            /// [`host`](SideConnect::host) and [`port`](SideConnect::port); a cluster with client
            /// encryption uses the `tls` section, which is a separate mechanism and not a bundle.
            ///
            /// Set this or [`host`](SideConnect::host) for the side, never both (`CFG-041`). For
            /// Astra you can skip the file entirely and set
            /// [`astra.database_id`](Astra::database_id) instead, which downloads the bundle
            /// through the DevOps API (`CON-004`).
            #[cdm(
                legacy = ["spark.cdm.connect.{side}.scb"],
                example = "file:///home/cdm/secure-connect-db.zip",
            )]
            pub scb: Option<PathBuf> = None,

            /// Username, or the literal `token` when authenticating to Astra.
            #[cdm(legacy = ["spark.cdm.connect.{side}.username"])]
            pub username: String = "cassandra".to_owned(),

            /// Password, or the Astra token.
            ///
            /// Supports `env:VAR`, `file:/path` and `exec:command` indirection (`CFG-012`).
            #[cdm(legacy = ["spark.cdm.connect.{side}.password"], secret = true)]
            pub password: Secret<String> = Secret::new("cassandra"),
        }
        sections {
            /// Astra DevOps API settings for downloading the bundle (`CFG-110`).
            #[cdm()]
            pub astra: Astra,

            /// TLS settings for a self-managed cluster with client encryption (`CFG-120`).
            ///
            /// This is how you reach an OSS Cassandra, DSE or HCD cluster over TLS — truststore,
            /// keystore and cipher suites. It is unrelated to the Astra secure-connect-bundle.
            #[cdm()]
            pub tls: Tls
        }
    }
}

cdm_properties! {
    /// Astra DevOps / secure-connect-bundle auto-download (`CFG-110`).
    ///
    /// **Astra DB only.** Every property in this section is ignored for self-managed Apache
    /// Cassandra, DSE, HCD and ScyllaDB.
    pub struct Astra {
        fields {
            /// The Astra database UUID, used to download a bundle through the DevOps API.
            #[cdm(legacy = ["spark.cdm.connect.{side}.astra.database.id"])]
            pub database_id: Option<AstraDatabaseId> = None,

            /// Whether to download the standard bundle or a custom-domain one.
            #[cdm(key = "scb_type", legacy = ["spark.cdm.connect.{side}.astra.scb.type"])]
            pub scb_type: ScbType = ScbType::Default,

            /// The Astra region whose bundle to download, for multi-region databases.
            #[cdm(legacy = ["spark.cdm.connect.{side}.astra.scb.region"], example = "us-east1")]
            pub region: Option<String> = None,

            /// The custom domain the bundle should be issued for.
            #[cdm(legacy = ["spark.cdm.connect.{side}.astra.scb.custom.domain"])]
            pub custom_domain: Option<String> = None,

            /// How CQL traffic reaches Astra (`CON-022`, `CON-026`).
            ///
            /// `sni` follows the bundle's `config.json`; `single_endpoint` is for deployments
            /// that terminate SNI upstream.
            #[cdm(stability = experimental,)]
            pub mode: AstraMode = AstraMode::Sni,

            /// How often to refresh Astra control-connection metadata (`CON-025`).
            #[cdm(unit = "duration",)]
            pub metadata_refresh_interval: DurationSetting = DurationSetting::from_secs(300),
        }
    }
}

cdm_properties! {
    /// TLS settings for one side (`CFG-120`).
    pub struct Tls {
        fields {
            /// Whether to use TLS.
            ///
            /// When enabled and no secure-connect-bundle is configured, the trust store, key
            /// store and cipher suites must all be supplied (`CFG-025`).
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.enabled"])]
            pub enabled: bool = false,

            /// Cipher suites offered during the handshake.
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.enabledAlgorithms"])]
            pub cipher_suites: Vec<String> = vec![
                "TLS_RSA_WITH_AES_128_CBC_SHA".to_owned(),
                "TLS_RSA_WITH_AES_256_CBC_SHA".to_owned(),
            ],

            /// Whether this side is Astra, which implies its own trust material.
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.isAstra"])]
            pub is_astra: bool = false,
        }
        sections {
            /// The certificates this side will trust.
            #[cdm()]
            pub truststore: TrustStore,

            /// The client certificate this side presents.
            #[cdm()]
            pub keystore: KeyStore
        }
    }
}

cdm_properties! {
    /// The trust store of one side (`CFG-120`).
    pub struct TrustStore {
        fields {
            /// Path to the trust store.
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.trustStore.path"])]
            pub path: Option<PathBuf> = None,

            /// Password protecting the trust store.
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.trustStore.password"], secret = true)]
            pub password: Option<Secret<String>> = None,

            /// The store's on-disk format.
            #[cdm(key = "type", legacy = ["spark.cdm.connect.{side}.tls.trustStore.type"])]
            pub store_type: TrustStoreType = TrustStoreType::Jks,
        }
    }
}

cdm_properties! {
    /// The key store of one side (`CFG-120`).
    pub struct KeyStore {
        fields {
            /// Path to the key store holding the client certificate.
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.keyStore.path"])]
            pub path: Option<PathBuf> = None,

            /// Password protecting the key store.
            #[cdm(legacy = ["spark.cdm.connect.{side}.tls.keyStore.password"], secret = true)]
            pub password: Option<Secret<String>> = None,
        }
    }
}

// =================================================================================================
// §3.5.4 Schema — CFG-130
// =================================================================================================

cdm_properties! {
    /// Which tables and columns take part in the run (`CFG-130`).
    pub struct SchemaSettings {
        sections {
            /// The origin table and its column handling.
            #[cdm()]
            pub origin: OriginSchema,

            /// The target table.
            #[cdm()]
            pub target: TargetSchema,

            /// How TTL and writetime are calculated.
            #[cdm()]
            pub ttl_writetime: TtlWritetime
        }
    }
}

cdm_properties! {
    /// The origin side of the schema (`CFG-130`).
    pub struct OriginSchema {
        fields {
            /// The origin table, as `keyspace.table`.
            ///
            /// The only unconditionally required property (`CFG-022`).
            #[cdm(
                legacy = ["spark.cdm.schema.origin.keyspaceTable"],
                example = "my_ks.my_table",
            )]
            pub keyspace_table: Option<String> = None,
        }
        sections {
            /// Which columns supply the TTL of a migrated row.
            #[cdm()]
            pub ttl: TtlColumns,

            /// Which columns supply the writetime of a migrated row.
            #[cdm()]
            pub writetime: WritetimeColumns,

            /// Column-level mapping.
            #[cdm()]
            pub column: OriginColumns
        }
    }
}

cdm_properties! {
    /// TTL source columns (`CFG-130`).
    pub struct TtlColumns {
        fields {
            /// Use every eligible non-key column as a TTL source.
            ///
            /// Naming columns explicitly disables this (`CFG-037`).
            #[cdm(legacy = ["spark.cdm.schema.origin.column.ttl.automatic"])]
            pub automatic: bool = true,

            /// The columns whose TTL to read, largest wins.
            #[cdm(legacy = ["spark.cdm.schema.origin.column.ttl.names"], example = "data,notes")]
            pub names: Vec<String> = Vec::new(),
        }
    }
}

cdm_properties! {
    /// Writetime source columns (`CFG-130`).
    pub struct WritetimeColumns {
        fields {
            /// Use every eligible non-key column as a writetime source.
            ///
            /// Naming columns explicitly disables this (`CFG-037`).
            #[cdm(legacy = ["spark.cdm.schema.origin.column.writetime.automatic"])]
            pub automatic: bool = true,

            /// The columns whose writetime to read, largest wins.
            #[cdm(
                legacy = ["spark.cdm.schema.origin.column.writetime.names"],
                example = "data,notes",
            )]
            pub names: Vec<String> = Vec::new(),
        }
    }
}

cdm_properties! {
    /// Column selection and renaming on the origin side (`CFG-130`).
    pub struct OriginColumns {
        fields {
            /// Origin columns to leave out of the run.
            #[cdm(legacy = ["spark.cdm.schema.origin.column.skip"])]
            pub skip: Vec<String> = Vec::new(),

            /// Column renames, each written `origin_name:target_name` (`CFG-038`).
            #[cdm(
                key = "rename",
                legacy = ["spark.cdm.schema.origin.column.names.to.target"],
                example = "id:new_id,name:full_name",
            )]
            pub rename: Vec<String> = Vec::new(),
        }
    }
}

cdm_properties! {
    /// The target side of the schema (`CFG-130`).
    pub struct TargetSchema {
        fields {
            /// The target table, as `keyspace.table`. Defaults to the origin table (`CFG-023`).
            #[cdm(
                legacy = ["spark.cdm.schema.target.keyspaceTable"],
                default_note = "the origin keyspace and table",
                example = "my_ks.my_table_v2",
            )]
            pub keyspace_table: Option<String> = None,
        }
    }
}

cdm_properties! {
    /// How TTL and writetime are calculated (`CFG-130`).
    pub struct TtlWritetime {
        fields {
            /// Allow collection columns to contribute TTL and writetime.
            ///
            /// Necessary when every non-key column is a collection, since Cassandra reports no
            /// row-level TTL or writetime in that case.
            #[cdm(legacy = ["spark.cdm.schema.ttlwritetime.calc.useCollections"])]
            pub use_collections: bool = false,
        }
    }
}

// =================================================================================================
// §3.5.5 Autocorrect — CFG-140
// =================================================================================================

cdm_properties! {
    /// What the validate job repairs when it finds a difference (`CFG-140`).
    pub struct Autocorrect {
        fields {
            /// Write rows that are present on origin and absent on target.
            #[cdm(legacy = ["spark.cdm.autocorrect.missing"])]
            pub missing: bool = false,

            /// Overwrite target rows whose values differ from origin.
            #[cdm(legacy = ["spark.cdm.autocorrect.mismatch"])]
            pub mismatch: bool = false,

            /// Write missing rows even for a counter table.
            ///
            /// Dangerous: re-inserting a deleted counter row double-counts it. Off by default.
            #[cdm(legacy = ["spark.cdm.autocorrect.missing.counter"])]
            pub missing_counter: bool = false,
        }
    }
}

// =================================================================================================
// §3.5.6 Run tracking — CFG-150
// =================================================================================================

cdm_properties! {
    /// Run tracking, resume and rerun (`CFG-150`).
    pub struct TrackRun {
        fields {
            /// Record each token range's outcome so the run can be resumed.
            #[cdm(legacy = ["spark.cdm.trackRun"])]
            pub enabled: bool = false,

            /// The identifier to record this run under. `0` allocates a new one.
            #[cdm(legacy = ["spark.cdm.trackRun.runId"])]
            pub run_id: i64 = 0,

            /// The run to resume. `0` starts from the beginning.
            #[cdm(legacy = ["spark.cdm.trackRun.previousRunId"])]
            pub previous_run_id: i64 = 0,

            /// Automatically rerun the ranges the previous run did not complete.
            #[cdm(legacy = ["spark.cdm.trackRun.autoRerun"])]
            pub auto_rerun: bool = false,

            /// How many times to subdivide a range that is rerun.
            #[cdm(legacy = ["spark.cdm.trackRun.rerunMultiplier"])]
            pub rerun_multiplier: u32 = 1,
        }
    }
}

// =================================================================================================
// §3.5.7 Performance and operations — CFG-160
// =================================================================================================

cdm_properties! {
    /// Throughput, concurrency and reliability tuning (`CFG-160`).
    pub struct PerfOps {
        fields {
            /// Number of token-range splits the ring is divided into.
            ///
            /// Rule of thumb: one part per 10 MB of table data.
            #[cdm(legacy = ["spark.cdm.perfops.numParts"], unit = "parts", example = "10000")]
            pub num_parts: u64 = 5000,

            /// Rows per write batch.
            ///
            /// Coerced to 1 for counter tables and when a writetime filter is active
            /// (`CFG-040`, `MIG-021`).
            #[cdm(legacy = ["spark.cdm.perfops.batchSize"], unit = "rows")]
            pub batch_size: u32 = 5,

            /// Rows read from the origin per page.
            #[cdm(legacy = ["spark.cdm.perfops.fetchSizeInRows"], unit = "rows")]
            pub fetch_size: u32 = 1000,

            /// Stop the run after this many row-level errors. `0` means never stop.
            #[cdm(legacy = ["spark.cdm.perfops.errorLimit"], unit = "errors")]
            pub error_limit: u64 = 0,

            /// Concurrent range workers.
            #[cdm(unit = "workers", default_note = "num_cpus", example = "16",)]
            pub workers: Option<u32> = None,

            /// Maximum write requests in flight across all workers.
            #[cdm(unit = "requests",)]
            pub max_inflight_writes: u32 = 2000,

            /// Maximum read requests in flight across all workers.
            #[cdm(unit = "requests",)]
            pub max_inflight_reads: u32 = 256,

            /// Per-request timeout.
            #[cdm(unit = "duration",)]
            pub request_timeout: DurationSetting = DurationSetting::from_secs(30),

            /// Connections per host, per side.
            #[cdm(unit = "connections",)]
            pub connection_pool_size: u32 = 4,

            /// Reduce the rate limit automatically when the target signals overload.
            #[cdm(stability = experimental,)]
            pub adaptive_ratelimit: bool = false,
        }
        sections {
            /// Rate limits, per side.
            #[cdm()]
            pub ratelimit: RateLimit,

            /// Consistency levels.
            #[cdm()]
            pub consistency: Consistency,

            /// Retry policy for transient failures.
            #[cdm()]
            pub retry: Retry
        }
    }
}

cdm_properties! {
    /// Rate limits (`CFG-160`).
    pub struct RateLimit {
        fields {
            /// Rows read per second, per cdm-rs process.
            #[cdm(legacy = ["spark.cdm.perfops.ratelimit.origin"], unit = "rows/s")]
            pub origin: u32 = 20_000,

            /// Rows written per second, per cdm-rs process.
            #[cdm(legacy = ["spark.cdm.perfops.ratelimit.target"], unit = "rows/s")]
            pub target: u32 = 20_000,
        }
    }
}

cdm_properties! {
    /// Consistency levels (`CFG-160`, `CFG-161`).
    pub struct Consistency {
        fields {
            /// Consistency level for reads from the origin.
            #[cdm(legacy = ["spark.cdm.perfops.consistency.read"])]
            pub read: ConsistencyLevel = ConsistencyLevel::LocalQuorum,

            /// Consistency level for writes to the target.
            #[cdm(legacy = ["spark.cdm.perfops.consistency.write"])]
            pub write: ConsistencyLevel = ConsistencyLevel::LocalQuorum,
        }
    }
}

cdm_properties! {
    /// Retry policy for transient failures (`CFG-160`).
    pub struct Retry {
        fields {
            /// How many times a failed request is retried before the range fails.
            #[cdm(unit = "attempts",)]
            pub max_attempts: u32 = 5,

            /// Delay before the first retry; doubles each attempt.
            #[cdm(unit = "duration",)]
            pub initial_backoff: DurationSetting = DurationSetting::from_millis(100),

            /// Ceiling on the exponential backoff.
            #[cdm(unit = "duration",)]
            pub max_backoff: DurationSetting = DurationSetting::from_secs(10),
        }
    }
}

// =================================================================================================
// §3.5.8 Transformations — CFG-170
// =================================================================================================

cdm_properties! {
    /// Changes applied to values on their way to the target (`CFG-170`).
    pub struct Transform {
        fields {
            /// Timestamp, in epoch milliseconds, substituted for a null key column.
            ///
            /// Partition and clustering columns cannot be null; when a schema change introduces
            /// a key column the origin does not have, this supplies a constant in its place.
            #[cdm(
                legacy = ["spark.cdm.transform.missing.key.ts.replace.value"],
                unit = "epoch ms",
            )]
            pub missing_key_ts_replace: Option<i64> = None,

            /// Writetime, in microseconds, to use instead of the origin's. `0` disables.
            #[cdm(legacy = ["spark.cdm.transform.custom.writetime"], unit = "µs")]
            pub custom_writetime: i64 = 0,

            /// Microseconds added to every writetime (`CFG-033`, `CFG-039`).
            #[cdm(legacy = ["spark.cdm.transform.custom.writetime.incrementBy"], unit = "µs")]
            pub custom_writetime_increment: i64 = 0,

            /// TTL, in seconds, to use instead of the origin's. `0` disables.
            #[cdm(legacy = ["spark.cdm.transform.custom.ttl"], unit = "s")]
            pub custom_ttl: i32 = 0,

            /// Codecs to enable, by name.
            #[cdm(
                legacy = ["spark.cdm.transform.codecs"],
                example = "INT_STRING,TIMESTAMP_STRING_MILLIS",
            )]
            pub codecs: Vec<String> = Vec::new(),

            /// The `java.time` pattern the `TIMESTAMP_STRING_FORMAT` codec parses and prints.
            ///
            /// Canonically `transform.codec_timestamp_format`. `docs/SPEC.md` §3.5.8 names this
            /// `transform.codecs.timestamp_format`, which cannot coexist with the
            /// `transform.codecs` list in a struct tree; the legacy alias is unaffected.
            #[cdm(legacy = ["spark.cdm.transform.codecs.timestamp.string.format"])]
            pub codec_timestamp_format: String = "yyyyMMddHHmmss".to_owned(),

            /// The IANA time zone the `TIMESTAMP_STRING_FORMAT` codec assumes.
            ///
            /// Canonically `transform.codec_timestamp_zone`, for the reason given on
            /// [`codec_timestamp_format`](Transform::codec_timestamp_format).
            #[cdm(legacy = ["spark.cdm.transform.codecs.timestamp.string.zone"])]
            pub codec_timestamp_zone: String = "UTC".to_owned(),

            /// Drop map entries whose value is null instead of writing them.
            #[cdm(legacy = ["spark.cdm.transform.map.remove.null.value"])]
            pub map_remove_null_value: bool = false,
        }
    }
}

// =================================================================================================
// §3.5.9 Filters — CFG-180
// =================================================================================================

cdm_properties! {
    /// Which rows take part in the run (`CFG-180`).
    pub struct Filter {
        fields {
            /// A CQL predicate appended to the origin `SELECT`, without the `WHERE` keyword.
            #[cdm(
                legacy = ["spark.cdm.filter.cassandra.whereCondition"],
                example = "status = 'active'",
            )]
            pub cql_where: Option<String> = None,

            /// Percentage of each token range to process, for sampling runs.
            #[cdm(legacy = ["spark.cdm.filter.java.token.percent"], unit = "%")]
            pub token_coverage_percent: u8 = 100,
        }
        sections {
            /// The slice of the token ring to process.
            #[cdm()]
            pub token: TokenFilter,

            /// A writetime window (`CFG-032`, `CFG-034`).
            #[cdm()]
            pub writetime: WritetimeFilter,

            /// A single-column equality filter applied in cdm-rs, not in CQL.
            #[cdm()]
            pub column: ColumnFilter
        }
    }
}

cdm_properties! {
    /// The slice of the token ring to process (`CFG-180`).
    pub struct TokenFilter {
        fields {
            /// Lowest token to process. Defaults to the partitioner's minimum.
            #[cdm(
                legacy = ["spark.cdm.filter.cassandra.partition.min"],
                default_note = "the partitioner minimum",
                example = "-9223372036854775808",
            )]
            pub min: Option<TokenBound> = None,

            /// Highest token to process. Defaults to the partitioner's maximum.
            #[cdm(
                legacy = ["spark.cdm.filter.cassandra.partition.max"],
                default_note = "the partitioner maximum",
                example = "9223372036854775807",
            )]
            pub max: Option<TokenBound> = None,
        }
    }
}

cdm_properties! {
    /// A writetime window (`CFG-180`).
    pub struct WritetimeFilter {
        fields {
            /// Skip rows whose writetime is below this, in microseconds.
            #[cdm(legacy = ["spark.cdm.filter.java.writetime.min"], unit = "µs")]
            pub min: Option<i64> = None,

            /// Skip rows whose writetime is above this, in microseconds.
            #[cdm(legacy = ["spark.cdm.filter.java.writetime.max"], unit = "µs")]
            pub max: Option<i64> = None,
        }
    }
}

cdm_properties! {
    /// A single-column equality filter applied in cdm-rs (`CFG-180`).
    pub struct ColumnFilter {
        fields {
            /// The origin column to test.
            #[cdm(legacy = ["spark.cdm.filter.java.column.name"])]
            pub name: Option<String> = None,

            /// The value the column must equal, compared as a string.
            #[cdm(legacy = ["spark.cdm.filter.java.column.value"])]
            pub value: Option<String> = None,
        }
    }
}

// =================================================================================================
// §3.5.10 Features — CFG-190
// =================================================================================================

cdm_properties! {
    /// Optional behaviours (`CFG-190`).
    pub struct Feature {
        sections {
            /// Columns written with a fixed value on every row.
            #[cdm()]
            pub constant_columns: ConstantColumns,

            /// Turning one origin map into many target rows.
            #[cdm()]
            pub explode_map: ExplodeMap,

            /// Lifting JSON properties out of a text column into target columns.
            #[cdm()]
            pub extract_json: ExtractJson,

            /// Limits that make a row be skipped and reported rather than written.
            #[cdm()]
            pub guardrail: Guardrail
        }
    }
}

cdm_properties! {
    /// Columns written with a fixed value on every row (`CFG-190`).
    pub struct ConstantColumns {
        fields {
            /// Target columns to populate with a constant.
            #[cdm(
                legacy = ["spark.cdm.feature.constantColumns.names"],
                example = "const1,const2",
            )]
            pub names: Vec<String> = Vec::new(),

            /// The constants, as CQL literals, in the order of
            /// [`names`](ConstantColumns::names) (`CFG-030`).
            #[cdm(legacy = ["spark.cdm.feature.constantColumns.values"], example = "'abcd',1234")]
            pub values: Option<String> = None,

            /// The regular expression that splits
            /// [`values`](ConstantColumns::values), for literals that contain commas.
            #[cdm(legacy = ["spark.cdm.feature.constantColumns.splitRegex"])]
            pub split_regex: String = ",".to_owned(),
        }
    }
}

cdm_properties! {
    /// Turning one origin map into many target rows (`CFG-190`, `CFG-031`).
    pub struct ExplodeMap {
        fields {
            /// The origin map column to explode.
            #[cdm(legacy = ["spark.cdm.feature.explodeMap.origin.name"])]
            pub origin_column: Option<String> = None,

            /// The target column that receives each map key.
            #[cdm(legacy = ["spark.cdm.feature.explodeMap.target.name.key"])]
            pub target_key_column: Option<String> = None,

            /// The target column that receives each map value.
            #[cdm(legacy = ["spark.cdm.feature.explodeMap.target.name.value"])]
            pub target_value_column: Option<String> = None,
        }
    }
}

cdm_properties! {
    /// Lifting JSON properties out of a text column into target columns (`CFG-190`).
    pub struct ExtractJson {
        fields {
            /// The origin column holding a JSON document.
            #[cdm(legacy = ["spark.cdm.feature.extractJson.originColumn"])]
            pub origin_column: Option<String> = None,

            /// The mapping from JSON property to target column, as `property:column` pairs.
            #[cdm(
                legacy = ["spark.cdm.feature.extractJson.propertyMapping"],
                example = "name:full_name,age:age",
            )]
            pub property_mapping: Option<String> = None,

            /// Overwrite a target column that already holds a value.
            #[cdm(legacy = ["spark.cdm.feature.extractJson.overwrite"])]
            pub overwrite: bool = false,

            /// Migrate only the extracted columns, not the JSON column itself.
            #[cdm(legacy = ["spark.cdm.feature.extractJson.exclusive"])]
            pub exclusive: bool = false,
        }
    }
}

cdm_properties! {
    /// Limits that make a row be skipped and reported rather than written (`CFG-190`).
    pub struct Guardrail {
        fields {
            /// Report any column whose serialised size exceeds this. `0` disables the check.
            ///
            /// A negative value is invalid (`CFG-035`).
            #[cdm(legacy = ["spark.cdm.feature.guardrail.colSizeInKB"], unit = "KB")]
            pub column_size_kb: f64 = 0.0,
        }
    }
}

// =================================================================================================
// §3.5.11 New cdm-rs sections — CFG-200
// =================================================================================================

cdm_properties! {
    /// The HTTP control plane (`CFG-200`).
    pub struct Server {
        fields {
            /// Start the HTTP control plane.
            #[cdm()]
            pub enabled: bool = false,

            /// The address the control plane listens on.
            ///
            /// Binding anywhere other than loopback without authentication is refused
            /// (`SEC-010`).
            #[cdm(example = "0.0.0.0:8080",)]
            pub bind: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
        }
        sections {
            /// How callers authenticate.
            #[cdm()]
            pub auth: Auth
        }
    }
}

cdm_properties! {
    /// How callers of the HTTP control plane authenticate (`SEC-010`).
    pub struct Auth {
        fields {
            /// The authentication scheme.
            #[cdm()]
            pub mode: AuthMode = AuthMode::None,
        }
    }
}

cdm_properties! {
    /// Metrics and event export (`CFG-200`).
    pub struct Metrics {
        sections {
            /// The Prometheus scrape endpoint (`MET-020`).
            #[cdm()]
            pub prometheus: Prometheus,

            /// OpenTelemetry export (`MET-021`).
            #[cdm()]
            pub otlp: Otlp,

            /// The run event stream (`MET-030`).
            #[cdm()]
            pub events: Events
        }
    }
}

cdm_properties! {
    /// The Prometheus scrape endpoint (`MET-020`).
    pub struct Prometheus {
        fields {
            /// Serve `/metrics`. Defaults to whether the control plane is enabled.
            #[cdm(default_note = "true when `server.enabled` is true",)]
            pub enabled: Option<bool> = None,
        }
    }
}

cdm_properties! {
    /// OpenTelemetry export (`MET-021`).
    pub struct Otlp {
        fields {
            /// The OTLP collector endpoint. Export is off when unset.
            #[cdm(example = "http://localhost:4317",)]
            pub endpoint: Option<String> = None,
        }
    }
}

cdm_properties! {
    /// The run event stream (`MET-030`).
    pub struct Events {
        fields {
            /// Where run events are written.
            #[cdm()]
            pub sink: EventSink = EventSink::None,
        }
    }
}

cdm_properties! {
    /// Distributed mode (`CFG-200`, `DST-001`).
    pub struct Cluster {
        fields {
            /// Coordinate token ranges with other cdm-rs processes through the tracking table.
            #[cdm(stability = experimental,)]
            pub enabled: bool = false,

            /// This node's identity in the membership table.
            #[cdm(default_note = "the host name and process id",)]
            pub node_id: Option<String> = None,

            /// How long a range lease is held before another node may reclaim it (`DST-012`).
            #[cdm(unit = "duration",)]
            pub lease_duration: DurationSetting = DurationSetting::from_secs(60),

            /// How often a node renews its leases and refreshes its membership row.
            #[cdm(unit = "duration",)]
            pub heartbeat_interval: DurationSetting = DurationSetting::from_secs(15),
        }
    }
}

cdm_properties! {
    /// Logging (`CFG-200`).
    pub struct Logging {
        fields {
            /// The `tracing` filter directive, e.g. `info` or `cdm_engine=debug,info`.
            #[cdm(example = "cdm_engine=debug,info",)]
            pub level: String = "info".to_owned(),

            /// The shape of log records.
            #[cdm()]
            pub format: LogFormat = LogFormat::Pretty,

            /// Where the validate job writes its row-level difference log.
            #[cdm()]
            pub diff_file: PathBuf = PathBuf::from("cdm_logs/cdm_diff.log"),
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

    #[test]
    fn cfg_001_the_default_configuration_is_the_java_default_configuration() {
        let config = CdmConfig::default();
        assert_eq!(config.connect.origin.host, "localhost");
        assert_eq!(config.connect.target.port, 9042);
        assert_eq!(config.connect.origin.username, "cassandra");
        assert_eq!(config.connect.target.password.expose(), "cassandra");
        assert_eq!(config.perfops.num_parts, 5000);
        assert_eq!(config.perfops.batch_size, 5);
        assert_eq!(config.perfops.ratelimit.origin, 20_000);
        assert_eq!(
            config.perfops.consistency.write,
            ConsistencyLevel::LocalQuorum
        );
        assert!(config.schema.origin.ttl.automatic);
        assert!(config.schema.origin.keyspace_table.is_none());
        assert_eq!(config.feature.constant_columns.split_regex, ",");
        assert_eq!(config.transform.codec_timestamp_format, "yyyyMMddHHmmss");
        assert_eq!(config.filter.token_coverage_percent, 100);
    }

    #[test]
    fn cfg_001_the_model_round_trips_through_json_apart_from_secrets() {
        let config = CdmConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["connect"]["origin"]["host"], "localhost");
        // SEC-001: serialising redacts, which is why the loader never re-serialises.
        assert_eq!(json["connect"]["origin"]["password"], "***");
        assert_eq!(
            json["connect"]["origin"]["tls"]["truststore"]["type"],
            "JKS"
        );
        assert_eq!(json["logging"]["diff_file"], "cdm_logs/cdm_diff.log");
        assert_eq!(json["server"]["bind"], "127.0.0.1:8080");
    }

    #[test]
    fn cfg_001_an_empty_document_deserialises_to_the_defaults() {
        let config: CdmConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(config, CdmConfig::default());
    }
}
