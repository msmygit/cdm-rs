# `tests/differential/` — the Java-parity corpus (`TST-020`)

`TST-020` requires a nightly harness that runs cdm-rs and Java CDM against the same seeded dataset
and asserts byte-identical target state and identical counter blocks, "over a generated corpus
covering every CQL type, nesting depth 3, nulls, empty collections, and edge-case values (min/max
integers, epoch boundaries, unicode, empty strings)".

This directory is **not** the corpus. The corpus is
[`crates/cdm-testkit/src/differential/corpus.rs`](../../crates/cdm-testkit/src/differential/corpus.rs),
which is the single source of truth and the thing to read. What is here is a checked-in *rendering*
of it, for three reasons:

1. a schema that only exists inside a Rust function cannot be reviewed in a diff, and a corpus that
   silently loses a type is the one failure mode this whole harness is designed to prevent;
2. the Java side of a differential run has no Rust in it. `schema.cql` is loadable with `cqlsh`
   and nothing else;
3. a change to the coverage matrix — a type gained, a gap opened — shows up as a line in a pull
   request, where somebody sees it.

## Files

| File | What it is |
|---|---|
| `schema.cql` | `Corpus::full(Seed::new(20)).schema_script()` — the `CREATE KEYSPACE`, `CREATE TYPE` and `CREATE TABLE` statements, in dependency order. |
| `coverage.tsv` | `Corpus::full(Seed::new(20)).coverage_manifest()` — every type cdm-rs models, and either the column that covers it or the reason it is absent. |

Both are asserted against the generator by
`tst_020_the_checked_in_schema_matches_the_generated_one` and
`tst_020_the_checked_in_coverage_manifest_matches_the_matrix`, so they cannot drift.

The rows are not checked in. They are a function of the seed — `Corpus::full(seed)` — and writing
137 rows of edge-case literals into a file would make the seed a lie.

## Regenerating

After changing the corpus:

```bash
cargo test -p cdm-testkit --lib tst_020_rewrite -- --ignored
```

## Loading it

```rust
let corpus = Corpus::full(Seed::from_env_or_entropy())?;
fixture.exec_cql(&corpus.load_script()).await?;
```

`load_script()` is the schema followed by every table's rows, one statement per line, terminated
with `;` — the shape `cqlsh -f` and
[`ClusterFixture::exec_cql`](../../crates/cdm-testkit/src/containers.rs) both accept. It is proved
against a real Cassandra node by `crates/cdm-testkit/tests/differential_corpus_it.rs`, which runs
under `cargo xtask it`.

## Two tables, not one

A `counter` column may not share a table with a non-counter one, and a counter is written with
`UPDATE ... SET c = c + n`, never `INSERT` (`MIG-030`). So `cdm_diff.all_types` holds everything
else and `cdm_diff.counters` holds the counters, and a harness that migrates only
`Corpus::table()` has not compared a counter. See `Corpus::tables`.

## What the comparison engine reads off it

The comparator builds its snapshot `SELECT` from `CorpusTable::key_columns()` and
`CorpusTable::value_columns()`, not from `system_schema`: a harness that introspects the schema it
is comparing takes its expectations from the same place as its observations, and then agrees with
itself.

Each `CorpusColumn` says whether `WRITETIME`/`TTL` may be selected for it as a single `bigint`.
Three categories may not be, and all three were **measured against `cassandra:5.0.9`**, because two
of them do not behave the way the restriction is usually described:

| Category | What the server actually does on 5.0 | Why it is excluded |
|---|---|---|
| primary-key columns | rejects: `Cannot use selection function writetime on PRIMARY KEY part pk` | asking breaks the whole `SELECT` |
| non-frozen collections | **accepts**, answering with a `list<bigint>` of per-cell timestamps | a different type from every other column, a length that depends on the value, and rejected outright before Cassandra 4.1 |
| counters | **accepts**, returning the coordinator's clock (`TTL` is null) | a counter update cannot carry `USING TIMESTAMP`, so the value differs between two runs by construction |

None of this is a gap in the comparison: those columns' *values* are still byte-compared. Only
per-cell metadata that is unreadable or meaningless — equally, on both sides — goes unread.

## Gaps

Every type that is *not* in the corpus is a line in `coverage.tsv` with the reason, and the module
documentation of `corpus.rs` explains each at length. In summary, and all of them measured rather
than assumed:

- the DSE geometry types and `DateRangeType` — no open-source image implements them;
- `vector<T, N>` — open-source Cassandra 5.0 and later only, so it is off by default;
- a lone surrogate in `text` — not valid UTF-8, so neither CQL nor Rust can express it;
- a `counter` delta of `i64::MIN` — Cassandra rejects the only literal that could write it.
