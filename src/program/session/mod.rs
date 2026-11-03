//! The incremental workspace model of the context-server design (§6, stage 4).
//!
//! [`classify`] holds the change-type classifier: the declaration projection of
//! one workspace file and the class of a revision. [`workspace`] holds the
//! session: the retained state and the refresh that turns it, with the next
//! revision's inputs, into the next revision's model.

pub mod classify;
pub(crate) mod dep_record;
pub mod workspace;

pub use workspace::{MemoryMode, RefreshReport, WorkspaceSession};
