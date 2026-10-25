//! LSP-surface infrastructure for the T3 program-engine migration.
//!
//! Home for the pieces that let the server talk to the fresh program-engine
//! backend: position-encoding negotiation (H-12, this task) today, and the
//! snapshot/updater/handlers modules later tasks add alongside it.

pub mod custom;
pub mod def_surface;
pub mod diagnostics;
pub mod encoding;
pub mod handlers;
pub mod l4_host;
pub mod lens;
pub mod snapshot;
pub mod updater;
