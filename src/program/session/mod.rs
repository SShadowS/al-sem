//! The incremental workspace model of the context-server design (§6, stage 4).
//!
//! [`classify`] holds the change-type classifier: the declaration projection of
//! one workspace file and the class of a revision.

pub mod classify;
