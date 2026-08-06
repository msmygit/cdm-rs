<!-- GENERATED FILE — run `cargo xtask docs`. Do not edit by hand. -->

# cdm-rs — Configuration property reference

Generated from `cdm_config::CdmConfig` (`CFG-001`, `CFG-002`). `legacy` is the Java `spark.cdm.*` name that cdm-rs still accepts (`CFG-011`); `canonical` is the cdm-rs name used by TOML, YAML, JSON, `CDM__*` environment variables and `--set`.

124 properties.


## `connect`

Origin and target are configured **independently** (`CON-001`), and each side uses **either** a
contact point **or** an Astra secure-connect-bundle — never both (`CFG-041`).

| Origin → Target | Origin | Target |
|---|---|---|
| Cassandra → Cassandra | `connect.origin.host` | `connect.target.host` |
| DSE → HCD | `connect.origin.host` | `connect.target.host` |
| Cassandra/DSE → Astra | `connect.origin.host` | `connect.target.scb` or `connect.target.astra.database_id` |
| Astra → Astra | `connect.origin.scb` | `connect.target.scb` |
| Astra → Cassandra | `connect.origin.scb` | `connect.target.host` |

**`connect.{side}.scb` and every `connect.{side}.astra.*` property apply to Astra DB only.** They
are ignored for self-managed Apache Cassandra, DSE, HCD and ScyllaDB, which use
`connect.{side}.host` and `connect.{side}.port`.

**TLS to a self-managed cluster is not a bundle.** A cluster with client encryption uses
`connect.{side}.tls.*` — truststore, keystore and cipher suites (`CFG-120`). That is a separate
mechanism, unrelated to the Astra bundle. The one exception is `connect.{side}.tls.is_astra`, a
Java compatibility path that synthesises a bundle from truststore material; new configurations
should not use it.

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `connect.origin.host` | `spark.cdm.connect.origin.host` | string | `localhost` | — | stable | Contact point host name or IP address. |
| `connect.origin.port` | `spark.cdm.connect.origin.port` | integer | `9042` | — | stable | Native transport port. |
| `connect.origin.scb` | `spark.cdm.connect.origin.scb` | path | — | — | stable | Path to an Astra DB secure-connect-bundle zip. |
| `connect.origin.username` | `spark.cdm.connect.origin.username` | string | `cassandra` | — | stable | Username, or the literal `token` when authenticating to Astra. |
| `connect.origin.password` | `spark.cdm.connect.origin.password` | secret | `***` | — | stable | Password, or the Astra token. |
| `connect.origin.local_datacenter` | — | string | auto-detected from `system.local` | — | stable | The datacenter the load-balancing policy treats as local (`CON-009`). |
| `connect.origin.astra.database_id` | `spark.cdm.connect.origin.astra.database.id` | uuid | — | — | stable | The Astra database UUID, used to download a bundle through the DevOps API. |
| `connect.origin.astra.scb_type` | `spark.cdm.connect.origin.astra.scb.type` | `default` \| `custom` | `default` | — | stable | Whether to download the standard bundle or a custom-domain one. |
| `connect.origin.astra.region` | `spark.cdm.connect.origin.astra.scb.region` | string | — | — | stable | The Astra region whose bundle to download, for multi-region databases. |
| `connect.origin.astra.custom_domain` | `spark.cdm.connect.origin.astra.scb.custom.domain` | string | — | — | stable | The custom domain the bundle should be issued for. |
| `connect.origin.astra.mode` | — | `sni` \| `single_endpoint` | `sni` | — | experimental | How CQL traffic reaches Astra (`CON-022`, `CON-026`). |
| `connect.origin.astra.metadata_refresh_interval` | — | duration | `5m` | duration | stable | How often to refresh Astra control-connection metadata (`CON-025`). |
| `connect.origin.speculative.enabled` | — | bool | `false` | — | stable | Whether to start speculative executions. |
| `connect.origin.speculative.delay` | — | duration | `200ms` | duration | stable | How long to wait for the previous execution before starting another. |
| `connect.origin.speculative.max_executions` | — | integer | `2` | executions | stable | How many *extra* executions a request may have. |
| `connect.origin.tls.enabled` | `spark.cdm.connect.origin.tls.enabled` | bool | `false` | — | stable | Whether to use TLS. |
| `connect.origin.tls.cipher_suites` | `spark.cdm.connect.origin.tls.enabledAlgorithms` | list | `TLS_RSA_WITH_AES_128_CBC_SHA,TLS_RSA_WITH_AES_256_CBC_SHA` | — | stable | Cipher suites offered during the handshake. |
| `connect.origin.tls.is_astra` | `spark.cdm.connect.origin.tls.isAstra` | bool | `false` | — | stable | Whether this side is Astra, which implies its own trust material. |
| `connect.origin.tls.truststore.path` | `spark.cdm.connect.origin.tls.trustStore.path` | path | — | — | stable | Path to the trust store. |
| `connect.origin.tls.truststore.password` | `spark.cdm.connect.origin.tls.trustStore.password` | secret | — | — | stable | Password protecting the trust store. |
| `connect.origin.tls.truststore.type` | `spark.cdm.connect.origin.tls.trustStore.type` | `JKS` \| `PKCS12` \| `PEM` | `JKS` | — | stable | The store's on-disk format. |
| `connect.origin.tls.keystore.path` | `spark.cdm.connect.origin.tls.keyStore.path` | path | — | — | stable | Path to the key store holding the client certificate. |
| `connect.origin.tls.keystore.password` | `spark.cdm.connect.origin.tls.keyStore.password` | secret | — | — | stable | Password protecting the key store. |
| `connect.target.host` | `spark.cdm.connect.target.host` | string | `localhost` | — | stable | Contact point host name or IP address. |
| `connect.target.port` | `spark.cdm.connect.target.port` | integer | `9042` | — | stable | Native transport port. |
| `connect.target.scb` | `spark.cdm.connect.target.scb` | path | — | — | stable | Path to an Astra DB secure-connect-bundle zip. |
| `connect.target.username` | `spark.cdm.connect.target.username` | string | `cassandra` | — | stable | Username, or the literal `token` when authenticating to Astra. |
| `connect.target.password` | `spark.cdm.connect.target.password` | secret | `***` | — | stable | Password, or the Astra token. |
| `connect.target.local_datacenter` | — | string | auto-detected from `system.local` | — | stable | The datacenter the load-balancing policy treats as local (`CON-009`). |
| `connect.target.astra.database_id` | `spark.cdm.connect.target.astra.database.id` | uuid | — | — | stable | The Astra database UUID, used to download a bundle through the DevOps API. |
| `connect.target.astra.scb_type` | `spark.cdm.connect.target.astra.scb.type` | `default` \| `custom` | `default` | — | stable | Whether to download the standard bundle or a custom-domain one. |
| `connect.target.astra.region` | `spark.cdm.connect.target.astra.scb.region` | string | — | — | stable | The Astra region whose bundle to download, for multi-region databases. |
| `connect.target.astra.custom_domain` | `spark.cdm.connect.target.astra.scb.custom.domain` | string | — | — | stable | The custom domain the bundle should be issued for. |
| `connect.target.astra.mode` | — | `sni` \| `single_endpoint` | `sni` | — | experimental | How CQL traffic reaches Astra (`CON-022`, `CON-026`). |
| `connect.target.astra.metadata_refresh_interval` | — | duration | `5m` | duration | stable | How often to refresh Astra control-connection metadata (`CON-025`). |
| `connect.target.speculative.enabled` | — | bool | `false` | — | stable | Whether to start speculative executions. |
| `connect.target.speculative.delay` | — | duration | `200ms` | duration | stable | How long to wait for the previous execution before starting another. |
| `connect.target.speculative.max_executions` | — | integer | `2` | executions | stable | How many *extra* executions a request may have. |
| `connect.target.tls.enabled` | `spark.cdm.connect.target.tls.enabled` | bool | `false` | — | stable | Whether to use TLS. |
| `connect.target.tls.cipher_suites` | `spark.cdm.connect.target.tls.enabledAlgorithms` | list | `TLS_RSA_WITH_AES_128_CBC_SHA,TLS_RSA_WITH_AES_256_CBC_SHA` | — | stable | Cipher suites offered during the handshake. |
| `connect.target.tls.is_astra` | `spark.cdm.connect.target.tls.isAstra` | bool | `false` | — | stable | Whether this side is Astra, which implies its own trust material. |
| `connect.target.tls.truststore.path` | `spark.cdm.connect.target.tls.trustStore.path` | path | — | — | stable | Path to the trust store. |
| `connect.target.tls.truststore.password` | `spark.cdm.connect.target.tls.trustStore.password` | secret | — | — | stable | Password protecting the trust store. |
| `connect.target.tls.truststore.type` | `spark.cdm.connect.target.tls.trustStore.type` | `JKS` \| `PKCS12` \| `PEM` | `JKS` | — | stable | The store's on-disk format. |
| `connect.target.tls.keystore.path` | `spark.cdm.connect.target.tls.keyStore.path` | path | — | — | stable | Path to the key store holding the client certificate. |
| `connect.target.tls.keystore.password` | `spark.cdm.connect.target.tls.keyStore.password` | secret | — | — | stable | Password protecting the key store. |

## `schema`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `schema.origin.keyspace_table` | `spark.cdm.schema.origin.keyspaceTable` | string | — | — | stable | The origin table, as `keyspace.table`. |
| `schema.origin.ttl.automatic` | `spark.cdm.schema.origin.column.ttl.automatic` | bool | `true` | — | stable | Use every eligible non-key column as a TTL source. |
| `schema.origin.ttl.names` | `spark.cdm.schema.origin.column.ttl.names` | list | — | — | stable | The columns whose TTL to read, largest wins. |
| `schema.origin.writetime.automatic` | `spark.cdm.schema.origin.column.writetime.automatic` | bool | `true` | — | stable | Use every eligible non-key column as a writetime source. |
| `schema.origin.writetime.names` | `spark.cdm.schema.origin.column.writetime.names` | list | — | — | stable | The columns whose writetime to read, largest wins. |
| `schema.origin.column.skip` | `spark.cdm.schema.origin.column.skip` | list | — | — | stable | Origin columns to leave out of the run. |
| `schema.origin.column.rename` | `spark.cdm.schema.origin.column.names.to.target` | list | — | — | stable | Column renames, each written `origin_name:target_name` (`CFG-038`). |
| `schema.target.keyspace_table` | `spark.cdm.schema.target.keyspaceTable` | string | the origin keyspace and table | — | stable | The target table, as `keyspace.table`. Defaults to the origin table (`CFG-023`). |
| `schema.ttl_writetime.use_collections` | `spark.cdm.schema.ttlwritetime.calc.useCollections` | bool | `false` | — | stable | Allow collection columns to contribute TTL and writetime. |

## `autocorrect`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `autocorrect.missing` | `spark.cdm.autocorrect.missing` | bool | `false` | — | stable | Write rows that are present on origin and absent on target. |
| `autocorrect.mismatch` | `spark.cdm.autocorrect.mismatch` | bool | `false` | — | stable | Overwrite target rows whose values differ from origin. |
| `autocorrect.missing_counter` | `spark.cdm.autocorrect.missing.counter` | bool | `false` | — | stable | Write missing rows even for a counter table. |

## `track_run`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `track_run.enabled` | `spark.cdm.trackRun` | bool | `false` | — | stable | Record each token range's outcome so the run can be resumed. |
| `track_run.run_id` | `spark.cdm.trackRun.runId` | integer | `0` | — | stable | The identifier to record this run under. `0` allocates a new one. |
| `track_run.previous_run_id` | `spark.cdm.trackRun.previousRunId` | integer | `0` | — | stable | The run to resume. `0` starts from the beginning. |
| `track_run.auto_rerun` | `spark.cdm.trackRun.autoRerun` | bool | `false` | — | stable | Automatically rerun the ranges the previous run did not complete. |
| `track_run.rerun_multiplier` | `spark.cdm.trackRun.rerunMultiplier` | integer | `1` | — | stable | How many times to subdivide a range that is rerun. |

## `perfops`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `perfops.num_parts` | `spark.cdm.perfops.numParts` | integer | `5000` | parts | stable | Number of token-range splits the ring is divided into. |
| `perfops.batch_size` | `spark.cdm.perfops.batchSize` | integer | `5` | rows | stable | Rows per write batch. |
| `perfops.fetch_size` | `spark.cdm.perfops.fetchSizeInRows` | integer | `1000` | rows | stable | Rows read from the origin per page. |
| `perfops.error_limit` | `spark.cdm.perfops.errorLimit` | integer | `0` | errors | stable | Stop the run after this many row-level errors. `0` means never stop. |
| `perfops.workers` | — | integer | num_cpus | workers | stable | Concurrent range workers. |
| `perfops.max_inflight_writes` | — | integer | `2000` | requests | stable | Maximum write requests in flight across all workers. |
| `perfops.max_inflight_reads` | — | integer | `256` | requests | stable | Maximum read requests in flight across all workers. |
| `perfops.request_timeout` | — | duration | `30s` | duration | stable | Per-request timeout. |
| `perfops.connection_pool_size` | — | integer | `4` | connections | stable | Connections per host, per side. |
| `perfops.adaptive_ratelimit` | — | bool | `false` | — | experimental | Reduce the rate limit automatically when the target signals overload. |
| `perfops.shutdown_grace` | — | duration | `1m` | duration | stable | How long a graceful shutdown lets in-flight ranges finish (`ENG-010`). |
| `perfops.ratelimit.origin` | `spark.cdm.perfops.ratelimit.origin` | integer | `20000` | rows/s | stable | Rows read per second, per cdm-rs process. |
| `perfops.ratelimit.target` | `spark.cdm.perfops.ratelimit.target` | integer | `20000` | rows/s | stable | Rows written per second, per cdm-rs process. |
| `perfops.consistency.read` | `spark.cdm.perfops.consistency.read` | `ANY` \| `ONE` \| `TWO` \| `THREE` \| `QUORUM` \| `LOCAL_ONE` \| `LOCAL_QUORUM` \| `EACH_QUORUM` \| `SERIAL` \| `LOCAL_SERIAL` \| `ALL` | `LOCAL_QUORUM` | — | stable | Consistency level for reads from the origin. |
| `perfops.consistency.write` | `spark.cdm.perfops.consistency.write` | `ANY` \| `ONE` \| `TWO` \| `THREE` \| `QUORUM` \| `LOCAL_ONE` \| `LOCAL_QUORUM` \| `EACH_QUORUM` \| `SERIAL` \| `LOCAL_SERIAL` \| `ALL` | `LOCAL_QUORUM` | — | stable | Consistency level for writes to the target. |
| `perfops.retry.max_attempts` | — | integer | `5` | attempts | stable | How many times a failed request is retried before the range fails. |
| `perfops.retry.initial_backoff` | — | duration | `100ms` | duration | stable | Delay before the first retry; doubles each attempt. |
| `perfops.retry.max_backoff` | — | duration | `10s` | duration | stable | Ceiling on the exponential backoff. |

## `transform`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `transform.missing_key_ts_replace` | `spark.cdm.transform.missing.key.ts.replace.value` | integer | — | epoch ms | stable | Timestamp, in epoch milliseconds, substituted for a null key column. |
| `transform.custom_writetime` | `spark.cdm.transform.custom.writetime` | integer | `0` | µs | stable | Writetime, in microseconds, to use instead of the origin's. `0` disables. |
| `transform.custom_writetime_increment` | `spark.cdm.transform.custom.writetime.incrementBy` | integer | `0` | µs | stable | Microseconds added to every writetime (`CFG-033`, `CFG-039`). |
| `transform.custom_ttl` | `spark.cdm.transform.custom.ttl` | integer | `0` | s | stable | TTL, in seconds, to use instead of the origin's. `0` disables. |
| `transform.codecs` | `spark.cdm.transform.codecs` | list | — | — | stable | Codecs to enable, by name. |
| `transform.codec_timestamp_format` | `spark.cdm.transform.codecs.timestamp.string.format` | string | `yyyyMMddHHmmss` | — | stable | The `java.time` pattern the `TIMESTAMP_STRING_FORMAT` codec parses and prints. |
| `transform.codec_timestamp_zone` | `spark.cdm.transform.codecs.timestamp.string.zone` | string | `UTC` | — | stable | The IANA time zone the `TIMESTAMP_STRING_FORMAT` codec assumes. |
| `transform.map_remove_null_value` | `spark.cdm.transform.map.remove.null.value` | bool | `false` | — | stable | Drop map entries whose value is null instead of writing them. |

## `filter`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `filter.cql_where` | `spark.cdm.filter.cassandra.whereCondition` | string | — | — | stable | A CQL predicate appended to the origin `SELECT`, without the `WHERE` keyword. |
| `filter.token_coverage_percent` | `spark.cdm.filter.java.token.percent` | integer | `100` | % | stable | Percentage of each token range to process, for sampling runs. |
| `filter.token.min` | `spark.cdm.filter.cassandra.partition.min` | bigint | the partitioner minimum | — | stable | Lowest token to process. Defaults to the partitioner's minimum. |
| `filter.token.max` | `spark.cdm.filter.cassandra.partition.max` | bigint | the partitioner maximum | — | stable | Highest token to process. Defaults to the partitioner's maximum. |
| `filter.writetime.min` | `spark.cdm.filter.java.writetime.min` | integer | — | µs | stable | Skip rows whose writetime is below this, in microseconds. |
| `filter.writetime.max` | `spark.cdm.filter.java.writetime.max` | integer | — | µs | stable | Skip rows whose writetime is above this, in microseconds. |
| `filter.column.name` | `spark.cdm.filter.java.column.name` | string | — | — | stable | The origin column to test. |
| `filter.column.value` | `spark.cdm.filter.java.column.value` | string | — | — | stable | The value the column must equal, compared as a string. |

## `feature`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `feature.constant_columns.names` | `spark.cdm.feature.constantColumns.names` | list | — | — | stable | Target columns to populate with a constant. |
| `feature.constant_columns.values` | `spark.cdm.feature.constantColumns.values` | string | — | — | stable | The constants, as CQL literals, in the order of [`names`](ConstantColumns::names) (`CFG-030`). |
| `feature.constant_columns.split_regex` | `spark.cdm.feature.constantColumns.splitRegex` | string | `,` | — | stable | The regular expression that splits [`values`](ConstantColumns::values), for literals that contain commas. |
| `feature.explode_map.origin_column` | `spark.cdm.feature.explodeMap.origin.name` | string | — | — | stable | The origin map column to explode. |
| `feature.explode_map.target_key_column` | `spark.cdm.feature.explodeMap.target.name.key` | string | — | — | stable | The target column that receives each map key. |
| `feature.explode_map.target_value_column` | `spark.cdm.feature.explodeMap.target.name.value` | string | — | — | stable | The target column that receives each map value. |
| `feature.extract_json.origin_column` | `spark.cdm.feature.extractJson.originColumn` | string | — | — | stable | The origin column holding a JSON document. |
| `feature.extract_json.property_mapping` | `spark.cdm.feature.extractJson.propertyMapping` | string | — | — | stable | The mapping from JSON property to target column, as `property:column` pairs. |
| `feature.extract_json.overwrite` | `spark.cdm.feature.extractJson.overwrite` | bool | `false` | — | stable | Overwrite a target column that already holds a value. |
| `feature.extract_json.exclusive` | `spark.cdm.feature.extractJson.exclusive` | bool | `false` | — | stable | Migrate only the extracted columns, not the JSON column itself. |
| `feature.guardrail.column_size_kb` | `spark.cdm.feature.guardrail.colSizeInKB` | float | `0.0` | KB | stable | Report any column whose serialised size exceeds this. `0` disables the check. |
| `feature.guardrail.mode` | — | `check` \| `warn` \| `block` | `check` | — | stable | What an inline guardrail violation does to the row that caused it (`GRD-004`). |

## `server`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `server.enabled` | — | bool | `false` | — | stable | Start the HTTP control plane. |
| `server.bind` | — | socket | `127.0.0.1:8080` | — | stable | The address the control plane listens on. |
| `server.auth.mode` | — | `none` \| `bearer` \| `mtls` | `none` | — | stable | The authentication scheme. |

## `metrics`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `metrics.prometheus.enabled` | — | bool | true when `server.enabled` is true | — | stable | Serve `/metrics`. Defaults to whether the control plane is enabled. |
| `metrics.otlp.endpoint` | — | string | — | — | stable | The OTLP collector endpoint. Export is off when unset. |
| `metrics.events.sink` | — | `none` \| `stdout_json` \| `file` | `none` | — | stable | Where run events are written. |
| `metrics.events.path` | — | path | `cdm_logs/cdm_events.ndjson` | — | stable | The file the `file` sink appends to (`MET-030`). |

## `cluster`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `cluster.enabled` | — | bool | `false` | — | experimental | Coordinate token ranges with other cdm-rs processes through the tracking table. |
| `cluster.node_id` | — | string | the host name and process id | — | stable | This node's identity in the membership table. |
| `cluster.lease_duration` | — | duration | `1m` | duration | stable | How long a range lease is held before another node may reclaim it (`DST-012`). |
| `cluster.heartbeat_interval` | — | duration | `15s` | duration | stable | How often a node renews its leases and refreshes its membership row. |
| `cluster.ratelimit_is_global` | — | bool | `false` | — | experimental | Treat `perfops.ratelimit.*` as a budget for the whole cluster (`ENG-004`). |

## `logging`

| canonical | legacy | type | default | unit | stability | description |
|---|---|---|---|---|---|---|
| `logging.level` | — | string | `info` | — | stable | The `tracing` filter directive, e.g. `info` or `cdm_engine=debug,info`. |
| `logging.format` | — | `pretty` \| `json` \| `compact` | `pretty` | — | stable | The shape of log records. |
| `logging.diff_file` | — | path | `cdm_logs/cdm_diff.log` | — | stable | Where the validate job writes its row-level difference log. |

