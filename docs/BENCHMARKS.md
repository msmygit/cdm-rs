# Benchmarks

How cdm-rs measures its own performance, what the numbers mean, and — importantly — what they do
not yet prove.

Requirements: `TST-060` (the suite) and `NFR-004` (the ≥ 2× Java CDM throughput claim).

---

## 1. Three tiers, three different questions

`TST-060` and `NFR-004` are often spoken of together, but they answer different questions and only
one of them is shaped like a CI job.

| Tier | Question | Needs | Where it runs |
|---|---|---|---|
| **1. Micro** | Did this commit make the hot path slower? | nothing | `bench.yml`, nightly + `bench` label |
| **2. Macro** | How many rows per second, end to end? | Cassandra containers | `bench.yml`, weekly, **not a gate** |
| **3. Java comparison** | Is it really ≥ 2× Java CDM? (`NFR-004`) | Spark + Java CDM + both clusters | `java-comparison.yml`, fortnightly, **not a gate** |

All three exist. Tier 3 is the one that actually settles `NFR-004`, and as of this commit it has a
harness and a schedule but no measured result: §5 says so and stays empty until the first
scheduled run fills it in.

Tier 2 records and reports; it never fails a build. End-to-end throughput on a shared runner is
dominated by the disk that runner was allocated, the tenants it shares a host with and how long
two Cassandra containers took to come up — none of which is a property of the commit under test.
A threshold on that signal would fire on the weather, and the response to a red build nobody can
act on is to stop reading it. §3 makes the same argument about tier 1, where the noise is smaller
and the gate is therefore merely loose rather than absent.

---

## 2. Tier 1 — criterion micro-benchmarks

```bash
cargo bench --workspace                       # everything, full statistics
cargo bench -p cdm-codec --bench conversion   # one target
cargo bench --workspace -- --output-format bencher   # what CI parses
```

A fast, low-fidelity pass while iterating:

```bash
cargo bench -p cdm-codec --bench conversion -- \
    --warm-up-time 0.3 --measurement-time 0.5 --sample-size 10
```

Benchmarks live in `crates/<crate>/benches/` and every benchmark function is named `tst_060_*`.
That naming is not cosmetic: `cargo xtask check-traceability` walks every `.rs` file under
`crates/`, so a benchmark citing a requirement ID that does not exist in `docs/SPEC.md` fails the
build exactly as a test would.

### What is covered

| Crate | Target | What it measures |
|---|---|---|
| `cdm-codec` | `conversion` | `ConversionPlan::apply` — per cell, per column, per row |
| `cdm-cql` | `bind` | PK extraction, the `MIG-012` `UNSET` decision, statement construction |
| `cdm-engine` | `compare` | `ComparisonPlan::compare`, plus instrumentation overhead |
| `cdm-engine` | `planner` | `split_ring` (`TOK-003`) and `shuffle_for_run` (`TOK-006`) — startup |
| `cdm-feature` | `pipeline` | Constant columns, extract-JSON, filters, explode |
| `cdm-core` | `token` | `TokenRange::split` — startup, not hot path |

### Reference numbers

Apple M-series, `--release` with debug symbols (`[profile.bench]`), on an otherwise idle machine.
**These are for orientation only.** They come from a laptop, not the reference hardware, and
cross-machine comparison of these figures is meaningless. What matters is the *shape*, and the
change over time.

Machine load matters enormously, which is itself worth recording: the same binaries measured while
three other builds were running gave figures **5–7× higher** (`explode_map/1` at 2,720 ns against
332 ns here). See §4.

```
                                          ns
conversion   passthrough/16              16     bind      key_extraction/1           61
             passthrough/256             16               key_extraction/8          172
             passthrough/4096            16               key_substitution/clean/128  5
             codec_int_to_text          111               key_substitution/subst/128 2,062
             codec_collection/1         453               bind_row/8                122
             codec_collection/16      4,285               bind_row/128            1,189
             codec_collection/256    57,199               bind_unset/value          382
                                                          bind_unset/null           341
compare      all_columns_match/4         93               bind_unset/empty_coll     398
             all_columns_match/64     1,401
             mismatch_last_col/4        296     pipeline  empty_filter_chain          0
             mismatch_last_col/64     1,662               disabled_constant_cols      3
             missing_target_row/*         5               filter_chain/accept        15
             keys_only/*                  5               extend_target_binding     232
                                                          extract_json/field/0      684
             ratelimit/none          45,432               extract_json/field/16   3,857
             ratelimit/instruments   66,577               extract_json/field/256 73,400
             adaptive_signal/off          2               explode_map/1             332
             adaptive_signal/on          54               explode_map/256        63,513

token        split_murmur3/65536    320,471     split_narrow/4096          10,744
             split_random/65536     321,139     split_narrow/65536         10,683

planner      split_ring/64            1,578     shuffle_for_run/64          1,853
             split_ring/1024         18,091     shuffle_for_run/1024       28,659
             split_ring/65536     1,290,261     shuffle_for_run/65536   1,963,146
             coverage/{1,25,100}   ~185,000     partition_floor/dense  11,971,279
                                                partition_floor/fallback       74
```

What is worth reading out of that:

- **Passthrough is flat from 16 B to 4 KiB.** The `MIG-040` zero-copy fast path is genuinely
  zero-copy — `RawCell` wraps `Bytes`, so the identity clone is a refcount bump. If this line ever
  starts tracking payload size, a copy has entered the hot path, which no correctness test catches.
- **Instrumentation is cheap.** `MET-010`'s per-request instruments cost **~21 ns per acquisition**
  (the ratelimit pair is 1024 acquisitions per iteration), and the `ENG-006` adaptive limiter costs
  **2 ns disabled, 54 ns enabled** per target write. Both were unmeasured before this suite existed;
  neither is a throughput concern.
- **The `MIG-012` `UNSET` branch is free.** Binding null (341 ns) is *cheaper* than binding a value
  (382 ns), and the empty-collection case that must inspect the serialised framing costs 4% over it.
  The most correctness-critical branch in the write path is not a performance trade-off, which is
  worth knowing before anyone is tempted to "optimise" it.
- **Validate's early exits are real.** A missing target row and a keys-only plan both cost 5 ns and
  are flat in table width. A mismatch costs ~200 ns over a match — one `Vec`, one cloned column
  name, two cloned cells — so a badly-migrated *narrow* table validates ~3× slower than a clean one,
  falling to ~1.2× by 64 columns.
- **Extract-JSON is superlinear** in document size: 16 → 256 filler fields is ~16× the fields but
  ~19× the time, because `serde_json` objects are `BTreeMap`-backed and the whole document is parsed
  to read one property. Reading by field name and by JSON Pointer converge exactly at 256 fields
  (73,400 vs 73,676 ns), confirming the parse dwarfs path resolution. This is the concrete argument
  for `exclusive` (`FEA-033`).
- **Collections cost roughly 2× per element** versus the same conversion on a scalar (~223
  ns/element at 256 against 111 ns for a bare `int -> text`), because native-protocol framing is
  re-encoded per element. Explode is similar at ~250 ns/entry. Both scale with *data*, not schema:
  a table averaging 256-entry maps does 256× the per-row work its row count implies.
- **`TokenRange::split` is not a cold-start risk.** 65,536 sub-ranges cost 0.32 ms against
  `NFR-002`'s 2-second budget. `split_narrow` at 4,096 and 65,536 parts costs the same 10.7 µs,
  which is the one-sub-range-per-token clamp working.
- **Ring planning is not a cold-start risk either, in any realistic configuration.** The `TOK-003`
  split plus the `TOK-006` shuffle costs ~3.3 ms combined at 65,536 ranges. Coverage sampling
  (`TOK-005`) is free — 1%, 25% and 100% all cost ~185 µs, so a run that samples pays nothing extra
  at plan time for scanning less.
- **`perfops.num_parts` has a 160,000× cliff in it.** At a fixed `num_parts` of 1,000,000, a range
  whose span is 1,000,000 tokens plans in 11.97 ms; a range of 1,000 tokens plans in 74 ns. The
  cause is `partition_size = span / num_parts` truncating to zero and falling back to a fixed
  100,000, which for a narrow range is wider than the whole span, so the split emits one range and
  stops. Both are correct Java behaviour, but the configuration gives no hint that the same setting
  means two wildly different things depending on the range it is applied to. `TRK-033`'s rerun path
  is where a large `num_parts` most easily meets a narrow range.

  Relatedly, and also correct Java behaviour: **`num_parts` is a request, not a guarantee.** The
  stride between ranges is `partition_size + 1`, so at `partition_size == 1` a `num_parts` of
  1,000,000 yields 500,001 ranges. For every realistic configuration the `+1` disappears into the
  rounding, but it is not an off-by-one and should not be "fixed".

### Known optimisation targets

Neither is acted on here — this PR measures, it does not tune — but both are now quantified rather
than suspected.

- `wire::take_element` copies each collection element out of the buffer with `.to_vec()`, so explode
  does two heap allocations per map entry. Slicing into a shared `Bytes` would remove both.
- `PrimaryKey::new` allocates per key component per row. `key_extraction/1` at 61 ns is half the
  cost of binding an entire 8-column row, and it is flat in table width, so it does not amortise —
  it is worst, relatively, on narrow tables.

---

## 3. Tier 2 — the macro-benchmark

```bash
cargo xtask bench                                  # the reference workload
just bench-macro                                   # the same thing
cargo xtask bench --rows 1000000 --columns 32      # a bigger one
cargo xtask bench --image cassandra:4.1            # a different engine
cargo xtask bench --bencher                        # one machine-readable line
```

Tier 1 measures functions. This measures a **migration**: two containers are started, an origin
table is populated with seeded data, `cdm` migrates it to the target, and the elapsed time is
divided into the row count. Everything tier 1 deliberately excludes — the driver, the network, the
two clusters, connection setup, paging, concurrency — is included here, which is exactly why the
two numbers cannot be derived from each other. A hot path that got 20% faster can leave end-to-end
throughput unchanged, because the run was never CPU-bound in that function.

The harness lives in `crates/cdm-testkit/src/macrobench.rs`; `cargo xtask bench` is a thin driver
over it. Every flag is optional and unset flags fall through to `MacroBenchSpec::default()` — a
fixed seed, so two runs migrate identical data — rather than to a second copy of the defaults in
the CLI.

### It needs a container runtime, and says so rather than failing

With no Docker or Podman the task prints why and exits **zero** (`TST-102`), as `cargo xtask it`
and `cargo xtask sit` do. A red result that only means "no container runtime on this laptop" trains
people to ignore the command.

### Reading the output

The human summary reports:

| Figure | What it tells you |
|---|---|
| `rows_migrated` vs `spec_rows` | whether the run actually completed. A throughput figure from a partial migration is not a throughput figure |
| `wall_clock` | the migration itself — token plan handed to the scheduler, until the last range completes. **Excludes** container startup and seeding, which are setup, not the thing being measured |
| `rows_per_second` | the headline, and the only figure `NFR-004` is about |
| `cold_start` | time to the first row read, against `NFR-002`'s 2-second budget |
| `peak_rss_bytes` | resident memory, against `NFR-003`. `None` where the platform does not report it |

`--bencher` emits the same run as a single `cargo bench --output-format bencher` line named
`nfr_004_macro_migrate`, which is what the workflow feeds to the trend store. It reports
**nanoseconds per migrated row** rather than rows per second, because the bencher format carries
one number and treats lower as better — so a throughput regression rises on the chart like every
tier-1 line, instead of inverting the axis for one series.

### Reference numbers

Apple M-series laptop, Docker Desktop, `cassandra:5.0` for both origin and target. Orientation
only: this is a laptop running both clusters and the migration on one machine, which is not a
configuration anybody migrates production data on.

| Workload | Migration | Throughput | Cold start |
|---|---|---|---|
| 100,000 rows × 16 cols | 10.52 s | **9,509 rows/s** | 0.25 s |
| 5,000 rows × 8 cols | 1.27 s | 3,936 rows/s | 0.14 s |

The small run is slower per row because a 1.27-second migration is still paying fixed startup cost;
throughput figures from short runs are not comparable with long ones, which is why the default
workload is 100,000 rows rather than something that finishes quickly.

`cold_start` at 0.25 s sits well inside `NFR-002`'s 2-second budget, but it does not by itself
discharge that requirement — see the caveat on the field's rustdoc. It is measured from the
`MET-010` observer seam on the first origin read, in-process against already-running nodes, so it
covers connect → introspect → prepare → plan → schedule → first page and excludes process spawn,
dynamic linking and config loading. It is a lower bound.

### Why it is weekly, and why it is not a gate

Weekly because it stands up containers and moves a hundred thousand rows: minutes per run, against
seconds for tier 1. Nightly would spend a lot of runner time to resolve a number that does not move
nightly.

Not a gate for the reason given in §1: the measurement is dominated by the machine, not the commit.
The `macro` job in `bench.yml` therefore sets `fail-on-alert: false` and writes to its own
`dev/bench-macro` series, disjoint from tier 1's — one series holding both would be trending
nanoseconds against rows per second and would mean nothing in either unit. What the job produces is
a trend line a human reads, and the honest way to investigate a step in it is to run
`cargo xtask bench` locally on hardware you control.

**These numbers do not satisfy `NFR-004`.** They establish cdm-rs's own throughput, on a runner, in
a shape that can be compared with itself over time. The ≥ 2× claim is a comparison against Java CDM
on identical hardware, which is §5.

---

## 4. The 10% gate

`TST-060` says: *"Regressions > 10% MUST fail CI."* **That requirement is not currently met, and
`bench.yml` does not pretend otherwise.**

The reason is that `ubuntu-latest` is a shared, virtualised runner. Wall-clock variance on
microbenchmarks between two runs of *identical* code is routinely 10–30%, driven by co-tenancy,
CPU model lottery and thermal state. A 110% alert threshold on that signal does not detect a 10%
regression; it fires on noise, and a gate that fires on noise is muted within a week — leaving no
gate at all, but with a green check mark suggesting otherwise.

This is not theoretical. Measuring these same benchmarks on a loaded laptop against an idle one
moved the figures by **5–7×** — `explode_map/1` from 332 ns to 2,720 ns, `filter_chain/accept` from
15 ns to 113 ns — with no code change whatsoever. A 10% threshold would have fired on every one of
them. It also means a real 10% regression is comfortably invisible beneath that noise floor, so the
gate would fail in both directions at once.

So the workflow ships a deliberately loose **200%** threshold. That reliably catches the class of
regression that actually matters — an accidental clone in a hot loop, a lost fast path, an O(n)
that became O(n²) — without crying wolf.

Closing the gap honestly needs a metric that does not depend on what else the runner is doing.
The candidate is **instruction-count benchmarking** (`iai-callgrind`), which counts retired
instructions under Valgrind and is deterministic to within a fraction of a percent, making a true
10% gate meaningful. It is not in this PR for two reasons: it requires Valgrind, which does not run
on Apple Silicon, so contributors on macOS could not reproduce a failure without Docker; and it is
a second, parallel set of benchmark files rather than a flag on the existing ones.

Until that lands, treat `NFR-004`'s companion gate as **partially satisfied**: regressions are
detected and trended, but the threshold is 200%, not 10%.

---

## 5. Tier 3 — the `NFR-004` claim

`NFR-004` asserts throughput ≥ 2× Java CDM on the same hardware for the reference workload. Tiers 1
and 2 cannot answer it: both measure cdm-rs against cdm-rs, and the claim is about something else
entirely. Tier 3 runs the other implementation.

```bash
just bench-java                                     # the reference workload, both implementations
bench/java-comparison/run.sh --workload all --repeats 2
bench/java-comparison/run.sh --rows 250000
bench/java-comparison/run.sh --skip-java            # the cdm-rs half only; no Spark needed
```

### What it is

`bench/java-comparison/` holds three parts, deliberately separable:

> **`environment/` has a second consumer, and it is not a benchmark.** `TST-020`'s differential
> suite runs both implementations over a hostile seeded corpus and asserts byte-identical target
> state — the same containers, the same jar, the same `spark-submit` line, a different question.
> It is documented in [`DIFFERENTIAL.md`](DIFFERENTIAL.md) rather than here for one reason: **it
> gates**, where nothing in this document ever does. Anyone changing `environment/` is changing
> both, and the differential suite will go red for it where tier 3 would only publish a footnote.


| Path | What it owns |
|---|---|
| `environment/` | Spark, the pinned Java CDM jar, the origin/target containers |
| `workloads/` | The three datasets (`narrow`, `wide`, `collections`) and `METHODOLOGY.md`'s config-equivalence audit |
| `run.sh` | The comparison itself: order, verification, timing, and the result documents |

`.github/workflows/java-comparison.yml` runs it fortnightly on a free `ubuntu-latest` runner —
4 vCPU, 16 GB RAM, ~14 GB free disk, no secrets, all OSS — and on `workflow_dispatch` with the
workload, row count, repeat count and starting implementation as inputs.

`run.sh` is a shell script rather than a `cargo xtask` subcommand because everything it does is
process orchestration against things that are not Rust: `docker`, `spark-submit`, `cqlsh`, `nb5`,
and two implementations invoked as binaries. Tier 2 is an xtask precisely because it drives the
library in process and can assert on typed results; tier 3 drives no library at all. Putting a
workspace build on the critical path of a benchmark that times a *released* binary is the concrete
cost, and having to compile before it can report that Spark is missing is the concrete symptom.

### The four things that make the number mean anything

**Fresh containers per implementation.** Not one pair reused across both runs. Whichever ran second
would read origin out of a warm page cache and write into a target still holding the first run's
SSTables and its compaction backlog. Both effects favour the second runner, and both are large
enough to fabricate the entire claim on their own. So the nodes are destroyed and recreated, the
schema recreated and the data reseeded, for every measured run — which costs a minute of container
startup per run and is not negotiable.

**Order is alternated and recorded.** `--first` fixes which implementation goes first; the default,
`auto`, alternates on the repeat index, so `--repeats 2` covers both orders in one invocation. A
single-repeat scheduled run has nothing to alternate within, so the starting side comes from the
ISO week number and consecutive fortnightly runs swap. Every result carries `order_index` — 0 for
the side that ran first — so the question can be asked of the data afterwards rather than assumed
away.

**Cold and steady-state are reported separately, never conflated.** Cold is the whole invocation:
process spawn, JVM boot, `SparkSession` creation, executor startup, the migration, exit. Steady
state subtracts a startup floor which is *measured, not assumed* — the same workload with zero rows
in it, run by the same procedure on both sides. That is a complete invocation minus only the row
work.

  Subtracting an empty run slightly over-estimates startup, because scanning the ranges of an empty
  table is not free. The direction is deliberate: over-estimating startup shortens the steady-state
  window and so raises the reported steady-state rate, by more for the implementation with the
  larger startup — which is Java. The bias works against cdm-rs's ratio, never for it. The
  alternative, grepping a marker out of Spark's log4j output, would make the measurement depend on
  a logging configuration neither implementation guarantees.

**Both targets are verified before either number is reported.** Two checks per run: an independent
`SELECT COUNT(*)` through `cqlsh`, which must agree with the job's own write counter — cdm-rs's
`MET-033` summary or Java's `Final Write Record Count` line, which cdm-rs reproduces character for
character (`COMPAT-004`) — and a full `cdm validate`, comparing both sides row by row and column by
column. A run whose target does not match is recorded with `status: "unverified"` and is excluded
from every aggregate. This is the same rule `crates/cdm-testkit/src/macrobench.rs` applies at tier
2: a throughput figure from a lossy migration measures nothing. Note the one asymmetry, which is
stated rather than hidden: `validate` is cdm-rs's comparator on both sides. It is independent of
Java CDM's writer and of cdm-rs's *migrate* path, but not of cdm-rs itself — the `COUNT(*)` is the
part that depends on neither implementation.

### What it emits

One JSON document per run under `runs/`, `comparison.json` aggregating them, and `results.md`
rendered *from* that JSON so the prose and the machine-readable form cannot drift apart. The
workflow uploads the whole directory as an artefact on every run, including a failed one, and puts
`results.md` in the job summary.

```jsonc
{
  "schema": "cdm-rs.java-comparison.run/v1",
  "workload": "narrow",
  "implementation": "cdm-rs",            // or "java-cdm"
  "version": "cdm 0.1.0",                // or the pinned jar
  "status": "ok",                        // ok | unverified | failed | unavailable
  "note": null,                          // why, whenever status is not "ok"
  "repeat": 0,
  "order_index": 0,                      // 0 = ran first in this pair
  "rows_expected": 100000,
  "rows_written": 100000,                // the job's own counter
  "origin_row_count": 100000,            // independent SELECT COUNT(*)
  "target_row_count": 100000,            // independent SELECT COUNT(*)
  "verification": { "method": "…", "validate": "clean", "unrepaired_differences": 0 },
  "verified": true,
  "cold_wall_clock_secs": 71.204,
  "startup_secs": 43.918,                // measured, not assumed
  "steady_state_secs": 27.286,
  "cold_rows_per_sec": 1404.42,
  "steady_rows_per_sec": 3664.86,
  "started_at": "…Z", "finished_at": "…Z",
  "properties_sha256": "…",              // the config both sides were handed
  "properties_equivalence": "file",      // "file" = byte-identical; "mapped" = see below
  "environment": { "cpus": 4, "memory_bytes": …, "disk_free_bytes": …, "cpu_model": "…",
                   "cassandra_image": "…", "spark": "…", "java": "…", "java_cdm": "…",
                   "cdm_rs_version": "…", "cdm_rs_commit": "…", "ci_run": "…" }
}
```

`comparison.json` adds `cold_ratio` and `steady_ratio` per workload — **and only where both sides
produced a verified run.** A failed Java run, an unverified target or a missing steady state
produces `null` and a sentence naming what is missing. A ratio computed against a partial run is
not a weaker result, it is a wrong one.

With `--repeats N > 1` the aggregate is the median of each side's verified runs, computed
identically for both, and `comparison.json` says so in its `aggregate` field. There is no best-of-N
anywhere, no retry, and no fallback that would give one implementation a second attempt the other
did not get.

### How it degrades

The Java side is optional to the harness and mandatory to the claim. If Spark will not start, the
jar is missing or the version triple is wrong, the run is recorded as `status: "unavailable"` with
the reason, the cdm-rs half is still measured and published, and **no ratio is emitted**. The
workflow does not fail: it never fails on a number, only when the harness itself breaks.

`run.sh` talks to the other two directories through three seams, each of which prefers the
sibling's script and falls back to something self-contained, so a checkout with neither still gives
`--skip-java` a working cdm-rs half. Where the Java seam is `submit-migrate.sh`, which builds its
own properties from a template, the workload's settings are carried across as the environment
variables that script documents and the result records `properties_equivalence: "mapped"` rather
than `"file"` — configured alike, not byte-identical, and the file says which.

### Results

**Empty. Nothing has been measured yet.** The harness, the workflow and the schedule exist as of
this commit; the first scheduled run — or the first `workflow_dispatch` — populates this section.

When it does, what goes here is the table from `results.md` with the runner's CPU, RAM and free
disk, the row count, both versions, both configurations and the artefact link. A throughput figure
without its hardware is not a result.

Two things are settled in advance, so that neither is decided by whoever first sees the number:

- **If the ratio comes in below 2×, it gets published as it is.** It is a finding to act on, not a
  number to adjust, and not a reason to add a "best of five" to the runner.
- **A free `ubuntu-latest` runner is not reference hardware.** Four shared vCPU hosting two
  Cassandra nodes, a Spark JVM and the migrator is a small, contended machine, and the figures will
  be lower than a dedicated box gives — for both implementations. What it buys is that the *ratio*
  is measured on one machine, at one moment, with the same containers and the same dataset, which
  is exactly what `NFR-004` asks for. Anyone with dedicated hardware can run the same script on it;
  that result belongs here too, next to this one rather than instead of it.
