//! `cdm connect test` — the first command to run against a new configuration (`CON-008`,
//! `CON-029`).
//!
//! It performs the same connect a job performs, through the same `cdm-cql` code path, and then
//! reports what was negotiated rather than "OK". The distinction matters: almost every connectivity
//! ticket in the Java tool is really "it connected to the wrong datacenter", "it fell back to a
//! single endpoint", or "the local DC was auto-detected as something surprising" — none of which a
//! boolean can express, and all of which are visible in the capabilities the driver already holds.
//!
//! # It never touches the migrated table
//!
//! Only `system.local`, and only through [`cdm_cql::connect`]'s existing probe. An operator running
//! this against production before a migration is not reading rows, and there is deliberately no
//! code path here that could.

use std::io::Write;

use cdm_core::{CdmError, ErrorKind, Side};
use cdm_cql::connect::{self, ClusterSession};
use serde::Serialize;

use crate::cli::{ConfigArgs, SideArg};
use crate::loader::load;
use crate::output::Report;

/// What `cdm connect test` reports (`CON-008`).
#[derive(Debug, Serialize)]
pub struct ConnectReport {
    /// One entry per side tested, origin first.
    pub sides: Vec<SideReport>,
}

/// One side's negotiated connection (`CON-008`, `CON-029`).
#[derive(Debug, Serialize)]
pub struct SideReport {
    /// `origin` or `target`.
    pub side: String,
    /// How the connection was made: `standard`, `astra`, and so on (`CON-020`).
    pub mode: String,
    /// The contact points actually used. Hosts and ports only — never a credential (`SEC-001`).
    pub contact_points: Vec<String>,
    /// The datacenter treated as local, whether configured or auto-detected (`CON-009`).
    pub local_datacenter: String,
    /// The cluster's name, as `system.local` reports it.
    pub cluster_name: String,
    /// Which implementation this is: Cassandra, DSE, ScyllaDB.
    pub flavour: String,
    /// The release version.
    pub release_version: String,
    /// The CQL version.
    pub cql_version: String,
    /// The negotiated native protocol version, when the driver exposes it.
    ///
    /// `None` today, and honestly so: `scylla-rust-driver` does not surface the negotiated version,
    /// and printing a guess would make this diagnostic worse than useless. `CON-008` asks for it,
    /// so the field exists and will populate the moment the driver offers it.
    pub native_protocol_version: Option<u8>,
    /// The partitioner, which decides how the ring is split (`TOK-001`).
    pub partitioner: String,
    /// The datacenter the contacted node is in.
    pub datacenter: String,
    /// The rack the contacted node is in.
    pub rack: String,
    /// Whether `vector<t, n>` is available (`CDC-004`).
    pub supports_vectors: bool,
    /// Whether `WRITETIME`/`TTL` may be applied to a collection column.
    pub supports_collection_writetime: bool,
    /// The Astra diagnostic of `CON-029`, for an Astra side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub astra: Option<AstraReport>,
}

/// The Astra half of the diagnostic (`CON-029`).
///
/// Its own section because Astra failures have their own shape: the CQL connection is fine and the
/// *metadata service* is not, or the bundle resolved to a single endpoint when SNI was expected.
/// Neither is visible in anything above.
#[derive(Debug, Serialize)]
pub struct AstraReport {
    /// The strategy actually in force: `sni` or `single_endpoint`.
    pub strategy: String,
    /// Where the bundle came from: a path, or a DevOps download URL.
    pub bundle_origin: String,
    /// The metadata service URL, whether or not it answered.
    pub metadata_url: String,
    /// The proxy address the metadata service reported, when it answered.
    pub sni_proxy_address: Option<String>,
    /// The local datacenter the metadata service reported.
    pub local_dc: Option<String>,
    /// How many host ids it reported, which is the size of the Astra database.
    pub host_id_count: usize,
    /// Why SNI was not used, when it was not. The single most useful line in this report when the
    /// strategy is not what the operator expected.
    pub sni_unavailable_reason: Option<String>,
}

impl Report for ConnectReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        for side in &self.sides {
            writeln!(out, "{} — connected via {}", side.side, side.mode)?;
            writeln!(out, "  cluster:           {}", side.cluster_name)?;
            writeln!(
                out,
                "  version:           {} {} (CQL {})",
                side.flavour, side.release_version, side.cql_version
            )?;
            writeln!(
                out,
                "  protocol:          {}",
                side.native_protocol_version.map_or_else(
                    || "not reported by the driver".to_owned(),
                    |v| format!("v{v}")
                )
            )?;
            writeln!(
                out,
                "  contact points:    {}",
                side.contact_points.join(", ")
            )?;
            writeln!(out, "  local datacenter:  {}", side.local_datacenter)?;
            writeln!(
                out,
                "  contacted node:    dc {}, rack {}",
                side.datacenter, side.rack
            )?;
            writeln!(out, "  partitioner:       {}", side.partitioner)?;
            writeln!(
                out,
                "  vectors:           {}",
                yes_no(side.supports_vectors)
            )?;
            writeln!(
                out,
                "  collection wt/ttl: {}",
                yes_no(side.supports_collection_writetime)
            )?;

            if let Some(astra) = &side.astra {
                writeln!(out, "  Astra:")?;
                writeln!(out, "    strategy:        {}", astra.strategy)?;
                writeln!(out, "    bundle:          {}", astra.bundle_origin)?;
                writeln!(out, "    metadata URL:    {}", astra.metadata_url)?;
                writeln!(
                    out,
                    "    SNI proxy:       {}",
                    astra.sni_proxy_address.as_deref().unwrap_or("—")
                )?;
                writeln!(
                    out,
                    "    local DC:        {}",
                    astra.local_dc.as_deref().unwrap_or("—")
                )?;
                writeln!(out, "    host ids:        {}", astra.host_id_count)?;
                if let Some(reason) = &astra.sni_unavailable_reason {
                    writeln!(out, "    SNI unavailable: {reason}")?;
                }
            }
            writeln!(out)?;
        }
        writeln!(out, "{} side(s) reachable.", self.sides.len())
    }
}

/// Connects to the requested side or sides and reports what was negotiated (`CON-008`, `CON-029`).
///
/// # Errors
///
/// [`ErrorKind::Config`] for a configuration that cannot be assembled, and whatever
/// [`cdm_cql::connect::connect`] returns for a side that cannot be reached — [`ErrorKind::Connect`],
/// [`ErrorKind::Auth`] or [`ErrorKind::Tls`], carrying which side failed (`ERR-002`). Failing is
/// the *point* of this command, so the error is the result rather than an interruption of it.
pub fn test(args: &ConfigArgs, side: SideArg) -> Result<ConnectReport, CdmError> {
    let outcome = load(args)?;
    let Some(config) = outcome.config else {
        return Err(CdmError::new(
            ErrorKind::Config,
            "the configuration could not be assembled; run `cdm config validate` to see why",
        ));
    };

    let sides: Vec<Side> = match side {
        SideArg::Origin => vec![Side::Origin],
        SideArg::Target => vec![Side::Target],
        SideArg::Both => vec![Side::Origin, Side::Target],
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot start the async runtime: {error}"),
            )
        })?;

    runtime.block_on(async {
        let mut reports = Vec::with_capacity(sides.len());
        for side in sides {
            // Sequential and origin-first for the reason `Sessions::open` gives: when both sides
            // are misconfigured, the operator gets the diagnostic they can act on first.
            let session = connect::connect(&config, side).await?;
            reports.push(describe(side, &session));
        }
        Ok(ConnectReport { sides: reports })
    })
}

/// Turns a live session into the report, reading only what the driver already holds.
fn describe(side: Side, session: &ClusterSession) -> SideReport {
    let capabilities = session.capabilities();
    SideReport {
        side: side.as_str().to_owned(),
        mode: session.mode().as_str().to_owned(),
        contact_points: session.contact_points().to_vec(),
        local_datacenter: session.local_datacenter().to_owned(),
        cluster_name: capabilities.cluster_name.clone(),
        flavour: capabilities.flavour.as_str().to_owned(),
        release_version: capabilities.release_version.clone(),
        cql_version: capabilities.cql_version.clone(),
        native_protocol_version: capabilities.native_protocol_version,
        partitioner: capabilities.partitioner.clone(),
        datacenter: capabilities.datacenter.clone(),
        rack: capabilities.rack.clone(),
        supports_vectors: capabilities.supports_vectors,
        supports_collection_writetime: capabilities.supports_collection_writetime,
        astra: session.astra().map(|astra| AstraReport {
            strategy: astra.strategy.as_str().to_owned(),
            bundle_origin: astra.bundle_origin.clone(),
            metadata_url: astra.metadata_url.clone(),
            sni_proxy_address: astra.sni_proxy_address.clone(),
            local_dc: astra.local_dc.clone(),
            host_id_count: astra.host_ids.len(),
            sni_unavailable_reason: astra.sni_unavailable_reason.clone(),
        }),
    }
}

const fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
