# cdm-rs — Functional & Technical Specification

| | |
|---|---|
| **Document** | `docs/SPEC.md` |
| **Status** | Draft v1 (baseline for implementation) |
| **Repository** | `git@github.com:msmygit/cdm-rs.git` |
| **Supersedes** | Java `cassandra-data-migrator` 6.0.x (feature parity is a hard gate) |
| **Companion docs** | [`ARCHITECTURE.md`](./ARCHITECTURE.md) · [`TRACEABILITY.md`](./TRACEABILITY.md) · [`ROADMAP.md`](./ROADMAP.md) · [`MIGRATION_FROM_JAVA.md`](./MIGRATION_FROM_JAVA.md) |

---

## 0. How to read this document

Every requirement carries a stable identifier of the form `<DOMAIN>-<NNN>`. Identifiers are
**append-only**: they are never renumbered or reused. Each identifier must appear in:

1. this document (the normative statement),
2. `docs/TRACEABILITY.md` (mapping to crate, module, tests and delivering PR),
3. at least one automated test whose name or `#[doc]` comment cites the ID,
4. the commit/PR that implements it (`Implements: MIG-014, MIG-015`).

CI enforces 1–4 mechanically (see [`OPS-011`](#17-cicd-supply-chain-and-release)).

Requirement keywords **MUST**, **SHOULD**, **MAY** follow RFC 2119.

**Parity marker.** Requirements marked **[P]** are direct parity requirements with the Java
implementation — behaviour must be observably identical. Requirements marked **[N]** are new
capabilities in cdm-rs. Requirements marked **[P+]** preserve Java behaviour but extend it in a
backward-compatible way.

### Domain prefixes

| Prefix | Domain |
|---|---|
| `CFG` | Configuration model, loading, validation |
| `CON` | Connectivity, TLS, Astra, cloud bundles |
| `SCH` | Schema introspection, column mapping |
| `TOK` | Token-range planning and splitting |
| `ENG` | Execution engine, scheduling, concurrency, rate limiting |
| `MIG` | Migrate job |
| `VAL` | Validate / diff job |
| `GRD` | Guardrail job |
| `FEA` | Optional features (constant columns, explode map, extract JSON, TTL/writetime, filters) |
| `CDC` | Codecs and type conversion |
| `TRK` | Run tracking, resume, rerun |
| `DST` | Distributed coordination |
| `MET` | Metrics, progress, observability |
| `API` | HTTP control plane / OpenAPI |
| `MCP` | Model Context Protocol server |
| `A2A` | Agent-to-Agent protocol |
| `UI`  | Config Builder web UI |
| `CLI` | Command line interface |
| `PLG` | Plugin / extension model |
| `ERR` | Error model and diagnostics |
| `TST` | Testing requirements |
| `OPS` | Build, CI/CD, packaging, release |
| `NFR` | Non-functional requirements |
| `SEC` | Security requirements |

---

## 1. Context and goals

### 1.1 Problem statement

The DataStax **Cassandra Data Migrator (CDM)** is a mature, battle-tested tool for moving and
validating data between Cassandra-compatible clusters (Apache Cassandra, DSE, HCD, Astra DB,
ScyllaDB, Azure Cosmos DB Cassandra API). It is implemented in Java/Scala and executes as an
Apache Spark job.

Operationally this imposes real costs on users:

* a JVM plus a **specific** Spark build must be installed and version-matched (`NoSuchMethodError:
  scala.runtime.Statics.releaseFence()` is the single most common support issue);
* configuration is a flat `spark.cdm.*` properties file with ~90 keys, no schema, no autocomplete,
  and errors surface only at runtime, often deep into a multi-hour job;
* memory footprint is high (`--driver-memory 25G --executor-memory 25G` is the documented default);
* progress observability is limited to log scraping and a final counter block;
* automation/integration requires driving `spark-submit` and parsing stdout.

### 1.2 Goals

| ID | Goal |
|---|---|
| G1 | **Zero functionality loss.** Every documented and undocumented behaviour of Java CDM 6.0.x is reproduced or explicitly superseded with a documented, better-defined behaviour. |
| G2 | **Single static binary.** No JVM, no Spark, no runtime dependency beyond libc (and optionally none, via musl). |
| G3 | **Highly modular, pluggable, reusable.** Every subsystem is a separate crate behind a trait; third parties can add codecs, features, sources, sinks, metric exporters and transports without forking. |
| G4 | **Rich, real-time metrics.** Prometheus, OpenTelemetry, structured JSON events, live TUI, and a queryable HTTP API — not just a final log line. |
| G5 | **Easy on users.** Guided config, schema-aware validation with actionable errors, dry-run planning, resumable runs, sane defaults, a web config builder, and a first-class CLI. |
| G6 | **API-first.** An OpenAPI 3.1 document is the source of truth for the control plane; MCP, A2A and any future transport are *generated adapters* over the same core, never re-implementations. |
| G7 | **Correctness is provable.** Property-based tests, differential tests against the Java implementation, and end-to-end integration tests against real Cassandra containers. |
| G8 | **DRY.** One definition of every concept: one property registry, one type-conversion table, one counter registry, one API schema. Everything else is generated or derived from it. |

### 1.3 Non-goals

* Migrating between fundamentally different data models (e.g. Cassandra → PostgreSQL). The
  source/sink traits make this *possible* for third parties, but no such adapter ships in v1.
* Change-data-capture / continuous replication. CDM is a bulk mover; ZDM Proxy handles live
  dual-writes. (See `docs/CDM_WITH_ZDM.md`.)
* Replacing `dsbulk` for file import/export.

### 1.4 Success criteria

| ID | Criterion |
|---|---|
| S1 | Every SIT case in the Java repo (`SIT/smoke`, `SIT/features`, `SIT/regression` — 19 cases) passes against cdm-rs with an equivalent harness and identical counter assertions. |
| S2 | A `cdm.properties` file written for Java CDM 6.x runs unmodified under cdm-rs (compatibility loader). |
| S3 | Throughput ≥ 2× Java CDM on the same single node for the `PERF/perf-iot` workload, at ≤ 25% of the resident memory. |
| S4 | `cargo test --workspace` covers ≥ 85% of lines (`cargo-llvm-cov`) at v1.0. CI enforces a **ratchet**: the floor is whatever the codebase currently clears, raised by the PR that raises the coverage and never lowered. Repository automation (`xtask`) has its own tests but is excluded from the product figure. |
| S5 | The OpenAPI document validates against the 3.1 meta-schema, and the MCP tool list and A2A agent card are generated from it with zero hand-written duplication. |

---

## 2. Glossary

| Term | Meaning |
|---|---|
| **Origin** | Source cluster/keyspace/table data is read from. |
| **Target** | Destination cluster/keyspace/table data is written to. |
| **Job** | One of `migrate`, `validate`, `guardrail` (extensible via `PLG-004`). |
| **Run** | A single execution of a job, identified by a `run_id`. |
| **Partition range** (also *split*, *token range*) | A half-open-ish `[min, max]` interval of the token ring; the unit of work, scheduling, tracking and resume. |
| **Record** | One origin row plus its derived target primary key and (for validate) the corresponding target row. |
| **Exploded record** | One of N records produced from a single origin row by the Explode Map feature. |
| **Worker** | An OS thread / Tokio task pair that owns one partition range at a time. |
| **Node** | One cdm-rs process participating in a distributed run. |
| **Lease** | A time-bounded claim by a node on a partition range, recorded in the tracking table. |

---

## 3. Configuration (`CFG`)

### 3.1 Model

**CFG-001 [N]** — Configuration MUST be defined **once**, as a strongly-typed Rust struct tree
(`cdm_config::CdmConfig`) with `serde` derives. Every other artefact — the property registry, the
JSON Schema, the OpenAPI component schemas, the documentation tables, the config-builder UI form,
the MCP tool input schema — MUST be derived from that single definition. No hand-maintained parallel
list of property names may exist anywhere in the repository. *(DRY, G8.)*

**CFG-002 [N]** — Each field MUST carry, via attribute macros: canonical name, legacy
`spark.cdm.*` alias(es), type, default, unit, whether secret, a one-line description, a long
description, an optional example, and a stability marker (`stable` / `experimental` / `deprecated`).

**CFG-003 [N]** — The generated JSON Schema MUST be published at `schema/cdm-config.schema.json`
and served at `GET /v1/config/schema`. CI MUST fail if the checked-in file differs from the
generated one.

### 3.2 Sources and precedence

**CFG-010 [P+]** — Configuration MUST be loadable from, in increasing order of precedence:

1. built-in defaults,
2. a config file: `.toml`, `.yaml`/`.yml`, `.json`, or **Java-style `.properties`** (`CFG-011`),
3. environment variables prefixed `CDM__` with `__` as the nesting separator (e.g.
   `CDM__CONNECT__ORIGIN__HOST`),
4. `--set key=value` CLI overrides / `--conf key=value` (Spark-compatible spelling),
5. explicit typed CLI flags (e.g. `--origin-host`),
6. values supplied in an API request body.

**CFG-011 [P]** — The `.properties` loader MUST accept the complete Java `spark.cdm.*` namespace
(§3.5), including keys nested under a `--properties-file`. Unknown `spark.*` keys that are not
`spark.cdm.*` MUST be ignored silently (Spark tuning keys). Unknown `spark.cdm.*` keys MUST produce
a warning naming the closest known key (Levenshtein suggestion) and MUST NOT abort unless
`--strict-config` is set. *Rationale: S2.*

**CFG-012 [N]** — Secrets (`*.password`, `*.token`, `*.keyStore.password`, `*.trustStore.password`)
MUST support indirection: `env:VAR_NAME`, `file:/path`, and `exec:command` forms, resolved at load
time. Resolved secrets MUST be wrapped in a `Secret<String>` newtype whose `Debug`/`Display`/
`Serialize` implementations emit `***`. **SEC-001.**

**CFG-013 [N]** — A configuration profile system: a file may declare `[profiles.<name>]` blocks that
are deep-merged over the base when `--profile <name>` is given.

### 3.3 Validation

**CFG-020 [P+]** — Validation MUST run in three escalating tiers, each independently invocable:

| Tier | Name | Requires cluster? | Checks |
|---|---|---|---|
| 1 | **Syntactic** | no | types, ranges, enum values, mutually-required groups, mutually-exclusive groups |
| 2 | **Semantic** | no | cross-field rules (§3.4) — e.g. writetime filter requires writetime columns |
| 3 | **Schema-bound** | yes | keyspace/table existence, column existence and types, PK compatibility, counter-table rules, codec availability for every mapped column pair |

**CFG-021 [N]** — All three tiers MUST run *before* any data is read or written, and MUST report
**every** violation at once (not fail-fast), each as a structured diagnostic (`ERR-002`) with the
offending key, the supplied value, the rule, and a suggested fix.

**CFG-022 [P]** — `spark.cdm.schema.origin.keyspaceTable` (canonical `schema.origin.keyspace_table`)
is the only unconditionally required property.

**CFG-023 [P]** — If the target keyspace/table is unset it MUST default to the origin
keyspace/table.

**CFG-024 [P]** — An origin connection MUST specify either a host or a secure-connect-bundle;
likewise for target. Violation is a Tier-1 error.

**CFG-025 [P]** — When TLS is enabled on a side and no SCB is configured, all of
`trustStore.path`, `trustStore.password`, `trustStore.type`, `keyStore.path`, `keyStore.password`,
`enabledAlgorithms` MUST be present. *(Java `PropertyHelper.isValidConfig`.)*

**CFG-026 [P]** — Empty username or password on either side MUST emit a warning, not an error.

**CFG-027 [P]** — A list-typed property set to an empty value MUST be rejected as invalid.
*(Java `PropertyHelper.validateType`.)*

**CFG-028 [N]** — `cdm config explain <key>` MUST print the description, type, default, current
effective value, and which source supplied it.

**CFG-029 [N]** — `cdm config diff <a> <b>` MUST print a normalised semantic diff of two configs,
ignoring ordering and defaults.

### 3.4 Cross-field semantic rules (Tier 2)

**CFG-030 [P]** — `constantColumns.names` and the split of `constantColumns.values` by
`constantColumns.split_regex` MUST have equal cardinality.

**CFG-031 [P]** — Explode Map requires all three of origin column name, target key column name,
target value column name, or none.

**CFG-032 [P]** — `filter.java.writetime.min` and `.max` MUST both be > 0 when either is set, and
`max` > `min`.

**CFG-033 [P]** — `transform.custom.writetime.incrementBy` MUST be ≥ 0.

**CFG-034 [P]** — A writetime filter requires at least one resolvable writetime column.

**CFG-035 [P]** — `guardrail.colSizeInKB` < 0 is invalid; `= 0` disables the feature.

**CFG-036 [P]** — TTL/writetime features MUST be rejected as invalid when the target is a counter
table.

**CFG-037 [P]** — Explicit `column.ttl.names` / `column.writetime.names` MUST disable the
corresponding `automatic` mode.

**CFG-038 [P]** — `column.names.to.target` entries MUST be `origin:target` pairs referencing columns
that exist on their respective sides.

**CFG-039 [P+]** — `transform.custom.writetime.incrementBy == 0` combined with an unfrozen `list`
column on origin MUST emit a warning about duplicate list entries on rerun (CASSANDRA-11368).

**CFG-040 [N]** — `perfops.batch_size > 1` combined with a counter table, or with an active
writetime filter, MUST emit a notice that batch size is being coerced to 1 (`MIG-021`).

**CFG-041 [P+]** — A side MUST be configured with **either** a contact point (`connect.{side}.host`)
**or** an Astra secure-connect-bundle (`connect.{side}.scb`), never both. Configuring both is a
Tier-1 error naming which one to drop.

*Rationale.* The secure-connect-bundle is an **Astra DB** mechanism. Self-managed Apache Cassandra,
DSE, HCD and ScyllaDB are reached with `host`/`port`, and a self-managed cluster with client
encryption uses `connect.{side}.tls.*` (`CFG-120`) — a different mechanism that is **not** a bundle.
When both are present one of them silently does nothing and the operator cannot tell which. Java
resolves this by letting the bundle win and ignoring the host; cdm-rs rejects it so that a stale
host left from a previous migration cannot masquerade as configuration that matters.
`--compat-java` restores the silent precedence, downgraded to a notice.

*Known limitation.* Tier 1 sees the resolved configuration, not the provenance of each value, so
"the operator set a host" is approximated by "the host differs from its default". Setting `host`
explicitly to `localhost` alongside a bundle is therefore not flagged.

**CFG-042 [N]** — The property reference MUST state, for `connect.{side}.scb` and the whole
`connect.{side}.astra.*` block, that they apply to Astra DB only and are ignored for self-managed
clusters. The four connection modes of `CON-002` MUST be documented as a table of origin/target
combinations, so that the common self-managed-to-self-managed case is visibly bundle-free.

### 3.5 Property registry (parity surface)

The following table is the **normative parity list**. It is generated from the Java
`KnownProperties` enum plus `cdm-detailed.properties`. The generated
`docs/generated/PROPERTIES.md` MUST match it; CI diffs the two.

`legacy` = the Java `spark.cdm.*` name that MUST be accepted. `canonical` = the cdm-rs name.

#### 3.5.1 Connection — **CFG-100**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.connect.origin.host` | `connect.origin.host` | string | `localhost` |
| `spark.cdm.connect.origin.port` | `connect.origin.port` | u16 | `9042` |
| `spark.cdm.connect.origin.scb` | `connect.origin.scb` | path | — |
| `spark.cdm.connect.origin.username` | `connect.origin.username` | string | `cassandra` |
| `spark.cdm.connect.origin.password` | `connect.origin.password` | secret | `cassandra` |
| `spark.cdm.connect.target.host` | `connect.target.host` | string | `localhost` |
| `spark.cdm.connect.target.port` | `connect.target.port` | u16 | `9042` |
| `spark.cdm.connect.target.scb` | `connect.target.scb` | path | — |
| `spark.cdm.connect.target.username` | `connect.target.username` | string | `cassandra` |
| `spark.cdm.connect.target.password` | `connect.target.password` | secret | `cassandra` |
| — **[N]** | `connect.{side}.local_datacenter` | string | — (auto-detected, `CON-009`) |
| — **[N]** | `connect.{side}.speculative.enabled` | bool | `false` (`CON-010`) |
| — **[N]** | `connect.{side}.speculative.delay` | duration | `200ms` (`CON-010`) |
| — **[N]** | `connect.{side}.speculative.max_executions` | u32 | `2` (`CON-010`) |

#### 3.5.2 Astra DevOps / SCB auto-download — **CFG-110**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.connect.{side}.astra.database.id` | `connect.{side}.astra.database_id` | uuid | — |
| `spark.cdm.connect.{side}.astra.scb.type` | `connect.{side}.astra.scb_type` | enum `default`\|`custom` | `default` |
| `spark.cdm.connect.{side}.astra.scb.region` | `connect.{side}.astra.region` | string | — |
| `spark.cdm.connect.{side}.astra.scb.custom.domain` | `connect.{side}.astra.custom_domain` | string | — |
| — **[N]** | `connect.{side}.astra.mode` | enum `sni`\|`single_endpoint` | `sni` (`CON-022`, `CON-026`) |
| — **[N]** | `connect.{side}.astra.metadata_refresh_interval` | duration | `5m` (`CON-025`) |

#### 3.5.3 TLS — **CFG-120** (per side)

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.connect.{side}.tls.enabled` | `connect.{side}.tls.enabled` | bool | `false` |
| `spark.cdm.connect.{side}.tls.trustStore.path` | `connect.{side}.tls.truststore.path` | path | — |
| `spark.cdm.connect.{side}.tls.trustStore.password` | `connect.{side}.tls.truststore.password` | secret | — |
| `spark.cdm.connect.{side}.tls.trustStore.type` | `connect.{side}.tls.truststore.type` | enum `JKS`\|`PKCS12`\|`PEM` | `JKS` |
| `spark.cdm.connect.{side}.tls.keyStore.path` | `connect.{side}.tls.keystore.path` | path | — |
| `spark.cdm.connect.{side}.tls.keyStore.password` | `connect.{side}.tls.keystore.password` | secret | — |
| `spark.cdm.connect.{side}.tls.enabledAlgorithms` | `connect.{side}.tls.cipher_suites` | list | `TLS_RSA_WITH_AES_128_CBC_SHA,TLS_RSA_WITH_AES_256_CBC_SHA` |
| `spark.cdm.connect.{side}.tls.isAstra` | `connect.{side}.tls.is_astra` | bool | `false` |

#### 3.5.4 Schema — **CFG-130**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.schema.origin.keyspaceTable` | `schema.origin.keyspace_table` | string | — **required** |
| `spark.cdm.schema.target.keyspaceTable` | `schema.target.keyspace_table` | string | = origin |
| `spark.cdm.schema.origin.column.ttl.automatic` | `schema.origin.ttl.automatic` | bool | `true` |
| `spark.cdm.schema.origin.column.ttl.names` | `schema.origin.ttl.names` | list | — |
| `spark.cdm.schema.origin.column.writetime.automatic` | `schema.origin.writetime.automatic` | bool | `true` |
| `spark.cdm.schema.origin.column.writetime.names` | `schema.origin.writetime.names` | list | — |
| `spark.cdm.schema.ttlwritetime.calc.useCollections` | `schema.ttl_writetime.use_collections` | bool | `false` |
| `spark.cdm.schema.origin.column.skip` | `schema.origin.column.skip` | list | — |
| `spark.cdm.schema.origin.column.names.to.target` | `schema.origin.column.rename` | list of `a:b` | — |

#### 3.5.5 Autocorrect — **CFG-140**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.autocorrect.missing` | `autocorrect.missing` | bool | `false` |
| `spark.cdm.autocorrect.mismatch` | `autocorrect.mismatch` | bool | `false` |
| `spark.cdm.autocorrect.missing.counter` | `autocorrect.missing_counter` | bool | `false` |

#### 3.5.6 Run tracking — **CFG-150**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.trackRun` | `track_run.enabled` | bool | `false` |
| `spark.cdm.trackRun.runId` | `track_run.run_id` | i64 | `0` |
| `spark.cdm.trackRun.previousRunId` | `track_run.previous_run_id` | i64 | `0` |
| `spark.cdm.trackRun.autoRerun` | `track_run.auto_rerun` | bool | `false` |
| `spark.cdm.trackRun.rerunMultiplier` | `track_run.rerun_multiplier` | u32 | `1` |

#### 3.5.7 Performance / operations — **CFG-160**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.perfops.numParts` | `perfops.num_parts` | u64 | `5000` |
| `spark.cdm.perfops.batchSize` | `perfops.batch_size` | u32 | `5` |
| `spark.cdm.perfops.ratelimit.origin` | `perfops.ratelimit.origin` | u32 rows/s | `20000` |
| `spark.cdm.perfops.ratelimit.target` | `perfops.ratelimit.target` | u32 rows/s | `20000` |
| `spark.cdm.perfops.consistency.read` | `perfops.consistency.read` | enum | `LOCAL_QUORUM` |
| `spark.cdm.perfops.consistency.write` | `perfops.consistency.write` | enum | `LOCAL_QUORUM` |
| `spark.cdm.perfops.fetchSizeInRows` | `perfops.fetch_size` | u32 | `1000` |
| `spark.cdm.perfops.errorLimit`&nbsp;† | `perfops.error_limit` | u64 | `0` (unlimited) |
| — **[N]** | `perfops.workers` | u32 | `num_cpus` |
| — **[N]** | `perfops.max_inflight_writes` | u32 | `2000` |
| — **[N]** | `perfops.max_inflight_reads` | u32 | `256` |
| — **[N]** | `perfops.request_timeout` | duration | `30s` |
| — **[N]** | `perfops.connection_pool_size` | u32 | `4` |
| — **[N]** | `perfops.retry.max_attempts` | u32 | `5` |
| — **[N]** | `perfops.retry.initial_backoff` | duration | `100ms` |
| — **[N]** | `perfops.retry.max_backoff` | duration | `10s` |
| — **[N]** | `perfops.adaptive_ratelimit` | bool | `false` |
| — **[N]** | `perfops.shutdown_grace` | duration | `60s` |

† `spark.cdm.perfops.errorLimit` appears only as a commented-out line in Java's
`src/resources/cdm-detailed.properties`. It is **not** in `KnownProperties` and is referenced
nowhere in Java's source, so Java silently ignores it. cdm-rs accepts the alias and implements the
behaviour (`ENG-009`); the parity baseline is "ignored", not "works differently".

Accepted consistency levels **[P]**: `ANY, ONE, TWO, THREE, QUORUM, LOCAL_ONE, LOCAL_QUORUM,
EACH_QUORUM, SERIAL, LOCAL_SERIAL, ALL` (case-insensitive). Unlike Java, an unrecognised value MUST
be a Tier-1 error rather than silently coerced to `LOCAL_QUORUM` — **CFG-161 [P+]** (this is a
deliberate, documented behaviour change; `--compat-java` restores silent coercion).

#### 3.5.8 Transformations — **CFG-170**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.transform.missing.key.ts.replace.value` | `transform.missing_key_ts_replace` | i64 (epoch ms) | — |
| `spark.cdm.transform.custom.writetime` | `transform.custom_writetime` | i64 (µs) | `0` |
| `spark.cdm.transform.custom.writetime.incrementBy` | `transform.custom_writetime_increment` | i64 (µs) | `0` |
| `spark.cdm.transform.custom.ttl` | `transform.custom_ttl` | i32 (s) | `0` |
| `spark.cdm.transform.codecs` | `transform.codecs` | list | — |
| `spark.cdm.transform.codecs.timestamp.string.format` | `transform.codec_timestamp_format` | string | `yyyyMMddHHmmss` |
| `spark.cdm.transform.codecs.timestamp.string.zone` | `transform.codec_timestamp_zone` | tz | `UTC` |
| `spark.cdm.transform.map.remove.null.value` | `transform.map_remove_null_value` | bool | `false` |

> **Correction.** Earlier drafts of this table gave the canonical names as
> `transform.codecs.timestamp_format` and `transform.codecs.timestamp_zone`, nested beneath
> `transform.codecs`. That is impossible in a typed struct tree (`CFG-001`): `transform.codecs`
> cannot be both a list and an object. Java has no such problem because its keys are flat strings.
> The canonical names are therefore siblings, and **both legacy aliases are unchanged**, so no
> existing `.properties` file is affected.

#### 3.5.9 Filters — **CFG-180**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.filter.cassandra.partition.min` | `filter.token.min` | i128 | partitioner min |
| `spark.cdm.filter.cassandra.partition.max` | `filter.token.max` | i128 | partitioner max |
| `spark.cdm.filter.cassandra.whereCondition` | `filter.cql_where` | string | — |
| `spark.cdm.filter.java.token.percent` | `filter.token_coverage_percent` | u8 1–100 | `100` |
| `spark.cdm.filter.java.writetime.min` | `filter.writetime.min` | i64 | — |
| `spark.cdm.filter.java.writetime.max` | `filter.writetime.max` | i64 | — |
| `spark.cdm.filter.java.column.name` | `filter.column.name` | string | — |
| `spark.cdm.filter.java.column.value` | `filter.column.value` | string | — |

#### 3.5.10 Features — **CFG-190**

| legacy | canonical | type | default |
|---|---|---|---|
| `spark.cdm.feature.constantColumns.names` | `feature.constant_columns.names` | list | — |
| `spark.cdm.feature.constantColumns.values` | `feature.constant_columns.values` | string | — |
| `spark.cdm.feature.constantColumns.splitRegex` | `feature.constant_columns.split_regex` | regex | `,` |
| `spark.cdm.feature.explodeMap.origin.name` | `feature.explode_map.origin_column` | string | — |
| `spark.cdm.feature.explodeMap.target.name.key` | `feature.explode_map.target_key_column` | string | — |
| `spark.cdm.feature.explodeMap.target.name.value` | `feature.explode_map.target_value_column` | string | — |
| `spark.cdm.feature.extractJson.originColumn` | `feature.extract_json.origin_column` | string | — |
| `spark.cdm.feature.extractJson.propertyMapping` | `feature.extract_json.property_mapping` | string | — |
| `spark.cdm.feature.extractJson.overwrite` | `feature.extract_json.overwrite` | bool | `false` |
| `spark.cdm.feature.extractJson.exclusive` | `feature.extract_json.exclusive` | bool | `false` |
| `spark.cdm.feature.guardrail.colSizeInKB` | `feature.guardrail.column_size_kb` | f64 | `0` |

**`feature.guardrail.column_size_kb` is a float, unlike Java.** Java types it as `NUMBER`, which it
parses with `Long.parseLong`, so a fractional threshold such as `0.5` fails to parse and becomes
`null` there. cdm-rs accepts a fraction, since `GRD-002` multiplies the value by 1000 to get bytes
and a fraction is meaningful. Integral values behave identically.

**`spark.cdm.feature.constantColumns.types` is deliberately not implemented.** It appears in Java's
`cdm-detailed.properties` but is absent from `KnownProperties` and from Java's source, so it has
never had an effect. Constant-column types are resolved from the target schema (`FEA-011`).

#### 3.5.11 New cdm-rs sections — **CFG-200**

| canonical | type | default | purpose |
|---|---|---|---|
| `server.enabled` | bool | `false` | start the HTTP control plane |
| `server.bind` | socket | `127.0.0.1:8080` | |
| `server.auth.mode` | enum `none`\|`bearer`\|`mtls` | `none` | `SEC-010` |
| `metrics.prometheus.enabled` | bool | `true` when server enabled | `MET-020` |
| `metrics.otlp.endpoint` | url | — | `MET-021` |
| `metrics.events.sink` | enum `none`\|`stdout_json`\|`file` | `none` | `MET-030` |
| `metrics.events.path` | path | `cdm_logs/cdm_events.ndjson` | the file the `file` sink appends to, `MET-030` |
| `cluster.enabled` | bool | `false` | distributed mode `DST-001` |
| `cluster.node_id` | string | hostname+pid | |
| `cluster.lease_duration` | duration | `60s` | `DST-012` |
| `cluster.heartbeat_interval` | duration | `15s` | |
| `cluster.ratelimit_is_global` | bool | `false` | `ENG-004` |
| `logging.level` | string | `info` | |
| `logging.format` | enum `pretty`\|`json`\|`compact` | `pretty` | |
| `logging.diff_file` | path | `cdm_logs/cdm_diff.log` | parity with Java diff log |

---

## 4. Connectivity (`CON`)

**CON-000 [N]** — The CQL driver MUST be
[`scylla-rust-driver`](https://github.com/scylladb/scylla-rust-driver) (crate `scylla`), which is
protocol-compatible with Apache Cassandra, DSE, HCD, Astra DB and ScyllaDB. It is the sole driver
dependency; the Java driver, `cdrs-tokio` and `cassandra-cpp` are not used. Required crate features:
`rustls-023` (TLS), `metrics`, `chrono-04`, `num-bigint-04`, `bigdecimal-04`. (Earlier revisions
also listed `cloud`. No such feature exists in `scylla` 1.x — the Scylla Cloud serverless support it
named was removed — and `CON-023` forbids relying on it for Astra in any case; see `ADR-0009`.)
All driver usage MUST be confined to the `cdm-cql` crate behind
the `RowSource`/`RowSink`/`SessionHandle` abstractions, so no other crate depends on `scylla`
directly and the driver remains replaceable. See `ADR-0002`.

**CON-001 [P]** — cdm-rs MUST connect independently to origin and target with fully separate
credentials, TLS material, consistency levels and rate limits.

**CON-002 [P]** — Four connection modes MUST be supported per side, selected exactly as Java's
`ConnectionFetcher` does:
1. secure-connect-bundle path,
2. truststore + `isAstra=true` → generate an SCB from the supplied material,
3. truststore/keystore TLS to a self-managed cluster,
4. plain (no TLS).

**CON-003 [P]** — Astra secure-connect-bundles MUST be supported, including SNI-proxy address
translation and per-node SNI names obtained from the bundle's metadata service. Because
`scylla-rust-driver` does **not** natively support the Astra SCB mechanism, cdm-rs implements it in
`cdm-cql`; the full procedure is normative in §4.1 (`CON-020`–`CON-029`) and `ADR-0009`.

**CON-004 [P]** — When `astra.database_id` is set and no SCB path is given, the bundle MUST be
downloaded from the Astra DevOps API
(`POST https://api.astra.datastax.com/v2/databases/{id}/secureBundleURL?all=true`, `Authorization:
Bearer <password>`), selecting by `scb_type` (`default` / `custom`) and `region`, and matching
`custom_domain` for custom bundles. `all=true` returns bundles for every region and every custom
domain, so selection happens client-side.

**CON-005 [P]** — Downloaded/generated bundles MUST be written to a process-scoped temporary
directory with `0700` permissions and deleted on run completion **and** on abnormal termination
(`Drop` guard plus a signal handler). *(Java `DataUtility.deleteGeneratedSCB`.)*

### 4.1 Astra secure-connect-bundle handling (`CON-020`–`CON-029`)

**CON-020 [N]** — The bundle is a zip named `secure-connect-<database>.zip` containing eight files.
cdm-rs MUST read it **in memory** (no extraction to disk beyond `CON-005`'s temp dir when a
download occurs) and MUST tolerate additional or missing optional members:

| File | Purpose | Required by cdm-rs |
|---|---|---|
| `config.json` | Connection metadata, incl. the metadata-service `host` and `port` | **yes** |
| `ca.crt` | DataStax CA public certificate — the trust anchor | **yes** |
| `cert` | Client certificate for mutual TLS, unique to the bundle | **yes** |
| `key` | Client private key for mutual TLS | **yes** |
| `cqlshrc` | Contains the CQL `port`; used by the fallback strategy (`CON-026`) | optional |
| `cert.pfx` | PKCS#12 archive of `cert` + `key` | ignored |
| `identity.jks` | Java keystore of `cert` + `key` | ignored |
| `trustStore.jks` | Java keystore of `ca.crt` | ignored |

The `.jks` and `.pfx` members MUST be ignored in favour of the PEM members — there is no reason to
parse a Java keystore when the same material is present as PEM. (`CON-006`'s JKS reader exists for
user-supplied truststores, not for bundles.)

**CON-021 [N]** — `config.json` MUST be parsed leniently: unknown fields ignored, and the fields
cdm-rs requires resolved by name with documented fallbacks — `host` (metadata service hostname),
`port` (metadata service port), and, when present, `keyspace`, `localDC`, `caCertLocation`,
`keyLocation`, `certLocation`, `sniHost`, `sniPort`, `hostIds`. A missing required field MUST be a
Tier-1 diagnostic naming the field and the bundle path, not a panic or a generic parse error.

**CON-022 [N]** — **Primary strategy — SNI-aware (full fidelity).** cdm-rs MUST:

1. Build a rustls `ClientConfig` with `ca.crt` as the sole trust anchor and (`cert`, `key`) as the
   client identity for mutual TLS.
2. `GET https://<config.host>:<config.port>/metadata` over that mTLS connection.
3. Parse the response, which has the shape:

   ```json
   {
     "version": 1,
     "region": "us-east1",
     "contact_info": {
       "type": "sni_proxy",
       "local_dc": "us-east1",
       "contact_points": ["<host-id-uuid>", "<host-id-uuid>", "..."],
       "sni_proxy_address": "<proxy-host>:<proxy-port>"
     }
   }
   ```

4. Use `contact_info.local_dc` as the local datacenter for the load-balancing policy (`CON-009`).
5. Open **every** CQL connection to `sni_proxy_address`, setting the TLS SNI `server_name` to the
   target node's **host id** (the UUID from `contact_points`, and thereafter any host id learned
   from `system.peers`). The proxy routes on SNI, which is how one TCP endpoint reaches every node
   independently.
6. Install an address translator mapping every node address advertised in `system.local` /
   `system.peers` to `sni_proxy_address`, so topology discovery does not produce unroutable
   addresses.

**CON-023 [N]** — Because the driver's own `cloud` feature targets Scylla Cloud bundles (a different
file layout and metadata contract), cdm-rs MUST NOT depend on it for Astra. Where the driver exposes
the necessary hooks — a custom TLS connector able to set per-connection `ServerName`, and a custom
`AddressTranslator` — cdm-rs MUST use them rather than forking the driver. If a required hook is
absent, the gap MUST be raised upstream and worked around locally in `cdm-cql`, with the workaround
documented in `ADR-0009`.

**CON-024 [N]** — Host-id-to-SNI mapping MUST be refreshed when topology changes: a node added to
`system.peers` becomes reachable through the same proxy with its own host id as SNI, without a
restart.

**CON-025 [N]** — The metadata response MUST be re-fetched when all connections fail with a TLS or
routing error, since `sni_proxy_address` can change. Re-fetch MUST be rate-limited (at most once per
`connect.{side}.astra.metadata_refresh_interval`, default `5m`).

**CON-026 [N]** — **Fallback strategy — single-endpoint mTLS.** When SNI routing is unavailable
(hook missing, metadata service unreachable, or `connect.{side}.astra.mode = single_endpoint`),
cdm-rs MUST fall back to the connection method documented for drivers without SCB support: connect
directly to `config.json`'s `host` on the port taken **from `cqlshrc`** — not from `config.json`,
whose other ports do not serve CQL — using the same mTLS material, with authentication by
Client ID / Client Secret (mapped from `connect.{side}.username` / `.password`).

**CON-027 [N]** — The fallback MUST emit a prominent warning: it terminates on a single endpoint, so
token-aware routing and per-node load balancing are lost, and throughput will be materially lower.
The warning MUST name `CON-026` and link to the migration guide.

**CON-028 [N]** — Astra authentication MUST accept both spellings: the literal token in
`connect.{side}.password` with username `token` (the modern form, also used as the DevOps API bearer
token per `CON-004`), and a Client ID / Client Secret pair. Validation MUST detect the mismatch where
a user supplies an `AstraCS:` token as the *username*.

**CON-029 [N]** — `cdm connect test --side <side>` MUST report, for an Astra side: the bundle path or
download source, the resolved strategy (`sni` or `single_endpoint`), the metadata service URL, the
resolved `sni_proxy_address`, `local_dc`, the discovered host-id count, and the negotiated TLS
version and cipher. This is the primary diagnostic for Astra connectivity problems.

**CON-006 [P]** — Truststores/keystores MUST be readable in `JKS`, `PKCS12` and `PEM` formats.
JKS support MUST be implemented natively (no JVM).

**CON-007 [P]** — `tls.cipher_suites` MUST be honoured. Requesting a cipher suite unsupported by the
TLS backend MUST produce a Tier-1 error listing the supported set, rather than silently negotiating
something else.

**CON-008 [N]** — `cdm connect test --side origin|target|both` MUST perform a full connect,
report negotiated protocol version, TLS version and cipher, cluster name, partitioner, datacenter
topology, and driver-visible schema version agreement.

**CON-009 [N]** — Connections MUST use a token-aware, DC-aware, latency-aware load-balancing policy
by default, with `connect.{side}.local_datacenter` configurable and auto-detected when unset.

**CON-010 [N]** — Speculative execution MUST be configurable per side and disabled by default for
target writes.

**CON-011 [P+]** — Retry policy: idempotent reads retry on timeout/unavailable up to
`perfops.retry.max_attempts` with exponential backoff and jitter. Writes MUST be treated as
idempotent **only** for non-counter tables; counter writes MUST NOT be retried automatically
(at-most-once), and a counter write failure MUST fail the partition range. **This is stricter than
Java** and prevents silent counter drift — **CON-012 [P+]**.

**CON-013 [N]** — Origin and target compatibility MUST be probed at startup: protocol version,
whether `WRITETIME`/`TTL` on collections is supported, whether vector types are supported. Findings
feed Tier-3 validation. (`scylla-rust-driver` 1.7 exposes no accessor for the negotiated native
protocol version, so the probe reports `system.local`'s `cql_version` and `release_version` and
leaves the protocol version unset until the driver exposes one; see `ADR-0009`.)

---

## 5. Schema handling (`SCH`)

**SCH-001 [P]** — Origin and target table metadata MUST be introspected from `system_schema`:
columns, CQL types (including UDTs, tuples, collections, vectors, frozen-ness), partition key
columns in order, clustering columns in order and direction, and whether the table is a counter
table.

**SCH-002 [P]** — Quoted, mixed-case and special-character identifiers MUST be handled correctly on
both read and write. Any identifier requiring quoting MUST be emitted double-quoted with internal
quotes doubled. *(SIT `05_reserved_keyword`, `02_ColumnRenameWithConstantsAndExplode`.)*

**SCH-003 [P]** — Column mapping: `schema.origin.column.rename` supplies explicit `origin:target`
pairs; all remaining identically-named columns map automatically. A pair referencing a non-existent
column MUST be a Tier-3 error naming the column and the side.

**SCH-004 [P]** — `schema.origin.column.skip` MUST remove the named non-key columns from the origin
projection. Attempting to skip a primary-key column MUST be a Tier-3 error.

**SCH-005 [P]** — Counter tables MUST be auto-detected; the write path switches from `INSERT` to
`UPDATE ... SET c = c + ?` (`MIG-030`).

**SCH-006 [P]** — The target primary key MUST be derivable from: mapped origin columns, constant
columns, and explode-map key/value columns. If any target PK component is underivable, the run MUST
fail Tier-3 validation naming the component.

**SCH-007 [P]** — Virtual projection columns `TTL(col)` and `WRITETIME(col)` MUST be appendable to
the origin select and addressable by index. *(Java `CqlTable.extendColumns`.)*

**SCH-008 [N]** — `cdm schema diff` MUST print a side-by-side origin/target schema comparison with
per-column mapping, conversion plan (`CDC-010`), and any incompatibilities, and MUST be available at
`GET /v1/schema/diff`.

**SCH-009 [N]** — Schema changes detected mid-run (via driver schema-agreement events) MUST abort
the run with a distinct error kind rather than producing partial/incorrect writes.

**SCH-010 [P]** — Materialized views MUST be rejected as a target with a clear message.

---

## 6. Token-range planning (`TOK`)

**TOK-001 [P]** — The origin partitioner MUST be detected. `Murmur3Partitioner`,
`RandomPartitioner` and `ByteOrderedPartitioner` MUST be recognised; unknown partitioners MUST
fail with a clear error.

**TOK-002 [P]** — Default token bounds: Murmur3 → `[i64::MIN, i64::MAX]`; Random →
`[0, 2^127 - 1]`. `filter.token.min` / `.max` override them.

**TOK-003 [P]** — The ring MUST be split into `perfops.num_parts` ranges using the Java algorithm
*exactly*, including its edge cases:

```text
if coverage_percent < 1 or coverage_percent > 100 { coverage_percent = 100 }   // Java's clamp
partition_size = (max - min) / num_parts;  if partition_size == 0 { partition_size = 100_000 }
cur_max = min
while cur_max <= max {                                 // the guard, not `exhausted`, ends the
    cur_min  = cur_max                                 // common case where the last range ends
    new_max  = cur_min + partition_size                // exactly on `max`
    exhausted = new_max < cur_max || new_max > max      // overflow or past end
    if exhausted { new_max = max }
    cur_max  = new_max
    span     = (cur_max - cur_min) * coverage_percent / 100
    emit(cur_min, cur_min + span)
    if exhausted { break }
    cur_max += 1
}
```

Both marked lines are load-bearing and were missing from earlier revisions of this section: without
the `while` guard the splitter emits a spurious inverted range past `max` (Java CDM's own unit test,
`min = 1, max = 100, num_parts = 10`, is the witness), and without the clamp a configured coverage
of `0` would mean *no* coverage where Java means *full* coverage.

All arithmetic MUST be performed in `i128` (Murmur3) or arbitrary precision (Random) to avoid the
overflow the Java code defends against. — **TOK-004 [P]**

**TOK-005 [P]** — `filter.token_coverage_percent` < 100 MUST shrink each emitted range from its
lower bound, producing a deterministic random *sample* of the ring. Java parity is exact.

**TOK-006 [P]** — The emitted range list MUST be shuffled (Java shuffles twice) before scheduling,
to spread load across replicas. The shuffle MUST be seeded from `run_id` so a run is reproducible —
**TOK-007 [N]**.

**TOK-008 [N]** — An alternative planner `plan.strategy = ring_aware` MUST be available: split along
actual ring ownership boundaries so every range maps to a single replica set, enabling
token-aware routing with zero coordinator hops. `plan.strategy = fixed` (default) reproduces Java.

**TOK-009 [N]** — `cdm plan` MUST emit the computed plan (range count, span histogram, estimated
rows from `system.size_estimates`, estimated duration at the configured rate limit) without touching
data, and MUST be available at `POST /v1/plan`.

**TOK-010 [N]** — Ranges SHOULD be sized adaptively when `plan.strategy = adaptive`: begin at
`num_parts`, and dynamically subdivide any range whose observed row count exceeds
`plan.max_rows_per_range`, so stragglers do not dominate wall clock.

---

## 7. Execution engine (`ENG`)

**ENG-001 [N]** — The engine MUST be a Tokio-based work-stealing scheduler. `perfops.workers` tasks
each pull the next unclaimed partition range, process it to completion, record its outcome, and
repeat. There is no Spark, no JVM and no external scheduler.

**ENG-002 [P]** — A partition range is the unit of atomicity for tracking and resume: it is marked
`STARTED` before work begins and `PASS`/`FAIL`/`DIFF`/`DIFF_CORRECTED` on completion.

**ENG-003 [P]** — Origin reads MUST be paged with page size `perfops.fetch_size` and streamed —
never fully materialised.

**ENG-004 [P]** — Two independent rate limiters MUST be enforced: origin rows read per second and
target rows written per second. Limits are **per node** (matching Java's per-worker semantics);
`cluster.ratelimit_is_global = true` **[N]** MUST divide the configured limit across live nodes.

**ENG-005 [N]** — Rate limiting MUST use a token-bucket with burst = 1 second of budget, and MUST
apply backpressure (await) rather than dropping or spinning.

**ENG-006 [N]** — `perfops.adaptive_ratelimit = true` MUST reduce the effective rate when the target
reports overload (write timeouts, `OVERLOADED`, rising p99 latency) and recover gradually — an
AIMD controller with configurable bounds.

**ENG-007 [N]** — In-flight requests MUST be bounded by `perfops.max_inflight_reads` /
`max_inflight_writes` semaphores so memory is bounded regardless of range size.

**ENG-008 [P+]** — Per-range failure handling: an error MUST NOT abort the run. The range is marked
`FAIL`, `PARTITIONS_FAILED` is incremented, `ERROR` is incremented by
`read − written − skipped` (migrate) or `read − valid − missing − mismatch − skipped` (validate),
the error is logged with the range bounds, and the worker proceeds to the next range.

All terms MUST be read at the **interim** level. `DiffJobSession` reads them at the committed level
on the failure path, where `flush()` has not yet run, so every term is `0` and **Java's `ERROR` is
always incremented by exactly `0` for a failed validate range** — the counter that exists to say how
many rows were lost reports none. `CopyJobSession` gets this right for migrate. cdm-rs uses interim
counts for both. `--compat-java` does not restore the bug.

**ENG-009 [P+]** — `perfops.error_limit > 0` MUST abort the run once total `ERROR` exceeds it,
draining in-flight work cleanly. *(Documented in Java's properties file but not implemented there;
cdm-rs implements it.)*

"Total `ERROR`" is the **run's committed count** (`MET-004`) across every range that has completed:
the rows a failed range lost (`ENG-008`) *and* the rows a job counted as errors without failing its
range. `ERROR` counts rows, not ranges, so a run that loses rows steadily without ever failing a
range MUST still reach the limit. The comparison MUST be made at each range boundary, after that
range's counters have been flushed and merged, and MUST be strictly greater than the limit. The run
is marked `ABORTED`.

**ENG-010 [N]** — `SIGINT`/`SIGTERM` MUST trigger graceful shutdown: stop claiming new ranges, let
in-flight ranges finish (bounded by `perfops.shutdown_grace`, default `60s`), flush metrics, mark
the run `INTERRUPTED`, and exit `4` (`CLI-004`) — the one exit code a supervisor may retry
unchanged.

The grace period is a deadline, not a request: a range that has not finished when it expires MUST
be abandoned rather than waited for, and is left `STARTED` so that `TRK-031` re-plans it. A second
signal MUST apply that deadline immediately instead of waiting for it. The run's status after a
second signal is still `INTERRUPTED`: escalating the abandonment does not change who stopped the
run.

**ENG-011 [N]** — Every range's processing MUST be wrapped in a `tracing` span carrying
`run_id`, `range_min`, `range_max`, `node_id`, so all logs and metrics are correlatable. This
supersedes Java's log4j `ThreadContext` "ThreadLabel" and MUST additionally emit the Java-compatible
`min:max` label when `logging.format = pretty` — **ENG-012 [P]**.

**ENG-013 [N]** — Panics inside a worker MUST be caught at the range boundary, converted to a range
failure, and MUST NOT poison the run.

**ENG-014 [N]** — The engine MUST expose a `Pause`/`Resume` control that stops issuing new work
without losing the plan, driven by `POST /v1/runs/{id}:pause`.

The same control MUST expose an operator-requested **stop**, driven by `POST /v1/runs/{id}:cancel`
and `cdm runs cancel` (`TRK-034`). A stop drains in-flight ranges exactly as `ENG-010` does,
subject to the same `perfops.shutdown_grace` deadline, and marks the run `ABORTED` — not
`INTERRUPTED`, which is reserved for a signal, because a run row must say whether the operator
ended this run deliberately or the process was stopped underneath it. Unlike a pause, a stop is
final: the plan is not resumed in this run, and the ranges nobody claimed are left unclaimed for a
later resume.

---

## 8. Migrate job (`MIG`)

**MIG-001 [P]** — For each origin row in a range: acquire an origin rate-limit permit, increment
`READ`, build the target primary key, apply filters, expand exploded records, bind, write, count.

**MIG-002 [P]** — Rows rejected by any filter MUST increment `SKIPPED` and MUST NOT be written.

**MIG-003 [P]** — A record whose bind produces no statement (e.g. all-null exploded map) MUST
increment `SKIPPED`.

**MIG-004 [P+]** — Writes MUST be issued asynchronously with bounded concurrency, and flushed when
`UNFLUSHED >= flush_threshold` where
`flush_threshold = min(fetch_size, max(batch_size * 10, 100))`.

> **Java's threshold is unreachable.** `CopyJobSession` compares the **committed** `UNFLUSHED`
> against the threshold, but `UNFLUSHED` is only ever incremented at the **interim** level and is
> reset before each flush, so the committed value is permanently `0`. Java therefore flushes exactly
> once, at the end of each range, buffering every write for the range in memory. That is a
> significant part of why Java CDM documents `--driver-memory 25G`.
>
> cdm-rs compares the interim count, so the threshold works as documented. This is required for
> `NFR-003` — with Java's behaviour, peak memory scales with range size and no configuration can
> bound it. `--compat-java` does **not** restore the unreachable comparison; reproducing an
> unbounded-memory bug has no legitimate use.

**MIG-005 [P]** — On flush, `WRITE` MUST be incremented by the number of successfully written rows.
A flush failure MUST fail the whole range (`ENG-008`).

**MIG-010 [P]** — INSERT statement shape:
`INSERT INTO ks.tbl (bind_cols..., const_cols...) VALUES (?, ..., <const literals>) [USING TTL ? [AND] TIMESTAMP ?]`.
Constant columns are inlined as CQL literals, never bound.

**MIG-011 [P]** — Bind order MUST be: mapped/derived columns in target-column order, then TTL, then
writetime.

**MIG-012 [P]** — A `null` value, or an **empty collection**, MUST be bound as `UNSET` rather than
`null`, to avoid creating tombstones. *(Java `CqlData.shouldUnsetValue`.)*

**MIG-013 [P]** — A `null` in a target primary-key column MUST be substituted: `String`-typed keys
become `""`; `Instant`-typed keys become `transform.missing_key_ts_replace`; if that is unset the
record MUST be counted as an error with an explanatory message.

**MIG-014 [P]** — `transform.map_remove_null_value = true` MUST strip map entries with null values
before binding.

**MIG-020 [P]** — Batching: when `batch_size > 1`, writes MUST be accumulated into an
`UNLOGGED` batch and executed when the batch reaches `batch_size`.

**MIG-021 [P]** — `batch_size` MUST be coerced to 1 when the table is a counter table, when a
writetime filter is active, or when the configured value is < 1.

**MIG-022 [N]** — Batches SHOULD be grouped by partition key so that a batch never spans partitions
(single-partition batches are the only performant kind). When `perfops.batch_grouping = strict`
(default **[N]**), rows for different partitions are never batched together; `= legacy` reproduces
Java's index-order batching exactly.

**MIG-030 [P]** — Counter tables MUST use
`UPDATE ks.tbl [USING TTL ? AND TIMESTAMP ?] SET c = c + ?, ... , <const> = <literal> WHERE <target pk>`,
with TTL/writetime bound **first**, then non-PK columns, then PK where-clause binds.

**MIG-031 [P]** — The counter delta MUST be `origin_value − (current_target_value or 0)`, obtained
by a rate-limited target SELECT by PK immediately before the write.

**MIG-032 [P]** — Counter migration MUST NOT be batched and MUST NOT be retried (`CON-012`).

**MIG-040 [N]** — When origin and target column types are identical and no feature transforms the
column, the raw serialized bytes MUST be passed through without deserialize/reserialize. This
"zero-copy passthrough" is the default fast path and MUST be provably lossless (property test
`TST-030`).

**MIG-041 [N]** — `migrate --dry-run` MUST execute the full read + transform + bind pipeline and
count everything, but issue no target writes, reporting exactly what would be written.

---

## 9. Validate / diff job (`VAL`)

**VAL-001 [P]** — For each origin row: build the target PK, apply filters, then issue an
asynchronous target SELECT by PK; buffer records and compare in batches of `fetch_size`.

**VAL-002 [P]** — Missing target row → increment `MISSING`, log at ERROR:
`Missing target row found for key: <pk>`.

**VAL-003 [P]** — `autocorrect.missing = true` → synchronously upsert the record, increment
`CORRECTED_MISSING`, log `Inserted missing row in target: <pk>`.

**VAL-004 [P]** — For counter tables, a missing row MUST NOT be auto-corrected unless
`autocorrect.missing_counter = true`; otherwise log and skip the correction.

**VAL-005 [P]** — Column comparison MUST convert the target value into the origin's type space and
compare for equality. Both null → equal. Origin null with target non-null → mismatch. Constant
columns MUST be excluded from comparison.

**VAL-006 [P]** — A mismatch MUST increment `MISMATCH` and log at ERROR:
`Mismatch row found for key: <pk> Mismatch: <detail>` where detail lists, per differing column,
`Target column:<name>-origin[<formatted>]-target[<formatted>]; `.

**VAL-007 [P]** — `autocorrect.mismatch = true` → upsert the record, increment
`CORRECTED_MISMATCH`, log `Corrected mismatch row in target: <pk>`.

**VAL-008 [P]** — A fully-matching record increments `VALID`.

**VAL-009 [P]** — Per-column comparison errors MUST be captured into the mismatch detail rather than
failing the range, in the form
`Target column:<name> Exception <e> targetIndex:<i> originIndex:<j>`.

**VAL-010 [P]** — Validation MUST never delete data from the target.

**VAL-011 [P]** — With `feature.extract_json` active and `overwrite = false`, an already-populated
target extract column MUST be skipped rather than compared.

**VAL-012 [P]** — The diff logger MUST write to a dedicated sink (`logging.diff_file`, default
`cdm_logs/cdm_diff.log`) at ERROR level, separate from the main log, exactly as Java's log4j2
`cdm_diff.log` appender does.

**VAL-013 [N]** — A machine-readable diff report MUST be produced when `validate.report.format` is
`json`/`ndjson`/`csv`/`parquet`, one record per discrepancy, containing run id, token range, primary
key, discrepancy kind, and per-column origin/target values (redactable via
`validate.report.redact_values`).

**VAL-014 [N]** — `GET /v1/runs/{id}/discrepancies` MUST page over that report.

**VAL-015 [N]** — `validate --sample <percent>` MUST be sugar for `filter.token_coverage_percent`,
and `validate --keys-only` MUST compare existence only (much faster pre-flight).

**VAL-016 [P]** — Run status resolution: any discrepancy with
`MISSING == CORRECTED_MISSING && MISMATCH == CORRECTED_MISMATCH` → `DIFF_CORRECTED`; any remaining
discrepancy → `DIFF`; none → `PASS`; exception → `FAIL`.

---

## 10. Guardrail job (`GRD`)

**GRD-001 [P]** — The guardrail job reads the **origin only**; no target connection is required or
opened.

**GRD-002 [P]** — For each row, every column's serialized size MUST be computed and compared against
`feature.guardrail.column_size_kb * 1000` bytes (note: Java uses a 1000, not 1024, factor —
parity is required).

**GRD-003 [P]** — A row with at least one oversized column increments `LARGE` and logs
`Large columns (KB): col(12.345),col2(...)` with three-decimal formatting; otherwise `VALID`.

**GRD-004 [N]** — Guardrail MUST additionally be runnable *inline* during migrate/validate
(`feature.guardrail.mode = check|warn|block`), where `block` skips oversized rows and counts them
in `LARGE` rather than writing them.

**GRD-005 [N]** — Additional guardrails MUST be pluggable via `PLG-003`: partition size, row count
per partition, collection cardinality, and tombstone density are shipped as built-ins in v1.1.

---

## 11. Features (`FEA`)

All features implement the `Feature` trait (`PLG-002`) with `load`, `validate`, `is_enabled`, and
the relevant pipeline hooks. Every feature MUST be independently unit-testable without a cluster.

### 11.1 Constant columns

**FEA-010 [P]** — `feature.constant_columns.names` and `.values` (split by `.split_regex`) define
target columns written with fixed literal values.
**FEA-011 [P]** — Values MUST be parsed and type-checked against the target column type at
validation time.
**FEA-012 [P]** — Constant columns that are part of the target primary key MUST participate in the
PK and appear as literals in generated WHERE clauses.
**FEA-013 [P]** — Constant columns MUST be excluded from validate comparison.
**FEA-014 [P]** — Constant columns present on origin but absent on target MUST be droppable, and
origin constants MUST be replaceable by different target constants. *(SIT `06`, `07`.)*

### 11.2 Explode map

**FEA-020 [P]** — `feature.explode_map.origin_column` MUST be a `map` column on origin; each entry
produces one target row with the key and value written into
`target_key_column` / `target_value_column`.
**FEA-021 [P]** — Key and value MUST be converted from the map's element types to the target column
types using the standard conversion machinery (`CDC-010`).
**FEA-022 [P]** — The exploded key and/or value MAY be part of the target primary key.
**FEA-023 [P]** — A null or empty map MUST produce zero target rows and count as `SKIPPED`.

### 11.3 Extract JSON

**FEA-030 [P]** — `feature.extract_json.origin_column` names a text column containing a JSON object;
`property_mapping` is `jsonField:targetColumn` (or a bare `name` used for both).
**FEA-031 [P]** — The extracted value MUST be written to the mapped target column.
**FEA-032 [P]** — `overwrite = false` MUST leave an already-populated target column untouched.
**FEA-033 [P+]** — `exclusive = true` MUST restrict the non-PK target columns to the extract column
alone, matched by **exact** name. Java matches with `name.endsWith(extractColumn)`, so configuring
`city` also retains `oldcity` — a silent widening of a setting whose only purpose is to narrow.
**FEA-034 [P+]** — Malformed JSON MUST increment `ERROR` for that record and log the primary key,
rather than failing the range.
**FEA-035 [N]** — `property_mapping` MUST accept JSON-Pointer paths (`/a/b/0`) in addition to
top-level field names.

### 11.4 TTL and writetime

**FEA-040 [P]** — A row's writetime is the **maximum** `WRITETIME(col)` over the eligible columns,
plus `transform.custom_writetime_increment`. A row's TTL is the **maximum** `TTL(col)` over eligible
columns (0 if none).
**FEA-041 [P]** — Eligible columns are non-key columns that are primitive, tuple, or frozen; unfrozen
collections are eligible only when `schema.ttl_writetime.use_collections = true`.
**FEA-042 [P]** — `ttl.automatic` / `writetime.automatic` (default true) select all eligible columns;
supplying explicit names disables automatic mode for that dimension.
**FEA-043 [P]** — When reading writetimes from a collection column, the result is a list of values
and the maximum across the list MUST be taken.
**FEA-044 [P]** — `transform.custom_writetime > 0` overrides the computed writetime;
`transform.custom_ttl > 0` overrides the computed TTL.
**FEA-045 [P]** — TTL/writetime MUST be disabled for counter tables. A counter column on *either*
side disables the feature: Java inspects the origin only, while `CFG-036` phrases the same rule in
terms of the target, and neither side can accept a TTL or a timestamp on write.
**FEA-046 [P]** — When no writetime is resolvable, `USING TIMESTAMP` MUST be omitted (server assigns);
likewise `USING TTL` when TTL is 0.

### 11.5 Filters

**FEA-050 [P+]** — `filter.cql_where` MUST be appended to the origin range select, prefixed with
` AND ` unless the user's string already begins with the `AND` **keyword**, before `ALLOW FILTERING`.
Java tests the prefix with `toUpperCase().startsWith("AND")`, which also matches a condition that
merely starts with those three letters — `android_id = 1` then loses its conjunction and the
statement fails to parse. cdm-rs requires a word boundary after the keyword.
**FEA-051 [P]** — `filter.writetime.min` / `.max` MUST skip rows whose computed row writetime falls
outside the window.
**FEA-052 [P]** — `filter.column.name` + `filter.column.value` MUST skip rows where the named text
column equals the value, compared case-insensitively after trimming.
**FEA-053 [P]** — `filter.token.min` / `.max` bound the planned ring segment (`TOK-002`).
**FEA-054 [N]** — Filters MUST be composable and pluggable (`PLG-003`); a filter chain evaluates in
declaration order and short-circuits.

### 11.6 Origin select

**FEA-060 [P]** — The origin range select MUST be
`SELECT <projection> FROM ks.tbl WHERE TOKEN(<pk>) >= ? AND TOKEN(<pk>) <= ? <where> ALLOW FILTERING`
with bind values typed per partitioner (`i64` for Murmur3, big integer for Random).
**FEA-061 [P+]** — `ALLOW FILTERING` MUST be omitted when no `filter.cql_where` is configured, since
a pure token-range scan does not require it. `--compat-java` restores unconditional emission.
**FEA-062 [N]** — The generated CQL for every statement MUST be logged once at startup and exposed
at `GET /v1/runs/{id}/statements`.

---

## 12. Codecs and type conversion (`CDC`)

**CDC-001 [P]** — All CQL primitive types MUST be supported: `ascii, bigint, blob, boolean, counter,
date, decimal, double, duration, float, inet, int, smallint, text, time, timestamp, timeuuid,
tinyint, uuid, varchar, varint`.

**CDC-002 [P]** — Collection types `list<T>`, `set<T>`, `map<K,V>`, `tuple<...>`, user-defined types
and `vector<T, N>` MUST be supported, including arbitrary nesting and frozen-ness.

**CDC-003 [P]** — DSE geometry types `PointType`, `LineStringType`, `PolygonType` and
`DateRangeType` MUST be supported (WKB encoding).

**CDC-004 [N]** — `vector<float, N>` MUST be first-class: read, written, validated, and comparable
with exact bit equality. *(SIT `06_vector`.)*

**CDC-010 [P]** — A **conversion plan** MUST be computed once per column pair at startup, not per
row. Plan kinds: `Passthrough` (identical types → raw bytes, `MIG-040`), `Codec`, `Udt`, `List`,
`Set`, `Map`, `Tuple`, `Vector`, `Unsupported`.

**CDC-011 [P]** — Identical origin/target types → `Passthrough`. Types whose Rust representations
are directly assignable → `Passthrough`. Otherwise a registered codec is required.

**CDC-012 [P]** — Collection-to-same-kind-collection conversion MUST recurse into element types.
Cardinality mismatch (e.g. `map` → `list`) is `Unsupported`.

**CDC-013 [P]** — UDT-to-UDT conversion MUST proceed field-by-field and requires equal field counts.
**CDC-014 [P+]** — Unlike Java, UDT field conversion MUST match by **name** when field names are
equal-as-sets, falling back to positional matching, and MUST recurse properly rather than
round-tripping through string formatting. Positional-only behaviour is available under
`--compat-java`.

**CDC-015 [P+]** — Tuple element conversion MUST be implemented. (Java explicitly leaves this
unsupported; cdm-rs closes the gap.)

**CDC-016 [P]** — An `Unsupported` plan MUST pass the value through unchanged and MUST log a warning
naming the column and both types, once per column, at startup — not per row.

**CDC-020 [P]** — The following named codecs MUST be available via `transform.codecs`, with exactly
the Java semantics:

| Codec name | Conversions provided |
|---|---|
| `INT_STRING` | `int` ↔ `text` |
| `DOUBLE_STRING` | `double` ↔ `text`, formatted `0.#########`, no grouping, `RoundingMode.FLOOR`, never scientific notation |
| `BIGINT_STRING` | `bigint` ↔ `text` |
| `DECIMAL_STRING` | `decimal` ↔ `text` |
| `BIGINT_BIGINTEGER` | `bigint` ↔ varint-like big integer, **and** `int` ↔ integer; **always registered** (required to read collection writetimes) |
| `STRING_BLOB` | `text` ↔ `blob` |
| `ASCII_BLOB` | `ascii` ↔ `blob` |
| `TIMESTAMP_STRING_MILLIS` | `timestamp` ↔ epoch-millis string; on decode, an 8-byte buffer is an `Instant`, anything else is UTF-8 text passthrough |
| `TIMESTAMP_STRING_FORMAT` | `timestamp` ↔ string via `transform.codecs.timestamp_format` and `..._zone` |
| `POLYGON_TYPE`, `POINT_TYPE`, `LINE_STRING`, `DATE_RANGE` | DSE geometry / date-range |

**CDC-021 [P]** — An empty `timestamp_format`, or an unparseable timezone, MUST be a Tier-1
configuration error.

**CDC-022 [P]** — Java `SimpleDateFormat`/`DateTimeFormatter` patterns (e.g. `yyyyMMddHHmmss`,
`yyMMddHHmmss`) MUST be accepted verbatim; a pattern translator maps them to the Rust formatter.
Unsupported pattern letters MUST be a Tier-1 error naming the letter.

**CDC-030 [N]** — Codecs MUST be pluggable: `CodecPlugin` registers `(from_type, to_type) →
Converter` pairs into the registry, and third-party crates may add codecs without modifying
cdm-rs (`PLG-001`).

**CDC-031 [N]** — `cdm codecs list` MUST print all registered codecs and the type pairs they serve;
`GET /v1/codecs` MUST return the same.

**CDC-032 [N]** — Every codec MUST have a round-trip property test (`TST-031`).

---

## 13. Run tracking, resume and rerun (`TRK`)

**TRK-001 [P]** — Tracking MUST be enabled when any of: `track_run.enabled = true`,
`track_run.run_id != 0`, `track_run.previous_run_id != 0`, `track_run.auto_rerun = true`.

**TRK-002 [P]** — When enabled and `run_id == 0`, a run id MUST be generated. Java uses
`System.nanoTime()`; cdm-rs MUST use a monotonically-increasing, time-sortable 64-bit id
(`unix_micros << 12 | counter`) — **TRK-003 [P+]** — which preserves the "latest = highest" ordering
Java relies on while eliminating collisions across nodes.

**TRK-010 [P]** — Two tables MUST be created if absent in the **target** keyspace, with exactly the
Java schema so the two tools interoperate:

```sql
CREATE TABLE IF NOT EXISTS "<ks>".cdm_run_info (
    table_name  text,
    run_id      bigint,
    run_type    text,
    prev_run_id bigint,
    start_time  timestamp,
    end_time    timestamp,
    run_info    text,
    status      text,
    PRIMARY KEY (table_name, run_id)
);

CREATE TABLE IF NOT EXISTS "<ks>".cdm_run_details (
    table_name text,
    run_id     bigint,
    start_time timestamp,
    token_min  bigint,
    token_max  bigint,
    status     text,
    run_info   text,
    PRIMARY KEY ((table_name, run_id), token_min)
);
```

**TRK-011 [N]** — cdm-rs MUST additionally maintain, in the same keyspace, `cdm_run_leases` for
distributed coordination (`DST-010`). It MUST be created only when `cluster.enabled = true`, so
single-node runs remain byte-compatible with Java's schema.

**TRK-012 [P]** — Statuses: `NOT_STARTED, STARTED, PASS, FAIL, DIFF, DIFF_CORRECTED, ENDED`.
cdm-rs adds `INTERRUPTED` and `ABORTED` **[N]** for `run_info` rows only.

**TRK-013 [P]** — `cdm_run_info.run_type` MUST be written as the exact upper-case job name
`MIGRATE`, `VALIDATE` or `GUARDRAIL`. Java writes `jobType.toString()` over
`enum JobType { MIGRATE, VALIDATE, GUARDRAIL }`
(`src/main/java/com/datastax/cdm/job/IJobSessionFactory.java`), and the previous-run lookup in
`TRK-030` filters on `run_type = ?`, so a different spelling would silently make every Java run
invisible to cdm-rs and vice versa. This string is part of the `COMPAT-003` contract and MUST NOT
be derived from a `Display` impl that is free to change.

**TRK-014 [P]** — Likewise `cdm_run_info.status` and `cdm_run_details.status` MUST use the exact
Java spellings of `TRK-012`, including the underscore in `NOT_STARTED` and `DIFF_CORRECTED`.
A round-trip test over every variant is required (`COMPAT-003`).

**TRK-020 [P]** — Run initialisation MUST: reject a `run_id` that already exists; insert the info row
as `NOT_STARTED`; insert one details row per planned range as `NOT_STARTED`; then set the info row
to `STARTED`.

**TRK-021 [P]** — Each range MUST be updated to `STARTED` (setting `start_time`) when work begins and
to its terminal status with the metrics string in `run_info` when it completes.

**TRK-022 [P]** — When a run stops, the info row MUST be updated with `end_time`, the aggregate
metrics string, and **the terminal status the scheduler reports**: `ENDED` for a run that
processed its whole plan (Java's only outcome), `INTERRUPTED` for one stopped by a signal
(`ENG-010`), or `ABORTED` for one stopped by the error limit (`ENG-009`). The aggregate metrics
string MUST be rendered from the **committed** counter level (`MET-004`): a run's registry only
ever receives committed values from its ranges, so its interim level is structurally zero, and
reading it would reproduce the `ENG-008` defect in durable form.

Writing `ENDED` unconditionally — as an earlier draft of this requirement said — records an
interrupted run as one that finished, which `TRK-030` then declines to adopt: the unfinished
ranges look complete and no resume ever re-plans them. `ENDED` is therefore reserved for a run
that genuinely completed, and the two cdm-rs statuses of `TRK-012` are what make the distinction
recordable. A Java reader ignores statuses it does not know, so a run row cdm-rs left
`INTERRUPTED` is simply "not `ENDED`" to Java, which is exactly how `TRK-030` should treat it.

**TRK-030 [P]** — `track_run.auto_rerun = true` MUST select the most recent run for
`(table_name, run_type)` and adopt it as the previous run **only if** its status is not `ENDED`, or
its `run_info` contains `Partitions Failed: N` with `N > 0`.

**TRK-031 [P]** — Resuming from a previous run MUST re-plan only ranges whose status is in
`{NOT_STARTED, STARTED, FAIL, DIFF}`, shuffled.

**TRK-032 [P]** — If the previous run's info row is missing or `NOT_STARTED`, resumption MUST fall
back to a fresh full plan (Java's `RunNotStartedException` path), with a warning.

**TRK-033 [P]** — `track_run.rerun_multiplier > 1` MUST subdivide each pending range into that many
sub-ranges at 100% coverage, to break up stragglers.

**TRK-034 [N]** — `cdm runs list|show|resume|cancel` MUST provide first-class run management, and
`GET /v1/runs`, `GET /v1/runs/{id}` the API equivalents.

**TRK-035 [N]** — Tracking writes MUST be batched and asynchronous with a bounded queue, so tracking
never becomes the throughput bottleneck; on queue overflow, tracking degrades to periodic checkpoints
and logs a warning rather than blocking data movement.

**TRK-036 [N]** — Tracking MUST be storable in a pluggable backend (`TrackingStore` trait): the
Cassandra target keyspace (default, Java-compatible), a local SQLite file, or an in-memory store for
tests. This makes tracking usable even when the target cannot host extra tables.

---

## 14. Distributed coordination (`DST`)

Selected design: **built-in coordinator from day one**, lease-based, no external dependency
(no ZooKeeper/etcd) — the tracking keyspace is the coordination substrate.

**DST-001 [N]** — `cluster.enabled = true` MUST allow N cdm-rs processes started with the same
`run_id` and the same config to cooperatively execute one run.

**DST-002 [N]** — Exactly one node MUST perform run initialisation (`TRK-020`). Election MUST use a
lightweight transaction: `INSERT ... IF NOT EXISTS` on the `cdm_run_info` row. Losers wait for the
info row to reach `STARTED`.

**DST-003 [N]** — Configuration consistency MUST be enforced: the initialising node records a hash
of the effective, secret-redacted config in `cdm_run_info.run_info`; joining nodes whose hash differs
MUST refuse to join with a diagnostic diff.

**DST-010 [N]** — Range claiming MUST use `cdm_run_leases`:

```sql
CREATE TABLE IF NOT EXISTS "<ks>".cdm_run_leases (
    table_name  text,
    run_id      bigint,
    token_min   bigint,
    node_id     text,
    lease_until timestamp,
    attempt     int,
    PRIMARY KEY ((table_name, run_id), token_min)
);
```

**DST-011 [N]** — A claim MUST be `INSERT ... IF NOT EXISTS` / `UPDATE ... IF lease_until < now`
(LWT, `SERIAL` consistency). At most one node may hold a range at a time.

**DST-012 [N]** — Leases MUST be renewed every `cluster.heartbeat_interval` while the range is being
processed, and MUST expire after `cluster.lease_duration`. An expired lease MUST be reclaimable by
any node, and the reclaiming node MUST increment `attempt`.

**DST-013 [N]** — A range that has been attempted more than `cluster.max_attempts` (default 3) times
MUST be marked `FAIL` and abandoned rather than looping forever.

**DST-014 [N]** — Reclaiming a range after a node death MUST be safe. For migrate this is guaranteed
by idempotent upserts with preserved writetimes; for counter tables, reclaim of an in-flight range
MUST be refused and the range marked `FAIL` with an explicit "manual reconciliation required"
message — **DST-015 [N]**. *(Counters are not idempotent; correctness beats convenience.)*

**DST-016 [N]** — Metrics MUST be aggregated across nodes: each node periodically writes its counter
snapshot; any node (and the API) can compute the run total. The final `printMetrics` output MUST be
produced by the node that observes the last range completing, and MUST be identical in shape to the
single-node output.

**DST-017 [N]** — A node MUST cleanly deregister on shutdown, releasing its leases immediately rather
than waiting for expiry.

**DST-018 [N]** — `cdm cluster status` and `GET /v1/cluster` MUST list live nodes, their leases,
their per-node throughput and their last heartbeat.

**DST-019 [N]** — Distributed mode MUST be fully exercised in integration tests with simulated node
death mid-range (`TST-042`).

---

## 15. Metrics and observability (`MET`)

### 15.1 Counters (parity)

**MET-001 [P]** — The following counters MUST exist with exactly these semantics:
`READ, WRITE, MISMATCH, CORRECTED_MISMATCH, MISSING, CORRECTED_MISSING, VALID, SKIPPED, LARGE,
ERROR, UNFLUSHED, PARTITIONS_PASSED, PARTITIONS_FAILED`.

**MET-002 [P]** — Per-job registration MUST match Java:

| Job | Registered counters |
|---|---|
| migrate | READ, WRITE, SKIPPED, ERROR, UNFLUSHED, PARTITIONS_PASSED, PARTITIONS_FAILED |
| validate | READ, VALID, MISMATCH, CORRECTED_MISMATCH, MISSING, CORRECTED_MISSING, SKIPPED, ERROR, PARTITIONS_PASSED, PARTITIONS_FAILED |
| guardrail | READ, VALID, SKIPPED, LARGE, PARTITIONS_PASSED, PARTITIONS_FAILED |

Using an unregistered counter MUST be a compile-time or startup error, never a runtime surprise —
**MET-003 [P+]** (Java throws at runtime).

**MET-004 [P]** — The interim/committed two-level accounting MUST be preserved: per-range interim
counts are folded into totals on range completion.

Per-range `run_info` MUST be rendered from the **committed** counts, after the flush — Java calls
`flush()` and then the non-interim `getMetrics()`. The numeric values are the same either way for a
per-range counter, but the interim rendering additionally includes `Unflushed`, which Java never
writes to `cdm_run_details.run_info`. Rendering the interim form would therefore break
`COMPAT-004`.

**MET-005 [P]** — The metrics string format MUST be reproduced exactly:
`Read: 10; Write: 9; Skipped: 1` (title-cased counter names, `; ` separated, `UNFLUSHED` omitted
from non-interim renderings), because SIT assertions and `cdm_run_info.run_info` depend on it.

**MET-006 [P]** — The final metrics block MUST be printed in the Java format so existing
`cdm-assert.sh`-style tooling keeps working:

```text
################################################################################################
RunId: 1712345678901234
Final Read Record Count: 1000000
Final Write Record Count: 999998
Final Skipped Record Count: 2
Final Error Record Count: 0
Final Partitions Passed: 5000
Final Partitions Failed: 0
################################################################################################
```

### 15.2 New observability

**MET-010 [N]** — In addition to counters, the following MUST be recorded: rows/sec (origin and
target, 1s/10s/60s EWMA), bytes/sec, request latency histograms (p50/p90/p99/p999) per side and per
operation, in-flight requests, batch size distribution, retry counts by cause, rate-limiter wait
time, ranges in each state, and estimated time to completion.

**MET-011 [N]** — Progress MUST be computable as `ranges_completed / ranges_total`, refined by
`system.size_estimates` row estimates, with an ETA.

**MET-020 [N]** — A Prometheus endpoint MUST be exposed at `GET /metrics` with metric names prefixed
`cdm_` and labels `{run_id, job, side, node_id, keyspace, table}`. Cardinality MUST NOT include
token ranges or primary keys.

**MET-021 [N]** — OpenTelemetry OTLP export of metrics **and** traces MUST be supported, configured
by `metrics.otlp.endpoint`.

**MET-030 [N]** — A structured event stream MUST be emitted (`RunStarted`, `RangeStarted`,
`RangeCompleted`, `Discrepancy`, `Warning`, `Error`, `RunCompleted`), serialisable as NDJSON, and
streamable over SSE at `GET /v1/runs/{id}/events`. Event schemas are part of the OpenAPI document.

**MET-031 [N]** — An interactive terminal UI (`cdm migrate --tui`) MUST show live throughput,
progress bar, ETA, per-node status in cluster mode, error tail, and latency sparklines. It MUST
degrade automatically to line-based progress when stdout is not a TTY.

**MET-032 [N]** — All logs MUST be `tracing`-based, with `logging.format = json` producing
structured records suitable for ingestion, and MUST never log secrets or, by default, row values
(`SEC-002`).

**MET-033 [N]** — A run summary MUST be writable to a file (`--summary-out report.json`) containing
config hash, plan, all counters, timings, per-node breakdown, and any discrepancies summary — the
artefact users attach to tickets.

---

## 16. Interfaces

### 16.1 CLI (`CLI`)

**CLI-001 [N]** — A single binary `cdm` with subcommands:

```text
cdm migrate            Run a migration
cdm validate           Run a validation (alias: diff)
cdm guardrail          Run guardrail checks
cdm plan               Compute and print the token-range plan (no data access)
cdm runs list|show|resume|cancel|watch
cdm config init|validate|explain|diff|convert|schema
cdm schema show|diff
cdm connect test
cdm codecs list
cdm cluster status
cdm serve              Start the HTTP control plane (+ UI, MCP, A2A)
cdm mcp                Start an MCP server on stdio
cdm completions <shell>
cdm version
```

**CLI-002 [P]** — Java invocation shapes MUST be accepted for a smooth transition:
`--properties-file <file>` and `--conf spark.cdm.<key>=<value>` MUST work on every job subcommand.

**CLI-003 [N]** — `cdm config convert --from cdm.properties --to cdm.toml` MUST translate a Java
config to canonical form, annotating deprecated and defaulted keys.

**CLI-004 [N]** — Exit codes MUST be meaningful and documented:
`0` success · `1` completed with failures/discrepancies · `2` configuration error ·
`3` connection error · `4` interrupted · `5` internal error.

**CLI-005 [N]** — `--output json` MUST render machine-readable output for every non-streaming
command.

**CLI-006 [N]** — `cdm config init` MUST run an interactive wizard (skippable with `--non-interactive`)
that connects, introspects the schema, and produces a tuned config with explanatory comments —
the CLI counterpart of the Config Builder UI.

**CLI-007 [N]** — Shell completions for bash/zsh/fish/powershell MUST be generated, plus a man page.

### 16.2 HTTP control plane and OpenAPI (`API`)

**API-001 [N]** — An **OpenAPI 3.1** document MUST be the single source of truth for the control
plane, checked in at `api/openapi.yaml`, and served at `GET /openapi.yaml` / `GET /openapi.json`.

**API-002 [N]** — The document MUST be **generated from Rust types** (`utoipa` derives on the same
structs used by the engine, including `CdmConfig` from `CFG-001`). CI MUST regenerate and fail on
drift. There is no hand-maintained schema. *(G6, G8.)*

**API-003 [N]** — Endpoints (v1):

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/health`, `/v1/ready` | liveness / readiness |
| `GET` | `/v1/version` | build info, features |
| `GET` | `/v1/config/schema` | JSON Schema of the config model |
| `POST` | `/v1/config/validate` | tiers 1–3 validation, returns all diagnostics |
| `POST` | `/v1/config/generate` | Config Builder: schema + hints → config |
| `POST` | `/v1/config/convert` | Java properties → canonical config |
| `GET` | `/v1/schema` | introspect origin/target schema |
| `GET` | `/v1/schema/diff` | mapping + conversion plan + incompatibilities |
| `POST` | `/v1/plan` | compute a token-range plan |
| `POST` | `/v1/runs` | submit a run (`job`, `config`, `dry_run`) |
| `GET` | `/v1/runs` | list runs (filter, page) |
| `GET` | `/v1/runs/{id}` | run detail incl. counters and progress |
| `GET` | `/v1/runs/{id}/ranges` | per-range status (paged) |
| `GET` | `/v1/runs/{id}/metrics` | counter + rate snapshot |
| `GET` | `/v1/runs/{id}/events` | **SSE** live event stream |
| `GET` | `/v1/runs/{id}/discrepancies` | validate findings (paged) |
| `GET` | `/v1/runs/{id}/statements` | generated CQL |
| `GET` | `/v1/runs/{id}/summary` | the `MET-033` artefact |
| `POST` | `/v1/runs/{id}:pause` / `:resume` / `:cancel` | control |
| `POST` | `/v1/runs/{id}:rerun` | resume failed/pending ranges |
| `GET` | `/v1/cluster` | node membership and leases |
| `GET` | `/v1/codecs`, `/v1/features` | registry introspection |
| `GET` | `/metrics` | Prometheus |

**API-004 [N]** — Run submission MUST be asynchronous: `POST /v1/runs` returns `202` with the run id
and a `Location` header; progress is polled or streamed.

**API-005 [N]** — All errors MUST use RFC 9457 `application/problem+json`, carrying the structured
diagnostic from `ERR-002` (including `key`, `value`, `rule`, `suggestion` for config errors).

**API-006 [N]** — Every mutating endpoint MUST accept an `Idempotency-Key` header; replaying a key
returns the original result rather than starting a second run.

**API-007 [N]** — Pagination MUST be cursor-based and uniform (`?cursor=&limit=`), with `next_cursor`
in the response envelope.

**API-008 [N]** — The API MUST be versioned by path prefix; breaking changes require `/v2`. An
OpenAPI diff check (`oasdiff`) MUST run in CI and fail on undeclared breaking changes.

**API-009 [N]** — Long-running operations MUST also be observable via `GET /v1/runs/{id}` returning
`status` from a documented state machine: `pending → planning → running → (paused) →
{succeeded, failed, cancelled, interrupted}`.

**API-010 [N]** — The server MUST run embedded in the same process that executes the job
(`cdm migrate --serve`) *or* standalone as a controller that executes submitted runs (`cdm serve`).
Both modes share one implementation.

### 16.3 MCP server (`MCP`)

**MCP-001 [N]** — An MCP server MUST be provided, over stdio (`cdm mcp`) and Streamable HTTP
(`/mcp` when serving).

**MCP-002 [N]** — MCP **tools** MUST be generated from the OpenAPI document — one tool per operation
marked `x-mcp: tool`, with the input schema derived from the operation's parameters and request body
and the description from its `summary`/`description`. No hand-written tool definitions.

**MCP-003 [N]** — MCP **resources** MUST expose: the config JSON Schema, the property reference, the
origin/target schema, the current run list, and each run's summary, under `cdm://` URIs.

**MCP-004 [N]** — MCP **prompts** MUST ship for the common workflows: "plan a migration for table X",
"explain this validation report", "tune this config for a 2 TB table".

**MCP-005 [N]** — Destructive or long-running tools (`submit_run`, `cancel_run`) MUST be annotated
with MCP tool hints (`destructiveHint`, `idempotentHint`, `openWorldHint`) so hosts can gate them.

**MCP-006 [N]** — Tool outputs MUST be structured content matching the OpenAPI response schema, not
prose.

### 16.4 A2A (`A2A`)

**A2A-001 [N]** — An A2A **Agent Card** MUST be served at `/.well-known/agent-card.json`, generated
from the same OpenAPI document plus a small hand-written capability manifest.

**A2A-002 [N]** — Declared skills MUST include: `plan-migration`, `run-migration`,
`validate-migration`, `explain-discrepancies`, `tune-configuration`.

**A2A-003 [N]** — A2A tasks MUST map onto the run lifecycle (`API-009`), with streaming task status
updates backed by the same event stream as `MET-030`.

**A2A-004 [N]** — Authentication schemes declared in the agent card MUST match those enforced by the
API (`SEC-010`).

**A2A-005 [N]** — The transport adapter MUST contain **no business logic**: it translates A2A
messages to `cdm-core` service calls, identically to the REST and MCP adapters. A conformance test
asserts all three adapters produce identical results for the same logical request (`TST-050`).

### 16.5 Config Builder UI (`UI`)

**UI-001 [N]** — The Config Builder MUST be reimplemented as a static web app embedded in the binary
(`rust-embed`) and served at `/ui` by `cdm serve`, requiring no Node.js at runtime.

**UI-002 [N]** — It MUST drive the same API as every other client: `POST /v1/config/validate`,
`/v1/config/generate`, `GET /v1/schema`. No client-side duplication of validation rules.

**UI-003 [P]** — Feature parity with the React `cdm-config-builder`: CQL DDL paste-and-parse,
sectioned form (connection, schema, performance, advanced features), live properties preview,
best-practice hints, dark/light theme honouring `prefers-color-scheme`.

**UI-004 [P]** — The best-practice rules engine MUST be preserved and MUST live **server-side** so
CLI, API and UI share it:
table size GB → `num_parts = size_gb × 1024 ÷ 10MB`; rows > 100M → `num_parts ≥ 50000`;
LOB columns → `batch_size = 1`, `fetch_size = 100`; PK == partition key → `batch_size = 1`;
only-collection non-PK columns → `ttl_writetime.use_collections = true`; counter table → warn about
`autocorrect.missing_counter`; > 1 TB → recommend cluster mode.

**UI-005 [N]** — The UI MUST additionally provide live run monitoring (progress, throughput, errors)
by consuming the SSE event stream.

**UI-006 [N]** — The UI MUST be usable offline and MUST NOT make third-party network requests.

---

## 17. Plugin and extension model (`PLG`)

**PLG-001 [N]** — `CodecPlugin` — register conversions between CQL type pairs.
**PLG-002 [N]** — `FeaturePlugin` — participate in config validation, statement construction, record
transformation, and comparison.
**PLG-003 [N]** — `FilterPlugin` / `GuardrailPlugin` — row-level predicates and checks.
**PLG-004 [N]** — `JobPlugin` — register an entirely new job type alongside migrate/validate/guardrail.
**PLG-005 [N]** — `SourcePlugin` / `SinkPlugin` — abstract the origin and target behind
`RowSource` / `RowSink` traits so alternative backends are possible without touching the engine.
**PLG-006 [N]** — `MetricsExporterPlugin` — additional metric sinks.
**PLG-007 [N]** — `TrackingStorePlugin` — alternative tracking backends (`TRK-036`).

**PLG-010 [N]** — All plugins MUST be registered through a single `Registry` built at startup;
built-in implementations MUST use exactly the same registration path as third-party ones (no
privileged built-ins). *(G3.)*

**PLG-011 [N]** — Plugin registration MUST be possible both at compile time (Cargo feature +
`inventory`-style linkage) and at runtime via a documented, opt-in `dylib` loader
(`plugins.load = [path]`, disabled by default, `SEC-020`).

**PLG-012 [N]** — Every plugin trait MUST be object-safe, `Send + Sync`, and documented with a
worked example in `docs/EXTENDING.md` plus a compiling example crate in `examples/`.

**PLG-013 [N]** — Plugins MUST be able to contribute configuration keys, which are then automatically
included in the JSON Schema, OpenAPI, docs and UI (`CFG-001`).

---

## 18. Error model and diagnostics (`ERR`)

**ERR-001 [N]** — A single `CdmError` enum (via `thiserror`) with stable, documented `kind` codes:
`Config`, `Connect`, `Auth`, `Tls`, `SchemaMismatch`, `TypeConversion`, `Read`, `Write`,
`RateLimited`, `Tracking`, `Lease`, `Cancelled`, `Internal`. Every variant carries context (side,
keyspace, table, column, range, primary key where applicable).

**ERR-002 [N]** — A `Diagnostic` type: `{ code, severity, title, detail, location, value, rule,
suggestion, docs_url }`. Every user-visible error MUST be a `Diagnostic`; the same value renders as
CLI text, `problem+json`, and an SSE event.

**ERR-003 [N]** — Every diagnostic code MUST have a page in `docs/errors/<CODE>.md`, and `docs_url`
MUST point at it. CI MUST fail if a code lacks a page.

**ERR-004 [N]** — `unwrap()`/`expect()`/`panic!` MUST be denied by Clippy in all non-test code
except in `main` startup and documented invariants with a `// SAFETY-INVARIANT:` comment.

**ERR-005 [P]** — Bind failures MUST log the column name, its CQL type, the column index, the bind
index, the statement CQL and the **primary key of the offending row** — matching Java's detailed
bind-error diagnostics in everything except the value itself.

> **Correction.** Earlier drafts required the *value* and its type to be logged, as Java's
> `TargetInsertStatement` does. That contradicts `SEC-002`, which forbids logging row values outside
> the validate diff path, and the two cannot both hold. `SEC-002` wins: the value is the one field
> that is a customer's data, every other field is enough to reproduce the failure, and the primary
> key — which Java does *not* log here — identifies the row far better than its contents do. Java's
> `bindValue.getClass().getName()` is likewise not reproduced: it is a Java type name, and cdm-rs
> never decodes the value into one.

---

## 19. Security (`SEC`)

**SEC-001 [N]** — Secrets MUST never appear in logs, metrics, API responses, events, run summaries,
config dumps, or error messages. Enforced by the `Secret<T>` newtype and a CI grep for
`password`/`token` in snapshot fixtures.

**SEC-002 [N]** — Row values MUST NOT be logged by default. `validate` discrepancy detail is the sole
exception, and MUST be redactable via `validate.report.redact_values` (which hashes values instead).

**SEC-010 [N]** — The HTTP control plane MUST support `none` (loopback only), `bearer` token, and
mTLS authentication. `server.auth.mode = none` MUST refuse to bind to a non-loopback address unless
`server.insecure_allow_remote = true` is explicitly set.

**SEC-011 [N]** — The server MUST support TLS termination natively, plus configurable CORS defaulting
to same-origin.

**SEC-012 [N]** — MCP and A2A endpoints MUST enforce the same auth as the REST API.

**SEC-020 [N]** — Runtime dynamic plugin loading MUST be disabled by default and MUST log a prominent
warning when enabled.

**SEC-030 [N]** — Supply chain: `cargo-deny` (licenses, advisories, bans, sources), `cargo-audit`,
`cargo-vet` or equivalent, SBOM (CycloneDX) generated per release, and signed release artefacts
(Sigstore/cosign) MUST be part of CI (`OPS-020`).

**SEC-031 [N]** — `#![forbid(unsafe_code)]` MUST apply to every crate except a documented, minimal,
reviewed allowance (currently: none expected).

---

## 20. Non-functional requirements (`NFR`)

**NFR-001** — Static binary for `linux-x86_64` (gnu + musl), `linux-aarch64`, `macos-x86_64`,
`macos-aarch64`, `windows-x86_64`. Musl builds MUST be fully static.

**NFR-002** — Cold start to first row read MUST be < 2 seconds for a single-table run.

**NFR-003** — Memory MUST be bounded and configurable: steady-state RSS MUST NOT exceed
`~200 MB + (max_inflight_reads + max_inflight_writes) × average_row_size × 2`. No configuration
may cause unbounded growth.

**NFR-004** — Throughput MUST be ≥ 2× Java CDM on the same hardware for the reference workload,
measured by the benchmark suite (`TST-060`).

**NFR-005** — MSRV MUST be an explicitly declared, tested Rust version, bumped only in a minor
release, and stated in `Cargo.toml` `rust-version`.

**NFR-006** — Every public item in every crate MUST have rustdoc; `#![deny(missing_docs)]` on all
library crates.

**NFR-007** — All timestamps in APIs, logs and reports MUST be RFC 3339 UTC. Writetimes remain
microseconds since epoch (Cassandra semantics) and MUST be labelled as such.

**NFR-008** — The tool MUST be usable with no network access to anything except origin, target and
(optionally) the Astra DevOps API.

---

## 21. Testing strategy (`TST`)

### 21.1 Layers

**TST-001** — **Unit tests** live beside the code (`#[cfg(test)]`) and MUST NOT require a cluster.
Every public function with branching logic MUST be tested. Target ≥ 90% line coverage per crate.

**TST-002** — **Integration tests** (`tests/`) MUST run against real clusters via `testcontainers`.

- **Every pull request**: Cassandra `3.11`, `4.0`, `4.1`, `5.0` (tags are `major.minor`, so the
  matrix tracks the latest patch of each line rather than pinning to one that ages out). This is
  the risk surface — it is what CDM migrates, and 3.11 exercises an older protocol.
- **Nightly**: ScyllaDB, plus DSE/HCD where licensing permits. `scylla-rust-driver` (`CON-000`) was
  chosen for its maturity in the Rust ecosystem, which makes ScyllaDB the driver's home turf and
  the least likely target to regress. It remains a separate implementation whose token ownership
  (tablets), LWT behaviour and collection `WRITETIME()` support diverge, so the support claimed in
  `CON-000` MUST be exercised rather than assumed.

A capability that only one engine implements MUST be version-gated and skipped elsewhere, never
failed — `vector<t, n>` is Cassandra 5.0 only (`CDC-004`).

**TST-003** — **End-to-end SIT parity tests**: every Java SIT case MUST be ported to a declarative
Rust harness and MUST assert the identical counter block.

| Java SIT case | cdm-rs test | Covers |
|---|---|---|
| `smoke/00_test_harness` | `sit::harness` | harness self-check |
| `smoke/01_basic_kvp` | `sit::basic_kvp` | MIG-001, VAL-001 |
| `smoke/02_autocorrect_kvp` | `sit::autocorrect` | VAL-003, VAL-007 |
| `smoke/03_ttl_writetime` | `sit::ttl_writetime` | FEA-040..046 |
| `smoke/04_counters` | `sit::counters` | MIG-030..032, VAL-004 |
| `smoke/05_reserved_keyword` | `sit::reserved_keyword` | SCH-002 |
| `smoke/06_vector` | `sit::vector` | CDC-004 |
| `features/01_constant_column` | `sit::constant_column` | FEA-010..013 |
| `features/02_explode_map` | `sit::explode_map` | FEA-020..023 |
| `features/03_codec` | `sit::codecs` | CDC-020..022 |
| `features/04_udt_mapper` | `sit::udt` | CDC-013, CDC-014 |
| `features/05_guardrail` | `sit::guardrail` | GRD-001..003 |
| `features/06_constant_column_remove` | `sit::constant_column_remove` | FEA-014 |
| `features/07_constant_column_replace` | `sit::constant_column_replace` | FEA-014 |
| `features/08_map_columns_origin_target` | `sit::column_mapping` | SCH-003, SCH-004 |
| `regression/01_explode_map_with_constants` | `sit::explode_with_constants` | FEA-010+FEA-020+CDC-020 |
| `regression/02_ColumnRenameWithConstantsAndExplode` | `sit::quoted_identifiers` | SCH-002, SCH-003 |
| `regression/03_performance` | `sit::bulk` | ENG-*, MIG-020 |
| `regression/04_null_ts_in_pk` | `sit::null_ts_in_pk` | MIG-013 |

**TST-010** — **Property-based tests** (`proptest`) MUST cover: the token splitter (`TOK-003`) —
ranges are contiguous, non-overlapping, and cover exactly the requested span; codec round-trips
(`CDC-032`); config round-trip (parse → serialise → parse is identity); and the comparison function
(reflexive, symmetric).

**TST-020** — **Differential tests against Java CDM**: a harness runs both implementations against
the same seeded dataset and asserts byte-identical target state and identical counter blocks. It
MUST run nightly in CI over a generated corpus covering every CQL type, nesting depth 3, nulls,
empty collections, and edge-case values (min/max integers, epoch boundaries, unicode, empty strings).

**TST-030** — Zero-copy passthrough (`MIG-040`) MUST be proven lossless by a property test comparing
passthrough output against full deserialize/serialize output for random values of every type.

**TST-031** — Every codec MUST have: an encode/decode round-trip property test, a known-vector test
with fixtures shared with the Java implementation, and an error-case test.

**TST-040** — **Fault injection**: a `FaultySession` test double MUST inject read timeouts, write
timeouts, unavailable, overloaded, connection drops and schema changes, asserting the documented
retry, counting and range-failure behaviour (`ENG-008`, `CON-011`).

**TST-041** — **Resume tests**: kill a run at a random point, restart with `auto_rerun`, and assert
the final target state equals a clean full run and that no range is processed twice in a way that
changes the result.

**TST-042** — **Distributed tests**: 3 nodes, one killed mid-range; assert lease reclaim, no double
processing of counter ranges (`DST-015`), and correct aggregate metrics.

**TST-050** — **Interface conformance**: the same logical operation issued via CLI, REST, MCP and A2A
MUST produce identical results and identical structured output.

**TST-051** — **OpenAPI contract tests**: every endpoint MUST be exercised and its response validated
against the schema (schemathesis or equivalent fuzzing over the spec).

**TST-060** — **Benchmarks**: `criterion` micro-benchmarks for the hot path (bind, convert, compare)
and a reproducible macro-benchmark (`bench/` with a nosqlbench-generated dataset mirroring
`PERF/perf-iot.yaml`). Regressions > 10% MUST fail CI.

**TST-070** — **Snapshot tests** (`insta`) for CLI output, generated CQL, generated config files,
error messages, and the OpenAPI document.

**TST-080** — **Fuzzing** (`cargo-fuzz`) of the properties parser, the CQL identifier quoter, the
JSON extractor, and the Java date-pattern translator.

**TST-090** — **Doc tests**: every rustdoc example MUST compile and run; `docs/` code blocks MUST be
extracted and compiled by `xtask doctest-md`.

### 21.2 Test infrastructure

**TST-100** — A `cdm-testkit` crate MUST provide: containerised origin/target fixtures, a schema and
data generator covering all CQL types, table builders, a counter-assertion DSL mirroring
`cdm-assert.sh`, and mock sessions. It replaces Java's `CommonMocks` with something composable
rather than a 45-field god object.

**TST-101** — Test data generation MUST be deterministic and seeded; failures MUST print the seed.

**TST-102** — Integration tests MUST be runnable locally with one command (`cargo xtask it`) and MUST
skip (not fail) with a clear message when no container runtime is available.

---

## 22. Build, CI/CD, supply chain and release (`OPS`)

**OPS-001** — Cargo workspace with resolver v2, shared `[workspace.dependencies]`, and
`[workspace.lints]` applied to every crate (DRY).

**OPS-002** — `rustfmt.toml` and `clippy.toml` checked in; `cargo fmt --check` and
`cargo clippy --all-targets --all-features -- -D warnings` MUST gate every PR.

**OPS-003** — **Pre-commit hooks** (`.pre-commit-config.yaml`, plus a `cargo xtask install-hooks`
that installs native git hooks for contributors who prefer no Python):

| Hook | Purpose |
|---|---|
| `cargo fmt --check` | formatting |
| `cargo clippy -D warnings` | lints |
| `cargo machete` / `udeps` | unused dependencies |
| `typos` | spelling |
| `cargo deny check` | licenses, advisories, bans |
| `taplo fmt --check` | TOML formatting |
| `yamllint`, `markdownlint`, `shellcheck` | config/docs/scripts |
| `conventional-pre-commit` | commit message format |
| `gitleaks` | secret scanning |
| `cargo xtask check-traceability` | every changed requirement ID is traced |
| `cargo xtask check-generated` | OpenAPI / JSON Schema / property docs are current |
| end-of-file, trailing-whitespace, large-file, merge-conflict | hygiene |

**OPS-004** — **Conventional Commits** MUST be enforced. Commit trailers MUST include
`Implements: <REQ-ID>[, ...]` for feature commits and `Fixes: #<issue>` for bug fixes.

**OPS-010** — GitHub Actions workflows:

| Workflow | Trigger | Contents |
|---|---|---|
| `ci.yml` | PR, push `main` | fmt, clippy, build, unit tests, doc build, mdBook build, MSRV check, feature-powerset check (`cargo hack`) |
| `integration.yml` | PR, push `main`, nightly | testcontainers matrix over Cassandra 4.1/5.0 × Rust stable/MSRV |
| `sit.yml` | PR, push `main` | the ported SIT parity suite |
| `coverage.yml` | PR | `cargo-llvm-cov`, upload, fail below threshold (`S4`) |
| `security.yml` | PR, daily | `cargo audit`, `cargo deny`, `gitleaks`, CodeQL, SBOM |
| `bench.yml` | nightly, `bench` label | criterion + macro benchmark, regression gate |
| `differential.yml` | nightly | Java-vs-Rust differential suite (`TST-020`) |
| `openapi.yml` | PR | regenerate spec, diff check, `oasdiff` breaking-change gate, schemathesis |
| `docs.yml` | push `main` | build and publish rustdoc + mdBook site (`docs/book`, whose chapters `{{#include}}` the repository's documents rather than copying them) |
| `release.yml` | tag `v*` | cross-compile all targets, sign, SBOM, GitHub Release, crates.io publish, container image |
| `container.yml` | push `main`, tag | multi-arch (amd64/arm64) distroless image |

**OPS-011** — A CI job MUST verify traceability: every `REQ-ID` in `SPEC.md` appears in
`TRACEABILITY.md`; every ID in `TRACEABILITY.md` exists in `SPEC.md`; every implemented ID has at
least one referencing test; no ID is orphaned. Implemented as `cargo xtask check-traceability`.

**OPS-012** — A CI job MUST verify that generated artefacts (`api/openapi.yaml`,
`schema/cdm-config.schema.json`, `docs/generated/PROPERTIES.md`, `docs/generated/METRICS.md`,
`docs/generated/CLI.md`) are byte-identical to freshly generated output.

**OPS-020** — Releases MUST publish: signed binaries for all `NFR-001` targets, checksums, a
CycloneDX SBOM, a multi-arch container image, crates.io packages for the library crates, and a
generated changelog (`git-cliff`) grouped by requirement domain.

**OPS-021** — Versioning MUST be SemVer. The workspace uses a single shared version; library crates
are published together.

**OPS-022** — The container image MUST be distroless, run as non-root, contain only the `cdm` binary,
and default to `cdm serve`. It MUST NOT bundle Spark, a JVM, or Maven (contrast with the Java
image).

**OPS-023** — `CODEOWNERS`, issue templates (bug/feature, mirroring the Java repo's fields), a PR
template with a traceability checklist, and Dependabot for `cargo`, `github-actions` and `docker`
MUST be present.

**OPS-024** — A `Makefile`/`justfile` and `cargo xtask` MUST provide one-command entry points:
`build`, `test`, `it`, `sit`, `lint`, `cover`, `bench`, `docs`, `openapi`, `release-dry-run`.

**OPS-030** — Every PR MUST be small, single-purpose, mapped to requirement IDs, and green on all
required checks. Direct pushes to `main` MUST be blocked by branch protection. Squash merge only.

---

## 23. Compatibility and migration from Java CDM

**COMPAT-001** — `cdm --compat-java` MUST enable a bundle of behaviours that exactly reproduce Java
quirks where cdm-rs deliberately improves on them: `CFG-161` (silent CL coercion), `FEA-061`
(unconditional `ALLOW FILTERING`), `CDC-014` (positional UDT field matching), `MIG-022` (legacy batch
grouping).

**COMPAT-002** — `docs/MIGRATION_FROM_JAVA.md` MUST list every behavioural difference, with rationale
and the flag that restores the old behaviour.

**COMPAT-003** — The run-tracking tables MUST remain schema-compatible so a Java run can be resumed by
cdm-rs and vice versa. A test MUST prove this (`TST-041`).

**COMPAT-004** — The final metrics block and per-range `run_info` strings MUST remain
character-identical (`MET-005`, `MET-006`) so existing assertion tooling works unmodified.

---

## 24. Open risks

| ID | Risk | Mitigation |
|---|---|---|
| R1 | `scylla-rust-driver` gaps. **Largely retired by the PR #2 spike**: raw-byte access, `UNSET` binding, full type coverage, paging and token-range scans are all confirmed against a live cluster, and `vector<T,N>` turned out to be native (`CqlValue::Vector`), not a gap. | Two gaps remain, each with a defined implementation inside `cdm-cql`: Astra SCB/SNI, since the driver's `cloud` feature targets Scylla Cloud bundles whose layout differs from Astra's (`CON-003`, `ADR-0009`); and DSE geometry + `DateRangeType` as WKB codecs in `cdm-codec` (`CDC-003`). JKS parsing (`CON-006`) is not a driver gap — nothing in Rust reads JKS — and is a self-contained reader. See `ADR-0002` for the evidence. |
| R2 | Exact parity of Java `DecimalFormat`/`DateTimeFormatter` semantics (rounding, pattern letters). | Known-vector fixtures shared with the Java build; differential tests (`TST-020`). |
| R3 | LWT-based lease coordination adds load to the target cluster. | Leases are per-range, not per-row; batch renewals; `cluster.enabled` is opt-in; SQLite/in-memory tracking stores available. |
| R4 | Counter semantics under distributed reclaim are inherently unsafe. | Refuse reclaim, mark `FAIL`, require manual action (`DST-015`). |
| R5 | Scope: parity + new API surface + UI is large. | Strict PR phasing (`ROADMAP.md`); parity (Phase 1–4) ships before API/UI (Phase 5–7). |

---

## 25. Requirement index

The authoritative index of every requirement ID, its implementing crate/module, its tests and its
delivering PR is maintained in [`docs/TRACEABILITY.md`](./TRACEABILITY.md) and validated by CI
(`OPS-011`).
