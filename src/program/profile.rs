//! What a graph build must keep (spec §4).
//!
//! Each tool states what it needs; nothing is chosen by default. `FULL` keeps
//! every fact the engine can produce and is today's behaviour exactly. Smaller
//! profiles may skip a fact, never change one.

/// Dependency syntax trees (the bodies of dependency routines).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DependencyBodies {
    /// Keep a per-file summary (nodes, `RoutineMeta`, parse status) and drop
    /// each tree as soon as it is summarized.
    Summary,
    /// Keep the summaries AND every tree, shared in the dependency tier, so an
    /// analysis can walk dependency code.
    Keep,
}

/// What a build must keep. Deliberately no `Default`: every call site states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BuildProfile {
    pub dependency_bodies: DependencyBodies,
}

impl BuildProfile {
    /// Everything. `alsem`, `aldump` and every tool unless it declares less.
    pub const FULL: BuildProfile = BuildProfile {
        dependency_bodies: DependencyBodies::Keep,
    };
    /// The LSP server's needs.
    pub const LIGHT: BuildProfile = BuildProfile {
        dependency_bodies: DependencyBodies::Summary,
    };
}
