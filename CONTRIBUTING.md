# Contributing to cdm-rs

Thank you for helping. This project has an unusual amount of process for its size, and it is
deliberate: cdm-rs is a reimplementation of a tool people trust with production data, so "we think
it works" is not good enough. Every behaviour is specified, traced and tested.

## The short version

```bash
git clone git@github.com:msmygit/cdm-rs.git && cd cdm-rs
pipx install pre-commit && pre-commit install --install-hooks -t pre-commit -t commit-msg
cargo test --workspace        # unit tests; no cluster required
cargo xtask it                # integration tests; needs Docker or Podman
```

If you would rather not install Python, `cargo xtask install-hooks` installs equivalent native git
hooks.

The full list of CI gates — every command, the tools you need to install to run them, and which are
worth running before every commit rather than before a push — lives in
[`AGENTS.md`](AGENTS.md#commands). That list is canonical; it is written for agents but it is the
same list humans need, so it is kept in one place rather than two.

## How work is organised

1. [`docs/SPEC.md`](docs/SPEC.md) states every requirement, with a stable ID like `MIG-012`.
   IDs are append-only: never renumber, never reuse.
2. [`docs/ROADMAP.md`](docs/ROADMAP.md) breaks delivery into numbered pull requests.
3. [`docs/TRACEABILITY.md`](docs/TRACEABILITY.md) maps every ID to its crate, its tests and its PR.

Before writing code, find the requirement ID you are implementing. If there isn't one, propose it:
open an issue, or include the `SPEC.md` change in your PR and say so in the description. Changing
specified behaviour without changing the spec is the one thing that will always get a PR rejected.

## Pull requests

Every PR:

- is **small and single-purpose** — one requirement cluster, reviewable in one sitting;
- uses [Conventional Commits](https://www.conventionalcommits.org/) (`feat(engine): ...`);
- carries an `Implements: <REQ-IDs>` trailer;
- updates `docs/TRACEABILITY.md` in the same PR;
- adds tests named `<req_id>_<description>`, e.g. `mig_012_empty_collection_is_unset`;
- leaves `main` compiling and green.

```text
feat(engine): bind empty collections as UNSET

Binding null or an empty collection as NULL creates a tombstone on every
migrated row. Bind UNSET instead, matching Java CDM.

Implements: MIG-012
```

CI enforces the traceability rules mechanically. A PR that marks a requirement done without a test
citing its ID will fail.

## Testing expectations

| Change | Minimum |
|---|---|
| Pure logic | unit tests covering every branch |
| Anything touching a cluster | an integration test (`tests/`, testcontainers) |
| Parity with Java CDM | the relevant ported SIT case must pass |
| A codec | round-trip property test **and** a known-vector test |
| The hot path | a benchmark, with no regression over 10% |

Coverage is a ratchet, not a fixed bar: `coverage.yml` fails below the floor the codebase clears
today (70%, with `xtask/` excluded as repository automation), and that floor is raised by the PR
that raises the coverage and never lowered. 85% workspace-wide is the v1.0 success criterion (S4),
not today's gate. Aim for 90% in the crate you touch.

## Behaviour differences from Java CDM

cdm-rs deliberately improves on Java in a handful of places (see
[`docs/MIGRATION_FROM_JAVA.md`](docs/MIGRATION_FROM_JAVA.md)). If your change adds one:

1. document it in `MIGRATION_FROM_JAVA.md` with the rationale;
2. put the old behaviour behind `--compat-java`;
3. test both paths.

Silent behavioural drift from Java is a bug, however sensible the new behaviour is. People will
migrate petabytes with this.

## Code style

- `cargo fmt` and `cargo clippy -D warnings` are not negotiable; pre-commit runs both.
- `#![forbid(unsafe_code)]` workspace-wide.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests. Where an invariant
  genuinely cannot fail, document it with a `// SAFETY-INVARIANT:` comment and a targeted `allow`.
- Every public item needs rustdoc.
- Secrets go in `Secret<T>`. Never log a row value outside the validate diff path.

## Adding functionality without touching the core

cdm-rs exposes plugin traits for codecs, features, filters, guardrails, jobs, sources, sinks,
tracking stores and metric exporters (`PLG-001`–`PLG-013`). If your need fits one of these, a plugin
will land faster than a core change and is easier to review. See `docs/EXTENDING.md` and the
compiling examples in `examples/`.

## Reporting bugs

Use the issue template. The two things that make a report actionable are the **run id** (if run
tracking was enabled) and the **run summary** (`--summary-out report.json`). Please redact secrets;
`cdm config explain` output is already redacted.

## Licensing

By contributing you agree that your contribution is licensed under the Apache License 2.0.
