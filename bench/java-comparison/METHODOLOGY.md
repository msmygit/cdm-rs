# Tier 3: the Java CDM comparison — methodology

How `NFR-004` is measured, what the number will mean, and the specific ways it could be wrong.

Requirements: `NFR-004` (throughput ≥ 2× Java CDM on the same hardware for the reference workload)
and `TST-060` (the suite). Tiers 1 and 2 are in [`docs/BENCHMARKS.md`](../../docs/BENCHMARKS.md);
this document covers tier 3 only, and does not restate them.

This file defines the **workloads** and the **configuration equivalence** half. The environment
(containers, versions, disk layout) and the runner (`run.sh`, the workflow) are separate and are
described where they live.

---

## 1. What is actually being claimed, and the one way to void it

`NFR-004` compares two programs that migrate the same rows between the same two clusters. The
number is a ratio, and a ratio is only meaningful if the numerator and the denominator did the
same work. Everything below exists to establish that they did.

The failure mode is not that someone cheats. It is that the two implementations have **different
defaults for the same property**, or **the same default for properties that mean different
things**, and the resulting figure gets published as though it were about the code. cdm-rs reads
Java's `spark.cdm.*` properties directly — `crates/cdm-cli/src/cli.rs` accepts `--properties-file`
and `--conf` verbatim, and `crates/cdm-config/src/model.rs` carries the legacy alias for every
property Java has — so one file really can drive both. §3 is the audit of how far that holds.

**Migrate is the headline job.** Validate is measured too (§8.4) but is reported separately: it is
a different code path, and cdm-rs has a keys-only mode (`VAL-015`) with no Java equivalent that
would make a validate comparison meaningless if anyone ever left it on.

---

## 2. The three workloads

Files are in [`workloads/`](workloads/). Each shape is one nosqlbench 5 workload file (schema plus
value distributions) and one `spark.cdm.*` properties file given to **both** implementations
unchanged. `workloads/cdm-rs-pins.properties` holds everything cdm-rs has and Java does not, and is
given to cdm-rs alone — see §5.

| Workload | Shape | What dominates | Default rows | In the default suite? |
|---|---|---|---|---|
| `narrow` | 1 key + 3 scalar columns, no clustering key | per-row fixed cost: paging, key extraction, bind, one round trip | 1,800,000 | yes |
| `collections` | Java CDM's own `PERF/perf-iot.yaml` | per-element collection framing: ~8 collection elements per row | 1,000,000 | yes |
| `wide` | 1 key + 24 scalar columns | per-column cost: 24 cells, and 48 more `TTL()`/`WRITETIME()` result columns | 600,000 | **opt-in** |

Row counts are **parameters with documented defaults**, not constants: `load-cycles` in each nb5
file, with `numParts` following the rule `rows / 2000` (§2.4). The defaults are a first guess to be
corrected by the first calibration run, not a measurement.

`collections` is a faithful mirror of upstream's `PERF/perf-iot.yaml`, which `docs/SPEC.md` S3
names by path as the reference workload. The schema, the bindings and the distributions are
upstream's; the three changes are listed in the file's header (keyspace templating, the `sources`
macro, and an unbalanced parenthesis in upstream's `alerts` recipe). `narrow` and `wide` are ours,
because upstream has no equivalent shape and `NFR-004` would be uninterpretable measured on one
point.

**`wide` is opt-in, and that is a budget decision rather than a judgement about the shape.** §2.4
sizes the suite; two shapes at three repetitions fit comfortably inside three hours and three
shapes do not. Three repetitions is the floor, not the flexible part: `ubuntu-latest` variance
makes a two-run median meaningless, so the number of *shapes* is what gives way. `narrow` and
`collections` are kept because one is the throughput ceiling and the other is upstream's own
reference workload; `wide` is enabled whenever a calibration run shows the room, and any result
published without it must say that `NFR-004` was substantiated on two shapes.

### 2.1 The workload that will mislead

`collections` will, in both directions.

Tier 1 measured cdm-rs's collection handling at roughly **2× the per-element cost of the same
conversion on a scalar** (`docs/BENCHMARKS.md` §2: ~223 ns/element at 256 elements against 111 ns
for a bare `int -> text`), because native-protocol framing is re-encoded per element. That reads
like cdm-rs's weak spot, and it is why the workload is in the suite: a comparison that omitted the
shape its own micro-benchmarks call worst would not be worth running.

But Java's cost profile on the same shape is *not* the mirror image. Java deserialises each
collection into a `java.util.Map` / `java.util.List` of boxed objects and re-serialises it, which
is allocation per element rather than framing per element. Which of the two is more expensive is an
empirical question this suite answers; predicting it from tier 1 alone is not possible.

What is certain is the **reporting hazard**: one `sensor_data` row carries a 3-entry map, a 1–5
entry list, a 2-field UDT and four decimals, and one `narrow` row carries three scalars. Rows per
second on those two workloads are not the same unit.

- Report **rows/s and cells/s** for every run, and bytes/s where the harness can get it.
- Never average a ratio across workloads, and never quote a single "×" for `NFR-004` without the
  workload beside it.
- If the three workloads disagree — and they may well straddle 2× — publish three numbers and
  amend `NFR-004` to say which workload it is about. `NFR-004` today says "the reference workload"
  and points at `perf-iot`; if the honest answer is "2.4× narrow, 1.6× wide, 1.1× collections",
  then that is the finding and the SPEC is what changes.

### 2.2 What is deliberately not measured

- **`extractJson`.** Tier 1 found it superlinear in document size (16 → 256 filler fields is ~16×
  the fields for ~19× the time, because the whole document is parsed to read one property). A
  workload built on it would be a `serde_json`-versus-Jackson benchmark wearing a migrator's
  clothes, and its result would depend almost entirely on a document-size parameter we chose.
- **Counters.** cdm-rs reads, computes, writes and awaits counter rows one at a time (`MIG-031`,
  `MIG-032`) where Java issues them asynchronously alongside everything else
  (`docs/MIGRATION_FROM_JAVA.md` item 36). That is a deliberate correctness difference with a large
  and entirely predictable throughput cost, and measuring it would be measuring the decision, not
  the implementation.
- **Type conversion.** Origin and target schemas are identical in all three workloads, so no codec
  runs on either side. That is the conservative choice: a schema-changing workload would exercise
  Java's string round-trip for UDTs (item 3) against cdm-rs's recursive name-matched conversion,
  and there is no reason to expect that comparison to favour Java.
- **Astra.** cdm-rs cannot set a per-connection TLS `server_name`, so against Astra it loses
  token-aware routing entirely (`CON-026`, item 20). Any Astra figure would be materially worse for
  cdm-rs. `NFR-004` does not say which cluster; this suite measures self-managed Cassandra, and the
  Astra caveat must be restated wherever the result is.

### 2.3 Sizing: disk

The runner is a free GitHub Actions `ubuntu-latest` host: 4 vCPU, 16 GB RAM, **~14 GB free disk**,
holding both Cassandra data directories, Spark, the images and the artefacts.

On-disk estimates are per side, post-LZ4, and include SSTable and per-cell-timestamp overhead.
They are estimates: the harness must record the actual `nodetool tablestats` figures and this
table must be corrected from them after the first run.

| Workload | Default rows | ~bytes/row on disk | Origin | Target | Both |
|---|---|---|---|---|---|
| `narrow` | 1,800,000 | ~100 | 180 MB | 180 MB | 360 MB |
| `collections` | ~999,996 | ~250 | 250 MB | 250 MB | 500 MB |
| `wide` (opt-in) | 600,000 | ~280 | 170 MB | 170 MB | 340 MB |
| | | | | | **1.2 GB** |

Everything else on the disk:

| Item | Size | Note |
|---|---|---|
| live data, both sides | 1.2 GB | above |
| origin snapshot tarball | ~0.6 GB | restored before every run, §8.1 |
| compaction headroom | ~0.3 GB | worst case, a full compaction of the largest table |
| commitlogs, 2 nodes | 1.0 GB | **only if capped.** Cassandra's default `commitlog_total_space` is min(8 GB, ¼ of the volume), which on a 14 GB disk is ~3.5 GB *per node*. Uncapped, this alone is 7 GB. The environment must set 512 MB per node. |
| images and binaries | ~1.5 GB | Cassandra image (layers shared by both containers), a JDK+Spark+CDM-jar image, nb5, the cdm-rs binary |
| logs, digests, artefacts | ~0.2 GB | |
| | **~4.8 GB** | of ~14 GB |

Roughly 9 GB of headroom. That is deliberate: the two things that consume disk without warning are
uncapped commitlogs and a compaction that transiently doubles a table, and both happen at the worst
possible moment.

### 2.4 Sizing: runtime, and how the suite survives being wrong about it

Two things are being balanced. Each Java run must last long enough that Spark's startup stays a
small fraction of it, and the whole suite must finish inside **~3 h** with margin, on a 6 h job
limit. The estimate that connects them is the throughput of a machine we have never measured, and
the design below assumes that estimate is wrong.

**The estimate.** Two independent data points, and they agree:

- Java CDM's own published run. `PERF/testing.txt` upstream records **999,996 `perf-iot` rows in
  ~200 s** by CDM v4 on an AWS `t2.2xlarge` — 8 vCPU, 16 GB, both Cassandra clusters on the same
  instance. That is ~5,000 rows/s in almost exactly our topology at twice our core count.
- cdm-rs's own tier-2 macro-benchmark measured **9,509 rows/s** for 100,000 rows of a 16-column
  table with both Cassandra containers *and* the tool on one Apple M-series laptop — a
  substantially faster machine than a free four-vCPU runner.

Halving upstream's figure for cores gives a working assumption of **~2,500 rows/s for Java on
`collections`**, and tier 2 is consistent with it: if a fast laptop gets cdm-rs to 9,509 rows/s on a
16-column table, a shared four-vCPU runner will get Java to a small fraction of that. Both numbers
also sit well below the 20,000 rows/s default rate limit, which is why §4.1 is written the way it
is.

Treat ±50% as the honest band, and note the asymmetry: a free `ubuntu-latest` runner is *slower*
than the 8 vCPU instance upstream used, so the risk is one-sided — the real rate is more likely to
be 1,200 rows/s than 5,000.

| Workload | Assumed Java rows/s | Rows for ~350 s of Java | Default `load-cycles` |
|---|---|---|---|
| `narrow` | ~2,500 (few cells, few bytes) | ~875,000 | 1,800,000 |
| `collections` | ~1,500 (the anchor, halved for cores and for the runner) | ~525,000 | 1,000,000 |
| `wide` | ~800 (24 cells + 48 TTL/WRITETIME columns) | ~280,000 | 600,000 |

**The defaults are deliberately about twice what the estimate needs.** A measured run is trimmed
*downward* by `spark.cdm.filter.java.token.percent`, which cannot trim upward, so a dataset loaded
too small can only be fixed by reloading — 15 minutes of budget — whereas one loaded too large
costs nothing but disk, of which there is 9 GB spare. The row counts are therefore sized for the
optimistic case and the calibration trims.

**The calibration run.** Before config A, for each enabled workload: one Java migrate at
`spark.cdm.filter.java.token.percent = 5`, which takes ~20 s and yields the runner's actual rate.
From it the harness computes the coverage percentage that gives ~350 s of Java steady state, and
uses that same value for **every** run of that workload — both implementations, every repetition,
config A and config B alike. Cost: ~2 min per workload including the container reset. It removes
the entire risk of the rate estimate being wrong, in both directions.

Coverage sampling is a legitimate knob for this and not a shortcut: `TOK-005` reproduces Java's
`SplitPartitions` shrink exactly, so both implementations narrow each range identically, and tier 1
measured the sampling as free at plan time. It changes how much is migrated without changing what
either tool does per row. What is *not* permitted is a different coverage between implementations,
between repetitions, or between config A and config B.

**Why ~350 s and not less.** `spark-submit` on a cold four-vCPU runner takes 30–60 s to reach the
first row — JVM start, jar load, `SparkContext` init. Against 350 s of steady state that is 8–15%
of Java's cold wall clock, where cdm-rs's `NFR-002` budget is under 2 s, i.e. under 1%. That gap is
reported, never blended (§8.2). It is also long enough for the JVM to leave the interpreter and C1
behind and spend most of the run in C2; a 60-second run would measure warm-up and would be a way of
manufacturing a result. **350 s is a floor.** If the budget ever conflicts with it, drop a workload
(§2) — never shorten a run.

**The budget.** Per (workload, repetition) pair: Java ~350 s + cdm-rs ~175 s at the claimed 2×,
plus 2 × (90 s container reset and origin restore + 60 s verification) = **~825 s, ~14 min**.

| Phase | Cost |
|---|---|
| nb5 load of both default workloads, flush, compact, origin snapshot tarball | ~15 min |
| calibration, 2 workloads | ~4 min |
| config A: 2 workloads × 3 repetitions | ~83 min |
| config B (§4.2): `narrow`, batchSize 5, 1 repetition | ~14 min |
| `numParts` sensitivity (§4.3): `narrow`, Java only, 1 run | ~8 min |
| **total, default suite** | **~2 h 5 min** |
| with `wide` enabled: + load, + calibration, + 3 repetitions | + ~45 min → **~2 h 50 min** |

The default suite leaves ~55 minutes of margin inside a 3 h envelope and nearly 4 h against the
hard job limit. That margin is the point: it is what absorbs the rate estimate being 2× wrong in
the bad direction even before the calibration run corrects for it. Enabling `wide` spends almost
all of it, which is why it is opt-in rather than default.

**What happens when a workload overruns anyway.** A job killed at the limit with no artefact is the
worst outcome available — worse than a partial result, and far worse than a missing one that is
labelled.

- Each workload has its own timeout: `2 × expected Java cold time + 120 s`, derived from the
  calibration. On expiry the run is terminated and the workload is reported as
  **`OVERRAN — no ratio`**, carrying its elapsed time and whatever rows reached the target. The
  suite moves to the next workload; it does not abort.
- The suite has a global deadline of 3 h. Workloads not started by then are reported as
  **`NOT RUN`**. A published result must list them.
- **A partial result is not symmetric between the implementations, and must not be presented as
  though it were.** On `SIGTERM` cdm-rs performs the `ENG-010` graceful shutdown — claiming stops,
  in-flight ranges drain, the counters are flushed and printed, the run is marked `INTERRUPTED` and
  it exits 4 — so a timed-out cdm-rs run yields honest counters. Java has no such path: `Ctrl-C`
  kills `spark-submit` mid-write and the final counter block is never printed
  (`docs/MIGRATION_FROM_JAVA.md` item 26). A timed-out Java run therefore yields elapsed time and a
  row count read from the target table afterwards, and nothing from the tool itself. Both are
  recorded as `OVERRAN`; neither produces a ratio.

---

## 3. Configuration equivalence: the property audit

Every property in the shared workload files, and what each implementation does with it. Sources:
`crates/cdm-config/src/model.rs` (cdm-rs defaults and legacy aliases), `docs/SPEC.md` §3.5,
`docs/generated/PROPERTIES.md`, and upstream's `KnownProperties.java` and
`cdm-detailed.properties`.

Java CDM version: the audit is against **6.x**, the baseline `docs/MIGRATION_FROM_JAVA.md` declares.
The exact release and the jar's checksum must be recorded with the result; upstream changes
defaults between minor versions and this table is only as current as the run.

### 3.1 Where the defaults already agree

These are identical in both implementations, which is a deliberate property of cdm-rs
(`cfg_001_the_default_configuration_is_the_java_default_configuration` asserts several of them):

| Property | Java default | cdm-rs default |
|---|---|---|
| `perfops.numParts` | 5000 | 5000 |
| `perfops.batchSize` | 5 | 5 |
| `perfops.fetchSizeInRows` | 1000 | 1000 |
| `perfops.ratelimit.origin` | 20000 | 20000 |
| `perfops.ratelimit.target` | 20000 | 20000 |
| `perfops.consistency.read` | `LOCAL_QUORUM` | `LOCAL_QUORUM` |
| `perfops.consistency.write` | `LOCAL_QUORUM` | `LOCAL_QUORUM` |
| `schema.origin.column.ttl.automatic` | true | true |
| `schema.origin.column.writetime.automatic` | true | true |
| `schema.ttlwritetime.calc.useCollections` | false | false |
| `autocorrect.*` | false | false |
| `trackRun` | false | false |
| `transform.*` | 0 / false | 0 / false |
| `filter.java.token.percent` | 100 | 100 |
| `feature.guardrail.colSizeInKB` | 0 | 0 |

**Every one of them is still pinned explicitly in the workload files.** Agreeing today is not the
same as agreeing at the next run, and a default that drifts in either project would change what was
measured with nothing in the artefacts to show it.

### 3.2 Where the same property is honoured differently

These are the ones that matter. Cross-checked against `docs/MIGRATION_FROM_JAVA.md`, whose item
numbers are cited.

| Property | Difference | Item | Effect on this comparison |
|---|---|---|---|
| `perfops.batchSize` | Same value, different grouping: cdm-rs never batches two partitions together (`MIG-022`), Java batches in read order and freely spans partitions | 6 | **Real and material.** On `narrow`, where one row is one partition, Java at batchSize 5 issues one 5-row unlogged batch per 5 rows and cdm-rs issues 5 single-row writes. On a single node an unlogged multi-partition batch is 5 mutations in one round trip and is likely *faster* — so the default would disadvantage cdm-rs, not flatter it. Neutralised by pinning batchSize 1 (§4.2). |
| `perfops.fetchSizeInRows` | Same page size, but it feeds a flush threshold that only cdm-rs reaches. `min(fetch_size, max(batch_size × 10, 100))` = **100** at these settings; cdm-rs flushes there, Java's equivalent branch is unreachable and it flushes once per range | 15 | **Real, and it favours Java.** cdm-rs's write pipeline is ~100 deep per worker; Java's is bounded only by range size, ~2,000 here. Not fixable from configuration — there is no Java property to shallow it and no cdm-rs property to deepen it past the threshold. See §5.2. |
| `perfops.errorLimit` | Implemented by cdm-rs (`ENG-009`), absent from Java's `KnownProperties` | 7 | None at the pinned value of 0 (unlimited), which is Java's only behaviour. |
| `filter.cassandra.whereCondition` (unset) | Java always appends `ALLOW FILTERING`; cdm-rs omits it when there is no CQL predicate | 2 | Negligible: on a pure token-range scan the server does no filtering either way. The two `SELECT` texts differ, and §7.2 requires both to be archived, so this will be visible in the artefacts and should not be mistaken for a discrepancy. |
| `perfops.consistency.*` | Identical values; Java silently coerces an unrecognised level to `LOCAL_QUORUM`, cdm-rs errors (`CFG-161`) | 1 | None — both values are valid. Worth knowing that a typo would have produced a silent asymmetry in Java and a startup failure in cdm-rs. |
| `feature.guardrail.colSizeInKB` | cdm-rs can run the guardrail *inside* a migrate (`GRD-004`); Java has it only as a separate job | 35 | None at 0. A non-zero value would be per-row work on one side only. |
| unknown `spark.cdm.*` keys | cdm-rs warns and names the closest known key; Java ignores | — | The shared file must be usable by both, so `--strict-config` must not be used. |
| unknown `spark.*` keys | cdm-rs ignores silently ("that configured Spark") | — | Means Spark tuning could live in the shared file. It deliberately does not: see §5.1. |

Two further differences are absent from this run but belong on the record, because they would
change a comparison run against different data: item 21 (a malformed JSON document costs Java a
whole token range and cdm-rs one row) and item 43 (Java cannot preserve TTL/writetime on a column
whose name is reserved or case-sensitive — it fails at prepare time). Neither workload triggers
either. Both mean Java's throughput on *dirty* data is not what this suite measures.

### 3.3 What is being propagated, and why that was left on

`schema.origin.column.{ttl,writetime}.automatic` defaults to `true` in both implementations, and
this suite leaves it there. The consequence is worth stating plainly, because it is most of the
`wide` workload:

- the origin `SELECT` gains a `TTL(col)` and a `WRITETIME(col)` expression for every eligible
  non-key column — on `wide`, 24 data columns become **73 result columns**;
- the target `INSERT` gains `USING TTL ? AND TIMESTAMP ?`, with the per-row maximum across those
  columns;
- collection columns are excluded from eligibility unless `useCollections` is set, so on
  `collections` the TTL and writetime come from the scalars only.

`docs/MIGRATION_FROM_JAVA.md` lists TTL and writetime semantics under "unchanged, and guaranteed to
stay so", so the two implementations are computing the same thing. Turning it off would have made
`wide` measure very little and would have diverged from what Java CDM does out of the box.

---

## 4. Where the pinned values depart from the shared default, and why

### 4.1 Rate limits: 1,000,000, not 20,000

Both implementations default to 20,000 rows/s per side and both honour it.

**The evidence says that ceiling does not bind on this hardware.** cdm-rs's tier-2 macro-benchmark
measured 9,509 rows/s for 100,000 rows of a 16-column table with both Cassandra containers and the
tool on one Apple M-series laptop — less than half the default ceiling, on a machine faster than a
free four-vCPU runner. §2.4's Java estimate of ~1,500–2,500 rows/s is an order of magnitude below
it. So this pin is **not** a claim that both tools are currently sleeping against a limiter; it
removes a ceiling that is probably not there.

It is pinned anyway, for two reasons that hold regardless of which way the estimate falls:

- it costs nothing when the limiter never binds, and voids the entire result when it does;
- a binding ceiling is **invisible in the output**. Both tools would report the same rate and the
  ratio would read 1.0, which is indistinguishable from a genuine tie. There is no way to discover
  after the fact that this happened, so it has to be excluded in advance.

What turns that from an assumption into a measurement is the void condition: **the run is void if
either implementation's observed rate comes within 20% of the ceiling.** The harness must check it
and fail rather than report. If a future runner is fast enough that the check ever fires, the
ceiling is raised and the run repeated — it is not a result.

One asymmetry survives whatever the value: Java's limiter is a Guava `RateLimiter` held per JVM,
cdm-rs's is per process. Under `--master local[N]` there is one JVM, so they are the same scope. On a real Spark
cluster they would not be, and a Java figure from a multi-executor deployment cannot be compared
with this one.

### 4.2 Batch size: 1, with a second configuration at 5

**Configuration A (headline): `batchSize = 1`.** Both implementations issue exactly one `INSERT`
per row, `MIG-022` has nothing to express, and the write traffic reaching Cassandra is structurally
identical. This isolates per-row cost, which is what `NFR-004` is about.

**Configuration B (secondary): `batchSize = 5`, cdm-rs in `perfops.batch_grouping = legacy`.** The
shared default, with cdm-rs's grouping switched to Java's index-order algorithm so the batches
themselves match. Run on `narrow` only — the shape where batching matters most, because every row
is its own partition — with 2 repetitions.

Reporting only A would hide that the shipped defaults produce different write traffic. Reporting
only B would confound the ratio with a batching-strategy difference. Both are published.

Note what B costs cdm-rs: `legacy` grouping is the behaviour cdm-rs deliberately does not default
to, because multi-partition batches are a Cassandra anti-pattern at scale. On a *single node* that
anti-pattern is not one, so B is, if anything, the configuration that flatters Java.

### 4.3 `numParts`: ~2,000 rows per range, not the default 5,000 parts

`numParts` is pinned per workload so that every range holds ~2,000 rows: 1,500 for `narrow`, 250
for `wide`, 500 for `collections`.

Leaving it at 5,000 would not have been neutral. It is the same number on both sides, but the two
implementations spend it differently:

- **Java** turns each part into a Spark task. 5,000 tasks on `local[4]` is 5,000 task launches,
  serialisations and accumulator merges. Task overhead of 10–20 ms is 50–100 s of work spread over
  four cores — 12–25 s of wall clock, on a 400-second run.
- **cdm-rs** turns each part into an async range worker item. Tier 1 measured the whole `TOK-003`
  split plus the `TOK-006` shuffle at ~3.3 ms for 65,536 ranges, and per-range scheduling is
  microseconds.

That difference is real and it is Spark's execution model, not a misconfiguration — but a
knowledgeable Java operator would tune it, and beating an untuned setting proves nothing about the
implementation. So:

- both run at the pinned per-workload value;
- **and** a sensitivity run is performed: `narrow`, Java only, `numParts = 5000` (the shared
  default) against `numParts = 1500`, one repetition each. The difference between them is the size
  of Spark's per-task overhead in this topology, and it is published next to the headline ratio. If
  it turns out to be a large fraction of the gap, that is the finding.

`numParts` also has a cliff worth knowing about before anyone tunes it upward: tier 1 found that
`partition_size = span / num_parts` truncating to zero falls back to a fixed 100,000, so on a
narrow token range a large `numParts` silently produces one range. Both implementations do this,
because it is Java's algorithm; it does not arise at the values used here, where the range is the
whole ring.

---

## 5. Settings where "equivalent configuration" is genuinely ambiguous

Four. Each is named here rather than resolved quietly, and each proposed mapping is a judgement
that a reader is entitled to disagree with.

### 5.1 Spark executor parallelism vs `perfops.workers`

**They are not the same knob.** A Spark task is a synchronous loop over one token range running on
one executor thread, with asynchronous writes accumulating inside it. A cdm-rs worker is an async
task multiplexed with others onto a Tokio runtime whose *thread* count is separate from its *task*
count.

**Proposed mapping: match OS threads, not logical units.** `--master local[4]` against
`perfops.workers = 4`, on a 4-vCPU box.

The reasoning is that the only quantity both implementations agree on is "how many hardware threads
this program may saturate". Matching "4 concurrent ranges" is a coincidence of this configuration,
not the principle; if cdm-rs's worker count were raised to 16 on the same 4 threads it would still
be using four cores, and the comparison would still be honest — but it would no longer be
*obviously* honest, which matters more here.

Spark's deployment settings (`spark.master`, `spark.driver.memory`, `spark.executor.cores`) are
deliberately **not** in the shared properties file, even though `spark-submit --properties-file`
would read them and cdm-rs would ignore them silently. A setting that one side acts on and the
other silently drops is exactly the kind of thing this document exists to prevent, and putting it
in the file that is supposed to prove equivalence would undermine the file. They belong to the
runner, and the runner must publish them.

### 5.2 In-flight request depth

Java has **no property for this at all**. Its depth is an accident of item 15: writes accumulate
for a whole token range because the flush branch is unreachable, so at ~2,000 rows per range its
in-flight write depth is up to ~2,000 per task. cdm-rs's is capped twice — by
`perfops.max_inflight_writes` (2,000 across all workers) and, far more tightly, by the `MIG-004`
flush threshold, which at `batchSize 1` and `fetchSize 1000` is **100 per worker**, roughly 400 in
all. The configured cap of 2,000 therefore never binds.

**No mapping exists.** Raising cdm-rs's cap would not deepen its pipeline, because the flush
threshold is what bounds it; and the flush threshold is a correctness-adjacent setting
(`NFR-003` bounded memory) that cdm-rs is not going to give up to win a benchmark.

**Proposed handling: leave both at their own behaviour, declare it unmatched, and report the
memory alongside the throughput.** cdm-rs's shallower pipeline is the price of `NFR-003`, and
`docs/SPEC.md` S3 already pairs the throughput claim with "at ≤ 25% of the resident memory". The
two halves of S3 are not independent: some of the memory cdm-rs saves is bought with write
concurrency it does not use. Reporting throughput without RSS would hide the trade.

### 5.3 Retry policy

cdm-rs retries a failed request 5 times with exponential backoff from 100 ms; Java uses the Java
driver's retry policy, which is a different policy with different triggers. Neither is expressible
in the other's terms.

**Proposed handling: leave both at their defaults, and void any run in which either retried.** A
run that hit retries was overload-bound rather than throughput-bound, and its number is about the
cluster, not the migrator. The harness must scrape both logs for retry, timeout and `OVERLOADED`
evidence and fail the measurement if it finds any.

### 5.4 Connection pooling

The Java driver defaults to one connection per local node with up to 1,024 concurrent requests on
it; cdm-rs defaults to `perfops.connection_pool_size = 4`. The two drivers do not multiplex the
same way, so "4 connections" and "1 connection" are not comparable numbers and forcing cdm-rs to 1
would not make them so.

**Proposed handling: leave each driver at its own default and record the observed concurrent
request ceiling of each.** This is the weakest of the four mappings, and it is stated as such: it
is defensible only because §5.3's no-retry condition means neither side was near saturation.

---

## 6. Asymmetric work

Anything one implementation does that the other does not must be switched off or matched. What
follows is the complete list found by walking cdm-rs's configuration model against Java's
`KnownProperties`.

**Switched off in `workloads/cdm-rs-pins.properties`** — cdm-rs features with no Java counterpart,
all of which cost something:

| Feature | Requirement | Why it would distort |
|---|---|---|
| keys-only validation | `VAL-015` | skips nearly all of validate's per-row work; Java has no equivalent |
| the discrepancy report | `VAL-013` | a per-mismatch export Java never writes |
| adaptive rate limiting | `ENG-006` | changes the write rate mid-run in response to the target |
| speculative execution | `CON-010` | a second copy of a request; Java issues none |
| distributed coordination | `DST-001` | lease renewals and membership writes |
| the control plane, Prometheus, the event bus | `SEC-010`, `MET-020`, `MET-030` | tier 1 measured instrumentation at ~21 ns per acquisition and the adaptive signal at 54 ns — negligible, and off anyway, because "negligible" is a claim and "off" is a fact |
| the inline guardrail | `GRD-004` | per-row work Java can only do as a separate job |

**Matched, not switched off:** TTL and writetime propagation (§3.3), run tracking (off on both),
autocorrect (off on both), transformations (none on either).

**Unmatchable, and therefore declared:**

- **Write pipeline depth** (§5.2) — favours Java.
- **Counter machinery.** Java accumulates counts through Spark accumulators, merged per task at the
  driver; cdm-rs uses atomics. Neither is switchable off, both are how the tool produces the counts
  the run is checked against, and both belong in the measurement.
- **Retry policy** (§5.3) — voided rather than matched.
- **Startup.** Reported separately (§8.2) rather than hidden.

---

## 7. What would make this comparison unfair to Java

Nobody in this repository is motivated to write this section, which is exactly why it is here. Each
item is a specific way the published number could overstate cdm-rs.

### 7.1 Spark on one 4-vCPU box is Spark at its worst

Java CDM is not a migration program; it is a Spark application. The driver JVM, the local
scheduler, task serialisation and accumulator merging are fixed costs that exist so that the same
code runs on a hundred executors. cdm-rs has no distributed layer running at all in this
configuration (`cluster.enabled = false`).

Part of any measured ratio is therefore "cdm-rs did not pay for a scheduler it did not need". That
is a real advantage for a single-node migration — it is much of why cdm-rs exists — but it is not
evidence about per-row efficiency, and stating a ratio without this sentence next to it would be
misleading. §4.3's `numParts` sensitivity run is the one place this cost is measured directly
rather than argued about.

### 7.2 We are configuring Java, and we are not its maintainers

Every value pinned in §4 is an opportunity to pick badly on Java's behalf, and we have far more
insight into cdm-rs's cost profile — tier 1 gave us a per-nanosecond map of it — than into Java's.
Mitigations, all of them mandatory:

- pin to Java's own documented defaults wherever the default is not pathological for *both* sides
  (§3.1);
- publish the exact properties files, the exact `spark-submit` command line, the Java CDM release
  and the jar checksum;
- **archive the generated CQL from both implementations and diff it.** cdm-rs prints its plan and
  statements (`cdm plan`, `cdm config explain`); Java logs its origin `SELECT` and target `INSERT`.
  Two statements that differ in anything but `ALLOW FILTERING` (§3.2) mean the two runs were not
  doing the same work, and no amount of properties-file symmetry substitutes for looking;
- run the `numParts` sensitivity check, which is a deliberate attempt to find a Java configuration
  that is *better* than the one we chose.

### 7.3 JIT warm-up and GC

A short run measures the interpreter. §2.4 sets a floor of 350 s of steady state for this reason,
and §8.2 measures steady state over the run's last 80% so that the warm-up window is excluded from
the steady-state figure and visible in the cold one.

Heap is the other half. On a 16 GB box already holding two Cassandra JVMs, `--driver-memory` left
at its 1 GB default would make Java GC constantly — and item 15 means Java's allocation scales with
range size, so it genuinely needs headroom. `--driver-memory 4g` is the floor, GC time must be
recorded from the JVM's own logs, and a run whose GC time exceeds 10% of wall clock is void.
`--executor-memory` is silently ignored in `local[*]` mode; setting it and believing it did
something is a classic way to starve a Spark job by accident.

### 7.4 The measurement is CPU-bound, and real migrations often are not

The largest workload is 300 MB per side on a 16 GB machine. After the first few seconds, both tools
read entirely from page cache. Storage is therefore not in the comparison at all, which shifts the
result toward whichever implementation spends less CPU per row — very likely cdm-rs.

Real migrations at petabyte scale are frequently bound by disk, by network, or by the target
cluster's ability to absorb writes, and in every one of those regimes the gap compresses toward
1.0. **The published result is a CPU-bound result, and must say so.**

Equally, log I/O must not silently become the difference: both implementations run at INFO to a
file, and §8.5 requires both logs' line counts to be recorded.

### 7.5 Four shared vCPU means neither tool shows its concurrency design

With 4 vCPU shared between two Cassandra nodes and the migrator, neither implementation reaches its
design point: Java cannot spread across executors, cdm-rs cannot use async concurrency it has no
cores for. The measurement drifts toward **"which wastes less CPU"**, which is a legitimate
question and a different one from "which migrates faster".

`ubuntu-latest` is also a shared, virtualised, co-tenanted runner. `docs/BENCHMARKS.md`, in its
discussion of why the tier-1 gate ships at 200% rather than 10%, records the same binaries
measuring **5–7× apart** between a loaded and an idle machine. That is why every workload is run
three times with min/median/max reported — and why three repetitions is the floor that the number
of *shapes* gives way to (§2), not the other way round — why this tier is not a CI gate, and why a
ratio between 1.8× and 2.2× should be read as "approximately 2×, on this hardware, once".

### 7.6 Single-node clusters make both tools degenerate

RF=1, one node per side. `LOCAL_QUORUM` is one replica; there is no replication traffic, no
coordinator forwarding, no cross-node hop.

- **cdm-rs's ring-aware planning collapses**: with one node owning the whole ring, `ring_aware` and
  `fixed` produce the same plan, which is part of why `plan.strategy` is pinned to `fixed`.
- **Spark's token partitioning collapses**: there is no locality to exploit and no data to move,
  so Spark's central reason for existing in this application does not apply.
- **Token-aware routing collapses on both sides**: every request goes to the only node.

The result therefore says nothing about either tool at cluster scale, in either direction. It is
the topology `docs/SPEC.md` S3 specifies ("the same single node"), and it is a limitation of what
S3 can prove, not a flaw in executing it.

### 7.7 Version and build symmetry

Pin the latest Java CDM 6.x GA release and record its checksum; measuring against an old release
would be unfair. Measure cdm-rs from its **released** artefact profile, not a locally tuned build
with non-default codegen flags — a user gets a release binary of cdm-rs just as they get a released
jar of Java CDM.

### 7.8 The workloads were designed by reading cdm-rs's profile

Tier 1 gave a detailed map of cdm-rs's cost curve and nothing at all about Java's. Three shapes
chosen against that map could easily have been three shapes cdm-rs happens to be good at. The
counterweight is `collections`: it is upstream's own reference workload, and tier 1 says it is
cdm-rs's worst shape. If Java wins there, that gets published as prominently as anything else.

---

## 8. Methodology decisions, and the reasoning behind them

### 8.1 Fresh containers per implementation

Both Cassandra containers are destroyed and rebuilt between every run, with the origin data
directory restored from a tarball produced once by nb5.

Without this, whichever implementation runs second reads an origin whose SSTables are already in
page cache and writes into a target with a different compaction state and different SSTable
counts — enough to manufacture a large difference on its own, in whichever direction the ordering
happens to favour.

Note what this does and does not achieve. On a 16 GB box with a 300 MB table, a genuinely *cold*
cache lasts seconds; the property being established is that both runs start **equally** warm, not
that either starts cold. Run order is also alternated across repetitions so that any residual
ordering effect shows up as variance rather than as a result.

### 8.2 Cold and steady-state reported separately

Two figures per run, never one:

- **Cold**: process start to last row written. Includes Spark's JVM and `SparkContext` startup for
  Java, and cdm-rs's sub-2-second start (`NFR-002`). This is what a user waits for, and startup is a
  genuine part of it.
- **Steady state**: measured over the last 80% of rows, from each implementation's own progress
  output, so that JVM warm-up and startup are excluded on both sides.

Both are honest. Conflating them is not: at these run lengths, startup alone is 8–15% of Java's
cold wall clock and under 1% of cdm-rs's, so a single blended number would carry a ~1.1× advantage
that has nothing to do with per-row throughput.

### 8.3 Target state is verified, every run

A throughput figure from a lossy or wrong migration measures nothing at all — and the ways a
migration goes wrong (dropped rows in a failed range, TTL not propagated, a collection written
empty) all make it *faster*.

After every run, before the containers are destroyed:

1. `nodetool flush` on the target, then an exact `SELECT COUNT(*)`.
   - At `filter.java.token.percent = 100` it must equal the origin's count.
   - When §2.4's calibration has trimmed the coverage below 100, the target legitimately holds
     fewer rows than the origin, and the origin-equality check does not apply. What still applies,
     and is the stronger check anyway, is that **the two implementations' targets must match each
     other exactly** — same count, same digest. Both migrated the same slice of the same ring with
     the same coverage value.
2. A content digest: for each of 256 evenly spaced token sub-ranges, select every column plus
   `WRITETIME()` and `TTL()` of a nominated non-key column, sort, and SHA-256 the result. The digest
   must be **identical** for the Java run and the cdm-rs run of the same workload.
   - Including writetime and TTL is the point: it is what proves the propagation described in §3.3
     actually happened on both sides. A tool that quietly skipped it would be doing less work and
     would look faster.
   - The sample is a **fixed row budget, not a fixed fraction**: the first ~50,000 rows encountered
     across a fixed, identical list of sub-ranges, which costs ~30 s and does not grow when the
     row counts are raised. A percentage would have made verification scale with the dataset and
     silently eat the budget the moment anyone increased `load-cycles`.
   - Full coverage once, for `collections` only, during calibration, to establish that the sampled
     digest agrees with the complete one. Doing it for every workload every run costs more than the
     migrations do. The residual risk — a defect confined to the rows not sampled — is stated
     rather than dismissed.
3. Both implementations' final counter blocks are parsed and must agree with each other and with
   the row counts. cdm-rs's block is character-identical to Java's by requirement (`MET-005`,
   `MET-006`, `COMPAT-004`), so one parser reads both, and any divergence is itself a finding.

A run failing any of these is void, not slow.

### 8.4 Validate is a separate measurement

Measured, reported separately, and never blended into the `NFR-004` figure. `validate.keys_only` is
off and `validate.report.format` is `none` (§6). Autocorrect is off on both sides, so validate is a
read-only comparison on both.

Tier 1 is worth knowing before reading a validate number: cdm-rs's early exits are 5 ns and flat in
table width, and a mismatch costs ~200 ns over a match. Validate throughput therefore depends
strongly on how *clean* the target is, so both implementations must validate a target that is known
identical — i.e. one produced by a run that passed §8.3.

### 8.5 What every run must record

The result is not reproducible without all of it: both properties files verbatim, the pins file,
the full command lines, the Java CDM release and jar checksum, the cdm-rs version and build
profile, the Cassandra version, the runner's CPU model and `/proc/cpuinfo`, both generated CQL
statements (§7.2), rows/s and cells/s cold and steady, peak RSS of both, JVM GC time, both logs'
line counts, retry and timeout counts, the observed rate against the rate-limit ceiling (§4.1), and
the §8.3 digests.

---

## 9. What the result will and will not support

It **will** support a statement of the form: "on 4 shared vCPU, single node per side, RF=1, CPU-
bound, at `batchSize` 1 with TTL and writetime propagation on, cdm-rs migrated the `<workload>`
workload at N× Java CDM 6.x's steady-state rate, with the run's target content verified identical."

It **will not** support: a claim about cluster-scale throughput, a claim about Astra, a single "×"
detached from a workload, or a claim about the tools' behaviour on dirty data.

Every workload ends in exactly one of four states, and all four are publishable:

| State | Meaning |
|---|---|
| `MEASURED` | ran to completion, verification passed, ratio reported with cold and steady-state figures |
| `VOID` | ran, but a precondition failed — the rate limiter bound (§4.1), either side retried (§5.3), GC exceeded 10% of wall clock (§7.3), or verification disagreed (§8.3). Recorded with the reason. Not a ratio. |
| `OVERRAN` | hit its timeout (§2.4). Elapsed time and rows migrated are reported; cdm-rs's figures come from its own flushed counters, Java's from the target table, and the two are not equally trustworthy. Not a ratio. |
| `NOT RUN` | the suite's 3 h deadline arrived first, or the workload is opt-in and was not enabled |

The states exist so that a suite which does not finish still produces something honest. A result
that lists `narrow: MEASURED 2.1×, collections: OVERRAN, wide: NOT RUN` is worth publishing; a job
killed at the limit with no artefact is not.

And if the measured ratio is below 2×, that is the result. `NFR-004` currently asserts ≥ 2×; if
reality disagrees, `docs/SPEC.md` is what changes, not the workload.
