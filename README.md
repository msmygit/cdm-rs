# cdm-rs

**A Rust reimplementation of the [Cassandra Data Migrator](https://github.com/datastax/cassandra-data-migrator) — no JVM, no Spark, no functionality lost.**

[![CI](https://github.com/msmygit/cdm-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/msmygit/cdm-rs/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

cdm-rs migrates and validates data between Cassandra-compatible clusters — Apache Cassandra, DSE,
HCD, Astra DB, ScyllaDB and Azure Cosmos DB Cassandra API — as a single static binary.

> **Status: specification and scaffolding.** No functionality is implemented yet. The complete
> design is in [`docs/SPEC.md`](docs/SPEC.md) and [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md);
> delivery is tracked PR-by-PR in [`docs/ROADMAP.md`](docs/ROADMAP.md).

---

## Why

The Java CDM is excellent at what it does. The friction is in operating it:

| Java CDM | cdm-rs |
|---|---|
| Requires a JVM **and** an exact Spark build (`NoSuchMethodError: scala.runtime.Statics.releaseFence()` is the most common support ticket) | One static binary, no runtime dependencies |
| `--driver-memory 25G --executor-memory 25G` | Bounded, computable memory — typically a few hundred MB |
| ~90 untyped `spark.cdm.*` properties; errors surface hours into a run | Typed config, three validation tiers up front, every error reported at once with a suggested fix |
| Progress by scraping logs for a final counter block | Live counters, Prometheus, OpenTelemetry, SSE event stream, terminal UI |
| Automation means driving `spark-submit` and parsing stdout | OpenAPI 3.1 control plane, with MCP and A2A generated from it |
| Distribution means standing up a Spark cluster | Built-in lease-based coordination; N processes, one shared run, zero extra infrastructure |

**Nothing is dropped.** Every documented and undocumented behaviour of Java CDM 6.0.x is a numbered
requirement in [`docs/SPEC.md`](docs/SPEC.md), traced to code and tests in
[`docs/TRACEABILITY.md`](docs/TRACEABILITY.md), and certified by the ported
[SIT suite](docs/SPEC.md#211-layers) plus a nightly differential run against the Java build.

## What it will do

- **Migrate** — token-range-parallel bulk copy preserving writetimes and TTLs, with counter-table
  support, batching, rate limiting and resumable runs.
- **Validate** — row-by-row comparison with optional autocorrection of missing and mismatched rows,
  and machine-readable discrepancy reports.
- **Guardrail** — flag oversized columns before they become a production problem.
- **Transform along the way** — column renaming and skipping, constant columns, map explosion, JSON
  extraction, type conversion via a pluggable codec registry, and filtering by token range, CQL
  predicate, writetime window or column value.
- **Resume** — every token range is tracked, so an interrupted run restarts exactly where it stopped.
  The tracking tables are schema-compatible with Java CDM, so a run started by either tool can be
  finished by the other.

## Quick start (planned interface)

```bash
# Generate a tuned configuration by introspecting your schema
cdm config init --origin-host origin.example.com \
                --target-host target.example.com \
                --keyspace-table ks.tbl -o cdm.toml

# See exactly what would happen — no data touched
cdm plan --config cdm.toml

# Migrate, with a live terminal UI
cdm migrate --config cdm.toml --tui

# Validate and auto-correct
cdm validate --config cdm.toml --autocorrect-missing --autocorrect-mismatch

# Resume whatever did not finish
cdm runs resume --config cdm.toml --auto

# Serve the control plane, web config builder, metrics and MCP endpoint
cdm serve --config cdm.toml --bind 0.0.0.0:8080
```

### Connecting to each side

Origin and target are configured **independently**, and each picks one of two connection styles.
A migration between self-managed clusters needs no bundle anywhere:

| Origin → Target | Origin | Target |
|---|---|---|
| Cassandra → Cassandra | `--origin-host` | `--target-host` |
| DSE → HCD | `--origin-host` | `--target-host` |
| Cassandra/DSE → Astra | `--origin-host` | `--target-scb` or `--target-astra-database-id` |
| Astra → Astra | `--origin-scb` | `--target-scb` |
| Astra → Cassandra | `--origin-scb` | `--target-host` |

Two things worth being explicit about, because they are easy to conflate:

- **A secure-connect-bundle is Astra DB only.** Apache Cassandra, DSE, HCD and ScyllaDB use
  `--{side}-host`. Setting both a host and a bundle for one side is rejected, naming which to drop.
- **TLS to a self-managed cluster is not a bundle.** A cluster with client encryption uses
  `connect.{side}.tls.*` — truststore, keystore, cipher suites. Different mechanism entirely.

For Astra you can skip the zip: give it the database id and your token, and cdm-rs downloads the
bundle through the DevOps API.

```bash
cdm migrate --config cdm.toml \
            --target-astra-database-id "$ASTRA_DB_ID" \
            --target-password "env:ASTRA_TOKEN"
```

Existing Java configurations work unchanged:

```bash
cdm migrate --properties-file cdm.properties \
            --conf spark.cdm.schema.origin.keyspaceTable=ks.tbl
```

## API-first

The HTTP control plane is described by an OpenAPI 3.1 document that is **generated from the Rust
types**, not hand-maintained. The MCP tool server and the A2A agent card are in turn generated from
that document. Adding a transport means writing an adapter over `CdmService`, never
re-implementing behaviour — and a conformance test asserts CLI, REST, MCP and A2A produce identical
results for the same request.

```text
Rust types ──> OpenAPI 3.1 ──> MCP tools
     │              ├────────> A2A agent card
     │              └────────> client SDKs
     ├────────> JSON Schema ──> config validation + web UI
     ├────────> clap CLI flags
     └────────> generated documentation
```

## Architecture at a glance

Sixteen crates, no cycles, everything behind a trait:

| Layer | Crates |
|---|---|
| Interfaces | `cdm-cli` · `cdm-api` · `cdm-mcp` · `cdm-a2a` · `cdm-ui` |
| Facade | `cdm-service` |
| Execution | `cdm-engine` · `cdm-cluster` · `cdm-track` · `cdm-metrics` |
| Domain | `cdm-feature` · `cdm-cql` · `cdm-codec` · `cdm-config` · `cdm-core` |
| Testing | `cdm-testkit` |

Only `cdm-cql` depends on the CQL driver
([`scylla-rust-driver`](https://github.com/scylladb/scylla-rust-driver)); only `cdm-api` depends on
HTTP. Codecs, features, filters, guardrails, jobs, sources, sinks, tracking stores and metric
exporters are all plugin traits, and the built-in implementations register through the same public
path a third party would use.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full picture, including flow diagrams and
the mapping from every Java class to its Rust home.

## Documentation

| Document | Contents |
|---|---|
| [`docs/SPEC.md`](docs/SPEC.md) | Every requirement, numbered and normative — the contract |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Crate topology, execution model, diagrams, decisions |
| [`docs/TRACEABILITY.md`](docs/TRACEABILITY.md) | Requirement → crate → test → PR matrix, CI-enforced |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | The phased, PR-by-PR delivery plan |
| [`docs/MIGRATION_FROM_JAVA.md`](docs/MIGRATION_FROM_JAVA.md) | Every intentional behavioural difference, and how to restore the old behaviour |
| [`docs/adr/`](docs/adr/) | Architecture decision records |

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) first. In short:

```bash
git clone git@github.com:msmygit/cdm-rs.git && cd cdm-rs
pipx install pre-commit && pre-commit install --install-hooks -t pre-commit -t commit-msg
cargo test --workspace          # unit tests, no cluster needed
cargo xtask it                  # integration tests (needs Docker or Podman)
```

Every pull request maps to one or more requirement IDs from `docs/SPEC.md`, updates
`docs/TRACEABILITY.md`, and ships tests that cite those IDs. CI enforces all three.

## License

Apache License 2.0 — see [`LICENSE`](LICENSE).

cdm-rs is an independent reimplementation. It is not affiliated with or endorsed by DataStax.
"Apache Cassandra" and "Apache Spark" are trademarks of the Apache Software Foundation.
