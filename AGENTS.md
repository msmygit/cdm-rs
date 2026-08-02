# AGENTS.md

Guidance for AI coding agents working in this repository. Humans should read
[`CONTRIBUTING.md`](CONTRIBUTING.md), which says the same things at more length.

## What this repository is

cdm-rs is a Rust reimplementation of the Java
[Cassandra Data Migrator](https://github.com/datastax/cassandra-data-migrator). It moves and
validates data between Cassandra-compatible clusters. People run it against production data, at
petabyte scale, often once and irreversibly. Correctness dominates every other consideration.

## Read before writing code

1. [`docs/SPEC.md`](docs/SPEC.md) — every behaviour, as a numbered requirement (`MIG-012`, `CFG-020`, …).
   This is the contract. IDs are append-only: never renumber, never reuse.
2. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — crate topology, execution model, why things are
   where they are.
3. [`docs/ROADMAP.md`](docs/ROADMAP.md) — the numbered PR that your change probably belongs to.
4. [`docs/TRACEABILITY.md`](docs/TRACEABILITY.md) — requirement → crate → test → PR.

## Non-negotiables

- **Every change maps to a requirement ID.** If no ID covers what you are doing, add one to
  `docs/SPEC.md` in the same change and say so explicitly. Never implement unspecified behaviour.
- **Update `docs/TRACEABILITY.md`** in the same change. CI fails otherwise.
- **Tests cite the ID** in the test name: `mig_012_empty_collection_is_unset`.
- **Commit messages** use Conventional Commits with an `Implements: <IDs>` trailer.
- **Never break Java parity silently.** Requirements marked `[P]` must behave identically to the
  Java implementation. If you improve on Java, put the old behaviour behind `--compat-java`, document
  it in `docs/MIGRATION_FROM_JAVA.md`, and test both paths.

## Code constraints enforced by CI

- `#![forbid(unsafe_code)]` workspace-wide.
- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` outside tests. If an invariant truly
  cannot fail, document it with `// SAFETY-INVARIANT:` and a targeted `allow`.
- `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings` must pass.
- Every public item needs rustdoc.
- Secrets live in `Secret<T>`; never log one. Never log row values outside the validate diff path.
- Only `cdm-cql` may depend on `scylla`. Only `cdm-api` may depend on HTTP crates. Do not add a
  dependency edge that is not in the graph in `docs/ARCHITECTURE.md` §3.
- Generated artefacts (`api/openapi.yaml`, `schema/*.json`, `docs/generated/*`) are never hand-edited.
  Run `cargo xtask openapi` and `cargo xtask docs`.

## Commands

```bash
cargo test --workspace          # unit tests, no cluster needed
cargo xtask it                  # integration tests (Docker/Podman)
cargo xtask sit                 # Java SIT parity suite
cargo xtask check-traceability  # the CI traceability gate
just lint                       # exactly what CI runs
```

## Things that are easy to get wrong here

- **`UNSET`, not `NULL`.** Binding null or an empty collection as NULL creates a tombstone on every
  row (`MIG-012`).
- **Counters are not idempotent.** Never retry a counter write, never batch one, never reclaim an
  in-flight counter range in cluster mode (`CON-012`, `MIG-032`, `DST-015`).
- **The token splitter's edge cases are load-bearing.** Reproduce the Java algorithm exactly,
  overflow behaviour included (`TOK-003`).
- **Metric strings are a public contract.** Users' scripts parse them; they must stay
  character-identical (`MET-005`, `MET-006`).
- **Astra's CQL port comes from `cqlshrc`**, not from `config.json` (`CON-026`).
- **Resolve work at startup, not per row.** Conversion plans, statements and PK extractors are built
  once into an immutable `ExecutionPlan` (`docs/ARCHITECTURE.md` §5.5).

## When you are unsure

Prefer asking to guessing. A wrong assumption about migration semantics does not surface as a failing
test — it surfaces as corrupted data in someone's production cluster.
