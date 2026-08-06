//! Container-runtime detection and the skip-don't-fail contract (`TST-102`).
//!
//! `cargo test --workspace` must stay green on a laptop with no Docker daemon. Every
//! containerised test therefore begins by *asking* whether a runtime is there, and returns early
//! with an explanation when it is not — rather than failing, or worse, hanging for the container
//! startup timeout before failing.
//!
//! Detection is deliberately a socket probe rather than `docker info`. Shelling out costs a
//! process spawn per test, and a `docker` binary on `$PATH` proves nothing about a *running*
//! daemon: Docker Desktop leaves the CLI installed and the socket absent when it is stopped,
//! which is exactly the state this module exists to recognise.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

/// How long to wait for a socket to accept a connection before treating it as absent.
///
/// A live daemon accepts immediately; the timeout only bounds the pathological case of a stale
/// TCP endpoint in `DOCKER_HOST` that neither accepts nor refuses.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Where the endpoint this runtime was found at came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeSource {
    /// `DOCKER_HOST`, the conventional override honoured by Docker, Podman and testcontainers.
    DockerHost,
    /// `TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE`, which testcontainers itself consults first.
    TestcontainersOverride,
    /// One of the well-known socket paths of [`well_known_sockets`].
    WellKnownSocket,
}

impl RuntimeSource {
    /// A short human-readable name, used in the diagnostic messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DockerHost => "DOCKER_HOST",
            Self::TestcontainersOverride => "TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE",
            Self::WellKnownSocket => "a well-known socket path",
        }
    }
}

/// A container runtime that answered a probe, and the endpoint it answered on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRuntime {
    endpoint: String,
    source: RuntimeSource,
}

impl ContainerRuntime {
    /// Probes for a usable container runtime (`TST-102`).
    ///
    /// The search order matches testcontainers' own, so that a runtime this function finds is one
    /// testcontainers will also use, and a runtime it rejects is one testcontainers would also
    /// have failed on:
    ///
    /// 1. `TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE`;
    /// 2. `DOCKER_HOST`;
    /// 3. the well-known socket paths of [`well_known_sockets`], in order.
    ///
    /// # Errors
    ///
    /// [`NoContainerRuntime`], carrying every endpoint that was tried, when nothing answered.
    /// The error's [`Display`](fmt::Display) is the message a skipping test should print.
    pub fn detect() -> Result<Self, NoContainerRuntime> {
        let mut probed = Vec::new();

        for (var, source) in [
            (
                "TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE",
                RuntimeSource::TestcontainersOverride,
            ),
            ("DOCKER_HOST", RuntimeSource::DockerHost),
        ] {
            let Ok(value) = std::env::var(var) else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }
            match probe(&value) {
                Ok(()) => {
                    return Ok(Self {
                        endpoint: value,
                        source,
                    })
                }
                Err(reason) => probed.push(Probe {
                    endpoint: value,
                    source,
                    reason,
                }),
            }
        }

        for path in well_known_sockets() {
            let endpoint = path.display().to_string();
            match probe(&endpoint) {
                Ok(()) => {
                    return Ok(Self {
                        endpoint,
                        source: RuntimeSource::WellKnownSocket,
                    })
                }
                Err(reason) => probed.push(Probe {
                    endpoint,
                    source: RuntimeSource::WellKnownSocket,
                    reason,
                }),
            }
        }

        Err(NoContainerRuntime { probed })
    }

    /// The endpoint that answered — a socket path, or a `tcp://`/`unix://` URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Which of the three sources named the endpoint.
    pub const fn source(&self) -> RuntimeSource {
        self.source
    }
}

impl fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (from {})", self.endpoint, self.source.as_str())
    }
}

/// One endpoint that was tried and did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Probe {
    endpoint: String,
    source: RuntimeSource,
    reason: String,
}

/// No container runtime answered (`TST-102`).
///
/// The `Display` form is written to be pasted into a bug report: it names every endpoint that was
/// tried and why each failed, so "the tests skipped on my machine" is a diagnosable statement
/// rather than a mystery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoContainerRuntime {
    probed: Vec<Probe>,
}

impl NoContainerRuntime {
    /// The endpoints that were tried, in the order they were tried.
    pub fn probed_endpoints(&self) -> Vec<&str> {
        self.probed.iter().map(|p| p.endpoint.as_str()).collect()
    }
}

impl fmt::Display for NoContainerRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "skipping: no container runtime is available, so this test cannot run (TST-102)."
        )?;
        if self.probed.is_empty() {
            writeln!(f, "  nothing to probe: no socket path is known on this platform, and neither DOCKER_HOST nor TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE is set")?;
        } else {
            for probe in &self.probed {
                writeln!(
                    f,
                    "  tried {} ({}): {}",
                    probe.endpoint,
                    probe.source.as_str(),
                    probe.reason
                )?;
            }
        }
        write!(
            f,
            "  start Docker, Podman, Colima or Rancher Desktop and re-run `cargo xtask it`"
        )
    }
}

impl std::error::Error for NoContainerRuntime {}

/// The socket paths probed when no environment variable names one.
///
/// Ordered by how likely each is to be the runtime the developer means: the system socket first,
/// then Docker Desktop's per-user socket, then the rootless and drop-in replacements. Paths that
/// depend on an unset environment variable are omitted rather than probed as a relative path.
pub fn well_known_sockets() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/var/run/docker.sock")];

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        // Docker Desktop for macOS since 4.13, and Docker Desktop for Linux.
        paths.push(home.join(".docker/run/docker.sock"));
        paths.push(home.join(".docker/desktop/docker.sock"));
        // Colima and Rancher Desktop, the two common macOS replacements.
        paths.push(home.join(".colima/default/docker.sock"));
        paths.push(home.join(".rd/docker.sock"));
    }

    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        paths.push(PathBuf::from(runtime_dir).join("podman/podman.sock"));
    }
    paths.push(PathBuf::from("/run/podman/podman.sock"));

    paths
}

/// Connects to one endpoint, returning the reason on failure.
///
/// `unix://`, `tcp://`, `http://` and bare paths are all understood, because all four turn up in
/// the wild in `DOCKER_HOST`.
fn probe(endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();

    for scheme in ["tcp://", "http://", "https://"] {
        if let Some(authority) = endpoint.strip_prefix(scheme) {
            let authority = authority.trim_end_matches('/');
            return probe_tcp(authority);
        }
    }

    let path = endpoint.strip_prefix("unix://").unwrap_or(endpoint);
    probe_unix(path)
}

/// Connects to a TCP `host:port` authority.
fn probe_tcp(authority: &str) -> Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs};

    let addrs = authority
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve: {e}"))?;
    let mut last = "resolved to no address".to_owned();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Connects to a Unix domain socket.
#[cfg(unix)]
fn probe_unix(path: &str) -> Result<(), String> {
    use std::os::unix::net::UnixStream;

    // `UnixStream::connect` has no timeout knob, but a Unix socket either has a listener — in
    // which case the connection is immediate — or it does not, in which case connect fails at
    // once with ECONNREFUSED or ENOENT. There is no slow case to bound.
    UnixStream::connect(path)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// On platforms without Unix domain sockets, only an explicit TCP `DOCKER_HOST` can be probed.
#[cfg(not(unix))]
fn probe_unix(path: &str) -> Result<(), String> {
    Err(format!(
        "{path} cannot be probed on this platform; set DOCKER_HOST to a tcp:// endpoint"
    ))
}

/// Skips the enclosing test unless a container runtime is available (`TST-102`).
///
/// Expands to the detected [`ContainerRuntime`], or prints the reason and `return`s. Use it as
/// the first statement of any test that starts a container:
///
/// ```
/// use cdm_testkit::skip_without_container_runtime;
///
/// // In a real suite this body carries `#[tokio::test]` and starts a container.
/// fn tst_102_example() {
///     let runtime = skip_without_container_runtime!();
///     println!("running against {runtime}");
/// }
/// tst_102_example();
/// ```
///
/// The macro deliberately does not consult a `CI` environment variable to turn the skip into a
/// failure. CI runs the containerised suite through a workflow that provides a runtime; if that
/// runtime disappears the container start fails loudly, which is a better signal than a skip
/// promoted to a failure on a developer's machine.
#[macro_export]
macro_rules! skip_without_container_runtime {
    () => {
        match $crate::ContainerRuntime::detect() {
            Ok(runtime) => runtime,
            Err(reason) => {
                eprintln!("{reason}");
                return;
            }
        }
    };
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
    fn tst_102_a_missing_runtime_is_reported_with_every_endpoint_tried() {
        let err = NoContainerRuntime {
            probed: vec![
                Probe {
                    endpoint: "tcp://127.0.0.1:2375".to_owned(),
                    source: RuntimeSource::DockerHost,
                    reason: "connection refused".to_owned(),
                },
                Probe {
                    endpoint: "/var/run/docker.sock".to_owned(),
                    source: RuntimeSource::WellKnownSocket,
                    reason: "No such file or directory".to_owned(),
                },
            ],
        };
        let message = err.to_string();
        assert!(message.starts_with("skipping:"), "{message}");
        assert!(message.contains("TST-102"), "{message}");
        assert!(message.contains("tcp://127.0.0.1:2375"), "{message}");
        assert!(message.contains("DOCKER_HOST"), "{message}");
        assert!(message.contains("/var/run/docker.sock"), "{message}");
        assert!(message.contains("connection refused"), "{message}");
        assert!(message.contains("cargo xtask it"), "{message}");
        assert_eq!(
            err.probed_endpoints(),
            vec!["tcp://127.0.0.1:2375", "/var/run/docker.sock"]
        );
    }

    #[test]
    fn tst_102_an_empty_probe_list_still_explains_itself() {
        let message = NoContainerRuntime { probed: Vec::new() }.to_string();
        assert!(message.contains("nothing to probe"), "{message}");
    }

    #[test]
    fn tst_102_probing_an_absent_endpoint_fails_rather_than_hanging() {
        // Port 1 on the loopback: nothing listens there, and the kernel refuses immediately.
        let reason = probe("tcp://127.0.0.1:1").unwrap_err();
        assert!(!reason.is_empty());
        // A path that cannot exist.
        let reason = probe("/nonexistent/cdm-testkit/docker.sock").unwrap_err();
        assert!(!reason.is_empty());
        // The `unix://` spelling takes the same path as a bare one.
        let reason = probe("unix:///nonexistent/cdm-testkit/docker.sock").unwrap_err();
        assert!(!reason.is_empty());
    }

    #[test]
    fn tst_102_an_unresolvable_authority_is_a_reason_not_a_panic() {
        let reason = probe("tcp://cdm-testkit.invalid:2375").unwrap_err();
        assert!(!reason.is_empty(), "{reason}");
    }

    #[test]
    fn tst_102_well_known_sockets_are_absolute_and_start_with_the_system_socket() {
        let sockets = well_known_sockets();
        assert_eq!(
            sockets.first().map(|p| p.display().to_string()).as_deref(),
            Some("/var/run/docker.sock")
        );
        for socket in &sockets {
            assert!(socket.is_absolute(), "{} is relative", socket.display());
        }
    }

    #[test]
    fn tst_102_the_runtime_names_where_it_was_found() {
        let runtime = ContainerRuntime {
            endpoint: "/var/run/docker.sock".to_owned(),
            source: RuntimeSource::WellKnownSocket,
        };
        assert_eq!(runtime.endpoint(), "/var/run/docker.sock");
        assert_eq!(runtime.source(), RuntimeSource::WellKnownSocket);
        assert_eq!(
            runtime.to_string(),
            "/var/run/docker.sock (from a well-known socket path)"
        );
        assert_eq!(RuntimeSource::DockerHost.as_str(), "DOCKER_HOST");
        assert_eq!(
            RuntimeSource::TestcontainersOverride.as_str(),
            "TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE"
        );
    }

    #[test]
    fn tst_102_detection_either_finds_a_runtime_or_explains_itself() {
        // Whichever way this machine is configured, `detect` must return a usable answer and
        // never panic — that is the whole contract the skip macro rests on.
        match ContainerRuntime::detect() {
            Ok(runtime) => assert!(!runtime.endpoint().is_empty()),
            Err(reason) => assert!(reason.to_string().contains("TST-102")),
        }
    }
}
