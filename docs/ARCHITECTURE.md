# cdm-rs — Architecture

| | |
|---|---|
| **Document** | `docs/ARCHITECTURE.md` |
| **Status** | Draft v1 |
| **Normative source** | [`SPEC.md`](./SPEC.md) — this document explains *how*; SPEC defines *what*. |

---

## 1. Architectural principles

| # | Principle | Consequence |
|---|---|---|
| P1 | **One definition, many projections** | The config model, the metric registry, the error catalogue and the API schema each exist exactly once as Rust types. Docs, JSON Schema, OpenAPI, the UI form, MCP tool schemas and CLI flags are all *generated*. |
| P2 | **Core is transport-agnostic** | `cdm-core` knows nothing about HTTP, MCP, A2A or the terminal. Every interface is a thin adapter over the same `CdmService`. |
| P3 | **Everything behind a trait** | Sources, sinks, codecs, features, filters, guardrails, jobs, tracking stores and metric exporters are all trait objects in one registry. Built-ins register through the same public path third parties use. |
| P4 | **Plan once, execute many** | Type conversions, statements, PK extraction and filter chains are resolved into an immutable `ExecutionPlan` at startup. The per-row hot path performs no lookups, no allocation of plans, no string formatting. |
| P5 | **The token range is the unit of everything** | Scheduling, tracking, resume, leasing, metrics attribution and failure isolation all key on the range. |
| P6 | **Bounded by construction** | Every queue, buffer and in-flight set has an explicit bound. There is no configuration that permits unbounded memory growth. |
| P7 | **Errors are data** | A single `Diagnostic` value renders identically as CLI text, `problem+json`, an SSE event and an MCP tool error. |
| P8 | **Compatibility is a feature, not an accident** | Java behavioural quirks that we improve upon are individually restorable via `--compat-java`, and every difference is documented and tested. |

---

## 2. System context

```mermaid
graph LR
    subgraph Users
        CLIU[Operator<br/>terminal]
        WEB[Operator<br/>browser]
        AGENT[AI agent /<br/>orchestrator]
        CI[CI pipeline]
    end

    subgraph cdmrs["cdm-rs process"]
        ADAPT[Adapters<br/>CLI · REST · MCP · A2A · UI]
        CORE[cdm-core<br/>CdmService]
        ENGINE[cdm-engine]
    end

    subgraph Clusters
        ORIGIN[(Origin<br/>Cassandra / DSE / Astra / Scylla)]
        TARGET[(Target<br/>Cassandra / DSE / Astra / Cosmos)]
    end

    subgraph Observability
        PROM[(Prometheus)]
        OTEL[(OTLP collector)]
        LOGS[(Log pipeline)]
    end

    CLIU --> ADAPT
    WEB --> ADAPT
    AGENT -->|MCP / A2A| ADAPT
    CI -->|REST| ADAPT
    ADAPT --> CORE --> ENGINE
    ENGINE -->|read| ORIGIN
    ENGINE -->|write| TARGET
    ENGINE -->|run tracking + leases| TARGET
    ENGINE --> PROM
    ENGINE --> OTEL
    ENGINE --> LOGS
```

---

## 3. Crate topology

Sixteen crates, each independently testable, publishable and reusable. Arrows point from dependant
to dependency; there are **no cycles** and no crate depends on an adapter crate.

```mermaid
graph TD
    CLI[cdm-cli<br/>binary]
    API[cdm-api<br/>axum + utoipa]
    MCP[cdm-mcp]
    A2A[cdm-a2a]
    UI[cdm-ui<br/>embedded assets]

    SVC[cdm-service<br/>CdmService facade]
    ENG[cdm-engine<br/>scheduler · jobs]
    CLUST[cdm-cluster<br/>leases · membership]
    TRACK[cdm-track<br/>run tracking stores]
    FEAT[cdm-feature<br/>features · filters · guardrails]
    CQL[cdm-cql<br/>sessions · statements · schema]
    CODEC[cdm-codec<br/>conversion registry]
    METR[cdm-metrics<br/>counters · exporters · events]
    CONF[cdm-config<br/>model · loaders · validation]
    CORE[cdm-core<br/>types · traits · errors · registry]
    TK[cdm-testkit<br/>dev-dependency]

    CLI --> SVC
    CLI --> API
    CLI --> MCP
    API --> SVC
    API --> UI
    MCP --> SVC
    A2A --> SVC
    API --> A2A

    SVC --> ENG
    SVC --> CONF
    SVC --> TRACK
    SVC --> METR

    ENG --> CLUST
    ENG --> TRACK
    ENG --> FEAT
    ENG --> CQL
    ENG --> METR
    CLUST --> CORE
    CLUST --> CONF
    TRACK --> CQL
    FEAT --> CQL
    FEAT --> CODEC
    CQL --> CODEC
    CQL --> CORE
    CODEC --> CORE
    CONF --> CORE
    METR --> CORE
    TRACK --> CORE
    FEAT --> CORE
    ENG --> CONF
```

### 3.1 Crate responsibilities

| Crate | Responsibility | Key public items |
|---|---|---|
| `cdm-core` | Vocabulary of the domain. Zero I/O. | `TokenRange`, `PartitionRangeId`, `RunId`, `JobKind`, `RunStatus`, `Record`, `PrimaryKey`, `CdmError`, `Diagnostic`, `Registry`, and every plugin trait (`CodecPlugin`, `FeaturePlugin`, `FilterPlugin`, `GuardrailPlugin`, `JobPlugin`, `RowSource`, `RowSink`, `TrackingStore`, `LeaseStore`, `MetricsExporter`). |
| `cdm-config` | The one `CdmConfig` struct tree; loaders for TOML/YAML/JSON/`.properties`/env/CLI/API; the three validation tiers; JSON Schema generation; the best-practice rules engine. | `CdmConfig`, `ConfigLoader`, `Validator`, `Tier`, `PropertyRegistry`, `BestPractices` |
| `cdm-codec` | Type taxonomy, the conversion planner, the codec registry and all built-in codecs. | `CqlTypeInfo`, `ConversionPlan`, `Converter`, `CodecRegistry`, `codecs::*` |
| `cdm-cql` | **The only crate that depends on `scylla`.** Driver wrapper: connection building (TLS/SCB/Astra SNI), schema introspection, identifier quoting, statement construction and binding, paging, token-range CQL, and the compatibility shims of §6.1. | `Cluster`, `Side`, `SessionHandle`, `TableSchema`, `Statements`, `OriginSelect`, `TargetUpsert`, `TargetSelectByPk` |
| `cdm-feature` | All optional behaviours as plugins: constant columns, explode map, extract JSON, TTL/writetime, filters, guardrails. | `feature::*`, `FilterChain`, `GuardrailChain` |
| `cdm-track` | Run/range persistence and resume logic behind `TrackingStore`; Cassandra, SQLite and in-memory implementations. Also implements `LeaseStore` (`DST-010`..`DST-013`), because `cdm_run_leases` lives in the keyspace and beside the tables this crate already owns (`TRK-011`) — and because this is where the one sanctioned `scylla` exception is. | `RunTracker`, `CassandraStore`, `SqliteStore`, `MemoryStore` |
| `cdm-cluster` | Lease acquisition/renewal/expiry, membership, leader election, cross-node metric aggregation. **The policy only:** the conditional writes themselves are `cdm-core`'s `LeaseStore`, implemented by `cdm-track` beside the tracking tables the lease table belongs to (`TRK-011`), so no driver reaches this crate. | `Coordinator`, `Lease`, `NodeId`, `CoordinatorSettings`, `ReclaimPolicy`, `Membership` |
| `cdm-metrics` | Counter registry (parity + new), rate/latency instruments, event bus, Prometheus/OTLP exporters, the Java-format reporter, the TUI. | `Counters`, `CounterKind`, `Instruments`, `EventBus`, `Event`, `JavaFormatReporter`, `Tui` |
| `cdm-engine` | The scheduler, the three built-in jobs, batching, rate limiting, backpressure, retry, failure isolation, graceful shutdown. | `Engine`, `ExecutionPlan`, `RangeWorker`, `jobs::{Migrate, Validate, Guardrail}` |
| `cdm-service` | The transport-agnostic facade every adapter calls. Owns run lifecycle, idempotency, and the run registry. | `CdmService`, `SubmitRunRequest`, `RunHandle`, `RunView`, `PlanView`, `SchemaView` |
| `cdm-api` | axum HTTP server; `utoipa` OpenAPI generation; SSE; auth; problem+json; static UI mounting. | `serve()`, `ApiDoc` |
| `cdm-mcp` | MCP server (stdio + Streamable HTTP), tools/resources/prompts generated from `ApiDoc`. | `McpServer` |
| `cdm-a2a` | Agent card generation and A2A task adapter. | `AgentCard`, `A2aServer` |
| `cdm-ui` | The Config Builder + run monitor SPA, embedded via `rust-embed`. | `assets()` |
| `cdm-cli` | Argument parsing (`clap` derive, generated from the config model), output rendering, exit codes, the `cdm` binary. | `main` |
| `cdm-testkit` | Containers, generators, assertions, mock sessions. Dev-dependency only. | `OriginTarget`, `SchemaGen`, `DataGen`, `assert_counters!` |

### 3.2 Why this decomposition

* **`cdm-core` has no dependencies on I/O crates**, so plugin authors can implement traits without
  pulling in axum, the driver, or Tokio's full feature set.
* **`cdm-service` sits between the engine and every adapter**, which is what makes `TST-050`
  (identical behaviour across CLI/REST/MCP/A2A) structurally true rather than aspirational.
* **`cdm-config` does not depend on `cdm-cql`**; Tier-3 (schema-bound) validation is expressed as a
  trait (`SchemaProvider`) that `cdm-cql` implements. This keeps config parsing testable without a
  cluster and keeps the dependency graph acyclic.
* **`cdm-codec` does not depend on `cdm-cql`**; it operates on a driver-independent `CqlTypeInfo`
  and raw byte buffers, so codecs are unit-testable with no session and remain valid if the
  underlying driver is ever swapped.
* **`cdm-cql` and `cdm-engine` do not depend on `cdm-metrics`**, and `MET-010`'s per-request
  measurements still come from them, because the only place a request exists is the crate that
  issues it. The seam is `cdm_core::observe::RequestObserver` — the same shape as `SchemaProvider`
  above — which `cdm_metrics::Instruments` implements: `cdm-cql` times a driver request and
  `cdm-engine` reports a rate-limiter wait against `dyn RequestObserver`, and `cdm-cli` is the
  crate that holds both ends and joins them. `Operation` and `RetryCause` live in `cdm-core` for
  the same reason and are re-exported by `cdm-metrics`. **Do not add a `cdm-cql → cdm-metrics`
  edge** to "simplify" this: it puts the metric registry underneath the driver and is the change
  the trait exists to prevent.

### 3.3 The one edge that is optional: `cdm-testkit --features macrobench`

The graph above is the graph of a **default** build, and `cdm-testkit` sits at the bottom of it:
no driver, no engine, only `cdm-core`, `cdm-codec` and `cdm-metrics`. That is what lets
`cdm-cql`, `cdm-engine` and `cdm-track` all dev-depend on it without dragging the world into
every test build.

`TST-060`'s **tier-2 macro-benchmark** (`docs/BENCHMARKS.md` §1) is the single exception, and it
is a genuine one rather than a convenience. It measures rows per second for a whole migration
between two containerised clusters, which is only a measurement of cdm-rs if it drives the
shipping code: `cdm-engine`'s scheduler and migrate job, `cdm-cql`'s executor and statements,
`cdm-config`'s model. A harness built on a hand-rolled copy loop would report a number about the
harness.

```
cdm-testkit --features macrobench  -->  cdm-engine, cdm-cql, cdm-config
```

Three properties make this acceptable rather than a hole in the rule:

* **It is off by default.** Nothing in a default `cargo build`, `cargo test` or any other crate's
  dev-dependency resolution sees these edges. The graph in §3 is unchanged for every consumer
  that does not ask.
* **The resulting cycle is a dev-dependency cycle, which Cargo permits.**
  `cdm-engine → (dev) cdm-testkit → cdm-engine` is not a cycle in any crate's own build; only the
  test targets close it. `cargo check --workspace --all-targets --all-features` is the standing
  proof, and it is in CI.
* **It is confined to one module.** `crates/cdm-testkit/src/macrobench.rs` is the only file
  behind the feature. Nothing else in the crate may use `cdm-cql`, `cdm-engine` or `cdm-config`,
  feature or no feature.

**Do not widen it.** In particular, do not make the feature a default, and do not reach for
`cdm-cql`'s session from the rest of `cdm-testkit` because it is now nominally available: the
`TestSession` seam exists precisely so the fixtures stay driver-free, and `crates/cdm-cql/tests/
testkit_fixture.rs` is where the two halves are meant to meet.

The alternative considered and rejected was a separate `cdm-bench` crate. It would keep the graph
literally untouched, at the cost of a sixteenth-plus-one crate whose entire contents are one
module, and of splitting the container fixtures from the only other thing that starts containers.
If tier 3 (`NFR-004`'s Java comparison) ever needs a home in the workspace, that trade reverses
and the crate should be created then.

---

## 4. Configuration pipeline

```mermaid
flowchart LR
    D[Defaults] --> M
    F["File<br/>toml/yaml/json/properties"] --> M
    E["Env CDM__*"] --> M
    C["CLI --set / --conf"] --> M
    T["Typed CLI flags"] --> M
    A["API request body"] --> M
    M[Layered merge<br/>figment] --> R["Secret resolution<br/>env:/file:/exec:"]
    R --> V1[Tier 1<br/>syntactic]
    V1 --> V2[Tier 2<br/>semantic cross-field]
    V2 --> V3[Tier 3<br/>schema-bound]
    V3 --> EC["EffectiveConfig<br/>immutable, hashed"]
    EC --> PLAN[ExecutionPlan]

    V1 -.-> DIAG[["Vec&lt;Diagnostic&gt;<br/>all violations, never fail-fast"]]
    V2 -.-> DIAG
    V3 -.-> DIAG
```

The `EffectiveConfig` is hashed (secrets excluded) to produce a `config_hash` used for
distributed-mode consistency checking (`DST-003`) and recorded in every run summary.

### 4.1 Single-source property definition

```rust
/// Performance and operational tuning.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, CdmProperties)]
pub struct PerfOps {
    /// Number of token-range splits the ring is divided into.
    ///
    /// Rule of thumb: `table_size / 10 MB`.
    #[cdm(legacy = "spark.cdm.perfops.numParts", default = 5000, min = 1)]
    pub num_parts: u64,

    /// Rows written per second, per node.
    #[cdm(legacy = "spark.cdm.perfops.ratelimit.target", default = 20_000, unit = "rows/s")]
    pub ratelimit_target: u32,
    // ...
}
```

The `CdmProperties` derive emits: the legacy-alias table used by the `.properties` loader, the
documentation row for `docs/generated/PROPERTIES.md`, the `clap` flag, and the UI form descriptor.
`JsonSchema` gives the config schema; `ToSchema` gives the OpenAPI component. **One struct, six
artefacts** — this is principle P1 in practice.

---

## 5. Execution model

### 5.1 Startup sequence

```mermaid
sequenceDiagram
    autonumber
    participant U as CLI / API
    participant S as CdmService
    participant C as cdm-config
    participant Q as cdm-cql
    participant P as Planner
    participant T as cdm-track
    participant E as Engine

    U->>S: submit_run(job, config)
    S->>C: load + merge + resolve secrets
    C->>C: Tier 1 + Tier 2 validation
    S->>Q: connect(origin), connect(target)
    Q-->>S: sessions + cluster capabilities
    S->>C: Tier 3 validation (SchemaProvider = cdm-cql)
    C-->>S: Vec<Diagnostic> (must be empty)
    S->>P: build ExecutionPlan
    Note over P: resolve column mapping,<br/>conversion plans, PK extractors,<br/>filter chain, statements,<br/>token-range list
    P-->>S: ExecutionPlan (immutable, Arc)
    S->>T: init_run(run_id, ranges) / resume(prev_run_id)
    T-->>S: work list
    S->>E: run(plan, work_list)
    E-->>U: RunHandle (id, events, metrics)
```

### 5.2 Scheduler

```mermaid
graph TD
    WL["Work list<br/>shuffled ranges"] --> CL{cluster.enabled?}
    CL -->|no| Q[In-process<br/>MPMC queue]
    CL -->|yes| CO["Coordinator<br/>LWT lease claim"]
    CO --> Q
    Q --> W1[Worker 1]
    Q --> W2[Worker 2]
    Q --> WN[Worker N]
    W1 --> RP[Range pipeline]
    W2 --> RP
    WN --> RP
    RP --> TR[TrackingStore<br/>async batched]
    RP --> MX[Counters + events]
```

`perfops.workers` defaults to the CPU count. Workers are Tokio tasks on a multi-threaded runtime;
each owns one range at a time and drives an independent read→transform→write pipeline. Work stealing
is implicit: a free worker takes the next range, so straggler ranges do not idle the fleet.

### 5.3 Per-range pipeline (migrate)

```mermaid
flowchart TD
    A["Paged read<br/>SELECT ... WHERE TOKEN(pk) BETWEEN ? AND ?"] --> RL1[Origin rate limiter]
    RL1 --> CNT1[["READ++"]]
    CNT1 --> PK[Build target PK<br/>+ TTL/writetime]
    PK --> FLT{Filter chain}
    FLT -->|reject| SK[["SKIPPED++"]]
    FLT -->|accept| EXP[Explode map<br/>1 row -> N records]
    EXP --> BIND[Bind statement<br/>via ConversionPlan]
    BIND -->|null statement| SK
    BIND --> CTR{counter table?}
    CTR -->|yes| RD[Target SELECT by PK<br/>compute delta]
    RD --> BATCH
    CTR -->|no| BATCH{batch_size &gt; 1?}
    BATCH -->|yes| ACC["Accumulate UNLOGGED batch<br/>grouped by partition key"]
    BATCH -->|no| SEND
    ACC --> SEND[Rate-limited async execute<br/>bounded by in-flight semaphore]
    SEND --> UNF[["UNFLUSHED++"]]
    UNF --> FL{UNFLUSHED &ge; flush_threshold?}
    FL -->|yes| FLUSH["Await in-flight<br/>WRITE += n; UNFLUSHED = 0"]
    FL -->|no| A
    FLUSH --> A
    A -->|range exhausted| FIN["Final flush<br/>PARTITIONS_PASSED++<br/>mark PASS + metrics"]
```

`flush_threshold = min(fetch_size, max(batch_size × 10, 100))` — identical to Java.

### 5.4 Per-range pipeline (validate)

```mermaid
flowchart TD
    A[Paged origin read] --> RL[Origin rate limiter]
    RL --> CNT[["READ++"]]
    CNT --> PK[Build target PK]
    PK --> FLT{Filter chain}
    FLT -->|reject| SK[["SKIPPED++"]]
    FLT -->|accept| ASY["Async target SELECT by PK<br/>rate-limited, pipelined"]
    ASY --> BUF["Buffer<br/>len &le; fetch_size"]
    BUF --> CMP{Buffer full or<br/>range exhausted?}
    CMP -->|no| A
    CMP -->|yes| DIFF[Compare batch]
    DIFF --> M1{target row null?}
    M1 -->|yes| MISS[["MISSING++"]] --> AC1{autocorrect.missing<br/>&amp; counter rules ok?}
    AC1 -->|yes| UPS1[Upsert] --> CM[["CORRECTED_MISSING++"]]
    AC1 -->|no| NEXT
    M1 -->|no| COL["Per-column compare<br/>target -> origin type space"]
    COL --> M2{any difference?}
    M2 -->|no| VAL[["VALID++"]]
    M2 -->|yes| MM[["MISMATCH++<br/>log to cdm_diff sink<br/>emit Discrepancy event"]]
    MM --> AC2{autocorrect.mismatch?}
    AC2 -->|yes| UPS2[Upsert] --> CMM[["CORRECTED_MISMATCH++"]]
    AC2 -->|no| NEXT[Next record]
    VAL --> NEXT
    CM --> NEXT
    CMM --> NEXT
```

### 5.5 The hot path is allocation-light

Resolved once into `ExecutionPlan`, never per row:

| Resolved artefact | Type |
|---|---|
| Origin projection and prepared select | `PreparedStatement` |
| Target upsert (insert or counter update) | `PreparedStatement` |
| Column index mapping origin↔target | `Vec<Option<usize>>` |
| Per-column conversion | `Vec<ConversionPlan>` |
| PK extractor | `PkExtractor` (closure-free, index-driven) |
| Filter chain | `Vec<Box<dyn Filter>>` |
| TTL/writetime source indexes | `Vec<usize>` |
| Constant column literals | pre-rendered CQL fragment |

Per row we do: one iteration over target column indexes, a `ConversionPlan` dispatch (usually
`Passthrough`, which copies a `&[u8]` slice), and one bind. No hashing by column name, no
`format!`, no per-row `Vec` growth beyond the bind buffer, which is reused.

---

## 6. Type conversion architecture

```mermaid
flowchart TD
    subgraph Startup
        OT[Origin column type] --> PLAN{Planner}
        TT[Target column type] --> PLAN
        PLAN -->|identical| PT[Passthrough<br/>copy raw bytes]
        PLAN -->|assignable repr| PT
        PLAN -->|registered codec| CO[Codec conversion]
        PLAN -->|both UDT| UD[Field-wise plan<br/>recursive]
        PLAN -->|same-kind collection| CC[Element-wise plan<br/>recursive]
        PLAN -->|tuple| TU[Positional plan<br/>recursive]
        PLAN -->|otherwise| UN["Unsupported<br/>pass through + warn ONCE"]
    end

    subgraph "Per row"
        PT --> OUT[Bound value]
        CO --> OUT
        UD --> OUT
        CC --> OUT
        TU --> OUT
        UN --> OUT
    end
```

### 6.1 Driver: `scylla-rust-driver` and its compatibility shims

The CQL driver is [`scylla`](https://github.com/scylladb/scylla-rust-driver) — async, Tokio-native,
token-aware, shard-aware, with a well-typed serialization layer (`SerializeValue` /
`DeserializeValue`) that maps cleanly onto our conversion planner. Crucially it exposes raw column
bytes, which is what makes zero-copy passthrough (`MIG-040`) possible; a driver that eagerly
deserializes into owned values would forfeit the largest single performance win in the design.

It is a **Scylla-first** driver used against Cassandra/DSE/Astra, so `cdm-cql` owns four shims. All
four are confined to that crate, and each has a dedicated test module:

```mermaid
graph TD
    APP["cdm-engine / cdm-feature<br/>(driver-agnostic)"] --> IF{{"SessionHandle · RowSource · RowSink<br/>cdm-core traits"}}
    IF --> CQL[cdm-cql]
    CQL --> DRV["scylla<br/>features: rustls-023, cloud, metrics,<br/>chrono-04, num-bigint-04, bigdecimal-04"]
    CQL --> S1["shim: Astra SCB<br/>bundle reader + metadata service<br/>+ AddressTranslator + per-node SNI"]
    CQL --> S2["shim: vector&lt;T,N&gt;<br/>custom type serde"]
    CQL --> S3["shim: DSE geo + DateRange<br/>WKB codecs (in cdm-codec)"]
    CQL --> S4["shim: JKS / PKCS12 / PEM<br/>pure-Rust keystore reader"]
```

| Shim | Why | Approach |
|---|---|---|
| **Astra SCB** (`CON-003`, `CON-020`–`CON-029`) | The driver has **no** Astra SCB support. Its `cloud` feature implements the same *shape* of mechanism for Scylla Cloud, but against a different bundle layout and metadata contract. | Read the zip in memory, call the metadata service over mTLS, then drive the driver with a custom `AddressTranslator` and a per-connection TLS `ServerName` set to the node's host id. Detailed below and in `ADR-0009`. |
| **`vector<T,N>`** (`CDC-004`) | Cassandra 5 / Astra vector type is absent from Scylla's type system, so the driver surfaces it as a custom/unknown type. | `CqlTypeInfo::Vector { element, dimensions }` with our own serde over the raw bytes (fixed-width elements are a contiguous array; variable-width use the collection framing). Comparison is exact-bit, per `CDC-004`. |
| **DSE geometry + `DateRangeType`** (`CDC-003`) | DSE-only custom types. | WKB encode/decode implemented as ordinary `CodecPlugin`s in `cdm-codec`; the driver just carries bytes. |
| **JKS keystores** (`CON-006`) | Java CDM accepts `.jks`; there is no JVM to parse it. | Pure-Rust JKS reader (JCEKS/JKS magic, SHA-1 keyed digest verification, PKCS#8 extraction) plus PKCS#12 and PEM readers, feeding rustls. |

Everything above sits behind `cdm-core` traits, so replacing the driver later means rewriting one
crate, not the codebase. `TST-002` runs the full integration matrix against real Cassandra 4.1/5.0
**and** ScyllaDB to keep both dialects honest.

#### Astra secure-connect-bundle flow

Astra never exposes node addresses. The bundle carries mTLS material plus the address of a metadata
service; that service returns an SNI proxy address and the cluster's host ids. Every connection goes
to the one proxy endpoint, and the node it lands on is selected by the TLS SNI `server_name`. This is
the mechanism `scylla-rust-driver` lacks, so `cdm-cql` implements it.

```mermaid
sequenceDiagram
    autonumber
    participant C as cdm-cql
    participant Z as secure-connect-*.zip
    participant M as Astra metadata service
    participant P as SNI proxy
    participant N as Astra nodes

    alt SCB path not configured
        C->>C: POST DevOps API /v2/databases/{id}/secureBundleURL?all=true
        Note over C: select by scb_type, region, custom_domain (CON-004)<br/>download to 0700 temp dir, Drop guard removes it (CON-005)
    end

    C->>Z: read in memory
    Z-->>C: config.json, ca.crt, cert, key, cqlshrc
    Note over C: ignore identity.jks / trustStore.jks / cert.pfx —<br/>same material already present as PEM (CON-020)

    C->>C: rustls ClientConfig<br/>trust anchor = ca.crt, client identity = (cert, key)

    alt Primary — SNI-aware (CON-022)
        C->>M: GET https://{config.host}:{config.port}/metadata (mTLS)
        M-->>C: { local_dc, contact_points: [host-id…], sni_proxy_address }
        loop per node
            C->>P: TLS connect to sni_proxy_address<br/>server_name = <host-id>
            P->>N: route by SNI
        end
        Note over C: AddressTranslator maps system.peers entries → proxy (CON-024)<br/>local_dc drives the LB policy (CON-009)
    else Fallback — single endpoint (CON-026)
        C->>C: host from config.json, port from cqlshrc
        C->>N: direct mTLS connect
        Note over C: WARN: no token-aware routing,<br/>materially lower throughput (CON-027)
    end
```

Two details are easy to get wrong and are therefore normative: the CQL port comes from `cqlshrc`,
**not** from `config.json` (the other ports in the bundle do not serve CQL), and the metadata
response must be re-fetched — rate-limited — when every connection fails, because
`sni_proxy_address` can change (`CON-025`).

---

`ConversionPlan` is a recursive enum; nesting is resolved at startup so a
`map<text, frozen<list<udt>>>` conversion costs one dispatch chain per row with no type inspection.

The **codec registry** is keyed by `(from: CqlTypeInfo, to: CqlTypeInfo)` and populated by
`CodecPlugin`s. `BIGINT_BIGINTEGER` is always registered (needed to read collection writetimes);
the rest are opt-in via `transform.codecs`, exactly as in Java.

---

## 7. Run tracking and resume

### 7.1 State machine

```mermaid
stateDiagram-v2
    [*] --> NOT_STARTED : plan created
    NOT_STARTED --> STARTED : worker claims range
    STARTED --> PASS : migrate ok / validate clean
    STARTED --> DIFF : validate found discrepancies
    STARTED --> DIFF_CORRECTED : all discrepancies auto-corrected
    STARTED --> FAIL : error / panic / lease lost
    FAIL --> STARTED : rerun
    DIFF --> STARTED : rerun
    NOT_STARTED --> STARTED : rerun
    PASS --> [*]
    DIFF_CORRECTED --> [*]
```

Run-level status mirrors this and terminates at `ENDED` (Java parity), with cdm-rs adding
`INTERRUPTED` and `ABORTED` on the run row only.

### 7.2 Resume

`auto_rerun` selects the newest run for `(table_name, run_type)` and adopts it if it did not reach
`ENDED`, or if its `run_info` reports `Partitions Failed: N > 0`. Pending ranges are those in
`{NOT_STARTED, STARTED, FAIL, DIFF}`. `rerun_multiplier` subdivides each pending range to break up
stragglers. If the previous run's info row is missing or `NOT_STARTED`, we fall back to a fresh full
plan with a warning — matching Java's `RunNotStartedException` path.

Resume correctness rests on migrate being **idempotent**: upserts carry the origin's writetime, so
re-writing a row is a no-op at the storage layer. The two exceptions are documented and handled:
unfrozen lists (mitigated by `transform.custom_writetime_increment`, warned about by `CFG-039`) and
counters (`DST-015`).

### 7.3 Pluggable stores

```mermaid
graph LR
    RT[RunTracker] --> TS{{TrackingStore trait}}
    TS --> CS["CassandraStore<br/>cdm_run_info / cdm_run_details<br/>Java-schema-compatible"]
    TS --> SS["SqliteStore<br/>local file, no target writes"]
    TS --> MS["MemoryStore<br/>tests"]
```

Writes are enqueued to a bounded channel and flushed in batches by a dedicated task, so tracking can
never become the throughput bottleneck. On overflow the tracker degrades to periodic checkpoints and
logs a warning rather than applying backpressure to data movement.

---

## 8. Distributed coordination

```mermaid
sequenceDiagram
    autonumber
    participant N1 as Node 1
    participant N2 as Node 2
    participant T as Target keyspace<br/>(coordination substrate)

    N1->>T: INSERT cdm_run_info IF NOT EXISTS
    T-->>N1: applied = true  (N1 is initialiser)
    N2->>T: INSERT cdm_run_info IF NOT EXISTS
    T-->>N2: applied = false (joins)
    N1->>T: write config_hash, insert range rows NOT_STARTED, status STARTED
    N2->>T: read config_hash -> compare -> ok

    loop until no unclaimed ranges
        N1->>T: UPDATE cdm_run_leases SET node_id=N1, lease_until=now+60s<br/>IF node_id=NULL OR lease_until < now
        T-->>N1: applied
        N1->>N1: process range
        par heartbeat every 15s
            N1->>T: renew lease
        end
        N1->>T: mark range PASS + metrics, then DELETE the lease
    end

    Note over N1: Node 1 dies mid-range
    N2->>T: lease_until expired -> claim, attempt += 1
    alt counter table
        N2->>T: mark range FAIL "manual reconciliation required"
    else idempotent upserts
        N2->>N2: reprocess range from scratch (safe)
    end
```

Design notes:

* **No external dependency.** Coordination lives in the target keyspace, which is already required.
  No ZooKeeper, etcd, Raft, or message broker.
* **LWT cost is bounded.** One LWT per range claim and one per renewal cycle — not per row. With the
  default 5000 ranges and 60 s leases, this is negligible next to the data traffic.
* **Failure is the normal case.** A node dying is indistinguishable from a slow node; leases handle
  both. `cluster.max_attempts` prevents infinite reclaim loops.
* **Counters are explicitly excluded** from safe reclaim, because counter updates are not idempotent.
  We fail loudly rather than corrupt silently.
* **Metrics aggregate through the same substrate**: each node checkpoints its counter snapshot; the
  node that observes the final range completing prints the Java-format summary.

---

## 9. Metrics and event architecture

```mermaid
graph LR
    subgraph "Hot path"
        W[Workers] -->|lock-free atomics| CT[Counter registry]
        W -->|histogram| IN[Instruments]
        W -->|non-blocking send| EB[Event bus<br/>broadcast channel]
    end

    CT --> AGG[Aggregator<br/>per-range -> per-run]
    IN --> AGG
    AGG --> PROMEXP["/metrics<br/>Prometheus"]
    AGG --> OTLP[OTLP exporter]
    AGG --> JAVA["Java-format<br/>final block"]
    AGG --> TRACKW[run_info / run_details]
    AGG --> API1["GET /v1/runs/{id}/metrics"]

    EB --> SSE["GET /v1/runs/{id}/events<br/>SSE"]
    EB --> NDJ[NDJSON file / stdout]
    EB --> TUI[Terminal UI]
    EB --> DIFFLOG[cdm_diff.log]
    EB --> REPORT[Discrepancy report<br/>json / csv / parquet]
```

Counters are `AtomicU64` in a fixed-size array indexed by a `CounterKind` enum — no map lookup, no
lock, no contention beyond the cache line. The two-level interim/committed model from Java is
preserved because per-range `run_info` strings depend on it.

The event bus is a bounded `tokio::sync::broadcast`. Slow consumers lag and are told so; they never
apply backpressure to the data path.

---

## 10. Interface architecture — one core, many transports

```mermaid
graph TD
    subgraph "Generated from Rust types"
        TYPES["cdm-service request/response types<br/>+ cdm-config CdmConfig"]
        TYPES --> OAPI["api/openapi.yaml<br/>OpenAPI 3.1"]
        TYPES --> JSCH["schema/cdm-config.schema.json"]
        OAPI --> MCPT["MCP tool definitions"]
        OAPI --> A2AC["A2A agent card skills"]
        OAPI --> SDK["Client SDKs<br/>(openapi-generator)"]
        TYPES --> CLAP["clap CLI flags"]
        TYPES --> FORM["UI form descriptors"]
        TYPES --> DOCS["docs/generated/*.md"]
    end

    subgraph Adapters
        REST[cdm-api] --> SVC
        MCPS[cdm-mcp] --> SVC
        A2AS[cdm-a2a] --> SVC
        CLII[cdm-cli] --> SVC
        UIA[cdm-ui] -->|fetch| REST
    end

    SVC[CdmService<br/>the only business logic] --> ENG[cdm-engine]
```

**This is the answer to "extensible to MCP, A2A, etc."**: adapters are mechanical translations. A new
transport (gRPC, AsyncAPI over Kafka, a Slack bot) is a new crate that calls `CdmService` and, if it
needs a schema, reads the generated OpenAPI document. `TST-050` asserts all adapters agree.

### 10.1 OpenAPI generation and drift control

```text
cargo xtask openapi          # regenerate api/openapi.yaml + schema/*.json
cargo xtask openapi --check  # CI: fail if the checked-in files drift
oasdiff breaking base.yaml api/openapi.yaml   # CI: fail on undeclared breaking changes
```

Operations carry vendor extensions consumed by the generators:

```yaml
x-mcp:
  expose: tool
  destructive: true          # -> MCP destructiveHint
  idempotent: true           # -> MCP idempotentHint
x-a2a:
  skill: run-migration
x-cli:
  command: "runs resume"
```

### 10.2 Run lifecycle as seen by every transport

```mermaid
stateDiagram-v2
    [*] --> pending : POST /v1/runs (202)
    pending --> planning
    planning --> running
    running --> paused : POST :pause
    paused --> running : POST :resume
    running --> succeeded
    running --> failed
    running --> cancelled : POST :cancel
    running --> interrupted : SIGTERM
    succeeded --> [*]
    failed --> [*]
    cancelled --> [*]
    interrupted --> [*]
```

---

## 11. Plugin architecture

```rust
pub trait FeaturePlugin: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    /// Contribute config keys; they flow into JSON Schema, OpenAPI, docs and the UI.
    fn config_schema(&self) -> Option<schemars::Schema> { None }
    /// Tier 2 + Tier 3 participation.
    fn validate(&self, cfg: &EffectiveConfig, schema: &SchemaPair) -> Vec<Diagnostic>;
    fn is_enabled(&self, cfg: &EffectiveConfig) -> bool;
    /// Contribute extra origin projection columns (e.g. TTL(col), WRITETIME(col)).
    fn extend_origin_projection(&self, _b: &mut ProjectionBuilder) {}
    /// Contribute target columns / literals.
    fn extend_target_binding(&self, _b: &mut BindingBuilder) {}
    /// Transform one origin record into zero or more output records.
    fn transform(&self, _rec: Record, _out: &mut RecordSink) -> Result<(), CdmError> { Ok(()) }
    /// Participate in validate comparison (e.g. skip constant columns).
    fn compare_hook(&self) -> Option<&dyn CompareHook> { None }
}
```

All ten built-in features implement this trait and are registered through the public
`Registry::register_feature` — there is no privileged internal path. The registry is built once at
startup:

```rust
let registry = Registry::builder()
    .with_builtin_codecs()      // CDC-020
    .with_builtin_features()    // FEA-*
    .with_builtin_jobs()        // migrate, validate, guardrail
    .with_plugins(cfg.plugins()) // PLG-011, opt-in
    .build()?;
```

Registration order is deterministic; conflicting registrations are a startup error naming both
providers.

---

## 12. Concurrency and memory model

| Concern | Mechanism | Bound |
|---|---|---|
| Worker parallelism | Tokio multi-thread runtime, `perfops.workers` tasks | configured |
| In-flight origin pages | semaphore | `perfops.max_inflight_reads` |
| In-flight target writes | semaphore | `perfops.max_inflight_writes` |
| Validate compare buffer | per-worker `Vec` | `perfops.fetch_size` records |
| Batch accumulation | per-worker map keyed by partition token | `perfops.batch_size` × distinct partitions in flight |
| Tracking writes | bounded mpsc + batching task | 4096 entries, then checkpoint-degrade |
| Event bus | bounded broadcast | 8192 events, slow consumers lag |
| Discrepancy report | streaming writer, never buffered whole | O(1) |

Steady-state RSS is therefore `base + workers × (fetch_size × row_size + inflight × row_size)` and is
computable from the config — which is what `NFR-003` requires and what `cdm plan` prints.

Blocking work (JKS parsing, file I/O, SCB extraction) runs on `spawn_blocking`; the async runtime is
never blocked.

---

## 13. Error handling and failure isolation

```mermaid
flowchart TD
    E[Error in worker] --> K{Kind}
    K -->|Config / Connect / Auth / Tls / SchemaMismatch / SchemaChanged| FATAL["is_fatal: never transient<br/>ENG-015"]
    K -->|Read / Write timeout, Unavailable, Overloaded| RETRY{attempts left<br/>&amp; idempotent?}
    RETRY -->|yes| BACK[Backoff + jitter, retry]
    RETRY -->|no| RANGEFAIL
    K -->|TypeConversion on one column| RECORD["Record-level: count ERROR,<br/>log PK + column, continue"]
    K -->|Panic| CATCH["Catch at range boundary<br/>ENG-013: contained, not fatal"] --> RANGEFAIL
    FATAL --> RANGEFAIL
    RANGEFAIL["Range FAIL<br/>PARTITIONS_FAILED++<br/>ERROR += read - done - skipped<br/>mark FAIL in tracking"] --> LIMIT{ERROR &gt; error_limit?}
    FATAL --> DRAIN
    LIMIT -->|yes| DRAIN["Graceful abort<br/>drain in-flight, mark ABORTED"]
    LIMIT -->|no| NEXT[Next range]
```

Three isolation levels — record, range, run — mean a single bad row cannot fail a range, and a single
bad range cannot fail a run. Everything a failed range touched is re-runnable, because the range is
the tracking unit.

The fatal path is not a fourth level but a short circuit through the third: the range is still marked
`FAIL` and still accounted for, and what changes is only that no further range is claimed. The
distinction is easiest to see in what each abort *means*. An error-limit abort says a lot of rows
failed; a fatal abort says one thing about the run is wrong, and would say it again for every
remaining range if isolation were applied to it. A panic is the deliberate exception: `ENG-013`
contains it even though it surfaces as `Internal`, because a payload caught inside `catch_unwind`
tells you about one range and not about the run.

---

## 14. Security architecture

* Secrets are `Secret<String>` throughout; `Debug`, `Display` and `Serialize` all emit `***`.
  Resolution (`env:`/`file:`/`exec:`) happens once, at load, into memory that is zeroized on drop.
* The control plane defaults to loopback with no auth, and **refuses** to bind a non-loopback address
  without either auth configured or an explicit `insecure_allow_remote` opt-in.
* MCP and A2A share the API's auth layer — there is no second, weaker door.
* `#![forbid(unsafe_code)]` workspace-wide.
* Supply chain gates (`cargo-deny`, `cargo-audit`, gitleaks, CodeQL, SBOM, signed artefacts) run in
  CI on every PR and every release.
* The container image is distroless, non-root, and contains only the binary.

---

## 15. Deployment topologies

```mermaid
graph TB
    subgraph "A. Single binary, single node (default)"
        A1["cdm migrate --config cdm.toml"] --> A2[(origin)]
        A1 --> A3[(target)]
    end

    subgraph "B. Single node + control plane"
        B1["cdm migrate --serve --tui"] --> B2["REST + SSE + /metrics + /ui"]
        B2 --> B3[Prometheus / Grafana]
        B2 --> B4[AI agent via MCP]
    end

    subgraph "C. Distributed run"
        C1[node-1] --- C4[(target keyspace<br/>leases + tracking)]
        C2[node-2] --- C4
        C3[node-3] --- C4
        C1 --> C5[(origin)]
        C2 --> C5
        C3 --> C5
    end

    subgraph "D. Controller / worker (Kubernetes)"
        D1["cdm serve<br/>Deployment"] -->|submit runs| D2["cdm worker Job<br/>replicas=N, cluster.enabled=true"]
        D2 --- D3[(target keyspace)]
    end
```

All four topologies run the **same binary** with different flags. There is no separate driver
artefact, no executor artefact, and no cluster manager to install.

---

## 16. Architecture decision records

ADRs live in `docs/adr/` and are referenced from requirements. Planned for PR #1–#3:

| ADR | Decision |
|---|---|
| `ADR-0001` | Replace Spark with a native Tokio scheduler; token range remains the unit of work. |
| `ADR-0002` | Adopt `scylla-rust-driver` as the sole CQL driver; confine it to `cdm-cql`; implement the four compatibility shims (Astra SNI, `vector<>`, DSE geo/date-range, JKS). |
| `ADR-0003` | Lease-based distributed coordination in the target keyspace; no external coordinator. |
| `ADR-0004` | OpenAPI generated from Rust types as the single interface source of truth. |
| `ADR-0005` | Config as one typed struct tree with generated projections; `.properties` compatibility layer. |
| `ADR-0006` | Conversion plans resolved at startup; zero-copy passthrough as the default fast path. |
| `ADR-0007` | Counter tables: no retry, no distributed reclaim, no batching — correctness over throughput. |
| `ADR-0008` | Java-identical metric strings and tracking-table schema as a hard compatibility contract. |
| `ADR-0009` | Implement Astra secure-connect-bundle support in `cdm-cql`: SNI-aware primary strategy, single-endpoint fallback. |

---

## 17. Mapping: Java component → cdm-rs home

| Java / Scala component | cdm-rs |
|---|---|
| `BaseJob`, `BasePartitionJob` (Spark drivers) | `cdm-engine::Engine` + `cdm-service::CdmService` |
| `Migrate`, `DiffData`, `GuardrailCheck` objects | `cdm-engine::jobs::{Migrate, Validate, Guardrail}` behind `JobPlugin` |
| `SplitPartitions` | `cdm-engine::planner` (`TOK-003`) |
| `PartitionRange` | `cdm-core::TokenRange` + `RangeState` |
| `JobCounter`, `CounterUnit`, `CDMMetricsAccumulator` | `cdm-metrics::Counters` (+ exporters, event bus) |
| `ConnectionFetcher`, `ConnectionDetails` | `cdm-cql::connect` |
| `AstraDevOpsClient` | `cdm-cql::astra` |
| `EnhancedSession`, `cql/statement/*` | `cdm-cql::{SessionHandle, statements}` |
| `CqlTable`, `BaseTable`, `Table` | `cdm-cql::schema::TableSchema` |
| `CqlData`, `CqlConversion`, `cql/codec/*` | `cdm-codec` |
| `PKFactory`, `EnhancedPK`, `Record` | `cdm-core::{PrimaryKey, Record}` + `cdm-engine::PkExtractor` |
| `feature/*` | `cdm-feature` (each a `FeaturePlugin`) |
| `TrackRun`, `TargetUpsertRunDetailsStatement` | `cdm-track` |
| `KnownProperties`, `PropertyHelper` | `cdm-config` (generated from `CdmConfig`) |
| `DataUtility` | split across `cdm-codec` (diff/format) and `cdm-cql` (identifiers, SCB) |
| log4j2 `ThreadContext` labels | `tracing` spans (+ Java-compatible label rendering) |
| `SIT/*.sh` harness | `cdm-testkit` + `tests/sit/` |
| `cdm-config-builder` (React) | `cdm-ui` + server-side `BestPractices` in `cdm-config` |
| Spark `spark-submit` | `cdm <subcommand>` |
| — | **new:** `cdm-service`, `cdm-api`, `cdm-mcp`, `cdm-a2a`, `cdm-cluster` |

---

## 18. What we deliberately do differently

| Java behaviour | cdm-rs | Why | Restore with |
|---|---|---|---|
| Unknown consistency level silently becomes `LOCAL_QUORUM` | hard error | silent weakening of consistency is a data-safety hazard | `--compat-java` |
| `ALLOW FILTERING` always appended | omitted when no CQL filter | a pure token scan does not need it; it can mask planner problems | `--compat-java` |
| UDT conversion via `format`/`parse` round-trip, positional | recursive, name-matched | lossy for nested and non-round-trippable types | `--compat-java` |
| Tuple element conversion unsupported | implemented | closes a real gap | — |
| Counter writes retried by driver policy | never retried; range fails | retrying a counter increment double-counts | — |
| `errorLimit` documented but unimplemented | implemented | users expect the documented behaviour | set to `0` |
| Batches formed in row order across partitions | grouped by partition key | multi-partition batches are a well-known anti-pattern | `perfops.batch_grouping = legacy` |
| Metrics only at end of run, via logs | live counters, Prometheus, OTLP, SSE, TUI | observability was the top operational complaint | — |
| Config errors surface at runtime | three validation tiers up front, all errors at once | fail in seconds, not hours | — |
| Requires JVM + exact Spark version | single static binary | eliminates the #1 support issue | — |
