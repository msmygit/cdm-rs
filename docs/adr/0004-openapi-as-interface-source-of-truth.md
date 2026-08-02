# ADR-0004: OpenAPI generated from Rust types is the interface source of truth

- **Status:** Accepted
- **Date:** 2026-08-02
- **Relates to:** `API-001`, `API-002`, `MCP-002`, `A2A-001`, `A2A-005`, `UI-002`, `TST-050`

## Context

cdm-rs must be drivable by humans (CLI, web UI) and by machines (CI, AI agents via MCP, other agents
via A2A). Each transport could be implemented independently — which is how tools accumulate four
subtly different behaviours for the same operation.

## Decision

1. `cdm-service::CdmService` is the **only** place business logic lives. Every adapter is a
   mechanical translation.
2. The OpenAPI 3.1 document is **generated** from the Rust request/response types with `utoipa`, and
   checked in at `api/openapi.yaml`. CI regenerates it and fails on drift (`OPS-012`).
3. MCP tools and the A2A agent card are generated from that document, driven by vendor extensions
   (`x-mcp`, `x-a2a`, `x-cli`) on each operation.
4. `TST-050` asserts that the same logical request issued via CLI, REST, MCP and A2A produces
   identical results.

## Consequences

**Positive.** A new transport is a new adapter crate, not a reimplementation. Client SDKs come free
from the spec. The web UI cannot drift from the API because it consumes the generated schema. Agents
get accurate, structured tool schemas rather than prose.

**Negative.** Adapters are constrained by what the REST shape can express; a transport with genuinely
different semantics would need its own translation layer. Generation puts `utoipa` derives on service
types, coupling them to a specific crate — acceptable, and reversible, since the derives are additive.

**Enforcement.** `oasdiff` gates undeclared breaking changes (`API-008`); schemathesis fuzzes every
endpoint against its schema (`TST-051`).

## Alternatives considered

- **Hand-write the OpenAPI document.** Guarantees drift.
- **Code-generate Rust from a hand-written spec.** Inverts the dependency, and the generated code
  fights the hand-written engine types.
- **Skip OpenAPI; write MCP and A2A directly.** Three hand-maintained schemas that will disagree.
