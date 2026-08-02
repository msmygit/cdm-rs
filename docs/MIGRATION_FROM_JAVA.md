# Migrating from Java CDM to cdm-rs

| | |
|---|---|
| **Baseline** | Java `cassandra-data-migrator` 6.0.x |
| **Normative source** | [`SPEC.md`](./SPEC.md) — see `COMPAT-001`–`COMPAT-004` |

cdm-rs is designed as a drop-in replacement. Your existing `cdm.properties` file works unchanged,
the run-tracking tables are schema-compatible in both directions, and the final metrics block is
character-identical so existing assertion tooling keeps working.

This document lists every place cdm-rs behaves differently, why, and how to restore the old
behaviour. It is maintained as a hard requirement: a PR that introduces a difference without adding
it here fails review.

## Command mapping

| Java | cdm-rs |
|---|---|
| `spark-submit ... --class com.datastax.cdm.job.Migrate cdm.jar` | `cdm migrate --properties-file cdm.properties` |
| `spark-submit ... --class com.datastax.cdm.job.DiffData cdm.jar` | `cdm validate --properties-file cdm.properties` |
| `spark-submit ... --class com.datastax.cdm.job.GuardrailCheck cdm.jar` | `cdm guardrail --properties-file cdm.properties` |
| `--conf spark.cdm.<key>=<value>` | `--conf spark.cdm.<key>=<value>` (accepted verbatim) |
| `--master`, `--driver-memory`, `--executor-memory` | not applicable — no Spark, no JVM heap to size |
| `--conf spark.driver.extraJavaOptions=-Dlog4j2...` | `--log-level`, `--log-format` |

Everything else — the whole `spark.cdm.*` namespace — is accepted as-is. Non-`cdm` `spark.*` keys
are ignored silently, since they configured Spark and no longer apply.

To move to the canonical typed configuration:

```bash
cdm config convert --from cdm.properties --to cdm.toml
```

## Intentional behavioural differences

| # | Java behaviour | cdm-rs behaviour | Why | Restore with |
|---|---|---|---|---|
| 1 | An unrecognised consistency level is silently coerced to `LOCAL_QUORUM` | Configuration error (`CFG-161`) | Silently weakening consistency is a data-safety hazard, and the typo is invisible in the logs | `--compat-java` |
| 2 | `ALLOW FILTERING` is always appended to the origin select | Omitted when no CQL `whereCondition` is configured (`FEA-061`) | A pure token-range scan does not need it; unconditional use masks planner problems | `--compat-java` |
| 3 | UDT conversion round-trips through string `format`/`parse`, matching fields positionally | Recursive, name-matched with positional fallback (`CDC-014`) | The string round-trip is lossy for nested and non-round-trippable types | `--compat-java` |
| 4 | Tuple element conversion is explicitly unsupported | Implemented (`CDC-015`) | It was a gap, not a decision | — |
| 5 | Counter writes are retried by the driver's retry policy | Never retried; the range fails (`CON-012`) | Retrying a counter increment double-counts. Silent counter drift is worse than a visible failure | — |
| 6 | Batches are formed in row order and may span partitions | Grouped by partition key (`MIG-022`) | Multi-partition batches are a well-known Cassandra anti-pattern that costs throughput | `perfops.batch_grouping = legacy` |
| 7 | `spark.cdm.perfops.errorLimit` is documented but not implemented | Implemented (`ENG-009`) | The documented behaviour is what users expect | set to `0` (unlimited) |
| 8 | Using an unregistered counter throws at runtime | Rejected at startup (`MET-003`) | Fail in seconds, not hours | — |
| 9 | Configuration errors surface during execution | Three validation tiers before any data moves, all errors reported at once (`CFG-020`, `CFG-021`) | The most common operational complaint | — |
| 10 | Distributed execution requires a Spark cluster | Built-in lease-based coordination (`DST-001`) | Removes the largest deployment dependency | run single-node |
| 11 | A bundle and a host on the same side: bundle wins, host silently ignored | Tier-1 error naming which to drop (`CFG-041`) | A stale host from a previous migration must not look like live configuration | `--compat-java` |
| 12 | `guardrail.colSizeInKB` is `Long.parseLong`, so `0.5` fails to parse and becomes `null` | accepts a fraction (`GRD-002`) | the threshold is multiplied by 1000 to get bytes, where a fraction is meaningful | — (integral values are identical) |
| 13 | `blob` → `text`/`ascii` substitutes U+FFFD and writes the corruption through | validates and fails the row, counted as `ERROR` | a counted failure beats silent corruption | — |
| 14 | Enabling `TIMESTAMP_STRING_MILLIS` and `TIMESTAMP_STRING_FORMAT` together: later registration silently wins | startup error naming both | which one won decided the on-disk format of every timestamp | — |
| 15 | The migrate flush threshold is unreachable, so every write for a range is buffered until the range ends | the documented threshold works | peak memory scaled with range size and nothing could bound it; this is much of why Java needs `--driver-memory 25G` | — |
| 16 | A failed **validate** range always records `ERROR: 0` | the real count of unaccounted rows | the counter exists to say how many rows were lost; Java reads it from committed counts that are still zero on the failure path | — |
| 11 | `blob` → `text`/`ascii` conversion replaces malformed bytes with U+FFFD and writes the replacement through | The row fails with a `TypeConversion` error and is counted as `ERROR` (`CDC-020`) | Java's `new String(bytes)` silently corrupts the value; a loud failure on one row beats a quiet corruption of it | — |
| 12 | `TIMESTAMP_STRING_MILLIS` and `TIMESTAMP_STRING_FORMAT` both claim the `(TIMESTAMP, String)` pair; the later registration silently wins | Enabling both is a startup error naming both codecs (`CDC-030`, `PLG-010`) | Which codec wins decided the on-disk format of every timestamp; it should not depend on registration order | enable only one |

`--compat-java` enables items 1, 2, 3 and 6 together, and additionally restores Java's
unconverted tuple elements (item 4) (`COMPAT-001`, `CDC-015`).

## What is unchanged, and guaranteed to stay so

These are contractual, tested by the ported SIT suite and the nightly differential run:

- **The token-splitting algorithm**, including its overflow edge cases (`TOK-003`).
- **The tracking tables** `cdm_run_info` and `cdm_run_details`: identical schema, identical statuses.
  A run started by Java can be resumed by cdm-rs and vice versa (`COMPAT-003`).
- **The metrics strings**: `Read: 10; Write: 9; Skipped: 1` in `run_info`, and the
  `Final <Name> Record Count: N` block bracketed by `#` banners (`MET-005`, `MET-006`,
  `COMPAT-004`). Scripts built on `cdm-assert.sh` keep working.
- **TTL and writetime semantics**: per-row maximum across eligible columns, collections excluded
  unless `ttlwritetime.calc.useCollections` is set (`FEA-040`–`FEA-046`).
- **`UNSET` rather than `NULL`** for null and empty-collection values, so no tombstones are created
  (`MIG-012`).
- **All codec semantics**, including `DOUBLE_STRING`'s `0.#########` / `FLOOR` formatting and
  `TIMESTAMP_STRING_MILLIS`'s buffer-length disambiguation (`CDC-020`).
- **The guardrail 1000-byte-per-KB factor** (`GRD-002`).
- **Validation never deletes** (`VAL-010`).

## Operational differences

| | Java CDM | cdm-rs |
|---|---|---|
| Install | JVM + exact Spark build + jar | one static binary |
| Memory | `--driver-memory 25G --executor-memory 25G` | bounded and computable; `cdm plan` prints the envelope |
| Startup | tens of seconds | under two seconds |
| Progress | final counter block in the log | live counters, Prometheus, OTLP, SSE, terminal UI |
| Diff output | `cdm_logs/cdm_diff.log` | same file, **plus** JSON/CSV/Parquet reports and an API |
| Automation | parse `spark-submit` stdout | OpenAPI control plane, MCP, A2A |
| Container | ~1 GB with JVM, Spark and Maven | distroless, binary only |

## Two Java bugs we do not reproduce

Items 15 and 16 above are not design differences; they are defects, and `--compat-java` does **not**
restore either. Reproducing an unbounded-memory bug or a permanently-zero error counter has no
legitimate use, and both would defeat requirements cdm-rs is committed to (`NFR-003` bounded memory,
`ENG-008` honest failure accounting).

Everything else that differs remains restorable with `--compat-java`.

## Properties Java documents but does not implement

These appear in Java's `cdm-detailed.properties` but are absent from `KnownProperties` and from
Java's source, so Java silently ignores them. cdm-rs accepts the legacy names, so no configuration
breaks, but the behaviour differs because Java had none:

| Property | Java | cdm-rs |
|---|---|---|
| `spark.cdm.perfops.errorLimit` | ignored | implemented (`ENG-009`): the run aborts once total errors exceed it |
| `spark.cdm.feature.constantColumns.types` | ignored | not implemented; constant-column types come from the target schema (`FEA-011`) |

## Known caveats carried over

These are properties of Cassandra, not of either implementation:

- **Unfrozen `list` columns can accumulate duplicate entries on rerun**
  ([CASSANDRA-11368](https://issues.apache.org/jira/browse/CASSANDRA-11368)). Mitigate with
  `spark.cdm.transform.custom.writetime.incrementBy`. cdm-rs warns when you are exposed (`CFG-039`).
- **Counter tables cannot be validated by re-inserting.** Auto-correcting a missing counter row
  requires the explicit `autocorrect.missing.counter` opt-in, and in distributed mode a counter range
  whose owner died is failed rather than reprocessed (`DST-015`).
- **Rate limits are per node**, not global, unless you set `cluster.ratelimit_is_global`.
