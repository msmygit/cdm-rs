# ADR-0002: Adopt scylla-rust-driver as the sole CQL driver

- **Status:** Accepted
- **Date:** 2026-08-02
- **Relates to:** `CON-000`, `CON-003`, `CDC-003`, `CDC-004`, `MIG-040`, `TST-002`

## Context

cdm-rs needs a CQL driver that works against Apache Cassandra, DSE, HCD, Astra DB, ScyllaDB and
Cosmos DB Cassandra API. The realistic candidates are
[`scylla-rust-driver`](https://github.com/scylladb/scylla-rust-driver) and `cdrs-tokio`.

The requirements that actually discriminate between them:

1. **Raw column bytes must be reachable.** Zero-copy passthrough (`MIG-040`) — copying serialized
   bytes when origin and target types are identical — is the largest single performance win in the
   design. A driver that only exposes eagerly-deserialized owned values forfeits it.
2. **`UNSET` binding** must be supported, or we create tombstones on every null (`MIG-012`).
3. **Token-aware routing and paging** must be first-class.
4. **Active maintenance**, because we will be filing issues.

## Decision

Use `scylla` (scylla-rust-driver) as the only CQL driver, with features `rustls-023`, `cloud`,
`metrics`, `chrono-04`, `num-bigint-04`, `bigdecimal-04`.

Confine **all** driver usage to the `cdm-cql` crate, behind the `SessionHandle`, `RowSource` and
`RowSink` traits defined in `cdm-core`. No other crate may depend on `scylla`.

## Consequences

The driver is Scylla-first and Cassandra-compatible, which leaves four gaps that `cdm-cql` fills:

| Gap | Why | Our approach |
|---|---|---|
| **Astra secure-connect-bundle** | The `cloud` feature targets Scylla Cloud bundles; Astra's zip layout and metadata-service handshake differ. | Read the bundle, query the metadata service over mTLS, drive the driver with a custom `AddressTranslator` and per-connection SNI `ServerName`. |
| **`vector<T, N>`** | A Cassandra 5 / Astra type absent from Scylla's type system. | `CqlTypeInfo::Vector { element, dimensions }` with our own serde over raw bytes, via the driver's custom-type escape hatch. |
| **DSE geometry and `DateRangeType`** | DSE-only custom types. | WKB codecs in `cdm-codec`; the driver only carries bytes. |
| **JKS keystores** | Java CDM accepts `.jks` and there is no JVM to parse it. | A pure-Rust JKS/PKCS12/PEM reader in `cdm-cql::tls`. |

Because everything sits behind `cdm-core` traits, replacing the driver later means rewriting one
crate rather than the codebase.

`TST-002` runs the integration matrix against Cassandra 4.1, Cassandra 5.0 **and** ScyllaDB. Testing
only one dialect with a Scylla-first driver would let divergences through.

## Alternatives considered

- **`cdrs-tokio`.** Less actively maintained, weaker paging and token-awareness.
- **`cassandra-cpp` (bindings to the DataStax C++ driver).** Brings a C dependency and an unsafe FFI
  surface, defeating `SEC-031` and the static-binary goal.
- **Implement the protocol ourselves.** Not justified; the gaps above are days of work, a driver is
  years.
