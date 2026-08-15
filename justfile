# One-command entry points (OPS-024). `just --list` to see them all.

default: lint test

# Build the workspace.
build:
    cargo build --workspace --all-features

# Build an optimised binary.
release:
    cargo build --profile dist --bin cdm

# Formatting and lints, with the exact flags CI uses. These are three of CI's gates, not all of
# them — see AGENTS.md "Commands" for the full list. A green `just lint` does not predict a green CI.
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    taplo fmt --check

# Apply formatting.
fmt:
    cargo fmt --all
    taplo fmt

# Unit tests; no cluster required.
test:
    cargo test --workspace --all-features

# Containerised integration tests (TST-002).
it:
    cargo xtask it

# Ported Java SIT parity suite (TST-003).
sit:
    cargo xtask sit

# The Java-parity differential suite (TST-020). Runs Java CDM 6.0.1 and cdm-rs over one seeded
# corpus on fresh clusters each and asserts byte-identical target state and identical counter
# blocks. Needs Docker; skips cleanly without one. Tens of minutes, and unlike every other
# Java comparison in this repository it is a GATE: a difference exits non-zero and the diff lands
# in reports/differential/. See docs/DIFFERENTIAL.md. Pass --corpus smoke to iterate, and
# --seed <n> to replay a failure exactly.
differential *ARGS:
    cargo xtask differential {{ARGS}}

# Coverage against the v1.0 target (success criterion S4), which is deliberately stricter than the
# gate: coverage.yml fails under 70% with `xtask/` excluded. This recipe can fail while CI passes.
cover:
    cargo llvm-cov --workspace --all-features --html --fail-under-lines 85

# Benchmarks (TST-060). See docs/BENCHMARKS.md for what these numbers do and do not prove.
bench:
    cargo bench --workspace

# A fast, low-fidelity benchmark pass for iterating. Too noisy to draw conclusions from; use
# `just bench` for anything you intend to act on.
bench-quick:
    cargo bench --workspace -- --warm-up-time 0.3 --measurement-time 0.5 --sample-size 10

# End-to-end throughput against real clusters (TST-060 tier 2, NFR-004). Needs a container
# runtime and takes minutes; skips cleanly without one. Reports a number, gates nothing.
bench-macro *ARGS:
    cargo xtask bench {{ARGS}}

# The tier-3 Java CDM comparison (NFR-004). Starts fresh containers for each implementation, runs
# both, verifies both targets and writes JSON plus a table. Needs Docker; needs Spark and the
# pinned jar for the Java half, and reports honestly when it does not have them. Gates nothing.
bench-java *ARGS:
    bench/java-comparison/run.sh {{ARGS}}

# Regenerate every generated artefact (OPS-012).
generate:
    cargo xtask openapi
    cargo xtask docs

# Verify requirement traceability (OPS-011).
trace:
    cargo xtask check-traceability

# Supply-chain checks (SEC-030).
audit:
    cargo deny check
    cargo audit --deny warnings

# Build the rustdoc API reference.
docs:
    cargo doc --workspace --no-deps --all-features --open

# Build the mdBook site (OPS-010). `mdbook-mermaid install` writes the vendored mermaid
# assets that book.toml references; they are build inputs, not sources, so they are
# gitignored and regenerated here and in .github/workflows/docs.yml.
book:
    mdbook-mermaid install docs/book
    mdbook build docs/book

# Serve the mdBook site locally with live reload.
book-serve:
    mdbook-mermaid install docs/book
    mdbook serve docs/book --open

# Install git hooks without a Python dependency.
hooks:
    cargo xtask install-hooks

# Report disk taken by agent worktrees under `.claude/worktrees/`. Reports only; changes nothing.
reclaim:
    scripts/reclaim-agent-space.sh

# Reclaim it. Skips worktrees being built in, and unmerged or dirty ones unless `--all` is added.
reclaim-apply *ARGS:
    scripts/reclaim-agent-space.sh --apply {{ARGS}}
