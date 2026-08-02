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

## Spike findings (PR #2)

The four claims above were asserted from the crate documentation. `crates/cdm-cql/tests/driver_spike.rs`
now verifies them against a real cluster, and the results revised the plan: **three of the four gaps
originally assumed do not exist.**

| Claim | Verdict | Evidence |
|---|---|---|
| Raw column bytes are reachable | **Confirmed** | `FrameSlice::as_slice() -> &'frame [u8]`, reached through a `DeserializeRow` impl that decodes nothing (`cdm_cql::raw::RawRow`). Verified live: an `int` arrives as its four big-endian bytes, so passthrough really is a byte copy. |
| Null and empty are distinguishable | **Confirmed** | `RawColumn::slice` is `Option<FrameSlice>`; empty text is `Some(&[])`, an unwritten column is `None`. Without this, `MIG-012` could not be implemented correctly. |
| `UNSET` can be bound | **Confirmed** | `scylla::value::MaybeUnset`. Verified live: rebinding a row with `MaybeUnset::Unset` left the existing column value intact, where `NULL` would have written a tombstone. |
| All CQL types round-trip | **Confirmed** | 26 columns covering every primitive, `list`/`set`/`map`, `tuple`, a UDT and a three-level nested `map<text, frozen<list<frozen<udt>>>>`, each readable both as raw bytes and as a `CqlValue`. |
| Token-range scans and paging | **Confirmed** | Two adjacent `token(pk)` ranges returned all 500 rows exactly once; a 37-row page size streamed the full result. Cluster metadata exposes the keyspace, so `TOK-001`/`SCH-001` have what they need. |
| `vector<T, N>` needs our own serde | **Wrong — it is native** | `CqlValue::Vector(Vec<CqlValue>)` and `ColumnType::Vector { typ, dimensions }` both exist in `scylla-cql-core` 1.7. No shim required. |

Live verification ran against Cassandra 4.0.4. The `vector` case is version-gated to Cassandra 5.0
and has been verified statically but **not yet at runtime** — that happens in CI, which runs the
full matrix.

## Consequences

The driver covers more than assumed. Only **two** gaps remain for `cdm-cql` to fill:

| Gap | Why | Our approach |
|---|---|---|
| **Astra secure-connect-bundle** | The driver has no Astra SCB support; its `cloud` feature targets Scylla Cloud bundles, whose zip layout and metadata-service handshake differ. | Read the bundle, query the metadata service over mTLS, drive the driver with a custom `AddressTranslator` and per-connection SNI `ServerName`. See `ADR-0009`. |
| **DSE geometry and `DateRangeType`** | DSE-only custom types, absent from `NativeType`. | WKB codecs in `cdm-codec`; the driver only carries bytes. |
| **JKS keystores** | Java CDM accepts `.jks` and there is no JVM to parse it. Not a driver gap as such — nothing in Rust reads JKS. | A pure-Rust JKS/PKCS12/PEM reader in `cdm-cql::tls`. |

`vector<T, N>` is handled natively and needs no work beyond treating it as a collection in the
conversion planner (`CDC-004`).

Because everything sits behind `cdm-core` traits, replacing the driver later means rewriting one
crate rather than the codebase.

### Test matrix

The driver was chosen for its maturity in the Rust ecosystem, not because Scylla is a primary
target. That shapes where the testing effort goes:

- **Cassandra 3.11, 4.0, 4.1 and 5.0 on every pull request.** This is what CDM migrates, and it is
  where the risk lives — 3.11 in particular exercises an older protocol.
- **ScyllaDB nightly.** It is the driver's home turf and the case its maintainers test hardest, so
  it is the least likely to regress. But it is a separate implementation: tablets change token
  ownership dynamically, LWT (which `DST-011` leases depend on) is a different implementation, and
  `WRITETIME()` on collections differs. Claiming support without testing it would be worse than not
  claiming it, and nightly coverage costs one container a day.

## Alternatives considered

- **`cdrs-tokio`.** Less actively maintained, weaker paging and token-awareness.
- **`cassandra-cpp` (bindings to the DataStax C++ driver).** Brings a C dependency and an unsafe FFI
  surface, defeating `SEC-031` and the static-binary goal.
- **Implement the protocol ourselves.** Not justified; the gaps above are days of work, a driver is
  years.
