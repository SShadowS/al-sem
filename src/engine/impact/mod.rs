//! **Impact of a change on the current revision** (agent-tasks design §8,
//! milestone B1): `alsem impact` and `alsem_impact`.
//!
//! - [`scan`]: what each source line declares (fields, keys, global
//!   variables, properties, actions, layout, enum values, the object header).
//! - [`accounting`]: every changed line of a diff in exactly one bucket, and
//!   the diff checked against the analyzed text.
//! - [`compare`]: the comparison tuple, resolved once (`--base REV`: the
//!   merge base, every working-tree layer, untracked `.al` files).
//!
//! B1 reports CURRENT effects only: no base revision is built, so there is no
//! before/after and no routine is called added or deleted (that needs both
//! sides' complete inventories, milestone B2).

pub mod accounting;
pub mod compare;
pub mod jobs;
pub mod render;
pub mod run;
pub mod scan;
pub mod up;
