//! The reads facade's tripwire (plan item S): every read call-site resolution
//! makes of the whole-program state goes through a recorder method, and
//! resolving through a recorder changes nothing.
//!
//! The counting hook ([`super::probe`]) sits on the raw state: the graph's node
//! sets, the index accessors, the surface and the topology. Each file below is
//! resolved through a [`Recorder`] inside a recording scope; a raw read outside
//! a recorder method is a bypass (a read its log would miss).

use std::collections::HashMap;
use std::path::Path;

use super::*;
use crate::program::resolve::full::{
    FileResolution, ProgramContext, build_context_res, resolve_file_obligations,
};

/// A file's resolution as text (edges with witnesses, site facts, parens-less
/// calls), for the "recording changes nothing" comparison.
fn text(r: &FileResolution) -> String {
    format!("{:?}\n{:?}\n{:?}", r.edges, r.site_facts, r.parenless)
}

/// What the tripwire saw over one context.
#[derive(Default)]
struct Seen {
    files: usize,
    hook: HookCounts,
    queries: usize,
    /// Files whose bypass count was not zero (path, count).
    bypassing: Vec<(String, u64)>,
}

/// Resolve every file of `ctx` (the workspace's and the dependency bodies')
/// plainly and through a recorder; assert the two resolutions are equal and
/// add up the hook's counts.
fn run_tripwire(ctx: &ProgramContext, seen: &mut Seen) {
    let index = ResolveIndex::build(ctx.graph());
    let surface = ctx.decl_surface();
    let mut units: Vec<(AppRef, &crate::snapshot::ParsedFile)> = Vec::new();
    for unit in ctx.parsed().iter().chain(ctx.dep_bodies().unwrap_or(&[])) {
        let Some(app) = ctx.graph().apps.find(&unit.app) else {
            continue;
        };
        units.extend(unit.files.iter().map(|pf| (app, pf)));
    }
    let mut maps: HashMap<AppRef, HashMap<ObjectNodeId, &ObjectNode>> = HashMap::new();
    for (app, _) in &units {
        maps.entry(*app)
            .or_insert_with(|| crate::program::resolve::full::app_object_map(ctx.graph(), *app));
    }
    for (app, pf) in units {
        let plain = resolve_file_obligations(
            pf,
            app,
            ctx.graph(),
            &index,
            &surface,
            &maps[&app],
            ctx.file_ix(app, &pf.virtual_path),
        );
        let (recorded, log, hook) =
            ctx.resolve_dependency_file_recorded(app, pf, &index, &surface, &maps[&app]);
        assert_eq!(
            text(&plain),
            text(&recorded),
            "{}: resolving through a recorder changed the resolution",
            pf.virtual_path
        );
        seen.files += 1;
        seen.hook.bypass += hook.bypass;
        seen.hook.facade += hook.facade;
        seen.queries += log.len();
        if hook.bypass > 0 {
            seen.bypassing.push((pf.virtual_path.clone(), hook.bypass));
        }
    }
}

fn assert_no_bypass(what: &str, seen: &Seen) {
    assert!(
        seen.bypassing.is_empty(),
        "{what}: {} raw read(s) bypassed the reads facade, in {} file(s), first: {:?}",
        seen.hook.bypass,
        seen.bypassing.len(),
        seen.bypassing.first()
    );
    assert!(seen.files > 0, "{what}: no file resolved");
    assert!(
        seen.hook.facade > 0 && seen.queries > 0,
        "{what}: the facade saw no read (is the counting hook wired?)"
    );
}

/// The tripwire over every r0-corpus workspace (their files and any
/// dependency bodies they load): no raw read bypasses the facade, and the
/// recorded resolution equals the plain one.
#[test]
fn tripwire_no_read_bypasses_the_facade_on_the_corpus() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut dirs: Vec<_> = std::fs::read_dir(&corpus)
        .expect("r0-corpus")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    dirs.sort();
    // The two fixtures with source-bearing dependencies too.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    dirs.push(root.join("tests/fixtures/lsp-diff-deps"));
    dirs.push(root.join("tests/r3a5-fixtures/ws"));
    let mut seen = Seen::default();
    let mut built = 0usize;
    for dir in &dirs {
        let Ok(ctx) = build_context_res(dir) else {
            continue;
        };
        built += 1;
        run_tripwire(&ctx, &mut seen);
    }
    assert!(built > 100, "only {built} corpus workspaces built");
    assert_no_bypass("r0-corpus", &seen);
}

/// The counting hook itself: a raw read inside a recording scope and outside
/// a recorder method is counted, and the same read inside a recorder method
/// is not.
#[test]
fn the_counting_hook_counts_a_raw_read() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-calls");
    let ctx = build_context_res(&corpus).expect("ws-calls");
    let index = ResolveIndex::build(ctx.graph());
    let surface = ctx.decl_surface();
    let rec = Recorder::new(ctx.graph(), &index, &surface);
    let s = scope();
    // A raw read, as resolution code holding the concrete graph would make.
    let _ = ctx.graph().routines.len();
    // The same kind of read, through the facade.
    let any = ctx.graph().objects.iter().next().map(|o| o.id.clone());
    let counts_mid = HookCounts {
        bypass: BYPASS.with(Cell::get),
        facade: FACADE.with(Cell::get),
    };
    if let Some(id) = &any {
        let _ = GraphReads::routines_of(&rec, id).count();
    }
    let counts = s.finish();
    assert_eq!(counts.bypass, 2, "len and iter outside the facade");
    assert!(counts.facade > 0, "the recorder's own read");
    assert!(counts_mid.bypass > 0);
    assert_eq!(rec.into_log().len(), usize::from(any.is_some()));
}

/// The tripwire over the pinned CDO workspace's dependency bodies (every
/// source-bearing dependency file of the real program; `scripts/cdo-gate`).
/// Skips without `CDO_WS`; fails without it under `ENFORCE_CDO_WS=1`.
#[test]
fn tripwire_no_read_bypasses_the_facade_on_cdo() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let ctx = build_context_res(Path::new(&ws)).expect("CDO context");
    let index = ResolveIndex::build(ctx.graph());
    let surface = ctx.decl_surface();
    let files = ctx.dependency_files();
    let maps = ctx.dependency_object_maps(&files);
    let results: Vec<(String, HookCounts, usize)> =
        crate::big_stack::big_stack_pool().install(|| {
            use rayon::prelude::*;
            files
                .par_iter()
                .map(|(app, pf)| {
                    let (_, log, hook) = ctx
                        .resolve_dependency_file_recorded(*app, pf, &index, &surface, &maps[app]);
                    (pf.virtual_path.clone(), hook, log.len())
                })
                .collect()
        });
    let mut seen = Seen::default();
    for (path, hook, queries) in results {
        seen.files += 1;
        seen.hook.bypass += hook.bypass;
        seen.hook.facade += hook.facade;
        seen.queries += queries;
        if hook.bypass > 0 {
            seen.bypassing.push((path, hook.bypass));
        }
    }
    assert_no_bypass("CDO dependency bodies", &seen);
}
