# ADR-0009: Implement Astra DB secure-connect-bundle support in `cdm-cql`

- **Status:** Accepted
- **Date:** 2026-08-02
- **Relates to:** `CON-003`, `CON-004`, `CON-005`, `CON-020`–`CON-029`, `ADR-0002`
- **References:**
  [Astra SCB documentation](https://docs.datastax.com/en/astra-db-serverless/databases/secure-connect-bundle.html),
  [connecting legacy drivers without SCB support](https://docs.datastax.com/en/astra-db-classic/drivers/cassandra-drivers-overview.html)

## Context

Astra DB is a first-class origin and target for CDM: Java CDM supports it directly, including
downloading the bundle from the DevOps API. Losing Astra support would disqualify cdm-rs for a large
fraction of its users.

Astra does not expose node addresses. Instead the bundle carries mutual-TLS material and the address
of a **metadata service**; that service returns an **SNI proxy address** plus the list of node
**host ids**. Every CQL connection goes to the one proxy endpoint, and the node it reaches is
selected by the TLS SNI `server_name` — the host id. This is what lets a single public endpoint
address every node in a private cluster independently.

`scylla-rust-driver` (`ADR-0002`) does **not** support this. Its `cloud` feature implements the same
*shape* of mechanism for **Scylla Cloud**, but against a different bundle layout (YAML, different
field names) and a different metadata contract. We cannot point it at an Astra bundle and expect it
to work.

## Decision

Implement Astra SCB support in `cdm-cql`, with two strategies.

### Primary: SNI-aware (`CON-022`)

1. Read the bundle zip in memory. Take `ca.crt`, `cert`, `key` (PEM) and `config.json`. Ignore
   `identity.jks`, `trustStore.jks` and `cert.pfx` — the same material is already present as PEM,
   and parsing a Java keystore to obtain it would be gratuitous.
2. Build a rustls `ClientConfig`: `ca.crt` as the only trust anchor, `(cert, key)` as the client
   identity.
3. `GET https://<config.host>:<config.port>/metadata` over that mTLS connection. The response gives
   `contact_info.local_dc`, `contact_info.contact_points` (host id UUIDs) and
   `contact_info.sni_proxy_address`.
4. Open every CQL connection to `sni_proxy_address`, setting the TLS SNI `server_name` to the target
   node's host id.
5. Install an address translator so that peers discovered through `system.peers` also resolve to the
   proxy, keeping topology discovery coherent.

This preserves everything that matters for a bulk mover: token awareness, per-node load balancing,
and connection pooling across the whole cluster.

### Fallback: single-endpoint mTLS (`CON-026`)

Where the driver does not expose a hook we need — specifically a per-connection `ServerName` on the
TLS connector, or a custom `AddressTranslator` — or where the metadata service is unreachable, fall
back to the method DataStax documents for legacy drivers: connect directly to `config.json`'s `host`
using the port **from `cqlshrc`** (the documentation is explicit that the other ports in the bundle
do not serve CQL), with the same mTLS material.

This works, and it is materially slower: one endpoint means no token-aware routing and no per-node
balancing. It is therefore a fallback with a loud warning (`CON-027`), never a default.

## Implementation findings (PR #7, #8, #9)

The decision above assumed the driver exposed two hooks: a custom `AddressTranslator`, and a TLS
connector able to set a per-connection `ServerName`. Against `scylla` 1.7 — the newest release —
one exists and the other does not.

| Hook | Verdict | Evidence |
|---|---|---|
| Custom `AddressTranslator` | **Exists.** `SessionBuilder::address_translator(Arc<dyn AddressTranslator>)` | `cdm_cql::astra::strategy::ProxyAddressTranslator` implements it. The trait's `#[async_trait]` signature elides the lifetime of `&UntranslatedPeer`, and an implementation must elide it identically: `&UntranslatedPeer<'_>` makes the lifetime early-bound and fails with `E0195`, which is what makes the trait look unimplementable from outside the crate. The bare `&UntranslatedPeer` compiles, at the cost of an `elided_lifetimes_in_paths` allow. |
| Per-connection TLS `ServerName` | **Absent.** | `SessionBuilder::tls_context` takes one `TlsContext` for the whole session. The `TlsProvider`/`TlsConfig` pair that would choose a name per endpoint is `pub(crate)` with a single `GlobalContext` variant — `network/tls.rs` records that its `CloudConfig` variant, which once carried an SNI hostname for Scylla Cloud serverless, has been removed. With `rustls-023`, `network/connection.rs` builds `ServerName::IpAddress(node_address.ip())` itself, and rustls sends no SNI extension for an IP name. |

`CON-000` also lists `cloud` among the required crate features. **There is no `cloud` feature in
`scylla` 1.7**: the list is `rustls-023`, `openssl-010`, `metrics`, the serialization features and a
set of `unstable-*`. The workspace manifest never enabled it, and `CON-023` forbids relying on it
for Astra in any case, so the requirement was stale rather than violated; `SPEC.md` is corrected in
the same change.

**Consequence.** `CON-022`'s primary strategy is not reachable today. cdm-rs implements everything
up to the missing hook — reading the bundle, the mutual-TLS metadata call, the metadata contract and
its rate-limited refresh, local-DC and host-id extraction, and the address translator — and then
selects `CON-026`'s single-endpoint fallback with the warning `CON-027` requires. The selection goes
through one predicate, `driver_supports_per_connection_sni()`, so the day the hook lands the SNI path
turns on with no configuration change and no other edit.

A second local workaround follows from the same gap: rustls' stock verifier would demand an IP SAN
on every server certificate, which no Cassandra or Astra deployment issues. `cdm_cql::tls::verifier`
therefore always verifies the chain against the configured trust store, and checks the *name* only
against a hostname the operator or the bundle supplied — never against the driver's synthetic IP
name. It is removable with the hook.

**Upstream.** The missing hook is worth raising: generic per-connection SNI benefits every Astra user
of `scylla-rust-driver`, not only cdm-rs. The ask is a public equivalent of `TlsProvider` — something
like `SessionBuilder::tls_provider(Arc<dyn TlsProvider>)`, where the provider is handed the endpoint
and returns the `ServerName` to use for it.

## Consequences

**Positive.** Astra works as an origin and a target, including the DevOps API auto-download that
Java CDM users rely on. Because the bundle is read in memory, credentials never linger on disk beyond
the `0700` temp directory used for downloads, which is removed by a `Drop` guard and a signal handler
(`CON-005`).

**Negative.** We own protocol surface that ideally belongs in the driver: the metadata contract is
not formally specified in public documentation, so `CON-021` mandates lenient parsing and
`CON-025` mandates re-fetching when connections fail wholesale. If Astra changes the metadata shape,
we will need to react. Integration tests against a real Astra database are therefore mandatory in the
nightly matrix, not the per-PR one, since they need credentials.

**Upstream.** Where a hook is missing rather than merely inconvenient, raise it with
`scylla-rust-driver` — generic SNI-per-connection support benefits every Astra user of that driver,
not only us. Local workarounds are documented here and marked for removal once upstream lands.

**Diagnostics.** Astra connectivity failures are historically opaque. `CON-029` requires
`cdm connect test` to print the resolved strategy, metadata URL, proxy address, local DC, host-id
count and negotiated TLS parameters, so a failure report is actionable without a packet capture.

## Alternatives considered

- **Use the driver's `cloud` feature and hope.** The bundle layouts and metadata contracts differ;
  this would fail at the first `config.json` read.
- **Ship only the single-endpoint fallback.** Simpler, but it discards token awareness for the
  workload where throughput matters most. Acceptable as a fallback, not as the design.
- **Shell out to a Java helper for the handshake.** Reintroduces the JVM dependency that `ADR-0001`
  exists to remove.
- **Fork the driver.** A maintenance burden out of proportion to two hooks.
