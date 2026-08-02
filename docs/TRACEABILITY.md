# cdm-rs — Requirement Traceability Matrix

| | |
|---|---|
| **Document** | `docs/TRACEABILITY.md` |
| **Normative source** | [`SPEC.md`](./SPEC.md) |
| **Delivery plan** | [`ROADMAP.md`](./ROADMAP.md) |
| **Enforced by** | `cargo xtask check-traceability` (CI gate `OPS-011`) |

## Purpose

This matrix is the contract between the specification and the code. For every requirement it records
**where it lives**, **what proves it works**, and **which pull request delivered it**. It is checked
mechanically on every pull request:

1. every requirement ID in `SPEC.md` appears here exactly once;
2. every ID here exists in `SPEC.md` (no phantom requirements);
3. every ID whose status is `done` has at least one test in the codebase whose name or `#[doc]`
   comment cites the ID;
4. every ID whose status is `done` appears in the `Implements:` trailer of a merged commit;
5. no ID is orphaned (present in code but absent from `SPEC.md`).

A pull request that adds a requirement without a row here, or marks a row `done` without a citing
test, fails CI.

## Conventions

* **Home** — the crate (and module, where it narrows usefully) that owns the requirement.
* **Verified by** — the test-name prefix that must exist. Tests are named
  `<id_lowercased_with_underscores>_<description>`, e.g. `mig_012_empty_collection_is_unset`.
  Integration and SIT tests cite IDs in a `#[doc]` comment instead of the name where the name would
  become unwieldy.
* **PR** — the pull request in [`ROADMAP.md`](./ROADMAP.md) that delivers it.
* **Superscript** — parity marker from `SPEC.md`: <sup>P</sup> exact Java parity ·
  <sup>P+</sup> parity with a documented, flag-restorable improvement · <sup>N</sup> new in cdm-rs.
* **Status** — tracked in the machine-readable companion `docs/traceability.toml`, which
  `cargo xtask` reads and writes. This markdown file is generated from it; do not hand-edit rows.

## Status summary

| Phase | Requirements | Status |
|---|---|---|
| Phase 0 — foundation | `ERR-*`, `PLG-*` | planned |
| Phase 1 — config & connectivity | `CFG-*`, `CON-*`, `SCH-001/002/008/010`, `CLI-*` | planned |
| Phase 2 — type system | `CDC-*`, `MIG-040`, `TST-100`–`TST-102` | planned |
| Phase 3 — engine & parity jobs | `TOK-*`, `ENG-*`, `MIG-*`, `VAL-001`–`VAL-012`, `GRD-*`, `TRK-*`, `MET-001`–`MET-006` | planned |
| Phase 4 — features & parity certification | `FEA-*`, `TST-003`–`TST-041`, `COMPAT-*` | planned |
| Phase 5 — observability | `MET-010`–`MET-033`, `VAL-013`–`VAL-015` | planned |
| Phase 6 — API/MCP/A2A/UI | `API-*`, `MCP-*`, `A2A-*`, `UI-*`, `SEC-010`–`SEC-012` | planned |
| Phase 7 — distribution & hardening | `DST-*`, `NFR-*`, `OPS-020`+, `TST-042`–`TST-080` | planned |

## Reverse index: Java CDM behaviour → requirement

| Java source | cdm-rs requirements |
|---|---|
| `properties/KnownProperties.java`, `PropertyHelper.java` | `CFG-100`–`CFG-200`, `CFG-020`–`CFG-040` |
| `job/ConnectionFetcher.scala`, `ConnectionDetails.scala` | `CON-001`, `CON-002`, `CON-006`, `CON-007` |
| `data/AstraDevOpsClient.java` | `CON-004`, `CON-005` |
| `schema/CqlTable.java`, `BaseTable.java` | `SCH-001`–`SCH-007` |
| `job/SplitPartitions.java` | `TOK-001`–`TOK-007` |
| `job/BaseJob.scala`, `BasePartitionJob.scala` | `ENG-001`–`ENG-003`, `TRK-030`–`TRK-033` |
| `job/CopyJobSession.java` | `MIG-001`–`MIG-032` |
| `job/DiffJobSession.java` | `VAL-001`–`VAL-012`, `VAL-016` |
| `job/GuardrailCheckJobSession.java` | `GRD-001`–`GRD-003` |
| `job/JobCounter.java`, `CounterUnit.java`, `CDMMetricsAccumulator.java` | `MET-001`–`MET-006` |
| `cql/statement/*.java` | `SCH-007`, `MIG-010`–`MIG-011`, `MIG-030`, `FEA-060`–`FEA-062`, `ERR-005` |
| `cql/statement/TargetUpsertRunDetailsStatement.java`, `feature/TrackRun.java` | `TRK-010`–`TRK-033` |
| `cql/codec/*.java`, `CodecFactory.java` | `CDC-020`–`CDC-022` |
| `data/CqlData.java`, `CqlConversion.java` | `CDC-001`–`CDC-016` |
| `data/PKFactory.java`, `EnhancedPK.java`, `Record.java` | `SCH-006`, `MIG-013`, `FEA-012`, `FEA-022` |
| `feature/ConstantColumns.java` | `FEA-010`–`FEA-014` |
| `feature/ExplodeMap.java` | `FEA-020`–`FEA-023` |
| `feature/ExtractJson.java` | `FEA-030`–`FEA-035` |
| `feature/WritetimeTTL.java` | `FEA-040`–`FEA-046` |
| `feature/OriginFilterCondition.java` | `FEA-050` |
| `feature/Guardrail.java` | `GRD-001`–`GRD-004` |
| `data/DataUtility.java` | `SCH-003`, `VAL-005`, `CON-005` |
| `src/resources/log4j2.properties` (diff appender) | `VAL-012`, `MET-032` |
| `SIT/**` | `TST-003` |
| `.github/workflows/**`, `pom.xml` plugins | `OPS-001`–`OPS-024` |
| `cdm-config-builder/**` | `UI-001`–`UI-004` |
| `Dockerfile` | `OPS-022` |

---

## Matrix

### CFG

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `CFG-001` <sup>N</sup> | Configuration MUST be defined **once**, as a strongly-typed Rust struct tree (`cdm_config::CdmConfig`) with `serde` derives | `cdm-config` | `cfg_001_*` | #4, #5, #6 |
| `CFG-002` <sup>N</sup> | Each field MUST carry, via attribute macros: canonical name, legacy `spark.cdm.*` alias(es), type, default, unit, whether… | `cdm-config` | `cfg_002_*` | #4, #5, #6 |
| `CFG-003` <sup>N</sup> | The generated JSON Schema MUST be published at `schema/cdm-config.schema.json` and served at `GET /v1/config/schema` | `cdm-config` | `cfg_003_*` | #4, #5, #6 |
| `CFG-010` <sup>P+</sup> | Configuration MUST be loadable from, in increasing order of precedence: 1 | `cdm-config` | `cfg_010_*` | #4, #5, #6 |
| `CFG-011` <sup>P</sup> | The `.properties` loader MUST accept the complete Java `spark.cdm.*` namespace (§3.5), including keys nested under a… | `cdm-config` | `cfg_011_*` | #4, #5, #6 |
| `CFG-012` <sup>N</sup> | Secrets (`*.password`, `*.token`, `*.keyStore.password`, `*.trustStore.password`) MUST support indirection: `env:VAR_NAME`,… | `cdm-config` | `cfg_012_*` | #4, #5, #6 |
| `CFG-013` <sup>N</sup> | A configuration profile system: a file may declare `[profiles.<name>]` blocks that are deep-merged over the base when… | `cdm-config` | `cfg_013_*` | #4, #5, #6 |
| `CFG-020` <sup>P+</sup> | Validation runs in three escalating tiers — syntactic, semantic (cross-field), schema-bound — each independently invocable | `cdm-config` | `cfg_020_*` | #4, #5, #6 |
| `CFG-021` <sup>N</sup> | All three tiers MUST run *before* any data is read or written, and MUST report **every** violation at once (not fail-fast),… | `cdm-config` | `cfg_021_*` | #4, #5, #6 |
| `CFG-022` <sup>P</sup> | `spark.cdm.schema.origin.keyspaceTable` (canonical `schema.origin.keyspace_table`) is the only unconditionally required property | `cdm-config` | `cfg_022_*` | #4, #5, #6 |
| `CFG-023` <sup>P</sup> | If the target keyspace/table is unset it MUST default to the origin keyspace/table | `cdm-config` | `cfg_023_*` | #4, #5, #6 |
| `CFG-024` <sup>P</sup> | An origin connection MUST specify either a host or a secure-connect-bundle; likewise for target | `cdm-config` | `cfg_024_*` | #4, #5, #6 |
| `CFG-025` <sup>P</sup> | When TLS is enabled on a side and no SCB is configured, all of `trustStore.path`, `trustStore.password`, `trustStore.type`,… | `cdm-config` | `cfg_025_*` | #4, #5, #6 |
| `CFG-026` <sup>P</sup> | Empty username or password on either side MUST emit a warning, not an error | `cdm-config` | `cfg_026_*` | #4, #5, #6 |
| `CFG-027` <sup>P</sup> | A list-typed property set to an empty value MUST be rejected as invalid | `cdm-config` | `cfg_027_*` | #4, #5, #6 |
| `CFG-028` <sup>N</sup> | `cdm config explain <key>` MUST print the description, type, default, current effective value, and which source supplied it | `cdm-config` | `cfg_028_*` | #4, #5, #6 |
| `CFG-029` <sup>N</sup> | `cdm config diff <a> <b>` MUST print a normalised semantic diff of two configs, ignoring ordering and defaults | `cdm-config` | `cfg_029_*` | #4, #5, #6 |
| `CFG-030` <sup>P</sup> | `constantColumns.names` and the split of `constantColumns.values` by `constantColumns.split_regex` MUST have equal cardinality | `cdm-config` | `cfg_030_*` | #4, #5, #6 |
| `CFG-031` <sup>P</sup> | Explode Map requires all three of origin column name, target key column name, target value column name, or none | `cdm-config` | `cfg_031_*` | #4, #5, #6 |
| `CFG-032` <sup>P</sup> | `filter.java.writetime.min` and `.max` MUST both be > 0 when either is set, and `max` > `min` | `cdm-config` | `cfg_032_*` | #4, #5, #6 |
| `CFG-033` <sup>P</sup> | `transform.custom.writetime.incrementBy` MUST be ≥ 0 | `cdm-config` | `cfg_033_*` | #4, #5, #6 |
| `CFG-034` <sup>P</sup> | A writetime filter requires at least one resolvable writetime column | `cdm-config` | `cfg_034_*` | #4, #5, #6 |
| `CFG-035` <sup>P</sup> | `guardrail.colSizeInKB` < 0 is invalid; `= 0` disables the feature | `cdm-config` | `cfg_035_*` | #4, #5, #6 |
| `CFG-036` <sup>P</sup> | TTL/writetime features MUST be rejected as invalid when the target is a counter table | `cdm-config` | `cfg_036_*` | #4, #5, #6 |
| `CFG-037` <sup>P</sup> | Explicit `column.ttl.names` / `column.writetime.names` MUST disable the corresponding `automatic` mode | `cdm-config` | `cfg_037_*` | #4, #5, #6 |
| `CFG-038` <sup>P</sup> | `column.names.to.target` entries MUST be `origin:target` pairs referencing columns that exist on their respective sides | `cdm-config` | `cfg_038_*` | #4, #5, #6 |
| `CFG-039` <sup>P+</sup> | `transform.custom.writetime.incrementBy == 0` combined with an unfrozen `list` column on origin MUST emit a warning about… | `cdm-config` | `cfg_039_*` | #4, #5, #6 |
| `CFG-040` <sup>N</sup> | `perfops.batch_size > 1` combined with a counter table, or with an active writetime filter, MUST emit a notice that batch size… | `cdm-config` | `cfg_040_*` | #4, #5, #6 |
| `CFG-100` | Property registry: connection properties (host, port, scb, username, password) for both sides | `cdm-config` | `cfg_100_*` | #4, #5, #6 |
| `CFG-110` | Property registry: Astra DevOps / secure-connect-bundle auto-download properties | `cdm-config` | `cfg_110_*` | #4, #5, #6 |
| `CFG-120` | Property registry: TLS properties per side (truststore, keystore, cipher suites, isAstra) | `cdm-config` | `cfg_120_*` | #4, #5, #6 |
| `CFG-130` | Property registry: schema properties (keyspaceTable, TTL/writetime names, column skip, column rename) | `cdm-config` | `cfg_130_*` | #4, #5, #6 |
| `CFG-140` | Property registry: autocorrect properties (missing, mismatch, missing.counter) | `cdm-config` | `cfg_140_*` | #4, #5, #6 |
| `CFG-150` | Property registry: run-tracking properties (trackRun, runId, previousRunId, autoRerun, rerunMultiplier) | `cdm-config` | `cfg_150_*` | #4, #5, #6 |
| `CFG-160` | Property registry: performance/operations properties, incl. new cdm-rs tuning knobs | `cdm-config` | `cfg_160_*` | #4, #5, #6 |
| `CFG-161` <sup>P+</sup> | (this is a deliberate, documented behaviour change; `--compat-java` restores silent coercion) | `cdm-config` | `cfg_161_*` | #4, #5, #6 |
| `CFG-170` | Property registry: transformation properties (custom writetime/TTL, codecs, map null removal) | `cdm-config` | `cfg_170_*` | #4, #5, #6 |
| `CFG-180` | Property registry: Cassandra-side and Java-side filter properties | `cdm-config` | `cfg_180_*` | #4, #5, #6 |
| `CFG-190` | Property registry: feature properties (constant columns, explode map, extract JSON, guardrail) | `cdm-config` | `cfg_190_*` | #4, #5, #6 |
| `CFG-200` | Property registry: new cdm-rs sections (server, metrics, cluster, logging) | `cdm-config` | `cfg_200_*` | #4, #5, #6 |

### CON

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `CON-000` <sup>N</sup> | `scylla-rust-driver` is the sole CQL driver, confined to `cdm-cql` behind the core traits | `cdm-cql` | `con_000_*` | #2 |
| `CON-001` <sup>P</sup> | cdm-rs MUST connect independently to origin and target with fully separate credentials, TLS material, consistency levels and… | `cdm-cql` | `con_001_*` | #7 |
| `CON-002` <sup>P</sup> | Four connection modes MUST be supported per side, selected exactly as Java's `ConnectionFetcher` does: 1 | `cdm-cql` | `con_002_*` | #7 |
| `CON-003` <sup>P</sup> | Astra secure-connect-bundles MUST be supported, including SNI-proxy address translation and per-node SNI names obtained from… | `cdm-cql` | `con_003_*` | #8 |
| `CON-004` <sup>P</sup> | When `astra.database_id` is set and no SCB path is given, the bundle MUST be downloaded from the Astra DevOps API (`POST… | `cdm-cql` | `con_004_*` | #8 |
| `CON-005` <sup>P</sup> | Downloaded/generated bundles MUST be written to a process-scoped temporary directory and deleted on run completion **and** on… | `cdm-cql` | `con_005_*` | #8 |
| `CON-006` <sup>P</sup> | Truststores/keystores MUST be readable in `JKS`, `PKCS12` and `PEM` formats | `cdm-cql` | `con_006_*` | #7 |
| `CON-007` <sup>P</sup> | `tls.cipher_suites` MUST be honoured | `cdm-cql` | `con_007_*` | #7 |
| `CON-008` <sup>N</sup> | `cdm connect test --side origin\|target\|both` MUST perform a full connect, report negotiated protocol version, TLS version… | `cdm-cql` | `con_008_*` | #10 |
| `CON-009` <sup>N</sup> | Connections MUST use a token-aware, DC-aware, latency-aware load-balancing policy by default, with… | `cdm-cql` | `con_009_*` | #7 |
| `CON-010` <sup>N</sup> | Speculative execution MUST be configurable per side and disabled by default for target writes | `cdm-cql` | `con_010_*` | #7 |
| `CON-011` <sup>P+</sup> | Retry policy: idempotent reads retry on timeout/unavailable up to `perfops.retry.max_attempts` with exponential backoff and jitter | `cdm-cql` | `con_011_*` | #22 |
| `CON-012` <sup>P+</sup> | . **CON-013 [N]** — Origin and target compatibility MUST be probed at startup: protocol version, whether `WRITETIME`/`TTL` on… | `cdm-cql` | `con_012_*` | #22 |
| `CON-013` <sup>N</sup> | Origin and target compatibility MUST be probed at startup: protocol version, whether `WRITETIME`/`TTL` on collections is… | `cdm-cql` | `con_013_*` | #9 |
| `CON-020` <sup>N</sup> | The bundle zip is read in memory; PEM members are used and the JKS/PFX members ignored | `cdm-cql` | `con_020_*` | #8 |
| `CON-021` <sup>N</sup> | `config.json` is parsed leniently, with a named Tier-1 diagnostic for each missing required field | `cdm-cql` | `con_021_*` | #8 |
| `CON-022` <sup>N</sup> | Primary strategy: mTLS to the metadata service, then per-connection SNI `server_name` = host id via the SNI proxy | `cdm-cql` | `con_022_*` | #8 |
| `CON-023` <sup>N</sup> | Do not depend on the driver's Scylla-Cloud `cloud` feature for Astra; use its TLS/translator hooks or raise the gap upstream | `cdm-cql` | `con_023_*` | #8 |
| `CON-024` <sup>N</sup> | Host-id → SNI mapping refreshes as topology changes, without a restart | `cdm-cql` | `con_024_*` | #8 |
| `CON-025` <sup>N</sup> | Re-fetch the metadata response, rate-limited, when all connections fail | `cdm-cql` | `con_025_*` | #8 |
| `CON-026` <sup>N</sup> | Fallback strategy: single-endpoint mTLS using the host from `config.json` and the port from `cqlshrc` | `cdm-cql` | `con_026_*` | #8 |
| `CON-027` <sup>N</sup> | The fallback warns prominently that token-aware routing is lost | `cdm-cql` | `con_027_*` | #8 |
| `CON-028` <sup>N</sup> | Accept both Astra auth spellings, and detect an `AstraCS:` token supplied as the username | `cdm-cql` | `con_028_*` | #8 |
| `CON-029` <sup>N</sup> | `cdm connect test` reports strategy, metadata URL, proxy address, local DC, host-id count and TLS parameters | `cdm-cql` | `con_029_*` | #10 |

### SCH

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `SCH-001` <sup>P</sup> | Origin and target table metadata MUST be introspected from `system_schema`: columns, CQL types (including UDTs, tuples,… | `cdm-cql` | `sch_001_*` | #9 |
| `SCH-002` <sup>P</sup> | Quoted, mixed-case and special-character identifiers MUST be handled correctly on both read and write | `cdm-cql` | `sch_002_*` | #9 |
| `SCH-003` <sup>P</sup> | Column mapping: `schema.origin.column.rename` supplies explicit `origin:target` pairs; all remaining identically-named columns… | `cdm-cql` | `sch_003_*` | #18 |
| `SCH-004` <sup>P</sup> | `schema.origin.column.skip` MUST remove the named non-key columns from the origin projection | `cdm-cql` | `sch_004_*` | #18 |
| `SCH-005` <sup>P</sup> | Counter tables MUST be auto-detected; the write path switches from `INSERT` to `UPDATE ... SET c = c + ?` (`MIG-030`) | `cdm-cql` | `sch_005_*` | #22 |
| `SCH-006` <sup>P</sup> | The target primary key MUST be derivable from: mapped origin columns, constant columns, and explode-map key/value columns | `cdm-cql` | `sch_006_*` | #18 |
| `SCH-007` <sup>P</sup> | Virtual projection columns `TTL(col)` and `WRITETIME(col)` MUST be appendable to the origin select and addressable by index | `cdm-cql` | `sch_007_*` | #18 |
| `SCH-008` <sup>N</sup> | `cdm schema diff` MUST print a side-by-side origin/target schema comparison with per-column mapping, conversion plan… | `cdm-cql` | `sch_008_*` | #10 |
| `SCH-009` <sup>N</sup> | Schema changes detected mid-run (via driver schema-agreement events) MUST abort the run with a distinct error kind rather than… | `cdm-cql` | `sch_009_*` | #18 |
| `SCH-010` <sup>P</sup> | Materialized views MUST be rejected as a target with a clear message | `cdm-cql` | `sch_010_*` | #9 |

### TOK

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `TOK-001` <sup>P</sup> | The origin partitioner MUST be detected | `cdm-engine::planner` | `tok_001_*` | #17 |
| `TOK-002` <sup>P</sup> | Default token bounds: Murmur3 → `[i64::MIN, i64::MAX]`; Random → `[0, 2^127 - 1]` | `cdm-engine::planner` | `tok_002_*` | #17 |
| `TOK-003` <sup>P</sup> | The ring is split into `perfops.num_parts` ranges using the Java algorithm exactly, including its overflow edge cases | `cdm-engine::planner` | `tok_003_*` | #17 |
| `TOK-004` <sup>P</sup> | All split arithmetic is performed in `i128` (Murmur3) or arbitrary precision (Random) so the overflow the Java code defends against cannot occur | `cdm-core::domain::token`, `cdm-engine::planner` | `tok_004_*` | #3, #17 |
| `TOK-005` <sup>P</sup> | `filter.token_coverage_percent` < 100 MUST shrink each emitted range from its lower bound, producing a deterministic random… | `cdm-engine::planner` | `tok_005_*` | #17 |
| `TOK-006` <sup>P</sup> | The emitted range list MUST be shuffled (Java shuffles twice) before scheduling, to spread load across replicas | `cdm-engine::planner` | `tok_006_*` | #17 |
| `TOK-007` <sup>N</sup> | . **TOK-008 [N]** — An alternative planner `plan.strategy = ring_aware` MUST be available: split along actual ring ownership… | `cdm-engine::planner` | `tok_007_*` | #17 |
| `TOK-008` <sup>N</sup> | An alternative planner `plan.strategy = ring_aware` MUST be available: split along actual ring ownership boundaries so every… | `cdm-engine::planner` | `tok_008_*` | #53 |
| `TOK-009` <sup>N</sup> | `cdm plan` MUST emit the computed plan (range count, span histogram, estimated rows from `system.size_estimates`, estimated… | `cdm-engine::planner` | `tok_009_*` | #17 |
| `TOK-010` <sup>N</sup> | Ranges SHOULD be sized adaptively when `plan.strategy = adaptive`: begin at `num_parts`, and dynamically subdivide any range… | `cdm-engine::planner` | `tok_010_*` | #53 |

### ENG

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `ENG-001` <sup>N</sup> | The engine MUST be a Tokio-based work-stealing scheduler | `cdm-engine` | `eng_001_*` | #20 |
| `ENG-002` <sup>P</sup> | A partition range is the unit of atomicity for tracking and resume: it is marked `STARTED` before work begins and… | `cdm-engine` | `eng_002_*` | #20 |
| `ENG-003` <sup>P</sup> | Origin reads MUST be paged with page size `perfops.fetch_size` and streamed — never fully materialised | `cdm-engine` | `eng_003_*` | #20 |
| `ENG-004` <sup>P</sup> | Two independent per-node rate limiters (origin rows read/s, target rows written/s), with an opt-in globally-divided mode | `cdm-engine` | `eng_004_*` | #51 |
| `ENG-005` <sup>N</sup> | Rate limiting MUST use a token-bucket with burst = 1 second of budget, and MUST apply backpressure (await) rather than… | `cdm-engine` | `eng_005_*` | #20 |
| `ENG-006` <sup>N</sup> | `perfops.adaptive_ratelimit = true` MUST reduce the effective rate when the target reports overload (write timeouts,… | `cdm-engine` | `eng_006_*` | #53 |
| `ENG-007` <sup>N</sup> | In-flight requests MUST be bounded by `perfops.max_inflight_reads` / `max_inflight_writes` semaphores so memory is bounded… | `cdm-engine` | `eng_007_*` | #20 |
| `ENG-008` <sup>P</sup> | Per-range failure handling: an error MUST NOT abort the run | `cdm-engine` | `eng_008_*` | #20 |
| `ENG-009` <sup>P+</sup> | `perfops.error_limit > 0` MUST abort the run once total `ERROR` exceeds it, draining in-flight work cleanly | `cdm-engine` | `eng_009_*` | #26 |
| `ENG-010` <sup>N</sup> | `SIGINT`/`SIGTERM` MUST trigger graceful shutdown: stop claiming new ranges, let in-flight ranges finish (bounded by… | `cdm-engine` | `eng_010_*` | #26 |
| `ENG-011` <sup>N</sup> | Every range's processing MUST be wrapped in a `tracing` span carrying `run_id`, `range_min`, `range_max`, `node_id`, so all… | `cdm-engine` | `eng_011_*` | #20 |
| `ENG-012` <sup>P</sup> | . **ENG-013 [N]** — Panics inside a worker MUST be caught at the range boundary, converted to a range failure, and MUST NOT… | `cdm-engine` | `eng_012_*` | #20 |
| `ENG-013` <sup>N</sup> | Panics inside a worker MUST be caught at the range boundary, converted to a range failure, and MUST NOT poison the run | `cdm-engine` | `eng_013_*` | #20 |
| `ENG-014` <sup>N</sup> | The engine MUST expose a `Pause`/`Resume` control that stops issuing new work without losing the plan, driven by `POST… | `cdm-engine` | `eng_014_*` | #26 |

### MIG

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `MIG-001` <sup>P</sup> | For each origin row in a range: acquire an origin rate-limit permit, increment `READ`, build the target primary key, apply… | `cdm-engine::jobs::migrate` | `mig_001_*` | #21 |
| `MIG-002` <sup>P</sup> | Rows rejected by any filter MUST increment `SKIPPED` and MUST NOT be written | `cdm-engine::jobs::migrate` | `mig_002_*` | #21 |
| `MIG-003` <sup>P</sup> | A record whose bind produces no statement (e.g | `cdm-engine::jobs::migrate` | `mig_003_*` | #21 |
| `MIG-004` <sup>P</sup> | Writes MUST be issued asynchronously with bounded concurrency, and flushed when `UNFLUSHED >= flush_threshold` where… | `cdm-engine::jobs::migrate` | `mig_004_*` | #21 |
| `MIG-005` <sup>P</sup> | On flush, `WRITE` MUST be incremented by the number of successfully written rows | `cdm-engine::jobs::migrate` | `mig_005_*` | #21 |
| `MIG-010` <sup>P</sup> | INSERT statement shape: `INSERT INTO ks.tbl (bind_cols..., const_cols...) VALUES (?, ..., <const literals>) [USING TTL ? [AND]… | `cdm-engine::jobs::migrate` | `mig_010_*` | #18 |
| `MIG-011` <sup>P</sup> | Bind order MUST be: mapped/derived columns in target-column order, then TTL, then writetime | `cdm-engine::jobs::migrate` | `mig_011_*` | #18 |
| `MIG-012` <sup>P</sup> | A `null` value, or an **empty collection**, MUST be bound as `UNSET` rather than `null`, to avoid creating tombstones | `cdm-engine::jobs::migrate` | `mig_012_*` | #18 |
| `MIG-013` <sup>P</sup> | A `null` in a target primary-key column MUST be substituted: `String`-typed keys become `""`; `Instant`-typed keys become… | `cdm-engine::jobs::migrate` | `mig_013_*` | #18 |
| `MIG-014` <sup>P</sup> | `transform.map_remove_null_value = true` MUST strip map entries with null values before binding | `cdm-engine::jobs::migrate` | `mig_014_*` | #18 |
| `MIG-020` <sup>P</sup> | Batching: when `batch_size > 1`, writes MUST be accumulated into an `UNLOGGED` batch and executed when the batch reaches… | `cdm-engine::jobs::migrate` | `mig_020_*` | #21 |
| `MIG-021` <sup>P</sup> | `batch_size` MUST be coerced to 1 when the table is a counter table, when a writetime filter is active, or when the configured… | `cdm-engine::jobs::migrate` | `mig_021_*` | #21 |
| `MIG-022` <sup>N</sup> | Batches SHOULD be grouped by partition key so that a batch never spans partitions (single-partition batches are the only… | `cdm-engine::jobs::migrate` | `mig_022_*` | #21 |
| `MIG-030` <sup>P</sup> | Counter tables MUST use `UPDATE ks.tbl [USING TTL ? AND TIMESTAMP ?] SET c = c + ?, ... , <const> = <literal> WHERE <target… | `cdm-engine::jobs::migrate` | `mig_030_*` | #22 |
| `MIG-031` <sup>P</sup> | The counter delta MUST be `origin_value − (current_target_value or 0)`, obtained by a rate-limited target SELECT by PK… | `cdm-engine::jobs::migrate` | `mig_031_*` | #22 |
| `MIG-032` <sup>P</sup> | Counter migration MUST NOT be batched and MUST NOT be retried (`CON-012`) | `cdm-engine::jobs::migrate` | `mig_032_*` | #22 |
| `MIG-040` <sup>N</sup> | Identical origin/target types pass through as raw bytes with no deserialize/reserialize | `cdm-cql::raw` | `con_000_raw_column_bytes_are_reachable`, `mig_040_*` | #2, #15 |
| `MIG-041` <sup>N</sup> | `migrate --dry-run` MUST execute the full read + transform + bind pipeline and count everything, but issue no target writes,… | `cdm-engine::jobs::migrate` | `mig_041_*` | #21 |

### VAL

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `VAL-001` <sup>P</sup> | For each origin row: build the target PK, apply filters, then issue an asynchronous target SELECT by PK; buffer records and… | `cdm-engine::jobs::validate` | `val_001_*` | #23 |
| `VAL-002` <sup>P</sup> | Missing target row → increment `MISSING`, log at ERROR: `Missing target row found for key: <pk>` | `cdm-engine::jobs::validate` | `val_002_*` | #23 |
| `VAL-003` <sup>P</sup> | `autocorrect.missing = true` → synchronously upsert the record, increment `CORRECTED_MISSING`, log `Inserted missing row in… | `cdm-engine::jobs::validate` | `val_003_*` | #23 |
| `VAL-004` <sup>P</sup> | For counter tables, a missing row MUST NOT be auto-corrected unless `autocorrect.missing_counter = true`; otherwise log and… | `cdm-engine::jobs::validate` | `val_004_*` | #23 |
| `VAL-005` <sup>P</sup> | Column comparison MUST convert the target value into the origin's type space and compare for equality | `cdm-engine::jobs::validate` | `val_005_*` | #23 |
| `VAL-006` <sup>P</sup> | A mismatch MUST increment `MISMATCH` and log at ERROR: `Mismatch row found for key: <pk> Mismatch: <detail>` where detail… | `cdm-engine::jobs::validate` | `val_006_*` | #23 |
| `VAL-007` <sup>P</sup> | `autocorrect.mismatch = true` → upsert the record, increment `CORRECTED_MISMATCH`, log `Corrected mismatch row in target: <pk>` | `cdm-engine::jobs::validate` | `val_007_*` | #23 |
| `VAL-008` <sup>P</sup> | A fully-matching record increments `VALID` | `cdm-engine::jobs::validate` | `val_008_*` | #23 |
| `VAL-009` <sup>P</sup> | Per-column comparison errors MUST be captured into the mismatch detail rather than failing the range, in the form `Target… | `cdm-engine::jobs::validate` | `val_009_*` | #23 |
| `VAL-010` <sup>P</sup> | Validation MUST never delete data from the target | `cdm-engine::jobs::validate` | `val_010_*` | #23 |
| `VAL-011` <sup>P</sup> | With `feature.extract_json` active and `overwrite = false`, an already-populated target extract column MUST be skipped rather… | `cdm-engine::jobs::validate` | `val_011_*` | #23 |
| `VAL-012` <sup>P</sup> | The diff logger MUST write to a dedicated sink (`logging.diff_file`, default `cdm_logs/cdm_diff.log`) at ERROR level, separate… | `cdm-engine::jobs::validate` | `val_012_*` | #23 |
| `VAL-013` <sup>N</sup> | A machine-readable diff report MUST be produced when `validate.report.format` is `json`/`ndjson`/`csv`/`parquet`, one record… | `cdm-engine::jobs::validate` | `val_013_*` | #40 |
| `VAL-014` <sup>N</sup> | `GET /v1/runs/{id}/discrepancies` MUST page over that report | `cdm-engine::jobs::validate` | `val_014_*` | #40 |
| `VAL-015` <sup>N</sup> | `validate --sample <percent>` MUST be sugar for `filter.token_coverage_percent`, and `validate --keys-only` MUST compare… | `cdm-engine::jobs::validate` | `val_015_*` | #40 |
| `VAL-016` <sup>P</sup> | Run status resolution: any discrepancy with `MISSING == CORRECTED_MISSING && MISMATCH == CORRECTED_MISMATCH` →… | `cdm-engine::jobs::validate` | `val_016_*` | #23 |

### GRD

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `GRD-001` <sup>P</sup> | The guardrail job reads the **origin only**; no target connection is required or opened | `cdm-engine::jobs::guardrail` | `grd_001_*` | #24 |
| `GRD-002` <sup>P</sup> | For each row, every column's serialized size MUST be computed and compared against `feature.guardrail.column_size_kb * 1000`… | `cdm-engine::jobs::guardrail` | `grd_002_*` | #24 |
| `GRD-003` <sup>P</sup> | A row with at least one oversized column increments `LARGE` and logs `Large columns (KB): col(12.345),col2(...)` with… | `cdm-engine::jobs::guardrail` | `grd_003_*` | #24 |
| `GRD-004` <sup>N</sup> | Guardrail MUST additionally be runnable *inline* during migrate/validate (`feature.guardrail.mode = check\|warn\|block`),… | `cdm-engine::jobs::guardrail` | `grd_004_*` | #24 |
| `GRD-005` <sup>N</sup> | Additional guardrails MUST be pluggable via `PLG-003`: partition size, row count per partition, collection cardinality, and… | `cdm-engine::jobs::guardrail` | `grd_005_*` | #54 |

### FEA

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `FEA-010` <sup>P</sup> | `feature.constant_columns.names` and `.values` (split by `.split_regex`) define target columns written with fixed literal values | `cdm-feature` | `fea_010_*` | #27 |
| `FEA-011` <sup>P</sup> | Values MUST be parsed and type-checked against the target column type at validation time | `cdm-feature` | `fea_011_*` | #27 |
| `FEA-012` <sup>P</sup> | Constant columns that are part of the target primary key MUST participate in the PK and appear as literals in generated WHERE… | `cdm-feature` | `fea_012_*` | #27 |
| `FEA-013` <sup>P</sup> | Constant columns MUST be excluded from validate comparison | `cdm-feature` | `fea_013_*` | #27 |
| `FEA-014` <sup>P</sup> | Constant columns present on origin but absent on target MUST be droppable, and origin constants MUST be replaceable by… | `cdm-feature` | `fea_014_*` | #27 |
| `FEA-020` <sup>P</sup> | `feature.explode_map.origin_column` MUST be a `map` column on origin; each entry produces one target row with the key and… | `cdm-feature` | `fea_020_*` | #28 |
| `FEA-021` <sup>P</sup> | Key and value MUST be converted from the map's element types to the target column types using the standard conversion… | `cdm-feature` | `fea_021_*` | #28 |
| `FEA-022` <sup>P</sup> | The exploded key and/or value MAY be part of the target primary key | `cdm-feature` | `fea_022_*` | #28 |
| `FEA-023` <sup>P</sup> | A null or empty map MUST produce zero target rows and count as `SKIPPED` | `cdm-feature` | `fea_023_*` | #28 |
| `FEA-030` <sup>P</sup> | `feature.extract_json.origin_column` names a text column containing a JSON object; `property_mapping` is… | `cdm-feature` | `fea_030_*` | #29 |
| `FEA-031` <sup>P</sup> | The extracted value MUST be written to the mapped target column | `cdm-feature` | `fea_031_*` | #29 |
| `FEA-032` <sup>P</sup> | `overwrite = false` MUST leave an already-populated target column untouched | `cdm-feature` | `fea_032_*` | #29 |
| `FEA-033` <sup>P</sup> | `exclusive = true` MUST restrict the non-PK target columns to the extract column alone | `cdm-feature` | `fea_033_*` | #29 |
| `FEA-034` <sup>P+</sup> | Malformed JSON MUST increment `ERROR` for that record and log the primary key, rather than failing the range | `cdm-feature` | `fea_034_*` | #29 |
| `FEA-035` <sup>N</sup> | `property_mapping` MUST accept JSON-Pointer paths (`/a/b/0`) in addition to top-level field names | `cdm-feature` | `fea_035_*` | #29 |
| `FEA-040` <sup>P</sup> | A row's writetime is the **maximum** `WRITETIME(col)` over the eligible columns, plus `transform.custom_writetime_increment` | `cdm-feature` | `fea_040_*` | #30 |
| `FEA-041` <sup>P</sup> | Eligible columns are non-key columns that are primitive, tuple, or frozen; unfrozen collections are eligible only when… | `cdm-feature` | `fea_041_*` | #30 |
| `FEA-042` <sup>P</sup> | `ttl.automatic` / `writetime.automatic` (default true) select all eligible columns; supplying explicit names disables… | `cdm-feature` | `fea_042_*` | #30 |
| `FEA-043` <sup>P</sup> | When reading writetimes from a collection column, the result is a list of values and the maximum across the list MUST be taken | `cdm-feature` | `fea_043_*` | #30 |
| `FEA-044` <sup>P</sup> | `transform.custom_writetime > 0` overrides the computed writetime; `transform.custom_ttl > 0` overrides the computed TTL.… | `cdm-feature` | `fea_044_*` | #30 |
| `FEA-045` <sup>P</sup> | TTL/writetime MUST be disabled for counter tables | `cdm-feature` | `fea_045_*` | #30 |
| `FEA-046` <sup>P</sup> | When no writetime is resolvable, `USING TIMESTAMP` MUST be omitted (server assigns); likewise `USING TTL` when TTL is 0 | `cdm-feature` | `fea_046_*` | #30 |
| `FEA-050` <sup>P</sup> | `filter.cql_where` MUST be appended to the origin range select, prefixed with ` AND ` unless the user's string already begins… | `cdm-feature` | `fea_050_*` | #31 |
| `FEA-051` <sup>P</sup> | `filter.writetime.min` / `.max` MUST skip rows whose computed row writetime falls outside the window | `cdm-feature` | `fea_051_*` | #31 |
| `FEA-052` <sup>P</sup> | `filter.column.name` + `filter.column.value` MUST skip rows where the named text column equals the value, compared… | `cdm-feature` | `fea_052_*` | #31 |
| `FEA-053` <sup>P</sup> | `filter.token.min` / `.max` bound the planned ring segment (`TOK-002`) | `cdm-feature` | `fea_053_*` | #31 |
| `FEA-054` <sup>N</sup> | Filters MUST be composable and pluggable (`PLG-003`); a filter chain evaluates in declaration order and short-circuits | `cdm-feature` | `fea_054_*` | #31 |
| `FEA-060` <sup>P</sup> | The origin range select MUST be `SELECT <projection> FROM ks.tbl WHERE TOKEN(<pk>) >= ? AND TOKEN(<pk>) <= ? <where> ALLOW… | `cdm-feature` | `fea_060_*` | #18 |
| `FEA-061` <sup>P+</sup> | `ALLOW FILTERING` MUST be omitted when no `filter.cql_where` is configured, since a pure token-range scan does not require it | `cdm-feature` | `fea_061_*` | #18 |
| `FEA-062` <sup>N</sup> | The generated CQL for every statement MUST be logged once at startup and exposed at `GET /v1/runs/{id}/statements` | `cdm-feature` | `fea_062_*` | #18 |

### CDC

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `CDC-001` <sup>P</sup> | All CQL primitive types MUST be supported: `ascii, bigint, blob, boolean, counter, date, decimal, double, duration, float,… | `cdm-codec` | `cdc_001_*` | #11, #12, #13, #14 |
| `CDC-002` <sup>P</sup> | Collection types `list<T>`, `set<T>`, `map<K,V>`, `tuple<...>`, user-defined types and `vector<T, N>` MUST be supported,… | `cdm-codec` | `cdc_002_*` | #11, #12, #13, #14 |
| `CDC-003` <sup>P</sup> | DSE geometry types `PointType`, `LineStringType`, `PolygonType` and `DateRangeType` MUST be supported (WKB encoding) | `cdm-codec` | `cdc_003_*` | #11, #12, #13, #14 |
| `CDC-004` <sup>N</sup> | `vector<float, N>` MUST be first-class: read, written, validated, and comparable with exact bit equality | `cdm-codec` | `cdc_004_*` | #11, #12, #13, #14 |
| `CDC-010` <sup>P</sup> | A **conversion plan** MUST be computed once per column pair at startup, not per row | `cdm-codec` | `cdc_010_*` | #11, #12, #13, #14 |
| `CDC-011` <sup>P</sup> | Identical origin/target types → `Passthrough` | `cdm-codec` | `cdc_011_*` | #11, #12, #13, #14 |
| `CDC-012` <sup>P</sup> | Collection-to-same-kind-collection conversion MUST recurse into element types | `cdm-codec` | `cdc_012_*` | #11, #12, #13, #14 |
| `CDC-013` <sup>P</sup> | UDT-to-UDT conversion MUST proceed field-by-field and requires equal field counts | `cdm-codec` | `cdc_013_*` | #11, #12, #13, #14 |
| `CDC-014` <sup>P+</sup> | Unlike Java, UDT field conversion MUST match by **name** when field names are equal-as-sets, falling back to positional… | `cdm-codec` | `cdc_014_*` | #11, #12, #13, #14 |
| `CDC-015` <sup>P+</sup> | Tuple element conversion MUST be implemented | `cdm-codec` | `cdc_015_*` | #11, #12, #13, #14 |
| `CDC-016` <sup>P</sup> | An `Unsupported` plan MUST pass the value through unchanged and MUST log a warning naming the column and both types, once per… | `cdm-codec` | `cdc_016_*` | #11, #12, #13, #14 |
| `CDC-020` <sup>P</sup> | The named codec set (INT_STRING, DOUBLE_STRING, BIGINT_STRING, DECIMAL_STRING, BIGINT_BIGINTEGER, STRING_BLOB, ASCII_BLOB, TIMESTAMP_STRING_MILLIS, TIMESTAMP_STRING_FORMAT, DSE geo) with exact Java semantics | `cdm-codec` | `cdc_020_*` | #11, #12, #13, #14 |
| `CDC-021` <sup>P</sup> | An empty `timestamp_format`, or an unparseable timezone, MUST be a Tier-1 configuration error | `cdm-codec` | `cdc_021_*` | #11, #12, #13, #14 |
| `CDC-022` <sup>P</sup> | Java `SimpleDateFormat`/`DateTimeFormatter` patterns (e.g | `cdm-codec` | `cdc_022_*` | #11, #12, #13, #14 |
| `CDC-030` <sup>N</sup> | Codecs MUST be pluggable: `CodecPlugin` registers `(from_type, to_type) → Converter` pairs into the registry, and third-party… | `cdm-codec` | `cdc_030_*` | #11, #12, #13, #14 |
| `CDC-031` <sup>N</sup> | `cdm codecs list` MUST print all registered codecs and the type pairs they serve; `GET /v1/codecs` MUST return the same | `cdm-codec` | `cdc_031_*` | #11, #12, #13, #14 |
| `CDC-032` <sup>N</sup> | Every codec MUST have a round-trip property test (`TST-031`) | `cdm-codec` | `cdc_032_*` | #11, #12, #13, #14 |

### TRK

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `TRK-001` <sup>P</sup> | Tracking MUST be enabled when any of: `track_run.enabled = true`, `track_run.run_id != 0`, `track_run.previous_run_id != 0`,… | `cdm-track` | `trk_001_*` | #25 |
| `TRK-002` <sup>P</sup> | When enabled and `run_id == 0`, a run id MUST be generated | `cdm-track` | `trk_002_*` | #25 |
| `TRK-003` <sup>P+</sup> | which preserves the "latest = highest" ordering Java relies on while eliminating collisions across nodes | `cdm-core::domain::run`, `cdm-track` | `trk_003_*` | #3, #25 |
| `TRK-010` <sup>P</sup> | Create `cdm_run_info` and `cdm_run_details` in the target keyspace with exactly the Java schema | `cdm-track` | `trk_010_*` | #25 |
| `TRK-011` <sup>N</sup> | cdm-rs MUST additionally maintain, in the same keyspace, `cdm_run_leases` for distributed coordination (`DST-010`) | `cdm-track` | `trk_011_*` | #25 |
| `TRK-012` <sup>P</sup> | Statuses: `NOT_STARTED, STARTED, PASS, FAIL, DIFF, DIFF_CORRECTED, ENDED` | `cdm-core::domain::run`, `cdm-track` | `trk_012_*` | #3, #25 |
| `TRK-013` <sup>P</sup> | `cdm_run_info.run_type` is the exact upper-case job name `MIGRATE`/`VALIDATE`/`GUARDRAIL`, matching Java's `jobType.toString()` | `cdm-core` | `trk_013_*` | #3 |
| `TRK-014` <sup>P</sup> | Run and range statuses use the exact Java spellings, underscores included | `cdm-core` | `trk_014_*` | #3 |
| `TRK-020` <sup>P</sup> | Run initialisation MUST: reject a `run_id` that already exists; insert the info row as `NOT_STARTED`; insert one details row… | `cdm-track` | `trk_020_*` | #25 |
| `TRK-021` <sup>P</sup> | Each range MUST be updated to `STARTED` (setting `start_time`) when work begins and to its terminal status with the metrics… | `cdm-track` | `trk_021_*` | #25 |
| `TRK-022` <sup>P</sup> | On run completion the info row MUST be updated with `end_time`, the aggregate metrics string, and status `ENDED` | `cdm-track` | `trk_022_*` | #25 |
| `TRK-030` <sup>P</sup> | `track_run.auto_rerun = true` MUST select the most recent run for `(table_name, run_type)` and adopt it as the previous run… | `cdm-track` | `trk_030_*` | #25 |
| `TRK-031` <sup>P</sup> | Resuming from a previous run MUST re-plan only ranges whose status is in `{NOT_STARTED, STARTED, FAIL, DIFF}`, shuffled | `cdm-track` | `trk_031_*` | #25 |
| `TRK-032` <sup>P</sup> | If the previous run's info row is missing or `NOT_STARTED`, resumption MUST fall back to a fresh full plan (Java's… | `cdm-track` | `trk_032_*` | #25 |
| `TRK-033` <sup>P</sup> | `track_run.rerun_multiplier > 1` MUST subdivide each pending range into that many sub-ranges at 100% coverage, to break up… | `cdm-track` | `trk_033_*` | #25 |
| `TRK-034` <sup>N</sup> | `cdm runs list\|show\|resume\|cancel` MUST provide first-class run management, and `GET /v1/runs`, `GET /v1/runs/{id}` the API… | `cdm-track` | `trk_034_*` | #25 |
| `TRK-035` <sup>N</sup> | Tracking writes MUST be batched and asynchronous with a bounded queue, so tracking never becomes the throughput bottleneck; on… | `cdm-track` | `trk_035_*` | #25 |
| `TRK-036` <sup>N</sup> | Tracking MUST be storable in a pluggable backend (`TrackingStore` trait): the Cassandra target keyspace (default,… | `cdm-track` | `trk_036_*` | #25 |

### DST

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `DST-001` <sup>N</sup> | `cluster.enabled = true` MUST allow N cdm-rs processes started with the same `run_id` and the same config to cooperatively… | `cdm-cluster` | `dst_001_*` | #50 |
| `DST-002` <sup>N</sup> | Exactly one node MUST perform run initialisation (`TRK-020`) | `cdm-cluster` | `dst_002_*` | #50 |
| `DST-003` <sup>N</sup> | Configuration consistency MUST be enforced: the initialising node records a hash of the effective, secret-redacted config in… | `cdm-cluster` | `dst_003_*` | #50 |
| `DST-010` <sup>N</sup> | Range claiming uses a `cdm_run_leases` table keyed by (table_name, run_id), token_min | `cdm-cluster` | `dst_010_*` | #50 |
| `DST-011` <sup>N</sup> | A claim MUST be `INSERT ... IF NOT EXISTS` / `UPDATE ... IF lease_until < now` (LWT, `SERIAL` consistency) | `cdm-cluster` | `dst_011_*` | #50 |
| `DST-012` <sup>N</sup> | Leases MUST be renewed every `cluster.heartbeat_interval` while the range is being processed, and MUST expire after… | `cdm-cluster` | `dst_012_*` | #50 |
| `DST-013` <sup>N</sup> | A range that has been attempted more than `cluster.max_attempts` (default 3) times MUST be marked `FAIL` and abandoned rather… | `cdm-cluster` | `dst_013_*` | #50 |
| `DST-014` <sup>N</sup> | Reclaiming a range after a node death MUST be safe | `cdm-cluster` | `dst_014_*` | #51 |
| `DST-015` <sup>N</sup> | . *(Counters are not idempotent; correctness beats convenience.)* **DST-016 [N]** — Metrics MUST be aggregated across nodes:… | `cdm-cluster` | `dst_015_*` | #51 |
| `DST-016` <sup>N</sup> | Metrics MUST be aggregated across nodes: each node periodically writes its counter snapshot; any node (and the API) can… | `cdm-cluster` | `dst_016_*` | #51 |
| `DST-017` <sup>N</sup> | A node MUST cleanly deregister on shutdown, releasing its leases immediately rather than waiting for expiry | `cdm-cluster` | `dst_017_*` | #51 |
| `DST-018` <sup>N</sup> | `cdm cluster status` and `GET /v1/cluster` MUST list live nodes, their leases, their per-node throughput and their last heartbeat | `cdm-cluster` | `dst_018_*` | #51 |
| `DST-019` <sup>N</sup> | Distributed mode MUST be fully exercised in integration tests with simulated node death mid-range (`TST-042`) | `cdm-cluster` | `dst_019_*` | #52 |

### MET

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `MET-001` <sup>P</sup> | The following counters MUST exist with exactly these semantics: `READ, WRITE, MISMATCH, CORRECTED_MISMATCH, MISSING,… | `cdm-metrics` | `met_001_*` | #19 |
| `MET-002` <sup>P</sup> | Per-job counter registration must match Java exactly for migrate, validate and guardrail | `cdm-metrics` | `met_002_*` | #19 |
| `MET-003` <sup>P+</sup> | (Java throws at runtime) | `cdm-metrics` | `met_003_*` | #19 |
| `MET-004` <sup>P</sup> | The interim/committed two-level accounting MUST be preserved: per-range interim counts are folded into totals on range… | `cdm-metrics` | `met_004_*` | #19 |
| `MET-005` <sup>P</sup> | The metrics string format MUST be reproduced exactly: `Read: 10; Write: 9; Skipped: 1` (title-cased counter names, `; `… | `cdm-metrics` | `met_005_*` | #19 |
| `MET-006` <sup>P</sup> | The final metrics block must be printed in the Java format so existing assertion tooling keeps working | `cdm-metrics` | `met_006_*` | #19 |
| `MET-010` <sup>N</sup> | In addition to counters, the following MUST be recorded: rows/sec (origin and target, 1s/10s/60s EWMA), bytes/sec, request… | `cdm-metrics` | `met_010_*` | #36 |
| `MET-011` <sup>N</sup> | Progress MUST be computable as `ranges_completed / ranges_total`, refined by `system.size_estimates` row estimates, with an… | `cdm-metrics` | `met_011_*` | #36 |
| `MET-020` <sup>N</sup> | A Prometheus endpoint MUST be exposed at `GET /metrics` with metric names prefixed `cdm_` and labels `{run_id, job, side,… | `cdm-metrics` | `met_020_*` | #37 |
| `MET-021` <sup>N</sup> | OpenTelemetry OTLP export of metrics **and** traces MUST be supported, configured by `metrics.otlp.endpoint` | `cdm-metrics` | `met_021_*` | #37 |
| `MET-030` <sup>N</sup> | A structured event stream MUST be emitted (`RunStarted`, `RangeStarted`, `RangeCompleted`, `Discrepancy`, `Warning`, `Error`,… | `cdm-metrics` | `met_030_*` | #38 |
| `MET-031` <sup>N</sup> | An interactive terminal UI (`cdm migrate --tui`) MUST show live throughput, progress bar, ETA, per-node status in cluster… | `cdm-metrics` | `met_031_*` | #39 |
| `MET-032` <sup>N</sup> | All logs MUST be `tracing`-based, with `logging.format = json` producing structured records suitable for ingestion, and MUST… | `cdm-metrics` | `met_032_*` | #38 |
| `MET-033` <sup>N</sup> | A run summary MUST be writable to a file (`--summary-out report.json`) containing config hash, plan, all counters, timings,… | `cdm-metrics` | `met_033_*` | #40 |

### CLI

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `CLI-001` <sup>N</sup> | A single `cdm` binary with subcommands: migrate, validate, guardrail, plan, runs, config, schema, connect, codecs, cluster, serve, mcp, completions, version | `cdm-cli` | `cli_001_*` | #10 |
| `CLI-002` <sup>P</sup> | Java invocation shapes MUST be accepted for a smooth transition: `--properties-file <file>` and `--conf… | `cdm-cli` | `cli_002_*` | #5 |
| `CLI-003` <sup>N</sup> | `cdm config convert --from cdm.properties --to cdm.toml` MUST translate a Java config to canonical form, annotating deprecated… | `cdm-cli` | `cli_003_*` | #10 |
| `CLI-004` <sup>N</sup> | Exit codes MUST be meaningful and documented: `0` success · `1` completed with failures/discrepancies · `2` configuration… | `cdm-cli` | `cli_004_*` | #10 |
| `CLI-005` <sup>N</sup> | `--output json` MUST render machine-readable output for every non-streaming command | `cdm-cli` | `cli_005_*` | #10 |
| `CLI-006` <sup>N</sup> | `cdm config init` MUST run an interactive wizard (skippable with `--non-interactive`) that connects, introspects the schema,… | `cdm-cli` | `cli_006_*` | #10 |
| `CLI-007` <sup>N</sup> | Shell completions for bash/zsh/fish/powershell MUST be generated, plus a man page | `cdm-cli` | `cli_007_*` | #10 |

### API

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `API-001` <sup>N</sup> | An **OpenAPI 3.1** document MUST be the single source of truth for the control plane, checked in at `api/openapi.yaml`, and… | `cdm-api` | `api_001_*` | #42 |
| `API-002` <sup>N</sup> | The document MUST be **generated from Rust types** (`utoipa` derives on the same structs used by the engine, including… | `cdm-api` | `api_002_*` | #42 |
| `API-003` <sup>N</sup> | The full v1 endpoint set (health, config, schema, plan, runs, ranges, metrics, events, discrepancies, cluster, registries, /metrics) | `cdm-api` | `api_003_*` | #42 |
| `API-004` <sup>N</sup> | Run submission MUST be asynchronous: `POST /v1/runs` returns `202` with the run id and a `Location` header; progress is polled… | `cdm-api` | `api_004_*` | #42 |
| `API-005` <sup>N</sup> | All errors MUST use RFC 9457 `application/problem+json`, carrying the structured diagnostic from `ERR-002` (including `key`,… | `cdm-api` | `api_005_*` | #42 |
| `API-006` <sup>N</sup> | Every mutating endpoint MUST accept an `Idempotency-Key` header; replaying a key returns the original result rather than… | `cdm-api` | `api_006_*` | #43 |
| `API-007` <sup>N</sup> | Pagination MUST be cursor-based and uniform (`?cursor=&limit=`), with `next_cursor` in the response envelope | `cdm-api` | `api_007_*` | #43 |
| `API-008` <sup>N</sup> | The API MUST be versioned by path prefix; breaking changes require `/v2` | `cdm-api` | `api_008_*` | #43 |
| `API-009` <sup>N</sup> | Long-running operations MUST also be observable via `GET /v1/runs/{id}` returning `status` from a documented state machine:… | `cdm-api` | `api_009_*` | #41 |
| `API-010` <sup>N</sup> | The server MUST run embedded in the same process that executes the job (`cdm migrate --serve`) *or* standalone as a controller… | `cdm-api` | `api_010_*` | #42 |

### MCP

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `MCP-001` <sup>N</sup> | An MCP server MUST be provided, over stdio (`cdm mcp`) and Streamable HTTP (`/mcp` when serving) | `cdm-mcp` | `mcp_001_*` | #45 |
| `MCP-002` <sup>N</sup> | MCP **tools** MUST be generated from the OpenAPI document — one tool per operation marked `x-mcp: tool`, with the input schema… | `cdm-mcp` | `mcp_002_*` | #45 |
| `MCP-003` <sup>N</sup> | MCP **resources** MUST expose: the config JSON Schema, the property reference, the origin/target schema, the current run list,… | `cdm-mcp` | `mcp_003_*` | #45 |
| `MCP-004` <sup>N</sup> | MCP **prompts** MUST ship for the common workflows: "plan a migration for table X", "explain this validation report", "tune… | `cdm-mcp` | `mcp_004_*` | #45 |
| `MCP-005` <sup>N</sup> | Destructive or long-running tools (`submit_run`, `cancel_run`) MUST be annotated with MCP tool hints (`destructiveHint`,… | `cdm-mcp` | `mcp_005_*` | #45 |
| `MCP-006` <sup>N</sup> | Tool outputs MUST be structured content matching the OpenAPI response schema, not prose | `cdm-mcp` | `mcp_006_*` | #45 |

### A2A

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `A2A-001` <sup>N</sup> | An Agent Card is served at `/.well-known/agent-card.json`, generated from the same OpenAPI document | `cdm-a2a` | `a2a_001_*` | #46 |
| `A2A-002` <sup>N</sup> | Declared skills: plan-migration, run-migration, validate-migration, explain-discrepancies, tune-configuration | `cdm-a2a` | `a2a_002_*` | #46 |
| `A2A-003` <sup>N</sup> | A2A tasks map onto the run lifecycle, with streaming updates backed by the `MET-030` event stream | `cdm-a2a` | `a2a_003_*` | #46 |
| `A2A-004` <sup>N</sup> | Authentication schemes in the agent card match those enforced by the API | `cdm-a2a` | `a2a_004_*` | #46 |
| `A2A-005` <sup>N</sup> | The adapter contains no business logic; a conformance test asserts parity with REST and MCP | `cdm-a2a` | `a2a_005_*` | #46 |

### UI

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `UI-001` <sup>N</sup> | The Config Builder MUST be reimplemented as a static web app embedded in the binary (`rust-embed`) and served at `/ui` by `cdm… | `cdm-ui` | `ui_001_*` | #47 |
| `UI-002` <sup>N</sup> | It MUST drive the same API as every other client: `POST /v1/config/validate`, `/v1/config/generate`, `GET /v1/schema` | `cdm-ui` | `ui_002_*` | #47 |
| `UI-003` <sup>P</sup> | Feature parity with the React `cdm-config-builder`: CQL DDL paste-and-parse, sectioned form (connection, schema, performance,… | `cdm-ui` | `ui_003_*` | #47 |
| `UI-004` <sup>P</sup> | The best-practice rules engine MUST be preserved and MUST live **server-side** so CLI, API and UI share it: table size GB →… | `cdm-config` | `ui_004_*` | #4, #5, #6 |
| `UI-005` <sup>N</sup> | The UI MUST additionally provide live run monitoring (progress, throughput, errors) by consuming the SSE event stream | `cdm-ui` | `ui_005_*` | #48 |
| `UI-006` <sup>N</sup> | The UI MUST be usable offline and MUST NOT make third-party network requests | `cdm-ui` | `ui_006_*` | #47 |

### PLG

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `PLG-001` <sup>N</sup> | `CodecPlugin` — register conversions between CQL type pairs | `cdm-core::registry` | `plg_001_*` | #3 |
| `PLG-002` <sup>N</sup> | `FeaturePlugin` — participate in config validation, statement construction, record transformation, and comparison | `cdm-core::registry` | `plg_002_*` | #3 |
| `PLG-003` <sup>N</sup> | `FilterPlugin` / `GuardrailPlugin` — row-level predicates and checks | `cdm-core::registry` | `plg_003_*` | #3 |
| `PLG-004` <sup>N</sup> | `JobPlugin` — register an entirely new job type alongside migrate/validate/guardrail | `cdm-core::registry` | `plg_004_*` | #3 |
| `PLG-005` <sup>N</sup> | `SourcePlugin` / `SinkPlugin` — abstract the origin and target behind `RowSource` / `RowSink` traits so alternative backends… | `cdm-core::registry` | `plg_005_*` | #3 |
| `PLG-006` <sup>N</sup> | `MetricsExporterPlugin` — additional metric sinks | `cdm-core::registry` | `plg_006_*` | #3 |
| `PLG-007` <sup>N</sup> | `TrackingStorePlugin` — alternative tracking backends (`TRK-036`) | `cdm-core::registry` | `plg_007_*` | #3 |
| `PLG-010` <sup>N</sup> | All plugins register through one `Registry`; built-ins use the same public registration path as third parties | `cdm-core::registry` | `plg_010_*` | #3 |
| `PLG-011` <sup>N</sup> | Plugin registration MUST be possible both at compile time (Cargo feature + `inventory`-style linkage) and at runtime via a… | `cdm-core::registry` | `plg_011_*` | #54 |
| `PLG-012` <sup>N</sup> | Every plugin trait MUST be object-safe, `Send + Sync`, and documented with a worked example in `docs/EXTENDING.md` plus a… | `cdm-core::registry` | `plg_012_*` | #3, #57 |
| `PLG-013` <sup>N</sup> | Plugins MUST be able to contribute configuration keys, which are then automatically included in the JSON Schema, OpenAPI, docs… | `cdm-core::registry` | `plg_013_*` | #3 |

### ERR

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `ERR-001` <sup>N</sup> | A single `CdmError` enum with stable documented kinds, each carrying side/keyspace/table/column/range context | `cdm-core::error` | `err_001_*` | #3 |
| `ERR-002` <sup>N</sup> | A `Diagnostic` type rendered identically as CLI text, problem+json and SSE events | `cdm-core::error` | `err_002_*` | #3 |
| `ERR-003` <sup>N</sup> | Every diagnostic code MUST have a page in `docs/errors/<CODE>.md`, and `docs_url` MUST point at it | `cdm-core::error` | `err_003_*` | #3, #57 |
| `ERR-004` <sup>N</sup> | `unwrap()`/`expect()`/`panic!` MUST be denied by Clippy in all non-test code except in `main` startup and documented… | `cdm-core::error` | `err_004_*` | #3 |
| `ERR-005` <sup>P</sup> | Bind failures MUST log the value, its type, the column name, the CQL type, the bind index and the statement CQL — matching… | `cdm-core::error` | `err_005_*` | #18 |

### SEC

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `SEC-001` <sup>N</sup> | Secrets MUST never appear in logs, metrics, API responses, events, run summaries, config dumps, or error messages | `workspace` | `sec_001_*` | #4 |
| `SEC-002` <sup>N</sup> | Row values MUST NOT be logged by default | `workspace` | `sec_002_*` | #4 |
| `SEC-010` <sup>N</sup> | The HTTP control plane MUST support `none` (loopback only), `bearer` token, and mTLS authentication | `workspace` | `sec_010_*` | #44 |
| `SEC-011` <sup>N</sup> | The server MUST support TLS termination natively, plus configurable CORS defaulting to same-origin | `workspace` | `sec_011_*` | #44 |
| `SEC-012` <sup>N</sup> | MCP and A2A endpoints MUST enforce the same auth as the REST API. **SEC-020 [N]** — Runtime dynamic plugin loading MUST be… | `workspace` | `sec_012_*` | #44 |
| `SEC-020` <sup>N</sup> | Runtime dynamic plugin loading MUST be disabled by default and MUST log a prominent warning when enabled | `workspace` | `sec_020_*` | #54 |
| `SEC-030` <sup>N</sup> | Supply chain: `cargo-deny` (licenses, advisories, bans, sources), `cargo-audit`, `cargo-vet` or equivalent, SBOM (CycloneDX)… | `workspace` | `sec_030_*` | #56 |
| `SEC-031` <sup>N</sup> | `#![forbid(unsafe_code)]` MUST apply to every crate except a documented, minimal, reviewed allowance (currently: none expected) | `workspace` | `sec_031_*` | #4 |

### NFR

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `NFR-001` | Static binary for `linux-x86_64` (gnu + musl), `linux-aarch64`, `macos-x86_64`, `macos-aarch64`, `windows-x86_64` | `workspace` | `nfr_001_*` | #56 |
| `NFR-002` | Cold start to first row read MUST be < 2 seconds for a single-table run | `workspace` | `nfr_002_*` | #55 |
| `NFR-003` | Memory MUST be bounded and configurable: steady-state RSS MUST NOT exceed `~200 MB + (max_inflight_reads +… | `workspace` | `nfr_003_*` | #55 |
| `NFR-004` | Throughput MUST be ≥ 2× Java CDM on the same hardware for the reference workload, measured by the benchmark suite (`TST-060`) | `workspace` | `nfr_004_*` | #55 |
| `NFR-005` | MSRV MUST be an explicitly declared, tested Rust version, bumped only in a minor release, and stated in `Cargo.toml`… | `workspace` | `nfr_005_*` | #57 |
| `NFR-006` | Every public item in every crate MUST have rustdoc; `#![deny(missing_docs)]` on all library crates | `workspace` | `nfr_006_*` | #57 |
| `NFR-007` | All timestamps in APIs, logs and reports MUST be RFC 3339 UTC. Writetimes remain microseconds since epoch (Cassandra… | `workspace` | `nfr_007_*` | #57 |
| `NFR-008` | The tool MUST be usable with no network access to anything except origin, target and (optionally) the Astra DevOps API. --- ## 21 | `workspace` | `nfr_008_*` | #57 |

### TST

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `TST-001` | **Unit tests** live beside the code (`#[cfg(test)]`) and MUST NOT require a cluster | `cdm-testkit / tests` | `tst_001_*` | #33 |
| `TST-002` | **Integration tests** (`tests/`) MUST run against real clusters via `testcontainers`, matrixed over Cassandra 4.1, Cassandra… | `cdm-testkit / tests` | `tst_002_*` | #33 |
| `TST-003` | End-to-end SIT parity: every one of the 19 Java SIT cases ported and asserting the identical counter block | `cdm-testkit / tests` | `tst_003_*` | #33 |
| `TST-010` | **Property-based tests** (`proptest`) MUST cover: the token splitter (`TOK-003`) — ranges are contiguous, non-overlapping, and… | `cdm-testkit / tests` | `tst_010_*` | #33 |
| `TST-020` | **Differential tests against Java CDM**: a harness runs both implementations against the same seeded dataset and asserts… | `cdm-testkit / tests` | `tst_020_*` | #16 |
| `TST-030` | Zero-copy passthrough (`MIG-040`) MUST be proven lossless by a property test comparing passthrough output against full… | `cdm-testkit / tests` | `tst_030_*` | #32 |
| `TST-031` | Every codec MUST have: an encode/decode round-trip property test, a known-vector test with fixtures shared with the Java… | `cdm-testkit / tests` | `tst_031_*` | #32 |
| `TST-040` | **Fault injection**: a `FaultySession` test double MUST inject read timeouts, write timeouts, unavailable, overloaded,… | `cdm-testkit / tests` | `tst_040_*` | #34 |
| `TST-041` | **Resume tests**: kill a run at a random point, restart with `auto_rerun`, and assert the final target state equals a clean… | `cdm-testkit / tests` | `tst_041_*` | #34 |
| `TST-042` | **Distributed tests**: 3 nodes, one killed mid-range; assert lease reclaim, no double processing of counter ranges… | `cdm-testkit / tests` | `tst_042_*` | #34 |
| `TST-050` | **Interface conformance**: the same logical operation issued via CLI, REST, MCP and A2A MUST produce identical results and… | `cdm-testkit / tests` | `tst_050_*` | #15 |
| `TST-051` | **OpenAPI contract tests**: every endpoint MUST be exercised and its response validated against the schema (schemathesis or… | `cdm-testkit / tests` | `tst_051_*` | #15 |
| `TST-060` | **Benchmarks**: `criterion` micro-benchmarks for the hot path (bind, convert, compare) and a reproducible macro-benchmark… | `cdm-testkit / tests` | `tst_060_*` | #49 |
| `TST-070` | **Snapshot tests** (`insta`) for CLI output, generated CQL, generated config files, error messages, and the OpenAPI document | `cdm-testkit / tests` | `tst_070_*` | #10 |
| `TST-080` | **Fuzzing** (`cargo-fuzz`) of the properties parser, the CQL identifier quoter, the JSON extractor, and the Java date-pattern… | `cdm-testkit / tests` | `tst_080_*` | #58 |
| `TST-090` | **Doc tests**: every rustdoc example MUST compile and run; `docs/` code blocks MUST be extracted and compiled by `xtask… | `cdm-testkit / tests` | `tst_090_*` | #57 |
| `TST-100` | A `cdm-testkit` crate MUST provide: containerised origin/target fixtures, a schema and data generator covering all CQL types,… | `cdm-testkit / tests` | `tst_100_*` | #16 |
| `TST-101` | Test data generation MUST be deterministic and seeded; failures MUST print the seed | `cdm-testkit / tests` | `tst_101_*` | #16 |
| `TST-102` | Integration tests MUST be runnable locally with one command (`cargo xtask it`) and MUST skip (not fail) with a clear message… | `cdm-testkit / tests` | `tst_102_*` | #16 |

### OPS

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `OPS-001` | Cargo workspace with resolver v2, shared `[workspace.dependencies]`, and `[workspace.lints]` applied to every crate (DRY) | `.github, xtask` | `ops_001_*` | #1 |
| `OPS-002` | `rustfmt.toml` and `clippy.toml` checked in; `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D… | `.github, xtask` | `ops_002_*` | #1 |
| `OPS-003` | Pre-commit hooks: fmt, clippy, unused deps, typos, cargo-deny, taplo, yamllint, markdownlint, shellcheck, commit-msg, gitleaks, traceability, generated-artefact freshness | `.github, xtask` | `ops_003_*` | #1 |
| `OPS-004` | **Conventional Commits** MUST be enforced | `.github, xtask` | `ops_004_*` | #1 |
| `OPS-010` | The GitHub Actions workflow set: ci, integration, sit, coverage, security, bench, differential, openapi, docs, release, container | `.github, xtask` | `ops_010_*` | #1 |
| `OPS-011` | A CI job MUST verify traceability: every `REQ-ID` in `SPEC.md` appears in `TRACEABILITY.md`; every ID in `TRACEABILITY.md`… | `.github, xtask` | `ops_011_*` | #1 |
| `OPS-012` | A CI job MUST verify that generated artefacts (`api/openapi.yaml`, `schema/cdm-config.schema.json`,… | `.github, xtask` | `ops_012_*` | #1 |
| `OPS-020` | Releases MUST publish: signed binaries for all `NFR-001` targets, checksums, a CycloneDX SBOM, a multi-arch container image,… | `.github, xtask` | `ops_020_*` | #56 |
| `OPS-021` | Versioning MUST be SemVer | `.github, xtask` | `ops_021_*` | #56 |
| `OPS-022` | The container image MUST be distroless, run as non-root, contain only the `cdm` binary, and default to `cdm serve` | `.github, xtask` | `ops_022_*` | #56 |
| `OPS-023` | `CODEOWNERS`, issue templates (bug/feature, mirroring the Java repo's fields), a PR template with a traceability checklist,… | `.github, xtask` | `ops_023_*` | #1 |
| `OPS-024` | A `Makefile`/`justfile` and `cargo xtask` MUST provide one-command entry points: `build`, `test`, `it`, `sit`, `lint`,… | `.github, xtask` | `ops_024_*` | #1 |
| `OPS-030` | Every PR MUST be small, single-purpose, mapped to requirement IDs, and green on all required checks | `.github, xtask` | `ops_030_*` | #1 |

### COMPAT

| ID | Requirement | Home | Verified by | PR |
|---|---|---|---|---|
| `COMPAT-001` | `cdm --compat-java` MUST enable a bundle of behaviours that exactly reproduce Java quirks where cdm-rs deliberately improves… | `workspace` | `compat_001_*` | #35 |
| `COMPAT-002` | `docs/MIGRATION_FROM_JAVA.md` MUST list every behavioural difference, with rationale and the flag that restores the old behaviour | `workspace` | `compat_002_*` | #35 |
| `COMPAT-003` | The run-tracking tables MUST remain schema-compatible so a Java run can be resumed by cdm-rs and vice versa | `workspace` | `compat_003_*` | #34 |
| `COMPAT-004` | The final metrics block and per-range `run_info` strings MUST remain character-identical (`MET-005`, `MET-006`) so existing… | `workspace` | `compat_004_*` | #34 |
