//! Containerised origin and target clusters (`TST-100`, `TST-102`).
//!
//! This is the container-startup logic of the driver spike (`crates/cdm-cql/tests/driver_spike.rs`,
//! PR #2) lifted into a reusable fixture, because every hard-won detail in it was learned by
//! watching half the version matrix fail:
//!
//! * **The host port equals the container port, and `broadcast_rpc_address = 127.0.0.1`.** A
//!   containerised node advertises its Docker bridge address (`172.17.0.x`) in
//!   `system.local.rpc_address`. Drivers honour that when building their connection pool, so the
//!   control connection succeeds on the mapped port and every pooled connection is then refused,
//!   because the host cannot route to the bridge network. Cassandra 3.11 and 4.0 happened to
//!   survive it; 4.1 and 5.0 did not. Publishing the container port on the *same* host port and
//!   telling the node to advertise `127.0.0.1` makes the address it reports one the host can
//!   actually reach.
//!
//!   The two ports must agree; nothing says *which* port. It is chosen per fixture, from the
//!   ephemeral range, because `9042` is one port and there are many test binaries — see
//!   [`free_port`] and `TST-103`.
//! * **`MAX_HEAP_SIZE` and `HEAP_NEWSIZE` are set together, or not at all.** Left alone, every
//!   image sizes its heap from the machine's RAM and commits it with `-XX:+AlwaysPreTouch` — about
//!   4 GiB per node on a developer laptop, which is fine for one container and fatal for two: the
//!   [`OriginTarget`] pair had the origin OOM-killed by the Docker VM the moment the target
//!   started. So the fixture bounds the heap, and does it in the one way that works across the
//!   whole matrix. `cassandra-env.sh` aborts with "please set or unset MAX_HEAP_SIZE and
//!   HEAP_NEWSIZE in pairs" if only one is set while CMS is in use, which today is 3.11, 4.0 *and*
//!   4.1; 5.0 runs G1 and ignores `HEAP_NEWSIZE` rather than rejecting it. Setting both is
//!   therefore the only spelling that starts on all four. (PR #2's spike recorded this as
//!   "`HEAP_NEWSIZE` is invalid under the G1GC default on 4.1 and 5.0". Measured against the
//!   current images that is wrong twice over: 4.1 is CMS, and 5.0 ignores the setting. The abort
//!   it saw was the pairing check, not G1.)
//! * **A readiness poll on the CQL port.** "Startup complete" is logged before the native
//!   transport binds, so a fixture that trusts the log line alone hands back a node that refuses
//!   the next connection.
//! * **ScyllaDB needs `--smp 1 --skip-wait-for-gossip-to-settle 0 --broadcast-rpc-address
//!   127.0.0.1`**, or it claims every core on the machine and waits 30 seconds for a gossip round
//!   that has nobody to gossip with.
//!
//! # The readiness probe speaks CQL, not TCP
//!
//! `cdm-testkit` may not depend on `scylla` — only `cdm-cql` may (`ARCHITECTURE.md` §3) — so the
//! fixture cannot poll by running a query. A bare TCP connect is not good enough either: the
//! kernel accepts a connection on a listening socket before the node is willing to serve
//! anything. The fixture therefore sends a native-protocol `OPTIONS` frame, which needs no
//! `STARTUP`, no authentication and no keyspace, and waits for the `SUPPORTED` response. That
//! proves the native transport is bound *and* answering, which is exactly the condition the log
//! line fails to establish.
//!
//! # What is not here
//!
//! Anything that needs a session: creating a keyspace, applying DDL, inserting rows. Those go
//! through the [`TestSession`](crate::TestSession) seam, which `cdm-cql` implements once its
//! session type lands. The fixture's job ends at "a node is up at this address, and here is what
//! it can do".

use std::fmt;
use std::time::{Duration, Instant};

use cdm_core::{CdmError, ErrorKind};
use testcontainers::core::{ExecCommand, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The CQL port a fixture uses when a caller pins one explicitly.
///
/// **Not** what [`ClusterFixture::start`] uses — see [`free_port`] and `TST-103`. It remains the
/// default of [`FixtureOptions`] so that a caller who wants the well-known port can ask for it,
/// and because it is the port a reader expects to see named in this file.
pub const DEFAULT_NATIVE_PORT: u16 = 9042;

/// A host port nothing is listening on, for a fixture to publish (`TST-103`).
///
/// # Why fixtures do not simply use 9042
///
/// Because there is more than one of them. Every `*_it.rs` suite is its own test binary, and
/// while `cargo test` runs binaries one after another and CI adds `--test-threads=1`, container
/// *teardown* is asynchronous: the previous binary can exit while Docker is still unbinding its
/// port. The next binary then dies on
///
/// ```text
/// Bind for 0.0.0.0:9042 failed: port is already allocated
/// ```
///
/// which looks like a product failure and is not one. It is also load-dependent, so it appears as
/// an intermittent red build on whichever suite happens to follow a slow one — and it gets more
/// likely with every Docker-backed suite added.
///
/// # Why this is not simply "bind to port 0"
///
/// A Cassandra node advertises `127.0.0.1` and **its own** native port, and the driver honours the
/// advertised address, so the host port and the container port have to agree — the fixture cannot
/// let Docker pick a host port and map it. Instead it picks a free one itself and tells the node
/// to listen on that same port, which [`FixtureOptions::with_native_port`] already supports.
///
/// The listener is bound and immediately dropped, so the port is free when the caller uses it.
/// That leaves a race, which is why [`ClusterFixture::start`] retries on a *new* port.
///
/// # Errors
///
/// [`ErrorKind::Connect`] if no ephemeral port can be bound at all.
pub fn free_port() -> Result<u16, CdmError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| {
        CdmError::new(
            ErrorKind::Connect,
            format!("cannot bind an ephemeral port for a fixture: {e}"),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| {
            CdmError::new(
                ErrorKind::Connect,
                format!("cannot read the ephemeral port back: {e}"),
            )
        })?
        .port();
    drop(listener);
    Ok(port)
}

/// Whether a fixture failure was the host port being taken, rather than anything about the node.
///
/// Matched on the message because that is all Docker gives us: the daemon reports it as a 500 with
/// prose. A miss costs a retry that would have succeeded, not a wrong result, so the match is
/// deliberately broad.
fn is_port_conflict(error: &CdmError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("port is already allocated") || message.contains("address already in use")
}

/// Cassandra lines cdm-rs supports, newest last (`TST-002`).
pub const CASSANDRA_VERSIONS: &[&str] = &["3.11", "4.0", "4.1", "5.0"];

/// ScyllaDB lines cdm-rs supports. Tags are `major.minor`, so the matrix tracks the latest patch.
pub const SCYLLA_VERSIONS: &[&str] = &["6.2"];

/// The environment variable that chooses which engines a run exercises.
pub const ENGINES_ENV: &str = "CDM_IT_ENGINES";

/// How much heap a fixture gives a node by default, in MiB.
///
/// Enough for a node to start, hold a test-sized dataset and flush it; small enough that two of
/// them plus the rest of a developer's machine fit in a default Docker VM. A test that needs more
/// says so with [`FixtureOptions::with_heap_mib`].
pub const DEFAULT_HEAP_MIB: u32 = 1024;

/// Which implementation an engine is, where behaviour genuinely differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flavour {
    /// Apache Cassandra, and images that are drop-in compatible with it.
    Cassandra,
    /// ScyllaDB.
    Scylla,
}

impl Flavour {
    /// The container image name this flavour is published under.
    pub const fn image_name(self) -> &'static str {
        match self {
            Self::Cassandra => "cassandra",
            Self::Scylla => "scylladb/scylla",
        }
    }

    /// A log line that appears only once the node is starting to accept CQL.
    ///
    /// Used as a cheap first gate, never as proof: see the module docs on why the readiness probe
    /// exists.
    pub const fn ready_message(self) -> &'static str {
        match self {
            Self::Cassandra => "Startup complete",
            Self::Scylla => "Starting listening for CQL clients",
        }
    }
}

impl fmt::Display for Flavour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.image_name())
    }
}

/// One container image to run against: a flavour and a tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Engine {
    flavour: Flavour,
    tag: String,
}

impl Engine {
    /// A Cassandra image with the given tag, e.g. `4.1`.
    pub fn cassandra(tag: impl Into<String>) -> Self {
        Self {
            flavour: Flavour::Cassandra,
            tag: tag.into(),
        }
    }

    /// A ScyllaDB image with the given tag, e.g. `6.2`.
    pub fn scylla(tag: impl Into<String>) -> Self {
        Self {
            flavour: Flavour::Scylla,
            tag: tag.into(),
        }
    }

    /// Parses an `image:tag` pair, as `CDM_IT_ENGINES` spells it.
    ///
    /// Both `scylla:6.2` and `scylladb/scylla:6.2` name ScyllaDB, because CI matrices and humans
    /// disagree about which is the image's name.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the specification has no tag, or names an unknown image.
    pub fn parse(spec: &str) -> Result<Self, CdmError> {
        let spec = spec.trim();
        let (image, tag) = spec.split_once(':').ok_or_else(|| {
            CdmError::new(
                ErrorKind::Config,
                format!("`{spec}` is not an image:tag pair, e.g. `cassandra:4.1`"),
            )
        })?;
        match image {
            "cassandra" => Ok(Self::cassandra(tag)),
            "scylla" | "scylladb/scylla" => Ok(Self::scylla(tag)),
            other => Err(CdmError::new(
                ErrorKind::Config,
                format!("unknown engine image `{other}`; expected `cassandra` or `scylla`"),
            )),
        }
    }

    /// Which implementation this is.
    pub const fn flavour(&self) -> Flavour {
        self.flavour
    }

    /// The image tag, e.g. `4.1`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The major version, when the tag begins with one. `latest` has none.
    pub fn major_version(&self) -> Option<u32> {
        let digits: String = self.tag.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// What this engine can be asked to do (`CDC-004`).
    pub fn capabilities(&self) -> Capabilities {
        Capabilities::of(self)
    }

    /// Whether this engine implements `vector<T, N>` (`CDC-004`).
    ///
    /// Open-source Cassandra introduced it in 5.0; 3.11, 4.0 and 4.1 reject the type outright and
    /// ScyllaDB does not implement it at all. Tests must gate on this rather than fail on the
    /// older half of the matrix, which is why the fixture exposes it at all.
    pub fn supports_vectors(&self) -> bool {
        self.flavour == Flavour::Cassandra && self.major_version().is_some_and(|major| major >= 5)
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.flavour.image_name(), self.tag)
    }
}

/// What an engine can be asked to do, so that generators produce schemas it will accept.
///
/// A capability is a *type-system* fact, not a version number: a test asks "can this node hold a
/// vector column?", never "is this Cassandra 5?". That keeps the version arithmetic in one place
/// and out of every generator and every test.
// Four independent yes/no facts about an engine, deliberately not collapsed into a bitflag or an
// enum: each is asked for by name at a different call site, and `capabilities.vectors` reads
// better at the point of use than any packed alternative would.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capabilities {
    /// `vector<T, N>` (`CDC-004`). Open-source Cassandra 5.0 and later only.
    pub vectors: bool,
    /// `duration`. Cassandra 3.10 and later, and ScyllaDB.
    pub duration: bool,
    /// The DSE geometry types `PointType`, `LineStringType`, `PolygonType` (`CDC-003`). No
    /// open-source image implements these; they need DSE.
    pub dse_geometry: bool,
    /// DSE `DateRangeType` (`CDC-003`). DSE only, as above.
    pub date_range: bool,
}

impl Capabilities {
    /// What `engine` supports.
    pub fn of(engine: &Engine) -> Self {
        Self {
            vectors: engine.supports_vectors(),
            duration: true,
            dse_geometry: false,
            date_range: false,
        }
    }

    /// Everything cdm-rs models, for unit tests that generate a schema without running it.
    pub const fn maximal() -> Self {
        Self {
            vectors: true,
            duration: true,
            dse_geometry: true,
            date_range: true,
        }
    }

    /// Only what every supported open-source image accepts — the safe default for a schema that
    /// must apply cleanly to the whole matrix.
    pub const fn portable() -> Self {
        Self {
            vectors: false,
            duration: true,
            dse_geometry: false,
            date_range: false,
        }
    }
}

/// The engines to exercise in this process, from `CDM_IT_ENGINES` (`TST-002`).
///
/// Four containers per test is slow on a laptop, so a bare local run covers the newest Cassandra
/// only. The variable accepts:
///
/// * `cassandra` — every supported Cassandra line (the per-PR CI matrix);
/// * `scylla` — every supported ScyllaDB line (the nightly CI job);
/// * `all` — both;
/// * a comma-separated list of explicit `image:tag` pairs, e.g. `cassandra:4.0`, which is the
///   quickest way to run against an image already cached locally.
///
/// # Errors
///
/// [`ErrorKind::Config`] if the variable is set to a list containing an unparseable entry. A
/// typo in a CI matrix must fail the job, not silently narrow it to nothing.
pub fn engines_under_test() -> Result<Vec<Engine>, CdmError> {
    let cassandra = || CASSANDRA_VERSIONS.iter().copied().map(Engine::cassandra);
    let scylla = || SCYLLA_VERSIONS.iter().copied().map(Engine::scylla);

    let raw = std::env::var(ENGINES_ENV).ok();
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(vec![Engine::cassandra("5.0")]),
        Some("cassandra") => Ok(cassandra().collect()),
        Some("scylla") => Ok(scylla().collect()),
        Some("all") => Ok(cassandra().chain(scylla()).collect()),
        Some(list) => list
            .split(',')
            .filter(|spec| !spec.trim().is_empty())
            .map(Engine::parse)
            .collect(),
    }
}

/// How a [`ClusterFixture`] should be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureOptions {
    native_port: u16,
    startup_timeout: Duration,
    readiness_timeout: Duration,
    heap_mib: Option<u32>,
    container_name: Option<String>,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            native_port: DEFAULT_NATIVE_PORT,
            // A cold Cassandra 3.11 on a loaded CI runner takes minutes to log "Startup complete".
            startup_timeout: Duration::from_secs(300),
            readiness_timeout: Duration::from_secs(180),
            heap_mib: Some(DEFAULT_HEAP_MIB),
            container_name: None,
        }
    }
}

impl FixtureOptions {
    /// The CQL port, published on the host unchanged.
    ///
    /// It is published unchanged deliberately: the node advertises `127.0.0.1` and *its own*
    /// native port, so a driver that honours the advertised address only reaches the node when
    /// the host port and the container port agree. That means two fixtures cannot share a port,
    /// which is why [`OriginTarget`] gives the target a different one.
    #[must_use]
    pub const fn with_native_port(mut self, port: u16) -> Self {
        self.native_port = port;
        self
    }

    /// How long to wait for the container to log its startup message.
    #[must_use]
    pub const fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    /// How long to keep probing the CQL port after the container has started.
    #[must_use]
    pub const fn with_readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }

    /// How much heap the node may use, in MiB. `None` leaves the image's own sizing alone, which
    /// is roughly a quarter of the machine's RAM, committed up front.
    ///
    /// A pair of unbounded nodes does not fit on a 16 GiB Docker VM; see the module docs.
    #[must_use]
    pub const fn with_heap_mib(mut self, heap_mib: Option<u32>) -> Self {
        self.heap_mib = heap_mib;
        self
    }

    /// The configured CQL port.
    pub const fn native_port(&self) -> u16 {
        self.native_port
    }

    /// The configured heap bound, in MiB.
    pub const fn heap_mib(&self) -> Option<u32> {
        self.heap_mib
    }

    /// Names the container, rather than letting the runtime invent one.
    ///
    /// A fixture held in a `static` — as the SIT parity suite's shared node is, because nineteen
    /// cases against nineteen containers would take longer to start than the suite takes to run —
    /// is never dropped, so nothing stops it when the test process exits. A known name is what
    /// lets `cargo xtask sit` stop exactly that container before and after the suite, instead of
    /// leaving one behind to collide with the next run on the fixed CQL port.
    #[must_use]
    pub fn with_container_name(mut self, name: Option<String>) -> Self {
        self.container_name = name;
        self
    }

    /// The name the container will be given, if any.
    pub fn container_name(&self) -> Option<&str> {
        self.container_name.as_deref()
    }
}

/// A running single-node cluster, ready to accept CQL (`TST-100`).
///
/// Dropping it stops the container.
#[derive(Debug)]
pub struct ClusterFixture {
    engine: Engine,
    native_port: u16,
    host_port: u16,
    container: ContainerAsync<GenericImage>,
}

impl ClusterFixture {
    /// Starts `engine` with the default options and waits until it answers CQL.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Connect`] if the container cannot be started (typically: no container
    /// runtime — call [`ContainerRuntime::detect`](crate::ContainerRuntime::detect) first and
    /// skip, per `TST-102`), or if it never becomes queryable within the readiness timeout. The
    /// latter carries the tail of the container's log, so a CI failure is diagnosable from the
    /// job output alone.
    ///
    /// The node is published on a free ephemeral port, not on `9042` (`TST-103`). See
    /// [`free_port`] for why that matters.
    pub async fn start(engine: &Engine) -> Result<Self, CdmError> {
        Self::start_on_a_free_port(engine, &FixtureOptions::default()).await
    }

    /// Starts `engine` on a port nothing else is using, retrying if it is taken in between.
    ///
    /// `TST-103`. There is an unavoidable gap between choosing a free port and Docker binding it,
    /// so a conflict is possible however carefully the port is chosen; what makes it a non-issue
    /// is that a *fresh* port is chosen on each attempt, so a retry does not simply collide again
    /// the way retrying on `9042` would.
    async fn start_on_a_free_port(
        engine: &Engine,
        options: &FixtureOptions,
    ) -> Result<Self, CdmError> {
        const ATTEMPTS: usize = 5;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            let options = options.clone().with_native_port(free_port()?);
            match Self::start_with(engine, &options).await {
                Ok(fixture) => return Ok(fixture),
                Err(error) if is_port_conflict(&error) => last = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last.unwrap_or_else(|| {
            CdmError::new(
                ErrorKind::Connect,
                "cannot find a free host port for the fixture",
            )
        }))
    }

    /// Starts `engine` with explicit options.
    ///
    /// # Errors
    ///
    /// As [`ClusterFixture::start`].
    pub async fn start_with(engine: &Engine, options: &FixtureOptions) -> Result<Self, CdmError> {
        let port = options.native_port;
        let image = GenericImage::new(engine.flavour.image_name(), engine.tag())
            .with_wait_for(WaitFor::message_on_stdout(engine.flavour.ready_message()))
            .with_startup_timeout(options.startup_timeout)
            .with_mapped_port(port, port.tcp());

        let image = match engine.flavour {
            Flavour::Cassandra => {
                let mut image = image.with_env_var("CASSANDRA_BROADCAST_RPC_ADDRESS", "127.0.0.1");
                if let Some(heap_mib) = options.heap_mib {
                    // Both, always: see the module docs. The young generation is a quarter of the
                    // heap, which is what `calculate_heap_sizes` would have chosen anyway.
                    image = image
                        .with_env_var("MAX_HEAP_SIZE", format!("{heap_mib}M"))
                        .with_env_var("HEAP_NEWSIZE", format!("{}M", heap_mib / 4));
                }
                // Leave the stock command alone on the default port: the entrypoint only applies
                // its `CASSANDRA_*` yaml substitutions when argv[0] is `cassandra`, so the
                // override has to keep that shape. `bin/cassandra` forwards `-D` flags to the
                // JVM, and `cassandra.native_transport_port` is read by `DatabaseDescriptor`.
                if port == DEFAULT_NATIVE_PORT {
                    image
                } else {
                    image.with_cmd([
                        "cassandra".to_owned(),
                        "-f".to_owned(),
                        format!("-Dcassandra.native_transport_port={port}"),
                    ])
                }
            }
            Flavour::Scylla => {
                let mut cmd = vec![
                    "--smp".to_owned(),
                    "1".to_owned(),
                    "--skip-wait-for-gossip-to-settle".to_owned(),
                    "0".to_owned(),
                    "--broadcast-rpc-address".to_owned(),
                    "127.0.0.1".to_owned(),
                    "--native-transport-port".to_owned(),
                    port.to_string(),
                ];
                if let Some(heap_mib) = options.heap_mib {
                    // ScyllaDB reserves most of the machine's memory unless told otherwise, and
                    // `--overprovisioned` stops it assuming it owns the CPU as well.
                    cmd.extend([
                        "--memory".to_owned(),
                        format!("{heap_mib}M"),
                        "--overprovisioned".to_owned(),
                        "1".to_owned(),
                    ]);
                }
                image.with_cmd(cmd)
            }
        };

        let image = match options.container_name.as_ref() {
            Some(name) => image.with_container_name(name.clone()),
            None => image,
        };

        let container = image.start().await.map_err(|e| {
            CdmError::new(
                ErrorKind::Connect,
                format!(
                    "cannot start {engine}: {e}. Is a container runtime running? \
                     (TST-102: detect one first and skip rather than fail)"
                ),
            )
        })?;

        let host_port = container
            .get_host_port_ipv4(port.tcp())
            .await
            .map_err(|e| {
                CdmError::new(
                    ErrorKind::Connect,
                    format!("{engine} started but published no host port for {port}: {e}"),
                )
            })?;

        let fixture = Self {
            engine: engine.clone(),
            native_port: port,
            host_port,
            container,
        };
        fixture.await_cql(options.readiness_timeout).await?;
        Ok(fixture)
    }

    /// Polls the CQL port until it answers a native-protocol `OPTIONS` frame.
    async fn await_cql(&self, timeout: Duration) -> Result<(), CdmError> {
        let deadline = Instant::now() + timeout;
        let mut last;
        loop {
            match cql_options_probe(&self.contact_point()).await {
                Ok(()) => return Ok(()),
                Err(reason) => last = reason,
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(CdmError::new(
            ErrorKind::Connect,
            format!(
                "{} never answered CQL on {} within {timeout:?}: {last}\n{}",
                self.engine,
                self.contact_point(),
                self.log_tail(40).await
            ),
        ))
    }

    /// The engine this fixture is running.
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// What this node can be asked to do.
    pub fn capabilities(&self) -> Capabilities {
        self.engine.capabilities()
    }

    /// Whether this node implements `vector<T, N>` (`CDC-004`).
    pub fn supports_vectors(&self) -> bool {
        self.engine.supports_vectors()
    }

    /// The address a driver should be pointed at, e.g. `127.0.0.1:9042`.
    pub fn contact_point(&self) -> String {
        format!("127.0.0.1:{}", self.host_port)
    }

    /// The port on the host. Equal to [`ClusterFixture::native_port`] by construction; both are
    /// exposed because the reason they are equal is a property worth asserting.
    pub const fn host_port(&self) -> u16 {
        self.host_port
    }

    /// The port the node itself listens on, and advertises.
    pub const fn native_port(&self) -> u16 {
        self.native_port
    }

    /// Runs a CQL script inside the container with the engine's own shell client, and returns
    /// what it printed (`TST-003`).
    ///
    /// This is the one thing the fixture does that needs no driver and no session: the SIT parity
    /// suite has to apply a `setup.cql`, a `break.cql` and a final `SELECT`, and compare the last
    /// one's output against a fixture that Java produced with `cqlsh`. Going through `cqlsh`
    /// rather than through a session keeps this crate free of the driver dependency
    /// `ARCHITECTURE.md` §3 reserves for `cdm-cql`, and it means the rendering the expectation was
    /// written against is the rendering it is compared against.
    ///
    /// The script may hold any number of statements separated by `;`. Comments and blank lines are
    /// stripped, because `cqlsh -e` parses its argument as one string and answers
    /// `no viable alternative at input ';'` for either, and a script longer than a single `execve`
    /// argument is split on statement boundaries and run in order.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Connect`] if the command cannot be started, and [`ErrorKind::Internal`] if the
    /// client exits non-zero — carrying its stderr, because a CQL syntax error in a fixture is
    /// otherwise indistinguishable from a node that went away.
    pub async fn exec_cql(&self, script: &str) -> Result<String, CdmError> {
        let mut output = String::new();
        for chunk in crate::sit::chunk_cql(&crate::sit::flatten_cql(script)) {
            output.push_str(&self.exec_cql_chunk(&chunk).await?);
        }
        Ok(output)
    }

    /// One `cqlsh -e` invocation, small enough to fit in a single argument.
    async fn exec_cql_chunk(&self, script: &str) -> Result<String, CdmError> {
        // Both images ship `cqlsh`; ScyllaDB's is a compatible reimplementation.
        let client = "cqlsh";
        let mut result = self
            .container
            .exec(ExecCommand::new([
                client,
                "-e",
                script,
                "127.0.0.1",
                &self.native_port.to_string(),
            ]))
            .await
            .map_err(|e| {
                CdmError::new(
                    ErrorKind::Connect,
                    format!("cannot run {client} in the {} container: {e}", self.engine),
                )
            })?;

        let stdout =
            String::from_utf8_lossy(&read_stream(result.stdout_to_vec().await)?).into_owned();
        let stderr =
            String::from_utf8_lossy(&read_stream(result.stderr_to_vec().await)?).into_owned();
        let code = result.exit_code().await.map_err(|e| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot read {client}'s exit code: {e}"),
            )
        })?;
        if code.is_some_and(|code| code != 0) {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "{client} exited {}: {}\n--- script ---\n{}",
                    code.unwrap_or(-1),
                    stderr.trim(),
                    truncate(script)
                ),
            ));
        }
        // `cqlsh` reports a failed statement on stdout and still exits zero when it was given
        // `-e`, so the exit code alone does not establish that the script ran.
        if stdout.contains("InvalidRequest") || stdout.contains("SyntaxException") {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "{client} rejected a statement: {}\n--- script ---\n{}",
                    stdout.trim(),
                    truncate(script)
                ),
            ));
        }
        Ok(stdout)
    }

    /// The last `lines` lines of the container's stdout and stderr.
    ///
    /// Never fails: a fixture that cannot fetch its own logs still has to be able to report why
    /// something else went wrong, so an unreadable stream becomes a note in the returned text.
    pub async fn log_tail(&self, lines: usize) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        for (stream, bytes) in [
            ("stdout", self.container.stdout_to_vec().await),
            ("stderr", self.container.stderr_to_vec().await),
        ] {
            // Writing into a String cannot fail.
            let _ = writeln!(out, "--- {} {stream} (last {lines} lines) ---", self.engine);
            match bytes {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let tail: Vec<&str> = text.lines().rev().take(lines).collect();
                    for line in tail.into_iter().rev() {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                Err(e) => {
                    let _ = writeln!(out, "unavailable: {e}");
                }
            }
        }
        out
    }
}

/// An origin and a target cluster, the pair every migration test needs (`TST-100`).
///
/// Both run the same engine unless [`OriginTarget::start_pair`] says otherwise — a
/// version-skewed migration (3.11 to 5.0, Cassandra to ScyllaDB) is a first-class case for CDM,
/// so the fixture makes it as easy to express as the homogeneous one.
#[derive(Debug)]
pub struct OriginTarget {
    origin: ClusterFixture,
    target: ClusterFixture,
}

impl OriginTarget {
    /// The host port the target publishes.
    ///
    /// One above the origin's. It cannot be the same port: both nodes advertise `127.0.0.1` and
    /// their own native port, and the host cannot publish one port twice.
    pub const TARGET_NATIVE_PORT: u16 = DEFAULT_NATIVE_PORT + 1;

    /// Starts two clusters of the same engine.
    ///
    /// # Errors
    ///
    /// As [`ClusterFixture::start`]. The origin is started first and dropped if the target fails,
    /// so a failure leaves no container behind.
    pub async fn start(engine: &Engine) -> Result<Self, CdmError> {
        Self::start_pair(engine, engine).await
    }

    /// Starts two clusters that need not be the same engine.
    ///
    /// # Errors
    ///
    /// As [`ClusterFixture::start`].
    pub async fn start_pair(origin: &Engine, target: &Engine) -> Result<Self, CdmError> {
        // TST-103: a free port each, not 9042 and 9043. The two are necessarily distinct because
        // the origin is still holding its port when the target picks one.
        let origin =
            ClusterFixture::start_on_a_free_port(origin, &FixtureOptions::default()).await?;
        let target =
            ClusterFixture::start_on_a_free_port(target, &FixtureOptions::default()).await?;

        // Starting the second node is the moment the first one is most likely to die: two JVMs
        // that each committed their whole heap is what exhausts a Docker VM. Re-probing turns
        // "the driver was refused for no apparent reason", minutes later and somewhere else, into
        // an error at the point of the cause.
        origin
            .await_cql(Duration::from_secs(30))
            .await
            .map_err(|e| {
                CdmError::new(
                    ErrorKind::Connect,
                    format!(
                        "the origin stopped answering while the target was starting, which is \
                         almost always the container runtime running out of memory and killing \
                         it: {}. Give the runtime more memory, or lower \
                         FixtureOptions::with_heap_mib.",
                        e.message()
                    ),
                )
            })?;
        Ok(Self { origin, target })
    }

    /// The origin cluster — the side CDM reads from.
    pub const fn origin(&self) -> &ClusterFixture {
        &self.origin
    }

    /// The target cluster — the side CDM writes to.
    pub const fn target(&self) -> &ClusterFixture {
        &self.target
    }

    /// What *both* sides support: the intersection, which is what a schema applied to both must
    /// stay inside.
    pub fn common_capabilities(&self) -> Capabilities {
        let origin = self.origin.capabilities();
        let target = self.target.capabilities();
        Capabilities {
            vectors: origin.vectors && target.vectors,
            duration: origin.duration && target.duration,
            dse_geometry: origin.dse_geometry && target.dse_geometry,
            date_range: origin.date_range && target.date_range,
        }
    }
}

/// Native-protocol frame constants, from the CQL binary protocol v4 specification.
mod frame {
    /// Request direction, protocol version 4 — understood by Cassandra 2.2 through 5.0 and by
    /// every ScyllaDB release. v5 exists but buys nothing for an `OPTIONS` probe.
    pub(super) const VERSION_REQUEST: u8 = 0x04;
    /// Response direction of the same version: the high bit is the direction flag.
    pub(super) const VERSION_RESPONSE: u8 = 0x84;
    /// `OPTIONS` — the one request that needs no `STARTUP`, no credentials and no keyspace.
    pub(super) const OPCODE_OPTIONS: u8 = 0x05;
    /// `SUPPORTED`, the answer to `OPTIONS`.
    pub(super) const OPCODE_SUPPORTED: u8 = 0x06;
    /// Every frame begins with version, flags, a two-byte stream id, an opcode and a length.
    pub(super) const HEADER_LEN: usize = 9;
    /// A `SUPPORTED` body lists a handful of short string multimaps. Anything larger than this is
    /// not a response we understand, and reading it unbounded would be a denial-of-service on
    /// ourselves.
    pub(super) const MAX_BODY_LEN: u32 = 64 * 1024;
}

/// How long one readiness probe may take before it is treated as a failure and retried.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Asks a node for its supported options over the native protocol.
///
/// Returns `Ok` only when the node replies `SUPPORTED`, which proves the native transport is
/// bound *and* serving — the condition "Startup complete" in the log does not establish.
///
/// # Errors
///
/// The reason as a string, suitable for a retry log. This is deliberately not a [`CdmError`]:
/// every failure here is expected and transient until the last one, and only the last one becomes
/// an error.
/// Turns a stream-read failure from [`ClusterFixture::exec_cql`] into a `CdmError`.
fn read_stream<E: fmt::Display>(read: Result<Vec<u8>, E>) -> Result<Vec<u8>, CdmError> {
    read.map_err(|e| {
        CdmError::new(
            ErrorKind::Internal,
            format!("cannot read the output of a command run in the container: {e}"),
        )
    })
}

/// The first few lines of a script, for an error message.
///
/// A SIT case's `setup.cql` can be four thousand statements long; quoting all of it in a failure
/// buries the one line that matters.
fn truncate(script: &str) -> String {
    const LINES: usize = 12;
    let head: Vec<&str> = script.lines().take(LINES).collect();
    let total = script.lines().count();
    if total <= LINES {
        return head.join("\n");
    }
    format!("{}\n… and {} more line(s)", head.join("\n"), total - LINES)
}

async fn cql_options_probe(address: &str) -> Result<(), String> {
    let attempt = tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut stream = TcpStream::connect(address)
            .await
            .map_err(|e| format!("connect: {e}"))?;

        let request = [
            frame::VERSION_REQUEST,
            0x00, // flags
            0x00,
            0x00, // stream id 0
            frame::OPCODE_OPTIONS,
            0x00,
            0x00,
            0x00,
            0x00, // body length 0
        ];
        stream
            .write_all(&request)
            .await
            .map_err(|e| format!("write OPTIONS: {e}"))?;

        let mut header = [0_u8; frame::HEADER_LEN];
        stream
            .read_exact(&mut header)
            .await
            .map_err(|e| format!("read SUPPORTED header: {e}"))?;

        // SAFETY-INVARIANT: `header` is a fixed nine-byte array and every index below is a
        // literal less than nine, so no access can be out of bounds. `get()` here would put an
        // `Option` on a path that has no failure case.
        #[allow(clippy::indexing_slicing)]
        let (version, opcode, length) = (
            header[0],
            header[4],
            u32::from_be_bytes([header[5], header[6], header[7], header[8]]),
        );

        if version != frame::VERSION_RESPONSE {
            return Err(format!(
                "expected a v4 response frame (0x{:02x}), got 0x{version:02x}",
                frame::VERSION_RESPONSE
            ));
        }
        if opcode != frame::OPCODE_SUPPORTED {
            return Err(format!(
                "expected SUPPORTED (0x{:02x}), got opcode 0x{opcode:02x}",
                frame::OPCODE_SUPPORTED
            ));
        }
        if length > frame::MAX_BODY_LEN {
            return Err(format!("SUPPORTED body of {length} bytes is implausible"));
        }

        let mut body = vec![0_u8; length as usize];
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| format!("read SUPPORTED body: {e}"))?;
        Ok(())
    })
    .await;

    match attempt {
        Ok(result) => result,
        Err(_) => Err(format!("no answer within {PROBE_TIMEOUT:?}")),
    }
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn tst_100_engines_render_as_the_image_reference_they_start() {
        assert_eq!(Engine::cassandra("4.1").to_string(), "cassandra:4.1");
        assert_eq!(Engine::scylla("6.2").to_string(), "scylladb/scylla:6.2");
        assert_eq!(Engine::cassandra("4.1").tag(), "4.1");
        assert_eq!(Engine::cassandra("4.1").flavour(), Flavour::Cassandra);
        assert_eq!(Flavour::Scylla.to_string(), "scylladb/scylla");
    }

    #[test]
    fn tst_100_both_spellings_of_the_scylla_image_parse() {
        assert_eq!(Engine::parse("scylla:6.2").unwrap(), Engine::scylla("6.2"));
        assert_eq!(
            Engine::parse("scylladb/scylla:6.2").unwrap(),
            Engine::scylla("6.2")
        );
        assert_eq!(
            Engine::parse("  cassandra:5.0  ").unwrap(),
            Engine::cassandra("5.0")
        );
    }

    #[test]
    fn tst_100_an_unparseable_engine_is_a_config_error_not_a_silent_skip() {
        let err = Engine::parse("cassandra").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("image:tag"), "{err}");

        let err = Engine::parse("postgres:16").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("postgres"), "{err}");
    }

    #[test]
    fn cdc_004_vectors_are_cassandra_five_and_later_only() {
        for tag in CASSANDRA_VERSIONS {
            let engine = Engine::cassandra(*tag);
            let expected = tag.starts_with("5.");
            assert_eq!(
                engine.supports_vectors(),
                expected,
                "{engine} vector support"
            );
            assert_eq!(engine.capabilities().vectors, expected);
        }
        for tag in SCYLLA_VERSIONS {
            assert!(
                !Engine::scylla(*tag).supports_vectors(),
                "ScyllaDB does not implement vector<>"
            );
        }
        // A future line must be assumed to have it, not assumed not to.
        assert!(Engine::cassandra("6.0").supports_vectors());
        // A tag with no version cannot be assumed to have anything.
        assert!(!Engine::cassandra("latest").supports_vectors());
        assert_eq!(Engine::cassandra("latest").major_version(), None);
        assert_eq!(Engine::cassandra("3.11").major_version(), Some(3));
    }

    #[test]
    fn tst_100_capabilities_describe_open_source_images_honestly() {
        // No open-source image implements the DSE geometry types, so a generator that trusted
        // `maximal()` against a container would produce DDL the node rejects.
        let cassandra = Engine::cassandra("5.0").capabilities();
        assert!(cassandra.vectors);
        assert!(cassandra.duration);
        assert!(!cassandra.dse_geometry);
        assert!(!cassandra.date_range);

        assert_eq!(
            Capabilities::maximal(),
            Capabilities {
                vectors: true,
                duration: true,
                dse_geometry: true,
                date_range: true,
            }
        );
        assert!(!Capabilities::portable().vectors);
        assert!(Capabilities::portable().duration);
    }

    #[test]
    fn tst_002_the_default_engine_set_is_one_container_and_the_matrices_are_complete() {
        // `engines_under_test` reads the environment, which is process-global; this test asserts
        // the parsing rules through the same code path without mutating it, by construction of
        // the constants it uses.
        assert_eq!(CASSANDRA_VERSIONS, ["3.11", "4.0", "4.1", "5.0"]);
        assert_eq!(SCYLLA_VERSIONS, ["6.2"]);
        assert_eq!(ENGINES_ENV, "CDM_IT_ENGINES");
    }

    #[test]
    fn tst_103_a_free_port_is_usable_and_is_not_the_well_known_one() {
        // The bug this guards: every fixture asking for 9042, so any lingering container from the
        // previous test binary makes the next one fail on a port conflict that has nothing to do
        // with the code under test.
        let port = free_port().unwrap();
        assert!(port > 0, "an ephemeral port was expected, got {port}");
        assert_ne!(
            port, DEFAULT_NATIVE_PORT,
            "an ephemeral port must not be the well-known one"
        );
        // And it really is free: binding it again must succeed, because the probe released it.
        std::net::TcpListener::bind(("127.0.0.1", port))
            .unwrap_or_else(|e| panic!("port {port} was reported free but cannot be bound: {e}"));
    }

    #[test]
    fn tst_103_successive_free_ports_do_not_collide_while_both_are_held() {
        // OriginTarget starts two nodes, and the second picks its port while the first still holds
        // its own — which is what stops the pair landing on one port.
        let first = free_port().unwrap();
        let held = std::net::TcpListener::bind(("127.0.0.1", first)).unwrap();
        let second = free_port().unwrap();
        assert_ne!(first, second, "a held port must not be handed out again");
        drop(held);
    }

    #[test]
    fn tst_103_a_taken_port_is_recognised_so_it_can_be_retried() {
        // Docker reports this as a 500 with prose, so the message is all there is to match on.
        let taken = CdmError::new(
            ErrorKind::Connect,
            "Docker responded with status code 500: Bind for 0.0.0.0:9042 failed: \
             port is already allocated",
        );
        assert!(is_port_conflict(&taken));
        assert!(is_port_conflict(&CdmError::new(
            ErrorKind::Connect,
            "Address already in use (os error 48)"
        )));
        // A real startup failure must not be retried as though it were a port clash.
        assert!(!is_port_conflict(&CdmError::new(
            ErrorKind::Connect,
            "the node never became queryable within the readiness timeout"
        )));
    }

    #[test]
    fn tst_100_fixture_options_default_to_the_published_cql_port() {
        let options = FixtureOptions::default();
        assert_eq!(options.native_port(), DEFAULT_NATIVE_PORT);
        assert_eq!(options.native_port, 9042);
        assert_eq!(options.with_native_port(9043).native_port(), 9043);
        assert_eq!(
            FixtureOptions::default()
                .with_startup_timeout(Duration::from_secs(1))
                .startup_timeout,
            Duration::from_secs(1)
        );
        assert_eq!(
            FixtureOptions::default()
                .with_readiness_timeout(Duration::from_secs(2))
                .readiness_timeout,
            Duration::from_secs(2)
        );
    }

    #[test]
    fn tst_100_the_two_sides_cannot_share_a_host_port() {
        assert_ne!(OriginTarget::TARGET_NATIVE_PORT, DEFAULT_NATIVE_PORT);
    }

    #[test]
    fn tst_100_the_heap_is_bounded_by_default_so_a_pair_fits_on_one_machine() {
        // Unbounded, each node commits about a quarter of the machine's RAM up front, and the
        // second one starting kills the first.
        assert_eq!(FixtureOptions::default().heap_mib(), Some(DEFAULT_HEAP_MIB));
        assert_eq!(DEFAULT_HEAP_MIB, 1024);
        // The young generation is a quarter of the heap, and must be a whole number of MiB.
        assert_eq!(DEFAULT_HEAP_MIB % 4, 0);
        assert_eq!(
            FixtureOptions::default().with_heap_mib(None).heap_mib(),
            None
        );
    }

    #[tokio::test]
    async fn tst_102_the_readiness_probe_reports_rather_than_hangs_when_nothing_listens() {
        // Port 1 on the loopback refuses immediately; the probe must turn that into a reason.
        let reason = cql_options_probe("127.0.0.1:1").await.unwrap_err();
        assert!(reason.starts_with("connect:"), "{reason}");
    }

    #[tokio::test]
    async fn tst_102_the_readiness_probe_rejects_a_listener_that_does_not_speak_cql() {
        // A plain TCP accept is exactly the false positive the probe exists to avoid: this
        // listener accepts the connection and then says nothing, which a connect-only probe
        // would have declared ready.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let accepting = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // Hold the connection open without answering.
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(stream);
        });

        let reason = cql_options_probe(&address).await.unwrap_err();
        assert!(reason.contains("no answer within"), "{reason}");
        accepting.abort();
    }

    #[tokio::test]
    async fn tst_102_the_readiness_probe_accepts_a_well_formed_supported_frame() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let serving = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; frame::HEADER_LEN];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(request[0], frame::VERSION_REQUEST);
            assert_eq!(request[4], frame::OPCODE_OPTIONS);
            // An empty SUPPORTED body is enough: the probe only needs the opcode.
            let response = [
                frame::VERSION_RESPONSE,
                0x00,
                0x00,
                0x00,
                frame::OPCODE_SUPPORTED,
                0x00,
                0x00,
                0x00,
                0x00,
            ];
            stream.write_all(&response).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        cql_options_probe(&address).await.unwrap();
        serving.await.unwrap();
    }

    #[tokio::test]
    async fn tst_102_the_readiness_probe_rejects_an_error_frame() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let serving = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; frame::HEADER_LEN];
            stream.read_exact(&mut request).await.unwrap();
            // Opcode 0x00 is ERROR: a node that is up but not serving answers this way.
            let response = [frame::VERSION_RESPONSE, 0, 0, 0, 0x00, 0, 0, 0, 0];
            stream.write_all(&response).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let reason = cql_options_probe(&address).await.unwrap_err();
        assert!(reason.contains("SUPPORTED"), "{reason}");
        serving.abort();
    }
}
