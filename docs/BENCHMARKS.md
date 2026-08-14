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
| **2. Macro** | How many rows per second, end to end? | Cassandra containers | nightly, not a gate |
| **3. Java comparison** | Is it really ≥ 2× Java CDM? (`NFR-004`) | Spark + Java CDM + fixed hardware | by hand, written up here |

Only tier 1 exists today. Tiers 2 and 3 are roadmap #55's remaining work.

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

| Crate | Target | Path |
|---|---|---|
| `cdm-codec` | `conversion` | `ConversionPlan::apply` — per cell, per column, per row |

Further targets land with the rest of #55.

### Reference numbers

Apple M-series, `--release` with debug symbols (`[profile.bench]`). **These are for orientation
only.** They come from a laptop, not the reference hardware, and cross-machine comparison of these
figures is meaningless. What matters is the *shape*, and the change over time.

```
tst_060_passthrough/16              16 ns
tst_060_passthrough/256             17 ns
tst_060_passthrough/4096            16 ns
tst_060_codec_int_to_text          115 ns
tst_060_codec_collection/1         447 ns
tst_060_codec_collection/16      4,187 ns
tst_060_codec_collection/256    57,204 ns
```

Two things worth reading out of that:

- **Passthrough is flat from 16 B to 4 KiB.** The `MIG-040` zero-copy fast path is genuinely
  zero-copy. If this line ever starts tracking payload size, a clone has been introduced into the
  hot path — which no correctness test would catch.
- **Collections cost roughly 2× per element** compared with the same conversion applied to a scalar
  (~223 ns/element at 256 vs 115 ns for a bare `int -> text`). Native-protocol framing is
  re-encoded per element. This is expected, not a defect, but it means collection-heavy schemas
  should be expected to migrate slower per row.

---

## 3. The 10% gate

`TST-060` says: *"Regressions > 10% MUST fail CI."* **That requirement is not currently met, and
`bench.yml` does not pretend otherwise.**

The reason is that `ubuntu-latest` is a shared, virtualised runner. Wall-clock variance on
microbenchmarks between two runs of *identical* code is routinely 10–30%, driven by co-tenancy,
CPU model lottery and thermal state. A 110% alert threshold on that signal does not detect a 10%
regression; it fires on noise, and a gate that fires on noise is muted within a week — leaving no
gate at all, but with a green check mark suggesting otherwise.

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

## 4. Tier 3 — the `NFR-004` claim (not yet run)

`NFR-004` asserts throughput ≥ 2× Java CDM on the same hardware for the reference workload. **This
has never been measured.** The number is currently an aspiration, and this document is the place it
will be substantiated or corrected.

It cannot be a CI job. Java CDM is a Spark application, driven by `spark-submit`; a meaningful
comparison means standing up Spark, both clusters and an identical dataset on hardware that is not
shared with anyone else. That is a benchmarking exercise with a written-up result, run on demand.

When it is run, the result belongs here, alongside the exact hardware, dataset, cluster topology,
row count, both versions and both configurations. A throughput figure without its hardware is not a
result.

If the measured ratio comes in below 2×, that is a finding to publish and act on, not a number to
quietly adjust.
