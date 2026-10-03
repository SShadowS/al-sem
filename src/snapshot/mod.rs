//! App-set snapshot ingestion substrate (Spec 1 / Plan 1A).
//!
//! Turns "workspace + symbol-only dep tables" into an explicit set of
//! identity-verified, per-app source roots ready for deep resolution.

pub mod cache;
pub mod compilation;
pub mod embedded;
pub mod identity;
pub mod parse;
pub mod provider;
#[allow(clippy::module_inception)]
pub mod snapshot;
pub mod verify;

pub use identity::{AppId, Provenance, TrustTier};
pub use parse::{ParsedFile, ParsedUnit, parse_snapshot};
pub use snapshot::{AppSetSnapshot, AppUnit, DependencySource, SnapshotBuilder, World};
