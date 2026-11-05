//! **Test selection** (agent-tasks design §4, milestone C): which tests to
//! run first for a change, with the scope they were chosen from, every
//! changed routine's status, and whether the selection is complete.
//!
//! - [`universe`]: a test universe (a test app with the production app loaded
//!   as an injected source dependency) and the production stamp both sides
//!   compare.

pub mod universe;
