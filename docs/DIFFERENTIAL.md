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
| Phase 1 | fresh clusters → seed the origin → `submit-migrate.sh` → verify → capture the target |
| Phase 2 | fresh clusters → seed the origin identically → `cdm migrate` → verify → capture the target |
| Compare | `cdm_testkit::differential::compare` over both target states and both counter blocks |
| Report | `reports/differential/`, uploaded by CI whatever the outcome |

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
2. **the write counter equals the corpus row count** — what the job believes it did;
3. **an independent `SELECT COUNT(*)` on the target equals the corpus row count** — the one check
   that takes nobody's word for anything.

## 5. The two things compared

**Target state.** A full `SELECT *` through the node's own `cqlsh`, with paging off. A full scan of
a single node returns rows in token order, which is a function of the partition keys and the
partitioner alone, so two nodes holding the same rows render them in the same order whatever vnodes
they were assigned.

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

## 7. Reproducing a failure

Every run writes `reports/differential/seed.txt` before it can fail, containing the command that
replays it. The nightly uploads the whole directory whatever the outcome:

```text
reports/differential/
  seed.txt              corpus, seed, and the exact replay command
  report.txt            the diff — this run's product
  java-cdm/
    submit.log          everything spark-submit and CDM printed
    counters.txt        the normalised MET-006 block
    target-state.txt    the captured target
    cdm-*.properties    the exact properties CDM was given
  cdm-rs/
    migrate.log, counters.txt, target-state.txt, cdm.properties, summary.json
```

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
