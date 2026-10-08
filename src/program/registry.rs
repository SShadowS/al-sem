//! The dependency target registry (engine-switch S2b.5; spec G15a).
//!
//! What the engine knows about a DEPENDENCY routine a workspace call or event can
//! reach: its declaration (parameters with names, types, `var`, `temporary`) and
//! the state of its body. It is a view over the program graph and the dependency
//! declaration metadata, kept SEPARATE from the detector model's routine
//! population: describing a dependency target here does not make the detectors
//! analyse its body (that is S7/S8).
//!
//! The body state answers one question the effect analysis must never get wrong:
//! may an empty set of facts for this routine be read as "has no effects"? Only
//! [`BodyState::AnalyzedClean`] says yes. A body that exists but was not analysed
//! is NOT clean, and calling it bodyless would lie to coverage.

use std::collections::HashSet;

use crate::program::graph::ProgramGraph;
use crate::program::node::RoutineNodeId;
use crate::program::node_extract::{AbiParams, RoutineNode};
use crate::program::resolve::decl_surface::DepMetaMap;
use crate::snapshot::identity::TrustTier;

/// The state of a dependency routine's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyState {
    /// The body was parsed cleanly and its facts are in the analysed model.
    AnalyzedClean,
    /// The body exists but its parse needed error recovery: content may be missing,
    /// so its facts can never prove absence.
    Recovered,
    /// Symbol-only: no body exists to analyse.
    Bodyless,
    /// The body exists and parsed, but has not been analysed (today: every
    /// source-bearing dependency routine, until S7/S8).
    NotAnalyzed,
}

impl BodyState {
    /// True only when an empty fact set means "no effects".
    #[must_use]
    pub fn proves_absence(self) -> bool {
        self == BodyState::AnalyzedClean
    }
}

/// One declared parameter of a dependency target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetParam {
    pub name: String,
    pub ty: Option<String>,
    pub by_ref: bool,
    /// `None` when the declaration does not say (a source parameter without type).
    pub temporary: Option<bool>,
}

/// A dependency target's parameter list, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetParams {
    Known(Vec<TargetParam>),
    /// No metadata (the symbol reference's parameter list was absent or broken,
    /// or a source declaration's metadata is missing).
    Unknown,
    /// The node is the arbitrary survivor of colliding ABI overloads: its
    /// parameters belong to one of several declarations, so they are not used.
    Untrusted,
}

/// What the registry says about one dependency routine.
#[derive(Debug, Clone)]
pub struct DepTarget<'g> {
    pub node: &'g RoutineNode,
    pub params: TargetParams,
    pub body: BodyState,
}

/// The registry over one built program (see the module doc).
pub struct DependencyRegistry<'g> {
    graph: &'g ProgramGraph,
    dep_meta: &'g DepMetaMap,
    /// Dependency routines whose bodies the model analysed. Empty until S7/S8.
    analyzed: HashSet<RoutineNodeId>,
    /// Every graph routine by id (first wins on a shared id, graph order).
    by_id: std::collections::HashMap<&'g RoutineNodeId, &'g RoutineNode>,
}

impl<'g> DependencyRegistry<'g> {
    #[must_use]
    pub fn new(graph: &'g ProgramGraph, dep_meta: &'g DepMetaMap) -> Self {
        let mut by_id = std::collections::HashMap::new();
        for n in graph.routines.iter() {
            by_id.entry(&n.id).or_insert(n);
        }
        DependencyRegistry {
            graph,
            dep_meta,
            analyzed: HashSet::new(),
            by_id,
        }
    }

    /// Mark the dependency routines whose bodies the model analysed (S7/S8).
    #[must_use]
    pub fn with_analyzed(mut self, analyzed: HashSet<RoutineNodeId>) -> Self {
        self.analyzed = analyzed;
        self
    }

    /// The registry's view of `node`; `None` for a workspace routine.
    #[must_use]
    pub fn describe(&self, node: &'g RoutineNode) -> Option<DepTarget<'g>> {
        if node.tier == TrustTier::Workspace {
            return None;
        }
        let meta = self.dep_meta.get(&node.id);
        let params = if node.tier == TrustTier::SymbolOnly {
            match &node.abi_params {
                AbiParams::Complete(ps) => TargetParams::Known(
                    ps.iter()
                        .map(|p| TargetParam {
                            name: p.name.to_string(),
                            ty: Some(p.type_text.to_string()),
                            by_ref: p.is_var,
                            temporary: Some(p.is_temporary),
                        })
                        .collect(),
                ),
                AbiParams::Missing => TargetParams::Unknown,
                AbiParams::CollapsedUntrusted => TargetParams::Untrusted,
            }
        } else {
            match meta {
                Some(m) => TargetParams::Known(
                    m.params
                        .iter()
                        .map(|p| TargetParam {
                            name: p.name.to_string(),
                            ty: p.ty.as_deref().map(str::to_string),
                            by_ref: p.by_ref,
                            temporary: p.is_temporary(),
                        })
                        .collect(),
                ),
                None => TargetParams::Unknown,
            }
        };
        let body = if node.tier == TrustTier::SymbolOnly {
            BodyState::Bodyless
        } else if meta.is_some_and(|m| m.parse_incomplete) {
            BodyState::Recovered
        } else if self.analyzed.contains(&node.id) {
            BodyState::AnalyzedClean
        } else {
            BodyState::NotAnalyzed
        };
        Some(DepTarget { node, params, body })
    }

    /// The registry's view of the routine with `id`; `None` when it is not a
    /// dependency routine of this graph.
    #[must_use]
    pub fn target(&self, id: &RoutineNodeId) -> Option<DepTarget<'g>> {
        let node = *self.by_id.get(id)?;
        self.describe(node)
    }

    /// Every dependency routine, in graph order.
    pub fn targets(&self) -> impl Iterator<Item = DepTarget<'g>> + '_ {
        self.graph.routines.iter().filter_map(|n| self.describe(n))
    }

    /// `(state, count)` over every dependency routine, in a fixed state order.
    #[must_use]
    pub fn census(&self) -> Vec<(BodyState, usize)> {
        let order = [
            BodyState::AnalyzedClean,
            BodyState::Recovered,
            BodyState::Bodyless,
            BodyState::NotAnalyzed,
        ];
        let mut counts = [0usize; 4];
        for t in self.targets() {
            counts[order.iter().position(|s| *s == t.body).unwrap()] += 1;
        }
        order.into_iter().zip(counts).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Body states on a real cross-app fixture: the symbol-only routine is
    /// bodyless, the source-bearing ones are NOT analysed (and so prove nothing),
    /// and no workspace routine is a dependency target.
    #[test]
    fn body_states_on_the_cross_app_fixture() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r3a5-fixtures/ws");
        let (ctx, _r, _) = crate::program::resolve::full::build_program_with_coverage(&ws).unwrap();
        let reg = ctx.registry();
        let by_name = |n: &str| reg.targets().find(|t| t.node.name == n).unwrap().body;
        assert_eq!(by_name("DoSomething"), BodyState::Bodyless);
        assert_eq!(by_name("DoWrite"), BodyState::NotAnalyzed);
        assert_eq!(by_name("DoIt"), BodyState::NotAnalyzed);
        assert!(reg.targets().all(|t| !t.body.proves_absence()));
        assert!(
            ctx.graph()
                .routines
                .iter()
                .filter(|n| n.tier == TrustTier::Workspace)
                .all(|n| reg.describe(n).is_none())
        );
        // Marking a routine analysed is the only way to clean.
        let id = reg
            .targets()
            .find(|t| t.node.name == "DoWrite")
            .unwrap()
            .node
            .id
            .clone();
        let reg = ctx.registry().with_analyzed([id.clone()].into());
        assert_eq!(reg.target(&id).unwrap().body, BodyState::AnalyzedClean);
        let census: usize = reg.census().iter().map(|(_, n)| n).sum();
        assert_eq!(census, reg.targets().count());
    }

    /// Hand-stated symbol-only dependency: `Post(var Rec: Record Customer
    /// temporary; Qty: Integer)`. The registry keeps both names, `var` and the
    /// `temporary` marker — ingestion used to drop the names and the marker.
    #[test]
    fn symbol_only_target_keeps_parameter_names_and_temporary() {
        use crate::engine::deps::app_package_zip::test_apps;
        let root = std::env::temp_dir().join(format!("registry-s2b5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".alpackages")).unwrap();
        let dep = "dddddddd-0005-0000-0000-00000000b2b5";
        std::fs::write(
            root.join("app.json"),
            format!(
                r#"{{"id":"11111111-0000-0000-0000-00000000b2b5","name":"Host","publisher":"P","version":"1.0.0.0","dependencies":[{{"id":"{dep}","name":"Dep","publisher":"Microsoft","version":"1.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("src/Host.al"),
            "codeunit 70000 Host { procedure A() begin end; }",
        )
        .unwrap();
        let symbols = br#"{"Codeunits":[{"Id":50500,"Name":"Dep Util","Methods":[{"Name":"Post","Parameters":[{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Temporary":true,"Subtype":{"Name":"Customer","Id":18}}},{"Name":"Qty","IsVar":false,"TypeDefinition":{"Name":"Integer"}}]}]}]}"#;
        let manifest = test_apps::manifest_xml(dep, "Dep");
        std::fs::write(
            root.join(".alpackages/dep.app"),
            test_apps::build_app(&[
                ("NavxManifest.xml", manifest.as_bytes()),
                ("SymbolReference.json", symbols),
            ]),
        )
        .unwrap();

        let (ctx, _r, _) =
            crate::program::resolve::full::build_program_with_coverage(&root).unwrap();
        let reg = ctx.registry();
        let post = reg
            .targets()
            .find(|t| t.node.name == "Post")
            .expect("dep routine");
        let body = post.body;
        let params = post.params.clone();
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(body, BodyState::Bodyless);
        let TargetParams::Known(ps) = params else {
            panic!("params: {params:?}")
        };
        let got: Vec<(&str, bool, Option<bool>)> = ps
            .iter()
            .map(|p| (p.name.as_str(), p.by_ref, p.temporary))
            .collect();
        assert_eq!(
            got,
            vec![("Rec", true, Some(true)), ("Qty", false, Some(false))]
        );
    }

    /// Source-bearing side: `RoutineMeta` keeps each parameter's name, and the
    /// `temporary` marker is read from the type text the lowerer keeps.
    #[test]
    fn source_param_meta_keeps_name_and_temporary() {
        let ir = al_syntax::parse(
            "codeunit 50100 X { procedure P(var Rec: Record Customer temporary; Qty: Integer) begin end; }",
        );
        let meta = crate::program::resolve::decl_surface::RoutineMeta::from_decl(
            &ir.objects[0].routines[0],
            "src/X.al",
        );
        let got: Vec<(&str, Option<bool>)> = meta
            .params
            .iter()
            .map(|p| (p.name.as_str(), p.is_temporary()))
            .collect();
        assert_eq!(got, vec![("Rec", Some(true)), ("Qty", Some(false))]);
    }
}
