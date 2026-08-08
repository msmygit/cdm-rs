//! Building sessions, and the policies they run under (`CON-001`..`CON-013`).
//!
//! | Module | Requirement |
//! |---|---|
//! | [`mode`] | which of the four connection modes a side uses (`CON-002`) |
//! | [`session`] | building the session itself (`CON-001`, `CON-009`) |
//! | [`policy`] | load balancing, speculative execution, retries (`CON-009`..`CON-012`) |
//! | [`probe`] | the start-up capability probe (`CON-013`) |
//!
//! Origin and target are built independently, by separate calls to [`session::connect`], and
//! share nothing (`CON-001`).

pub mod mode;
pub mod policy;
pub mod probe;
pub mod session;

pub use mode::ConnectionMode;
pub use policy::{Backoff, CdmRetryPolicy, SpeculativeSettings};
pub use probe::{Capabilities, Flavour};
pub use session::{connect, ClusterNode, ClusterSession};
