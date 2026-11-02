//! The resident session core (context-server design §4, §5; stage 2).
//!
//! - [`store`]: artifact snapshots and `ensure_prepared` (§4.2).
//! - [`revision`]: one built revision and the request path off it (§4.3,
//!   §4.4).
//! - [`manifest`]: the full input manifest, its stamp and the three-way
//!   check (§5.2).
//! - [`session`]: the session that keeps the published revision, checks
//!   freshness per request and refreshes through the stage-4
//!   `WorkspaceSession` (§5.1, §6).

pub mod manifest;
pub mod revision;
pub mod session;
pub mod store;
