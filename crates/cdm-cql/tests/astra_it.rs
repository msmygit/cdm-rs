//! Astra DB connectivity against a **real** database (`CON-003`, `CON-004`, `CON-005`,
//! `CON-020`..`CON-029`).
//!
//! Every other Astra test in this repository is a unit test over a synthesised bundle, a stubbed
//! DevOps response or a hand-built metadata document. Those prove that cdm-rs parses what Astra is
//! documented to send. They cannot prove that Astra sends it.
//!
//! This suite is the other half: the DevOps API answering a real database id, a real bundle
//! unpacking, the metadata service returning real host ids over mutual TLS, and the SNI proxy
//! routing a real CQL connection to one of them. Those four legs are the parts of `CON-020`..`029`
//! that no container can stand in for.
//!
//! ```text
//! CDM_ASTRA_DB_ID=... CDM_ASTRA_REGION=... CDM_ASTRA_TOKEN=AstraCS:... \
//!   cargo test -p cdm-cql --test astra_it -- --ignored --test-threads=1
//! ```
//!
//! # Skipping versus failing
//!
//! With no credentials configured the suite **skips**, exactly as `TST-102` has the container
//! suites skip without a runtime: a contributor without an Astra database must still see a green
//! `cargo test --workspace`. With credentials configured it **fails** rather than skips, because
//! at that point "cannot reach Astra" is the finding, not the reason to stay quiet.
//!
//! # Hibernation
//!
//! A serverless Astra database parks itself after about 48 hours idle. The first connection after
//! that does not fail fast — it resumes the database, which takes minutes, and until it finishes
//! the endpoint refuses or times out. A weekly job is *always* in that state.
//!
//! So the first connection is retried, patiently, and the wait is treated as the expected path
//! rather than an error: [`connect_waking`] logs the resume so a slow run reads as "the database
//! was asleep" instead of "the tests hung". Only after [`WAKE_BUDGET`] does it call it a failure.
//!
//! # Secrets
//!
//! The token is read into a [`Secret`], never printed, and never interpolated into an assertion
//! message (`SEC-001`). A panic message here goes into a public CI log.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::time::{Duration, Instant};

use cdm_config::CdmConfig;
use cdm_core::{Side, TableRef};
use cdm_cql::connect::{self, ClusterSession, ConnectionMode};
use cdm_cql::schema::introspect;
use uuid::Uuid;

/// How long to keep trying the first connection before calling it a failure.
///
/// DataStax documents a serverless resume as "a few minutes"; fifteen is generous enough that a
/// failure here means something is actually wrong, and short enough to stay well inside the job
/// timeout.
const WAKE_BUDGET: Duration = Duration::from_secs(900);

/// How long to wait between attempts while the database resumes.
///
/// Deliberately unhurried. Hammering a resuming database neither speeds it up nor tells us
/// anything, and each attempt costs a full TLS handshake against the proxy.
const WAKE_INTERVAL: Duration = Duration::from_secs(15);

/// The keyspace to work in, overridable because Astra names it at creation time.
fn keyspace() -> String {
    // An unset *repository variable* arrives as an empty string rather than an absent one, so
    // `unwrap_or_else` alone would yield a keyspace of "" and a syntax error three calls later.
    std::env::var("CDM_ASTRA_KEYSPACE")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| "default_keyspace".to_owned())
}

/// The credentials, or `None` when the suite should skip.
///
/// All three must be present. Two out of three is a misconfigured CI job rather than a machine
/// without credentials, and silently skipping would hide it — so that case is a panic naming the
/// variable that is missing.
fn credentials() -> Option<(String, String, String)> {
    let id = std::env::var("CDM_ASTRA_DB_ID")
        .ok()
        .filter(|v| !v.is_empty());
    let region = std::env::var("CDM_ASTRA_REGION")
        .ok()
        .filter(|v| !v.is_empty());
    let token = std::env::var("CDM_ASTRA_TOKEN")
        .ok()
        .filter(|v| !v.is_empty());

    match (id, region, token) {
        (Some(id), Some(region), Some(token)) => Some((id, region, token)),
        (None, None, None) => {
            eprintln!(
                "skipping: no Astra credentials configured. Set CDM_ASTRA_DB_ID, \
                 CDM_ASTRA_REGION and CDM_ASTRA_TOKEN to run this suite."
            );
            None
        }
        (id, region, token) => panic!(
            "Astra credentials are partly configured, which is a broken job rather than an \
             absent database: CDM_ASTRA_DB_ID is {}, CDM_ASTRA_REGION is {}, CDM_ASTRA_TOKEN is {}",
            present(id.as_ref()),
            present(region.as_ref()),
            present(token.as_ref()),
        ),
    }
}

/// Whether a variable was set — never its value, which for the token is a credential.
const fn present(value: Option<&String>) -> &'static str {
    if value.is_some() {
        "set"
    } else {
        "missing"
    }
}

/// A configuration that reaches Astra by downloading its bundle through the DevOps API
/// (`CON-004`).
///
/// This is the path with no file on disk: the database id and the token are the whole input, which
/// is also the shape an operator is most likely to use in automation.
fn astra_config(id: &str, region: &str, token: &str) -> CdmConfig {
    let mut config = CdmConfig::default();
    let origin = &mut config.connect.origin;
    let uuid: Uuid = id.parse().expect("CDM_ASTRA_DB_ID must be a UUID");
    origin.astra.database_id = Some(cdm_config::types::AstraDatabaseId::new(uuid));
    origin.astra.region = Some(region.to_owned());
    // `token` is the literal username Astra expects; the token itself is the password (`CON-003`).
    "token".clone_into(&mut origin.username);
    origin.password = cdm_config::Secret::new(token);
    config.schema.origin.keyspace_table = Some(format!("{}.cdm_astra_it", keyspace()));
    config
}

/// Connects, waiting out a hibernating database (see the module documentation).
async fn connect_waking(config: &CdmConfig) -> ClusterSession {
    let started = Instant::now();
    let mut attempt = 0_u32;
    let mut last: Option<String> = None;

    while started.elapsed() < WAKE_BUDGET {
        attempt += 1;
        match connect::connect(config, Side::Origin).await {
            Ok(session) => {
                if attempt > 1 {
                    eprintln!(
                        "connected on attempt {attempt} after {}s — the database was hibernating",
                        started.elapsed().as_secs()
                    );
                }
                return session;
            }
            Err(error) => {
                // `CdmError`'s Display never carries a secret (`SEC-001`), so this is safe to log.
                eprintln!(
                    "attempt {attempt} at {}s: {error}",
                    started.elapsed().as_secs()
                );
                last = Some(error.to_string());
                tokio::time::sleep(WAKE_INTERVAL).await;
            }
        }
    }

    panic!(
        "could not reach Astra within {}s and {attempt} attempts. A serverless database resuming \
         from hibernation should answer well inside that. Last error: {}",
        WAKE_BUDGET.as_secs(),
        last.unwrap_or_else(|| "none recorded".to_owned())
    )
}

/// The one test that wakes the database; everything else reuses its session.
///
/// Kept as a single test on purpose. Cargo gives each `#[test]` no ordering guarantee, and three
/// tests each independently waiting out a fifteen-minute resume is three quarters of an hour of CI
/// for one database. One test, several assertions, one wake.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real Astra DB; run with --ignored and CDM_ASTRA_* set"]
async fn con_020_a_real_astra_database_is_reachable_end_to_end() {
    let Some((id, region, token)) = credentials() else {
        return;
    };
    let config = astra_config(&id, &region, &token);

    // CON-004, CON-005, CON-020..CON-026: DevOps bundle download, unpack into a 0700 directory,
    // metadata handshake over mutual TLS, SNI-routed CQL connection. All four legs are exercised
    // by this one call, and none of them can be stood up locally.
    let session = connect_waking(&config).await;

    assert_eq!(
        session.mode(),
        &ConnectionMode::BundleDownload {
            database_id: id.clone()
        },
        "a database id with no bundle path must resolve to the download path"
    );
    assert!(
        !session.local_datacenter().is_empty(),
        "CON-024: the local datacenter comes from the bundle, not from a guess"
    );

    // CON-029: the capability probe against a real Astra release, which is neither an OSS version
    // string nor a Scylla one. Vectors are supported on current Astra; asserting the probe
    // *answered* rather than what it answered keeps this from breaking on an Astra upgrade.
    let capabilities = session.capabilities();
    assert!(
        !capabilities.release_version.is_empty(),
        "CON-029: the probe must report a release version"
    );

    round_trip(&session).await;
    introspection(&session).await;
}

/// Writes and reads a row through the SNI proxy.
///
/// A connection that completes its handshake and then cannot carry a query is a connection that
/// looks fine in `cdm connect test` and fails in a migration. The write matters more than the
/// read: Astra's proxy routes each connection by TLS `server_name`, so a coordinator that never
/// received traffic is the failure this catches.
async fn round_trip(session: &ClusterSession) {
    let keyspace = keyspace();
    let table = format!("{keyspace}.cdm_astra_it");
    let driver = session.session();

    driver
        .query_unpaged(
            format!("CREATE TABLE IF NOT EXISTS {table} (k text PRIMARY KEY, v int)"),
            &[],
        )
        .await
        .expect("Astra must accept DDL in the configured keyspace");

    // A value that identifies this run, so a leaked row from a failed cleanup is traceable to a
    // job rather than anonymous. No credential material, and nothing that would be a secret in a
    // public log.
    let key = format!("run-{}", std::process::id());

    driver
        .query_unpaged(
            format!("INSERT INTO {table} (k, v) VALUES (?, ?)"),
            (&key, 42),
        )
        .await
        .expect("a write must reach a coordinator through the SNI proxy");

    let read = driver
        .query_unpaged(format!("SELECT v FROM {table} WHERE k = ?"), (&key,))
        .await
        .expect("a read must reach a coordinator through the SNI proxy")
        .into_rows_result()
        .expect("the read must return rows");
    let (value,): (i32,) = read
        .first_row()
        .expect("the row just written must be readable");
    assert_eq!(value, 42, "the value written must be the value read back");

    driver
        .query_unpaged(format!("DELETE FROM {table} WHERE k = ?"), (&key,))
        .await
        .expect("cleanup must succeed");
}

/// Introspects the schema through the Astra connection (`SCH-001`, `SCH-002`).
///
/// Astra restricts parts of `system_schema` that a self-managed cluster exposes freely, which is
/// exactly the kind of difference a container cannot reproduce and a migration would hit on its
/// first table.
async fn introspection(session: &ClusterSession) {
    let keyspace = keyspace();
    let table = TableRef::new(&keyspace, "cdm_astra_it");

    let snapshot = introspect::fetch_table(Side::Origin, session.session(), &table)
        .await
        .expect("SCH-001: introspection must succeed on Astra")
        .expect("the table just created must be found");

    assert_eq!(
        snapshot.partition_key().len(),
        1,
        "the table has one partition key column"
    );
    assert!(
        snapshot.column("v").is_some(),
        "SCH-002: a declared column must appear in the snapshot"
    );
}
