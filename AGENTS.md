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

Three rules before any of them:

- **Clean tree.** `git status --porcelain` must be empty. A gate verified on a dirty tree says
  nothing about the commit. This shipped a broken rustdoc fix in PR #64 — the file was edited and
  verified, then `git commit --amend` ran without staging it.
- **Never pipe a gate into `head`/`tail`.** The pipeline reports the *last* command's status, so a
  failure looks green. That has masked real failures three times here. GitHub Actions' default
  shell is `bash -e` **without** `pipefail`, which is why the multi-line `run:` steps in
  `bench.yml` and `java-comparison.yml` set `shell: bash` explicitly.
- **`--all-features` is not optional** on clippy, test and doc — CI uses it, and a feature-gated
  compile error is invisible without it.

### While editing

```bash
cargo fmt --all -- --check
taplo fmt --check                 # TOML formatting; PR #65 went red on one over-long Cargo.toml line
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features   # unit tests, no cluster needed
cargo xtask check-traceability          # OPS-011; the subcommand is check-traceability, not traceability
```

### Before pushing

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo xtask openapi --check       # OPS-012: api/openapi.yaml, schema/*.json match generated output
cargo xtask docs --check          # OPS-012: docs/generated/*. Without --check it rewrites them
cargo machete                     # unused dependencies
cargo deny check                  # licences, advisories, bans (SEC-030)
cargo audit                       # RUSTSEC advisories; CI deliberately omits --deny warnings
cargo hack check --workspace --feature-powerset --depth 2   # slow: every feature combination compiles
mdbook-mermaid install docs/book && mdbook build docs/book  # ci.yml builds the site on every PR
cargo llvm-cov --workspace --all-features --ignore-filename-regex 'xtask/' --fail-under-lines 70
```

`cargo deny check` rejects any licence absent from `deny.toml`'s allow-list, including one pulled
in transitively by a dependency bump you did not write. To fix a generated-artefact failure, run
`cargo xtask openapi` / `cargo xtask docs` without `--check` and commit the result; never hand-edit
the output. CI also runs the test job on macOS and Windows and on the MSRV in `Cargo.toml`
(`rust-version`, NFR-005) — a stable-only Linux pass is not the whole test gate. Touching
`crates/**`, `api/**` or `schema/**` additionally triggers a Redocly lint and an `oasdiff`
breaking-change gate on `api/openapi.yaml` (API-008); both need `npx`/Docker.

### Container and parity suites (pre-push, minutes each)

```bash
cargo xtask it            # TST-002 integration suite; Docker or Podman
cargo xtask sit           # TST-003 Java SIT parity; starts Cassandra 5.0
cargo xtask differential  # TST-020 differential vs Java CDM; needs a JDK
```

`cargo xtask it` **exits 0 when no container runtime is present** — it skips rather than fails
(TST-102). A green run on a machine without Docker has tested nothing; read the output before
treating it as evidence.

### What `cargo` never sees

```bash
pre-commit run --all-files
```

Runs `typos`, `markdownlint`, `gitleaks`, `shellcheck` (`scripts/**`, `bench/**/*.sh`) and
`yamllint --strict -c .yamllint.yaml`. `--strict` makes warnings fatal, so a YAML line over 120
columns fails. `cargo xtask install-hooks` installs equivalents without the Python dependency.

Three gates have no local equivalent and can only fail after you push: CodeQL, the full-history
`gitleaks` scan and the CycloneDX SBOM build. Two more run weekly rather than per-PR — the
real-Astra connectivity suite (`astra.yml`) and the Java throughput comparison
(`java-comparison.yml`) — so a break there surfaces days later, against no particular commit.

### Tooling

None of these ship with the Rust toolchain; CI installs them, so it is possible to be green
locally and red in CI purely by not having them:

```bash
cargo install taplo-cli cargo-hack cargo-deny cargo-machete cargo-audit \
              cargo-llvm-cov mdbook mdbook-mermaid
rustup component add llvm-tools-preview   # cargo llvm-cov errors out without it
pipx install pre-commit && pre-commit install --install-hooks -t pre-commit -t commit-msg
brew install shellcheck && pipx install yamllint   # to run those two on a single file
```

The last line needs a rustup-managed toolchain; a Homebrew or distro Rust has no way to add the
component, so coverage is a CI-only gate on those machines.

`just lint` is `cargo fmt` + `cargo clippy` + `taplo fmt` and nothing else. It is three of the
gates above, not all of them; a green `just lint` does not predict a green CI.

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
