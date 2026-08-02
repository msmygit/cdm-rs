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
