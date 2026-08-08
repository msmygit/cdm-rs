//! Who a node says it is (`DST-001`).

use std::fmt;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind};

/// The longest node identity cdm-rs will write.
///
/// The value goes into a `text` column that an operator reads while diagnosing a stuck range, and
/// the default is a host name and a process id. A kilobyte of identity is a mistake, not a
/// requirement, and the cap catches it where it is made rather than in the lease table.
const MAX_NODE_ID: usize = 128;

/// A node's identity in the lease table (`DST-010`).
///
/// Two nodes that share an identity are, to every conditional write in this crate, one node: each
/// could renew the other's lease and each would believe it holds a range the other is processing.
/// The type therefore exists to be *validated* — it is not a newtype for its own sake — and the
/// defaulting rule that makes collisions unlikely lives in `cdm-config`, which combines the host
/// name with the process id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(String);

impl NodeId {
    /// Validates a node identity.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if the identity is empty, longer than 128 characters, or
    /// contains a control character. Control characters are refused because the identity is
    /// echoed into logs and into `cdm cluster status`, where an embedded newline turns one node's
    /// name into a forged second line.
    pub fn new(value: impl Into<String>) -> Result<Self, CdmError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(config_error(
                "cluster.node_id is empty; leave it unset to have the host name and process id \
                 used, or give this node a name unique across the run",
            ));
        }
        if trimmed.chars().count() > MAX_NODE_ID {
            return Err(config_error(
                "cluster.node_id is longer than 128 characters; it names a node in diagnostics, \
                 not a document",
            ));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(config_error(
                "cluster.node_id contains a control character; it is printed in logs and in \
                 `cdm cluster status`, where that would forge a second line",
            ));
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// This node's identity, as `cluster.node_id` resolved it (`DST-001`).
    ///
    /// `cdm-config` has already defaulted the value to the host name and the process id, so this
    /// only validates what came out.
    ///
    /// # Errors
    ///
    /// As [`NodeId::new`].
    pub fn from_config(config: &EffectiveConfig) -> Result<Self, CdmError> {
        Self::new(config.node_id())
    }

    /// The identity as written to `cdm_run_leases.node_id`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn config_error(message: &str) -> CdmError {
    CdmError::new(ErrorKind::Config, message.to_owned())
        .with_context(|ctx| ctx.with_config_key("cluster.node_id"))
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
    use cdm_config::CdmConfig;

    use super::*;

    #[test]
    fn dst_001_a_node_identity_is_trimmed_and_non_empty() {
        assert_eq!(NodeId::new("  node-a  ").unwrap().as_str(), "node-a");
        assert_eq!(NodeId::new("node-a").unwrap().to_string(), "node-a");
        assert_eq!(NodeId::new("   ").unwrap_err().kind(), ErrorKind::Config);
        assert_eq!(NodeId::new("").unwrap_err().kind(), ErrorKind::Config);
    }

    #[test]
    fn dst_001_an_identity_that_could_forge_a_log_line_is_refused() {
        assert!(NodeId::new("node-a\nnode-b").is_err());
        assert!(NodeId::new("node\u{7}a").is_err());
        assert!(NodeId::new("n".repeat(129)).is_err());
        assert!(NodeId::new("n".repeat(128)).is_ok());
    }

    #[test]
    fn dst_001_the_default_identity_comes_from_the_resolved_configuration() {
        let config = EffectiveConfig::resolve(CdmConfig::default());
        let node = NodeId::from_config(&config).unwrap();
        assert_eq!(node.as_str(), config.node_id());
        assert!(!node.as_str().is_empty());
    }
}
