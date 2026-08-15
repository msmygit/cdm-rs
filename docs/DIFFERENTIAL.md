# The Java-parity differential suite

`TST-020`. Java CDM 6.0.1 and cdm-rs are run over one seeded corpus, and their targets must hold
byte-identical state and their jobs must print identical counter blocks.

```bash
just differential                                          # the full corpus, a fresh seed
cargo xtask differential --corpus smoke                     # plumbing only, far fewer rows
cargo xtask differential --corpus full --seed 8814235700    # replay a failure exactly
cargo xtask differential --keep-clusters                    # leave the second pair up to poke at
```

Requirements: `TST-020`, and the parity claims it enforces — `COMPAT-003`, `COMPAT-004`, `MET-005`,
`MET-006`.

---

## 1. Why this is not in `BENCHMARKS.md`

It shares an environment with tier 3 of [`BENCHMARKS.md`](BENCHMARKS.md) and shares nothing else.

|  | Tier 3 (`java-comparison.yml`) | This suite (`differential.yml`) |
|---|---|---|
| Question | how much faster? | did it write the same bytes? |
| Answer | a ratio | a diff, or nothing |
| On a bad result | publishes it | **fails the build** |
| Dataset | large, plain, realistic | small, hostile, exhaustive |
| Cadence | fortnightly, Saturday | nightly |

The difference that matters is the third row. A benchmark that went red on a number would be muted
within a month, and `BENCHMARKS.md` says so at length. This one is a gate, and a document whose
opening premise is "these numbers gate nothing" is the wrong place to keep it.

The dataset differs for the same reason. A throughput workload wants rows that are representative;
a differential corpus wants rows that are *awkward* — every CQL type, nesting to depth 3, nulls,
empty collections, minimum and maximum integers, epoch boundaries, unicode, empty strings — because
the cheapest place for the two implementations to disagree is a value nobody would think to write.

## 2. What it reuses, and what it adds

The whole Java side is [`bench/java-comparison/environment/`](../bench/java-comparison/environment),
delivered and verified by the tier-3 work: Java CDM 6.0.1 on Spark 4.1.2 on Temurin 17, both
downloads SHA-512 verified, two `cassandra:5.0` nodes at fixed addresses on a fixed bridge network,
and a `spark-submit` line carrying two workarounds — a generated `/etc/passwd` and
`-Duser.home=/work/out` — for failures that cost a day to diagnose the first time. Read
[`README-ENVIRONMENT.md`](../bench/java-comparison/README-ENVIRONMENT.md) before changing anything
there; it has eight caveats and at least two of them will bite again.

`cargo xtask differential` adds only the parts that are about *correctness*:

| Step | What it does |
|---|---|
| Runtime check | `ContainerRuntime::detect`, then `docker info`. Neither → say why, exit 0 (`TST-102`) |
| Corpus | `cdm_testkit::differential::Corpus`, from a `Seed`; the same seed gives the same statements |
| Phase 1 | fresh clusters → seed the origin → `submit-migrate.sh` per table → verify → snapshot the target |
| Phase 2 | fresh clusters → seed the origin identically → `cdm migrate` per table → verify → snapshot the target |
| Compare | `compare_target_state` over two `TargetSnapshot`s and `compare_counter_blocks` over two `FinalBlock`s, one `DifferentialReport` per table |
| Report | `reports/differential/`, uploaded by CI whatever the outcome |

The corpus is two tables and therefore **two jobs per phase**: a `counter` may not share a table
with anything else (`MIG-030`), so `cdm_diff.counters` migrates separately and prints its own
`MET-006` block. Each table is verified, snapshotted and reported on independently — "the counter
block for `counters` differs and `all_types` is byte-identical" is a useful sentence, and a single
rolled-up verdict cannot say it.

Both implementations are built before either runs. A compile error discovered forty minutes into a
run has wasted forty minutes of cluster time.

## 3. Fresh clusters per implementation

`versions.env` declares one origin/target pair, so the two phases take turns: Java's nodes are
destroyed after its target has been captured, and cdm-rs gets a pair created from nothing.

Reusing one pair would be cheaper and wrong. Tier 3 refuses it because the second runner would read
a warm page cache and write into the first run's compaction backlog, which biases a *number*. Here
it is worse than bias: a target that was not empty when the second job started is a target whose
final state is a function of two runs, and the diff would be over something neither implementation
produced.

The origin is reseeded rather than shared, which is only sound because the corpus is a pure function
of its seed. That is `TST-101`'s property, and it is the property that makes the whole arrangement
work: two origins, generated independently, holding byte-identical rows.

## 4. Both halves are proved complete before anything is compared

A partial migration diffed against a complete one produces a large, confident, meaningless report —
every missing row is a difference, and the real difference, if there is one, is buried. So each half
must clear three checks, and the first one exists because of a specific finding:

**Java CDM exits 0 after losing data.** `README-ENVIRONMENT.md` caveat 4 records a run reporting
`Final Error Record Count: 34454`, with 1,465 rows missing on the target, that returned success —
nothing in Java CDM's `src/main/scala` calls `System.exit`, so `spark-submit` succeeds unless the
job throws. `docs/MIGRATION_FROM_JAVA.md` item 42 records the same shape for `DiffData`. The exit
status is therefore not evidence.

1. **the error counter is zero**, read out of the `MET-006` block rather than inferred from `$?`;
2. **the write counter equals that table's corpus row count** — what the job believes it did;
3. **an independent `SELECT COUNT(*)` on the target equals it too** — the one check that takes
   nobody's word for anything.

All three are applied per table, per phase: four jobs, twelve checks, and nothing is read back
until every one of them has passed.

## 5. The two things compared

**Target state, as bytes.** Each target is read back over the **native protocol**, not through
`cqlsh`, and this is the single most load-bearing decision in the suite.

`TST-020` asks for byte-identical target state. `cqlsh` cannot answer that question, because its
output is decoded and then formatted: a `varint` of `0x01` and a `varint` of `0x0001` both print as
`1`, a `decimal` prints at whatever scale the formatter chose, a `map` prints in the driver's
iteration order, and `NULL` and an empty value are hard to tell apart on screen (`MIG-012` turns on
that distinction). Every one of those is two different things on disk and one string in a terminal,
and the corpus generates them **on purpose** — `compare.rs` carries a test asserting that the
comparator catches the `varint` case specifically. A comparison over rendered text would call them
identical and report a green run over data it never saw, which is worse than having no suite at
all.

So `xtask` opens a `ClusterSession` to the target and calls
`cdm_testkit::differential::compare::snapshot_target` for each corpus table. Rows come back as
`cdm_cql::raw::RawRow` — the same undeserialised view the zero-copy passthrough of `MIG-040` uses —
and reach `compare_target_state` as `cdm_core::RawCell`, compared with `==`. Nothing between the
server's bytes and the verdict decodes anything or consults a codec.

Rows are keyed by primary key rather than compared in order, so token order stops mattering; the
`WRITETIME` and `TTL` of each cell are compared too (`MIG-020`, `FEA-040`). Which columns those are
selected for comes from the corpus's own `timestamp_eligible` metadata and from nowhere else — it
measured, against `cassandra:5.0.9`, that `WRITETIME` is rejected for a primary-key column and
*accepted* for a counter and for a non-frozen collection, the latter answering with a `list<bigint>`
rather than a `bigint`. A second rule in the runner would be a second source of truth, and the one
that had not been measured.

A `SELECT COUNT(*)` still goes through `cqlsh` for the completion check in §4, and that is
deliberate rather than an oversight: a count is a number the server computed, not a value whose
encoding is under test, and running it through a different client than the snapshot keeps it
genuinely independent.

**The counter block.** `COMPAT-004` requires cdm-rs's final block to be character-identical to
Java's, because users' scripts parse it (`MET-005`, `MET-006`). Java's arrives wrapped in log4j
furniture — `26/08/14 13:15:00 INFO JobCounter: Final Read Record Count: 200000` — and the runner
strips exactly that prefix and nothing else. Normalising further would be the harness hiding the
difference it exists to expose.

## 6. Configuration parity

A differential run whose two sides were configured differently compares configurations, not
implementations. Every setting that changes what a job *does* is therefore set explicitly on both
sides, from constants in `xtask/src/main.rs`, which are passed to `submit-migrate.sh` as the
environment variables it documents and written into cdm-rs's generated `.properties`:

| Setting | Value | Why |
|---|---|---|
| `numParts` | 64 | Java's default of 5000 is 5000 Spark tasks over a handful of rows; 64 still exercises range boundaries on both sides |
| `ratelimit.{origin,target}` | 100000 | Identical on both sides and far above anything this corpus can reach, so it is inert — but present, because it is also Java CDM's only write-path backpressure |
| `batchSize` / `fetchSizeInRows` | 5 / 1000 | `submit-migrate.sh`'s defaults, restated so both sides read one source |
| `autocorrect.{missing,mismatch}` | false | Autocorrect belongs to `validate`; a migrate run that repaired its own output would hide the difference |
| `trackRun` | false | Tracking writes rows to the target, and the target is the thing being compared |
| `consistency.{read,write}` | `LOCAL_QUORUM` | Java CDM's default, and single-node with `NetworkTopologyStrategy` makes it equivalent to `ONE` |
| `errorLimit` | 0 | A row that fails aborts the run rather than quietly shortening the target |

Two further points where "identically configured" needed work rather than assertion:

**The origin is seeded with a pinned `USING TIMESTAMP`.** Both implementations propagate the
origin's writetime onto the target by default (`schema.origin.writetime.automatic`, `true` on both
sides), so each target row is written `USING TIMESTAMP <the largest writetime on the origin row>`.
The two phases seed two origins minutes apart, so left to the coordinator's clock the two targets
would carry different writetimes for identical data and the comparison would report a `WRITETIME`
difference on every eligible column of every row — a failure manufactured entirely by the harness,
over the quantity `MIG-020`/`FEA-040` most need compared. Pinning the seed writetime takes the
harness out of the question: what is compared is whether the two implementations carry an identical
origin writetime identically. It buys no slack — `TtlPolicy::Exact` still applies — and a side that
dropped the writetime, rounded it or substituted its own clock still fails. The counter table is
excluded because CQL forbids `USING TIMESTAMP` on a counter update.

**The counter table's cdm-rs job sets `schema.origin.{ttl,writetime}.automatic false`.** The only
line in the generated `.properties` that has no counterpart in Java's, and it exists to make the two
sides *do* the same thing: neither can write a counter with a timestamp, but Java CDM disables its
`WritetimeTTL` feature silently when the origin is a counter table while cdm-rs reports `FEA-045`
and refuses the configuration. That divergence is recorded in `MIGRATION_FROM_JAVA.md`; leaving it
to fire here would test the divergence instead of the migration.

## 7. Reproducing a failure

Every run writes `reports/differential/seed.txt` before it can fail, containing the command that
replays it. The nightly uploads the whole directory whatever the outcome:

```text
reports/differential/
  seed.txt                  corpus, seed, and the exact replay command
  report.txt                the diff — this run's product, one section per table
  java-cdm/
    all_types/
      submit.log            everything spark-submit and CDM printed
      counters.txt          the normalised MET-006 block
      target-state.txt      the snapshotted target, hex, one column per line
      cdm-*.properties      the exact properties CDM was given
    counters/               the same four files for the counter table's job
  cdm-rs/
    all_types/              migrate.log, counters.txt, target-state.txt,
                            cdm.properties, summary.json
    counters/
```

`target-state.txt` is rendered from the same `TargetSnapshot` the comparison consumed rather than
read back a second time: an artefact produced by a different query than the verdict beside it is an
artefact that can disagree with it. It is hex for `SEC-002`'s reason, and because the claim being
made is about bytes — a decoded rendering would be the thing this suite refuses to compare, kept as
though it were the evidence.

A difference that turns out to be deliberate is not fixed here. It belongs in
[`MIGRATION_FROM_JAVA.md`](MIGRATION_FROM_JAVA.md), behind `--compat-java` (`COMPAT-001`), with a
test for both paths — that is the rule in `AGENTS.md`, and this suite is what enforces it.

## 8. What this does not establish

* **It has never run on a real GitHub Actions runner.** Neither had the environment it is built on:
  `README-ENVIRONMENT.md` caveat 7 says its disk budget is arithmetic from measured sizes rather
  than an observed near-miss. The first scheduled run is the real test of that figure.
* **`DiffData` is untested.** This suite compares two `Migrate` runs. Java CDM's validate job is the
  same jar and the same properties with a different `--class`, and comparing two *validate* runs is
  a second comparison worth making (`VAL-*`); it is not made here.
* **One corpus is not every corpus.** The corpus is seeded, so a nightly explores a new region of
  the space each night — which is the point — but a green run last night says nothing about a value
  it did not generate. `--corpus smoke` says even less: it proves the plumbing, never parity.
* **Counter parity is asserted on `Migrate` only.** `COMPAT-003`'s run-tracking schema compatibility
  is a separate claim, and `trackRun` is deliberately off here.
* **The suite requires the target to be routable from the host process.** Reading a target over the
  native protocol means connecting to the address `versions.env` fixes on the bench bridge network.
  On Linux — every CI runner this job uses — that address is routable and this is a non-issue. On
  macOS the Docker bridge is not reachable from the host, so the snapshot step cannot run locally
  there even though the Java migration itself can. Neither `cqlsh`-in-the-container nor the Java
  half notices, so the failure appears at the first snapshot with a message saying so.
* **It has never been run end to end.** The wiring is exercised by unit tests over the real corpus
  and by the comparator's own suite; the two-phase run against live clusters has not been executed
  on a Linux host. In particular Java CDM's behaviour migrating a counter table, and both
  implementations' handling of a row whose every value column is `NULL` (and which therefore has no
  writetime to propagate), are unverified here. Both fail loudly rather than silently if they are
  wrong — which is the property that matters, but is not the same as having been observed.
