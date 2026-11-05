//! **Test selection** (agent-tasks design §4, milestone C): which tests to
//! run first for a change, with the scope they were chosen from, every
//! changed routine's status, and whether the selection is complete.
//!
//! - [`scope`]: which test code is considered (the workspace's own `[Test]`
//!   procedures and the sibling test apps: named, configured or discovered,
//!   bounded).
//! - [`universe`]: a test universe (a test app with the production app loaded
//!   as an injected source dependency) and the production stamp both sides
//!   compare.
//! - [`select`]: the reverse walk over one test universe's sited graph.
//! - [`jobs`]: the roots part (production model) and the select part (test
//!   universe's sited graph) as jobs.
//! - [`run`]: one answer, part by part (the CLI and the MCP tool alike).
//! - [`render`]: the answer, human or `al-sem.tests/1`.

pub mod jobs;
pub mod render;
pub mod run;
pub mod scope;
pub mod select;
pub mod universe;
