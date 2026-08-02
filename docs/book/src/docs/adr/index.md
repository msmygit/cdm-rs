# Architecture decision records

Each ADR records one decision that shaped cdm-rs: the context, the alternatives, and the
consequences we accepted. They are immutable — a decision that changes is superseded by a new
ADR rather than edited in place. Numbers are sparse on purpose; gaps are reserved for decisions
whose PR has not landed yet.

| ADR | Decision |
|---|---|
| [ADR-0001](0001-replace-spark-with-native-scheduler.md) | Replace Spark with a native async scheduler |
| [ADR-0002](0002-scylla-rust-driver.md) | Adopt scylla-rust-driver as the sole CQL driver |
| [ADR-0003](0003-lease-based-coordination.md) | Lease-based distributed coordination in the target keyspace |
| [ADR-0004](0004-openapi-as-interface-source-of-truth.md) | OpenAPI generated from Rust types is the interface source of truth |
| [ADR-0005](0005-config-as-one-typed-model.md) | One typed configuration model with generated projections |
| [ADR-0009](0009-astra-secure-connect-bundle.md) | Implement Astra secure-connect-bundle support in `cdm-cql` |
