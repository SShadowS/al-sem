//! Per-file dependency summaries (spec §5): what the engine needs from a
//! dependency file, produced right after parsing so the syntax tree can go.
//! The format is the pack spec's `PackedFile`; nothing here persists it.

use al_syntax::ir::{AlFile, ParseStatus};

use crate::program::node::{AppRef, AppRegistry};
use crate::program::node_extract::extract_nodes;
use crate::program::pack::PackedFile;
use crate::program::profile::{BuildProfile, DependencyBodies};
use crate::program::resolve::decl_surface::file_routine_meta;
use crate::snapshot::{AppId, AppSetSnapshot, ParsedFile, ParsedUnit, TrustTier};

/// One dependency app's summaries, in the app's file order.
pub struct DepUnitSummary {
    pub app: AppId,
    pub files: Vec<PackedFile>,
}

/// What a build parse produced (spec §5).
pub struct BuildParse {
    pub workspace: Option<ParsedUnit>,
    pub dep_summaries: Vec<DepUnitSummary>,
    /// `Some` only under `DependencyBodies::Keep`.
    pub dep_bodies: Option<Vec<ParsedUnit>>,
}

/// Parse `snap` for a graph build. Each dependency file is parsed and
/// summarized in the same parallel task; under `Summary` its tree is dropped
/// right there, so no more trees are alive than there are worker threads.
/// `skip_dependencies` (a shared-tier hit) parses only the workspace.
#[must_use]
pub fn parse_for_build(
    snap: &AppSetSnapshot,
    profile: BuildProfile,
    skip_dependencies: bool,
) -> BuildParse {
    parse_for_build_observed(snap, profile, skip_dependencies, None)
}

/// Counts dependency trees alive at once, for the bound test. Production
/// passes `None`; the test passes a counter, so it watches the real code path.
#[derive(Default)]
pub(crate) struct LiveTrees {
    pub(crate) live: std::sync::atomic::AtomicUsize,
    pub(crate) peak: std::sync::atomic::AtomicUsize,
}

pub(crate) fn parse_for_build_observed(
    snap: &AppSetSnapshot,
    profile: BuildProfile,
    skip_dependencies: bool,
    observe: Option<&LiveTrees>,
) -> BuildParse {
    use rayon::prelude::*;
    use std::sync::atomic::Ordering::SeqCst;

    let keep = profile.dependency_bodies == DependencyBodies::Keep;
    // Same interning order as `build_dep_layer_cached`'s Step 1, so these
    // `AppRef`s are the ones the dependency layer uses.
    let mut apps = AppRegistry::default();
    let refs: Vec<AppRef> = snap.apps.iter().map(|u| apps.intern(&u.id)).collect();

    crate::big_stack::big_stack_pool().install(|| {
        let mut workspace = None;
        let mut dep_summaries = Vec::new();
        let mut dep_bodies = keep.then(Vec::new);
        for (unit, &app_ref) in snap.apps.iter().zip(&refs) {
            if unit.id == snap.workspace_app {
                // At most one unit matches (`snap.apps` is GUID-deduped
                // upstream); the first one is the workspace, as before.
                if workspace.is_none() {
                    workspace = crate::snapshot::parse::parse_unit(unit);
                }
                continue;
            }
            let Some(source) = unit.source.as_ref() else {
                continue;
            };
            if skip_dependencies {
                continue;
            }
            // A source without its text (deferred, S10.1b) would parse to a
            // tier with none of this app's nodes, shared with every root.
            assert!(
                !source.files.is_empty(),
                "dependency {} reached the parse without its text",
                unit.id.name
            );
            #[cfg(test)]
            crate::snapshot::parse::parse_log::record(unit);
            let per_file: Vec<(PackedFile, Option<ParsedFile>)> = source
                .files
                .par_iter()
                .map(|f| {
                    let pf = crate::snapshot::parse::parse_file(unit, f);
                    if let Some(o) = observe {
                        let now = o.live.fetch_add(1, SeqCst) + 1;
                        o.peak.fetch_max(now, SeqCst);
                    }
                    let summary =
                        summarize_file(app_ref, pf.provenance.tier, &pf.virtual_path, &pf.file);
                    let kept = if keep {
                        Some(pf)
                    } else {
                        drop(pf);
                        None
                    };
                    if let Some(o) = observe {
                        o.live.fetch_sub(1, SeqCst);
                    }
                    (summary, kept)
                })
                .collect();
            let (files, trees): (Vec<_>, Vec<_>) = per_file.into_iter().unzip();
            dep_summaries.push(DepUnitSummary {
                app: unit.id.clone(),
                files,
            });
            if let Some(bodies) = dep_bodies.as_mut() {
                bodies.push(ParsedUnit {
                    app: unit.id.clone(),
                    files: trees.into_iter().flatten().collect(),
                });
            }
        }
        BuildParse {
            workspace,
            dep_summaries,
            dep_bodies,
        }
    })
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

/// Fixtures here are shared with `build.rs`'s sibling-dedup test and the
/// CDO profile test in `dep_cache.rs`.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::program::abi_ingest::AbiCache;
    use crate::program::build::build_dep_layer;
    use crate::program::resolve::decl_surface::{DeclSurface, DepMetaMap};
    use crate::snapshot::compilation::CompilationContext;
    use crate::snapshot::provider::SourceRoot;
    use crate::snapshot::{AppSetSnapshot, AppUnit, Provenance, World, parse_snapshot};

    pub(crate) fn app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.to_string(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    pub(crate) fn unit(id: &AppId, tier: TrustTier, files: &[(&str, &str)]) -> AppUnit {
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
    pub(crate) const DEP_FILES: [(&str, &str); 3] = [
        ("Over.al", OVERLOADS),
        ("Broken.al", BROKEN),
        ("Plain.al", PLAIN),
    ];

    pub(crate) fn ws_unit(ws: &AppId) -> AppUnit {
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

    /// The pre-summary frozen tier, straight from the trees: every non-primary
    /// unit's `RoutineMeta`, in parsed order (last write wins).
    pub(crate) fn old_frozen_tier(
        graph: &crate::program::graph::ProgramGraph,
        parsed: &[ParsedUnit],
        primary: AppRef,
    ) -> DepMetaMap {
        let mut dep = DepMetaMap::new();
        for unit in parsed {
            let Some(app) = graph.apps.find(&unit.app) else {
                continue;
            };
            if app == primary {
                continue;
            }
            for pf in &unit.files {
                dep.extend(file_routine_meta(app, &pf.file, &pf.virtual_path));
            }
        }
        dep
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

        let ctx =
            build_context_from_snapshot(multi_app_snapshot(), BuildProfile::FULL).expect("context");
        let fresh = parse_snapshot(&ctx.snap);
        assert_eq!(fresh.len(), 4, "precondition: every unit parsed");

        // The old construction: one local tier over every parsed unit.
        let old = DeclSurface::build(&ctx.graph, &fresh);
        let new = ctx.decl_surface();
        let primary = ctx.primary_app_ref;
        let old_dep_meta = old_frozen_tier(&ctx.graph, &fresh, primary);
        assert_eq!(
            *ctx.dep_layer.dep_nodes.dep_meta, old_dep_meta,
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

    /// Workspace + two dependency apps, one of them present twice (sibling:
    /// embedded AND workspace multi-app source), so the LAST unit carries
    /// routines no other unit has. `Dep` carries a `Recovered` file.
    fn multi_app_snapshot() -> AppSetSnapshot {
        let (ws, dep, other) = (app_id("Ws"), app_id("Dep"), app_id("Other"));
        let other_src = "codeunit 60100 \"Other Cu\"\n{\n    procedure Qux(a: Integer; var b: Text)\n    begin\n    end;\n}\n";
        AppSetSnapshot {
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
        }
    }

    // ── Task 6: parse, summarize, drop ───────────────────────────────────────

    /// Spec §5 check 2: under `Summary` no more dependency trees are alive at
    /// once than the parse pool has worker threads.
    #[test]
    fn summary_holds_at_most_one_dependency_tree_per_worker() {
        use std::sync::atomic::Ordering::SeqCst;
        let threads = crate::big_stack::big_stack_pool().install(rayon::current_num_threads);
        // At least 64, and always well above the worker count, so the old
        // shape (every tree alive at once) cannot pass by accident.
        let n = 64.max(4 * threads);
        let texts: Vec<(String, String)> = (0..n)
            .map(|i| {
                (
                    format!("F{i}.al"),
                    format!(
                        "codeunit {} \"C{i}\"\n{{\n    procedure P()\n    begin\n    end;\n}}\n",
                        61000 + i
                    ),
                )
            })
            .collect();
        let files: Vec<(&str, &str)> = texts
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let (ws, dep) = (app_id("Ws"), app_id("Dep"));
        let snap = AppSetSnapshot {
            apps: vec![ws_unit(&ws), unit(&dep, TrustTier::EmbeddedSource, &files)],
            workspace_app: ws,
            world: World::Closed,
        };

        let live = LiveTrees::default();
        let parse = parse_for_build_observed(&snap, BuildProfile::LIGHT, false, Some(&live));
        let peak = live.peak.load(SeqCst);

        assert!(parse.dep_bodies.is_none(), "Summary keeps no bodies");
        assert!(parse.workspace.is_some(), "the workspace is parsed");
        assert_eq!(parse.dep_summaries.len(), 1);
        assert_eq!(
            parse.dep_summaries[0].files.len(),
            n,
            "precondition: every file was summarized"
        );
        assert!(peak >= 1, "precondition: the counter watched the parse");
        assert_eq!(live.live.load(SeqCst), 0, "every tree was dropped");
        assert!(
            peak <= threads,
            "{peak} dependency trees alive at once, more than the {threads} workers"
        );
    }

    /// The report fields both profiles must agree on, as comparable text.
    pub(crate) fn report_text(ctx: &crate::program::resolve::full::ProgramContext) -> String {
        let r = crate::program::resolve::full::resolve_full_program_with(ctx);
        let edges: Vec<_> = r
            .edges
            .iter()
            .map(|c| (&c.obligation_id, &c.edge))
            .collect();
        format!(
            "{edges:#?}\n{:?}\n{:?}\n{:?}",
            r.histogram, r.primary_histogram, r.recovered_files
        )
    }

    /// `FULL` and `LIGHT` resolve every CDO-free fixture of `full.rs`'s tests
    /// and the multi-app snapshot identically.
    #[test]
    fn full_and_light_profiles_give_the_same_report() {
        use crate::program::resolve::full::{build_context_from_snapshot, build_snapshot_res};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut snaps: Vec<(String, Box<dyn Fn() -> AppSetSnapshot>)> = [
            "tests/fixtures/ws-d2",
            "tests/fixtures/full_program_fixture",
            "tests/r0-corpus/ws-e2e",
            "tests/r0-corpus/ws-baseapp-closure",
            "tests/r0-corpus/ws-empty-abi-dep",
            "tests/r3a4-fixtures/ws",
        ]
        .into_iter()
        .map(|p| {
            let path = root.join(p);
            let f: Box<dyn Fn() -> AppSetSnapshot> =
                Box::new(move || build_snapshot_res(&path).expect("fixture snapshot"));
            (p.to_string(), f)
        })
        .collect();
        snaps.push(("multi-app".into(), Box::new(multi_app_snapshot)));

        let mut with_bodies = 0;
        for (name, snap) in &snaps {
            let full = build_context_from_snapshot(snap(), BuildProfile::FULL).expect("full");
            let light = build_context_from_snapshot(snap(), BuildProfile::LIGHT).expect("light");
            assert!(
                light.dep_bodies().is_none(),
                "{name}: LIGHT keeps no bodies"
            );
            let bodies = full.dep_bodies().expect("FULL keeps bodies");
            if bodies.iter().any(|u| !u.files.is_empty()) {
                with_bodies += 1;
            }
            assert_eq!(report_text(&full), report_text(&light), "{name}");
        }
        assert!(
            with_bodies >= 2,
            "precondition: fixtures with dependency source exercise both paths"
        );
    }

    /// Review Focus 1: a dependency file whose parse is `Recovered` is still
    /// reported after its tree was dropped, under both profiles, and counted
    /// by `build_program_with_coverage` (which builds with `Summary`).
    #[test]
    fn a_recovered_dependency_file_is_reported_under_both_profiles() {
        use crate::engine::deps::app_package_zip::test_apps;
        use crate::program::resolve::full::{
            build_context_from_snapshot, build_program_with_coverage, build_snapshot_res,
            resolve_full_program_with,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("ws");
        let alpackages = root.join(".alpackages");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&alpackages).unwrap();
        let dep_guid = "dddddddd-0000-0000-0000-000000000006";
        std::fs::write(
            root.join("app.json"),
            format!(
                r#"{{"id":"eeeeeeee-0000-0000-0000-000000000006","name":"Ws","publisher":"probe","version":"1.0.0.0","dependencies":[{{"id":"{dep_guid}","name":"Broken Dep","publisher":"Microsoft","version":"1.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("src/Ws.Codeunit.al"),
            "codeunit 50000 \"Ws Cu\"\n{\n    procedure One()\n    begin\n    end;\n}\n",
        )
        .unwrap();
        let manifest = test_apps::manifest_xml(dep_guid, "Broken Dep");
        let symbols =
            r#"{"Codeunits":[{"Id":60002,"Name":"Plain Cu","Methods":[{"Name":"Baz","Id":1}]}]}"#;
        let app = test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
            ("Broken.al", BROKEN.as_bytes()),
            ("Plain.al", PLAIN.as_bytes()),
        ]);
        std::fs::write(alpackages.join("Microsoft_Broken Dep_1.0.0.0.app"), app).unwrap();

        let expected = vec!["Broken Dep::Broken.al".to_string()];
        for profile in [BuildProfile::FULL, BuildProfile::LIGHT] {
            let snap = build_snapshot_res(&root).expect("snapshot");
            let ctx = build_context_from_snapshot(snap, profile).expect("context");
            assert_eq!(
                resolve_full_program_with(&ctx).recovered_files,
                expected,
                "{profile:?}"
            );
        }
        assert_eq!(
            build_program_with_coverage(&root)
                .expect("fresh coverage")
                .2
                .recovered_files,
            1
        );
    }
}
