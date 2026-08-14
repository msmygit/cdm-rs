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

| Crate | Target | What it measures |
|---|---|---|
| `cdm-codec` | `conversion` | `ConversionPlan::apply` — per cell, per column, per row |
| `cdm-cql` | `bind` | PK extraction, the `MIG-012` `UNSET` decision, statement construction |
| `cdm-engine` | `compare` | `ComparisonPlan::compare`, plus instrumentation overhead |
| `cdm-feature` | `pipeline` | Constant columns, extract-JSON, filters, explode |
| `cdm-core` | `token` | `TokenRange::split` — startup, not hot path |

### Reference numbers

Apple M-series, `--release` with debug symbols (`[profile.bench]`), on an otherwise idle machine.
**These are for orientation only.** They come from a laptop, not the reference hardware, and
cross-machine comparison of these figures is meaningless. What matters is the *shape*, and the
change over time.

Machine load matters enormously, which is itself worth recording: the same binaries measured while
three other builds were running gave figures **5–7× higher** (`explode_map/1` at 2,720 ns against
332 ns here). See §3.

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

### Known optimisation targets

Neither is acted on here — this PR measures, it does not tune — but both are now quantified rather
than suspected.

- `wire::take_element` copies each collection element out of the buffer with `.to_vec()`, so explode
  does two heap allocations per map entry. Slicing into a shared `Bytes` would remove both.
- `PrimaryKey::new` allocates per key component per row. `key_extraction/1` at 61 ns is half the
  cost of binding an entire 8-column row, and it is flat in table width, so it does not amortise —
  it is worst, relatively, on narrow tables.

---

## 3. The 10% gate

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
