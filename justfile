# One-command entry points (OPS-024). `just --list` to see them all.

default: lint test

# Build the workspace.
build:
    cargo build --workspace --all-features

# Build an optimised binary.
release:
    cargo build --profile dist --bin cdm

# Formatting and lints, exactly as CI runs them.
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

# Nightly differential suite against Java CDM (TST-020).
differential:
    cargo xtask differential

# Coverage, with the CI threshold applied.
cover:
    cargo llvm-cov --workspace --all-features --html --fail-under-lines 85

# Benchmarks (TST-060).
bench:
    cargo bench --workspace

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

# Build the documentation site.
docs:
    cargo doc --workspace --no-deps --all-features --open

# Install git hooks without a Python dependency.
hooks:
    cargo xtask install-hooks
