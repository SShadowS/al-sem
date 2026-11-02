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
    /// Keep the summaries; the shared tier keeps no tree (it is the `Summary`
    /// tier, and shares its `DepKey`). A build's own context holds the
    /// dependency trees only when it parsed them: a tier miss parses every
    /// dependency file anyway and hands the trees to the context, and on a hit
    /// a step that reads them asks first
    /// (`ProgramContext::ensure_dep_bodies`). They die with that context.
    /// The resident engine's `--low-mem` (context-server design §7): a
    /// workspace session then holds no dependency tree between revisions.
    OnDemand,
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
    /// `FULL`'s facts, with the dependency trees parsed per build instead of
    /// kept in the tier ([`DependencyBodies::OnDemand`]): a workspace
    /// session's `--low-mem` mode.
    pub const ON_DEMAND: BuildProfile = BuildProfile {
        dependency_bodies: DependencyBodies::OnDemand,
    };

    /// Whether a build of this profile reads dependency trees (now or when a
    /// step asks for them).
    #[must_use]
    pub fn reads_dependency_trees(self) -> bool {
        matches!(
            self.dependency_bodies,
            DependencyBodies::Keep | DependencyBodies::OnDemand
        )
    }
}
