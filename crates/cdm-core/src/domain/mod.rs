//! The vocabulary of the domain: token ranges, run identity, records and keys.
//!
//! Everything here is a plain value type. Nothing in this module performs I/O, allocates a
//! runtime, or knows that a CQL driver exists. Fallible constructors return the crate-wide
//! [`CdmError`](crate::CdmError) rather than bespoke error types, because `ERR-001` mandates a
//! single error enum.

pub mod record;
pub mod run;
pub mod token;

pub use record::{ColumnRef, PrimaryKey, RawCell, Record, Row, TableRef};
pub use run::{JobKind, RunId, RunIdGenerator, RunStatus, Side};
pub use token::{PartitionRangeId, TokenRange};
