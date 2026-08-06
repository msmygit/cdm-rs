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
| 17 | The Astra bundle is downloaded only when **both** `astra.database.id` and `astra.scb.region` are set, and a download failure is logged and ignored | `database_id` alone triggers the download, and a failure stops the run (`CON-004`) | Falling through to `localhost:9042` because a token expired is how a migration silently runs against the wrong cluster | set the bundle path explicitly |
| 18 | `truststore` + `isAstra` writes a synthetic bundle zip (`config.json`, `identity.jks`, `trustStore.jks`) into the working directory | The same material is held in memory; nothing is written to disk (`CON-002`, `SEC-001`) | A private key left on disk with no cleanup, for a file only the Spark connector ever wanted | — |
| 19 | `tls.enabledAlgorithms` defaults to `TLS_RSA_WITH_AES_128_CBC_SHA,TLS_RSA_WITH_AES_256_CBC_SHA`, and JSSE silently drops a suite it cannot offer | The default is read as "no preference" and the TLS backend chooses; an **explicitly** named suite the backend cannot offer is a startup error listing what it can (`CON-007`) | rustls implements no static-RSA suite — they have no forward secrecy and TLS 1.3 removed them — so honouring that default literally would make TLS unusable, and silently ignoring a pinned suite is how operators come to believe in ciphers that were never offered | name a supported suite |
| 20 | The Java driver supports the Astra secure-connect-bundle natively, including SNI routing | The single-endpoint fallback is used (`CON-026`), because `scylla-rust-driver` cannot set a per-connection TLS `server_name` | See [Astra connectivity](#astra-connectivity) | — |
| 21 | A malformed JSON document throws out of the bind and fails the whole token range | The row is counted `ERROR`, its primary key is logged, and the range continues (`FEA-034`) | One bad document should cost one row, not a partition's worth of work — and the failing key is what an operator needs in order to fix it | — |
| 22 | `extractJson.exclusive` filters columns with `name.endsWith(extractColumn)`, so `city` also retains `oldcity` | Exact name matching (`FEA-033`) | The setting exists to narrow the column set; a suffix match silently widens it, and the extra column is then read and written for every row | — |
| 23 | `filter.cassandra.whereCondition` is prefixed with ` AND ` unless it `startsWith("AND")` — so `android_id = 1` loses its conjunction | The prefix is only suppressed when `AND` is followed by whitespace or `(` (`FEA-050`) | Java's test produces invalid CQL for any condition on a column whose name begins with those three letters | — |
| 24 | Whether a counter table disables TTL/writetime is decided from the **origin** table alone | A counter column on either side disables it, and an explicit TTL/writetime configuration is rejected at validation time (`FEA-045`, `CFG-036`) | Neither side can accept a TTL or a timestamp on write; deciding from the origin lets an origin→counter-target run fail per row at write time instead of once at startup | — |
| 25 | `ExplodeMap.explode` collects entries into a `Set`, so the order in which a map's entries are written is unspecified | Wire order, which for a Cassandra map is key order (`FEA-020`) | The same entries reach the same target rows either way, but a reproducible order makes two runs' logs comparable | — |
| 26 | `Ctrl-C` kills the `spark-submit` process: in-flight ranges stop mid-write, the run row is left `STARTED`, and the final counter block is never printed | Graceful shutdown (`ENG-010`): claiming stops, in-flight ranges drain within `perfops.shutdown_grace` (default `60s`), the counters are flushed and reported, the run is marked `INTERRUPTED` and the process exits `4` | The numbers an operator needs in order to decide what to do next are exactly the ones an abrupt stop destroys. A range still running at the deadline is abandoned rather than waited for, and left `STARTED` so a resume re-plans it (`TRK-031`) | — |
| 27 | The validate diff log prints the differing origin and target **values** | Every value position renders `<redacted>`; the discrepancy is identified by primary key and column name (`VAL-017`) | `SEC-002` forbids logging row values by default, and `ERR-005` resolved the identical conflict the same way. The key is what an operator needs in order to look at the row, and looking is then a decision a human takes deliberately rather than one a log file takes on their behalf | `VAL-013`'s machine-readable report, with `validate.report.redact_values = false` |
| 28 | Mismatch-detail columns are emitted in whatever order a `parallel()` stream over a shared `StringBuffer` happened to produce | Target-column order (`VAL-006`) | Two runs over the same data must produce comparable diff logs; a nondeterministic ordering makes `diff` on two of them useless | — |
| 29 | A mismatch on an explode-map key or value column calls `List.get(-1)`, throws `IndexOutOfBoundsException`, and reports the exception's text where the values should be | The same column is reported through `VAL-009`'s exception form, with an explanation rather than a Java stack frame | The observable behaviour — a counted `MISMATCH` carrying a message — is preserved; only the message changes, from an accident to a sentence | — |
| 30 | The guardrail report formats `colSize / 1000`, where both are `int`. The division is integer division, so its `DecimalFormat("0.###")` never has a fraction to render and every column between 1000 and 1999 bytes reports `(1)` | The real quotient, with the trailing zeros `0.###` would have dropped: `value(1.474)`, `fruits(2.5)` (`GRD-003`) | A report whose purpose is to rank oversized columns has to be able to tell two of them apart. The prefix, separator and threshold comparison are untouched and are exact parity | `--compat-java` |
| 31 | A guardrail finding names the offending columns and nothing else, so it cannot be traced back to a row | The finding also names the row's **primary key**, rendered in hex (`GRD-003`) | Without it the report says a table has three oversized rows and gives no way to find them. The key is the only field added, and a column *value* is still never logged (`SEC-002`) | — |
| 32 | `GuardrailCheckJobSession` logs `is disabled - is it configured correctly?` when `colSizeInKB` is unset and then runs anyway: `guardrailChecks` returns `null` for every row, every row counts `VALID`, and the whole table is reported clean | Startup error naming the property (`GRD-001`) | "No oversized columns" from a run that was never looking is the worst possible answer to the question being asked, and it is indistinguishable from the right one | set `feature.guardrail.column_size_kb` |
| 33 | A guardrail run always exits `0`, whatever it found | Exit `1` when any row was `LARGE`, `0` when none was (`CLI-004`) | A pipeline that gates a migration on a guardrail run can see only the exit code. The per-range tracking status stays `PASS`, so `cdm_run_details` remains byte-compatible either way (`TRK-012`) | read the `LARGE` counter instead of the code |
| 34 | `Guardrail.guardrailChecks` accumulates into a `HashMap`, so two runs over the same row can print its oversized columns in different orders | Projection order (`GRD-003`) | The same finding either way, but a reproducible order makes two runs' logs comparable — the same reasoning as item 25 | — |
| 35 | The guardrail is a job of its own and cannot run during a migration | It can, through `feature.guardrail.mode` (`GRD-004`); `block` withholds an oversized row from the target and counts it `SKIPPED` | A target with a hard column-size limit makes a migration that skips the handful of rows over it far more useful than one that fails on them. `SKIPPED` rather than `LARGE` because `MET-002` does not register `LARGE` for migrate — see the correction under `GRD-004` | leave `mode` at its `check` default |

`--compat-java` enables items 1, 2, 3 and 6 together, and additionally restores Java's
unconverted tuple elements (item 4) and its truncated guardrail sizes (item 30)
(`COMPAT-001`, `CDC-015`, `GRD-003`).

## Astra connectivity

Astra publishes no node addresses. Every CQL connection goes to one SNI proxy endpoint, and the
node it lands on is chosen by the TLS `server_name` of that connection — the target node's host id.
The Java driver sets that name per connection. `scylla-rust-driver` 1.7 cannot:

- a session takes **one** TLS context, and the per-endpoint hook (`TlsProvider`) is private, with a
  single `GlobalContext` variant;
- with the `rustls-023` backend the driver derives the name itself, as
  `ServerName::IpAddress(node_address.ip())`, and rustls sends no SNI extension for an IP name, so
  the proxy receives nothing to route on.

cdm-rs therefore uses the connection method DataStax documents for drivers without bundle support
(`CON-026`): one mutual-TLS connection to the host from `config.json`, on the port from `cqlshrc`,
with a warning at startup (`CON-027`). Everything else about Astra behaves as in Java CDM — the
DevOps API download (`CON-004`), the bundle contents (`CON-020`), and both credential spellings
(`CON-028`).

**What it costs.** Token-aware routing and per-node load balancing are lost: every request is
coordinated by whichever node the proxy picks, adding a hop. Throughput against Astra will be
materially lower than against a self-managed cluster reached directly. Nothing is *incorrect* —
Cassandra coordinates the request either way.

**What to do about it.** Nothing, today. The gap is a missing driver hook; it is raised upstream,
and the SNI path in `cdm-cql::astra::strategy` is written and tested up to the point where the hook
would plug in. When it lands, `driver_supports_per_connection_sni()` becomes a real capability check
and the strategy switches with no configuration change.

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
