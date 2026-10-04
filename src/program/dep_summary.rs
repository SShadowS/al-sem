//! Per-file dependency summaries (spec §5): what the engine needs from a
//! dependency file, produced right after parsing so the syntax tree can go.
//! The format is the pack spec's `PackedFile`; nothing here persists it.

use al_syntax::ir::{AlFile, ParseStatus};

use crate::program::node::AppRef;
use crate::program::node_extract::extract_nodes;
use crate::program::pack::PackedFile;
use crate::program::resolve::decl_surface::file_routine_meta;
use crate::snapshot::{AppId, TrustTier};

/// One dependency app's summaries, in the app's file order.
pub struct DepUnitSummary {
    pub app: AppId,
    pub files: Vec<PackedFile>,
}

/// Summarize one parsed dependency file. Uses the SAME extraction
/// (`extract_nodes`) and the SAME `RoutineMeta` helper as the tree-based path,
/// so a summary carries exactly what today's build reads from the tree.
#[must_use]
pub fn summarize_file(
    app: AppRef,
    tier: TrustTier,
    virtual_path: &str,
    file: &AlFile,
) -> PackedFile {
    let mut objects = Vec::new();
    let mut routines = Vec::new();
    extract_nodes(app, file, tier, &mut objects, &mut routines);
    PackedFile {
        virtual_path: virtual_path.to_string(),
        parse_status_recovered: file.parse_status == ParseStatus::Recovered,
        objects,
        routines,
        routine_meta: file_routine_meta(app, file, virtual_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::abi_ingest::AbiCache;
    use crate::program::build::{
        build_dep_layer, build_program_graph_from_parsed,
        dedup_routines_preserving_genuine_overloads,
    };
    use crate::program::resolve::decl_surface::{DeclSurface, DepMetaMap};
    use crate::snapshot::compilation::CompilationContext;
    use crate::snapshot::provider::SourceRoot;
    use crate::snapshot::{AppSetSnapshot, AppUnit, Provenance, World, parse_snapshot};

    fn app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.to_string(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn unit(id: &AppId, tier: TrustTier, files: &[(&str, &str)]) -> AppUnit {
        AppUnit {
            id: id.clone(),
            provenance: Provenance {
                app: id.clone(),
                tier,
                content_hash: String::new(),
            },
            source: Some(SourceRoot {
                files: files
                    .iter()
                    .map(|(path, text)| crate::snapshot::embedded::SourceFile {
                        virtual_path: path.to_string(),
                        text: (*text).into(),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                tier,
                content_hash: String::new(),
            }),
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        }
    }

    const OVERLOADS: &str = r#"
codeunit 60000 "Over Cu"
{
    procedure F(a: Integer)
    begin
    end;

    procedure F(a: Text)
    begin
    end;
}
"#;
    /// Unbalanced `#if` forces `ParseStatus::Recovered` (see `snapshot::parse` tests).
    const BROKEN: &str = r#"
codeunit 60001 "Broken Cu"
{
    procedure Foo()
    begin
#if NEVER_CLOSED
        Bar();
    end;
}
"#;
    const PLAIN: &str = r#"
codeunit 60002 "Plain Cu"
{
    procedure Baz()
    begin
    end;
}
"#;
    const DEP_FILES: [(&str, &str); 3] = [
        ("Over.al", OVERLOADS),
        ("Broken.al", BROKEN),
        ("Plain.al", PLAIN),
    ];

    fn ws_unit(ws: &AppId) -> AppUnit {
        unit(
            ws,
            TrustTier::Workspace,
            &[(
                "Ws.al",
                "codeunit 50000 \"Ws Cu\"\n{\n    procedure One()\n    begin\n    end;\n}\n",
            )],
        )
    }

    #[test]
    fn summary_matches_tree_extraction_per_file() {
        let (ws, dep) = (app_id("Ws"), app_id("Dep"));
        let snap = AppSetSnapshot {
            apps: vec![
                ws_unit(&ws),
                unit(&dep, TrustTier::EmbeddedSource, &DEP_FILES),
            ],
            workspace_app: ws,
            world: World::Closed,
        };
        let parsed = parse_snapshot(&snap);
        let layer = build_dep_layer(&snap, &AbiCache::new(), &parsed);
        let app = layer.apps.find(&dep).unwrap();
        let pu = parsed.iter().find(|u| u.app == dep).unwrap();
        assert_eq!(pu.files.len(), 3);
        for pf in &pu.files {
            let s = summarize_file(app, pf.provenance.tier, &pf.virtual_path, &pf.file);
            let (mut o, mut r) = (Vec::new(), Vec::new());
            extract_nodes(app, &pf.file, pf.provenance.tier, &mut o, &mut r);
            assert_eq!(s.objects, o, "{}", pf.virtual_path);
            assert_eq!(s.routines, r, "{}", pf.virtual_path);
            assert_eq!(
                s.routine_meta,
                file_routine_meta(app, &pf.file, &pf.virtual_path),
                "{}",
                pf.virtual_path
            );
            let broken = pf.virtual_path == "Broken.al";
            // A recovered parse may legitimately yield no routines.
            assert!(broken || !s.routine_meta.is_empty(), "{}", pf.virtual_path);
            assert_eq!(s.parse_status_recovered, broken);
        }
    }

    /// Review Focus 3: the same non-primary app present TWICE (workspace
    /// multi-app source AND embedded dependency) must reduce, through the
    /// summaries + Step 4's sort/dedup, to exactly `build_dep_layer`'s nodes,
    /// and the summaries' meta must equal `build_split`'s frozen map.
    #[test]
    fn sibling_app_summaries_reduce_to_the_layer_nodes_and_frozen_meta() {
        let (ws, dep) = (app_id("Ws"), app_id("Dep"));
        let snap = AppSetSnapshot {
            apps: vec![
                ws_unit(&ws),
                unit(&dep, TrustTier::EmbeddedSource, &DEP_FILES),
                unit(&dep, TrustTier::Workspace, &DEP_FILES),
            ],
            workspace_app: ws.clone(),
            world: World::Closed,
        };
        let cache = AbiCache::new();
        let parsed = parse_snapshot(&snap);
        let layer = build_dep_layer(&snap, &cache, &parsed);

        let units: Vec<DepUnitSummary> = parsed
            .iter()
            .filter(|u| u.app != snap.workspace_app)
            .map(|u| {
                let app = layer.apps.find(&u.app).unwrap();
                DepUnitSummary {
                    app: u.app.clone(),
                    files: u
                        .files
                        .iter()
                        .map(|pf| {
                            summarize_file(app, pf.provenance.tier, &pf.virtual_path, &pf.file)
                        })
                        .collect(),
                }
            })
            .collect();
        assert_eq!(units.len(), 2, "the sibling must really appear twice");

        let mut objects = Vec::new();
        let mut routines = Vec::new();
        let mut dep_meta = DepMetaMap::new();
        for f in units.iter().flat_map(|u| &u.files) {
            objects.extend(f.objects.iter().cloned());
            routines.extend(f.routines.iter().cloned());
            dep_meta.extend(f.routine_meta.iter().cloned());
        }
        let raw_routines = routines.len();
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        objects.dedup_by(|a, b| a.id == b.id);
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        dedup_routines_preserving_genuine_overloads(&mut routines);

        assert!(routines.len() < raw_routines, "dedup must have fired");
        assert_eq!(objects, *layer.dep_objects);
        assert_eq!(routines, *layer.dep_routines);

        let graph = build_program_graph_from_parsed(&snap, &cache, &parsed);
        let primary = graph.apps.find(&ws).unwrap();
        let (_surface, frozen) = DeclSurface::build_split(&graph, &parsed, primary);
        assert_eq!(dep_meta, *frozen);
    }

    /// The tier-read `ctx.decl_surface()` and `ctx.recovered_files()` equal
    /// what the old all-units build gave: `DeclSurface::build` over a fresh
    /// `parse_snapshot` of every unit, and `recovered_file_paths` over it.
    /// Two dependency apps, one of them present twice (sibling), so the LAST
    /// unit carries routines no other unit has.
    #[test]
    fn tier_decl_surface_and_recovered_files_match_the_all_units_build() {
        use crate::program::profile::BuildProfile;
        use crate::program::resolve::full::build_context_from_snapshot;
        use crate::snapshot::parse::recovered_file_paths;

        let (ws, dep, other) = (app_id("Ws"), app_id("Dep"), app_id("Other"));
        let other_src = "codeunit 60100 \"Other Cu\"\n{\n    procedure Qux(a: Integer; var b: Text)\n    begin\n    end;\n}\n";
        let snap = AppSetSnapshot {
            apps: vec![
                ws_unit(&ws),
                unit(&dep, TrustTier::EmbeddedSource, &DEP_FILES),
                unit(&dep, TrustTier::Workspace, &DEP_FILES),
                unit(
                    &other,
                    TrustTier::EmbeddedSource,
                    &[("Other.al", other_src)],
                ),
            ],
            workspace_app: ws,
            world: World::Closed,
        };
        let ctx = build_context_from_snapshot(snap, BuildProfile::FULL).expect("context");
        let fresh = parse_snapshot(&ctx.snap);
        assert_eq!(fresh.len(), 4, "precondition: every unit parsed");

        // The old construction: one local tier over every parsed unit.
        let old = DeclSurface::build(&ctx.graph, &fresh);
        let new = ctx.decl_surface();
        let primary = ctx.primary_app_ref;
        let (_, old_dep_meta) = DeclSurface::build_split(&ctx.graph, &fresh, primary);
        assert_eq!(
            *ctx.dep_layer.dep_nodes.dep_meta, *old_dep_meta,
            "the tier's dep_meta is the old frozen tier, key for key"
        );
        let other_ref = ctx.graph.apps.find(&app_id("Other")).unwrap();
        let mut dep_ids = 0;
        for r in ctx.graph.routines.iter() {
            assert_eq!(new.get(&r.id), old.get(&r.id), "{:?}", r.id);
            if r.id.object.app != primary && old.get(&r.id).is_some() {
                dep_ids += 1;
            }
        }
        assert!(dep_ids > 0, "precondition: dependency routines resolve");
        assert!(
            old_dep_meta.keys().any(|id| id.object.app == other_ref),
            "precondition: the last unit has routines of its own"
        );

        let recovered = ctx.recovered_files();
        assert_eq!(recovered, recovered_file_paths(&fresh));
        assert_eq!(
            recovered,
            vec!["Dep::Broken.al".to_string(), "Dep::Broken.al".to_string()],
            "precondition: both copies of the broken dependency file"
        );
    }
}
