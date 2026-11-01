//! `aldump <workspace>` — R0 differential-harness producer.
//! `aldump --l2 <workspace>` — R1a L2 features projection producer (Task 3).
//!
//! DEFAULT (no `--l2`): parses an AL workspace and emits the OBJECT/ROUTINE
//! IDENTITY SUBSET as JSON on stdout, in the EXACT shape of al-sem's committed
//! R0 "golden" files. R0 Task 5 diffs this output against those goldens; the
//! extraction logic (`engine::snapshot`) reproduces al-sem's identity derivation
//! precisely so the diff can pass.
//!
//! `--l2`: parses the workspace and emits the ALLOWLISTED L2 FEATURES PROJECTION
//! (`program::body::l2_workspace`) — objects + routines with metadata + per-routine
//! `features` (loops/operations/call-sites/record-ops/CFN skeleton/…), matching
//! the R1a goldens (`scripts/r1a-goldens/<fixture>.l2.golden.json`). Forbidden
//! later-gate / L3-resolved fields are structurally absent from the projection
//! types, so they can never appear here.
//!
//! DESIGN DEVIATION (R0, deliberate): the default mode emits the identity-subset
//! JSON directly rather than a v3-shaped CapabilitySnapshot — that subset carries
//! fields (routine sub-kind, `canonicalSignatureText`) a v3 envelope cannot.
//!
//! Output discipline: ONLY JSON goes to stdout; all logs/warnings go to stderr.
//! No absolute paths appear anywhere in the output.

use std::path::PathBuf;
use std::process::ExitCode;

use al_sem::engine::snapshot::snapshot_workspace;
use al_sem::program::body::l2_workspace::project_workspace;

/// The model the detector-output modes (`--r3a1/2/3`, `--r4-findings`, `--r4f-*`)
/// project (engine-switch S6.9): program-backed, as `alsem analyze` builds it.
fn program_model(workspace: &std::path::Path) -> Option<al_sem::program::model::workspace::Model> {
    al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &al_sem::program::inputs::WorkspaceInputs::read(workspace, &Default::default()),
        false,
        // What a default `analyze` builds: none of these modes reads rules.
        al_sem::program::model::workspace::AssemblyNeeds::BASE,
    )
}

/// [`program_model`] for a mode that runs `detectors`: built with what they
/// read (`registry::assembly_needs`; the rules for d65).
fn program_model_for(
    workspace: &std::path::Path,
    detectors: &[al_sem::engine::l5::registry::Detector],
) -> Option<al_sem::program::model::workspace::Model> {
    al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &al_sem::program::inputs::WorkspaceInputs::read(workspace, &Default::default()),
        false,
        al_sem::engine::l5::registry::assembly_needs(detectors),
    )
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: aldump [--l2 | \
         --r3a1-combined-graph | --r3a2-summary-core | --r3a3-cone-coverage | \
         --r3a4-dep-hooks | --r3a5-cross-app-summary | --r4-findings | --r4-findings-cross-app | --dependency-bodies-stats [--sites] | \
         --r4f-root-classifications | --r4f-return-summaries | --r4f-snapshot | \
         --r4f-digest-effects | --r4f-scoped-guarantees | --program-call-graph-stats | \
         --graphify-export | --graphify-export-fragments | --integration-points] \
         <workspace-or-.app>\n\
         \x20      aldump --switch-dump <workspace> <out-dir>\n\
         \x20      aldump --dump-dependency-source <workspace> <out-dir>\n\
         \x20      aldump --switch-compare <dump-dir-A> <dump-dir-B> [--sample N] [--width N]\n\
         \x20      aldump --snapshot-memory <workspace> [--single-app] [--hash] [--pd-dump <file>]\n\
         \x20      aldump --session-refresh <workspace-copy> [--file <src-relative .al>] [--verify]"
    );
    ExitCode::FAILURE
}

/// `aldump --session-refresh <workspace-copy> [--file <rel>] [--verify]`:
/// measure the stage-4 workspace session (context-server design §6,
/// `program::session::WorkspaceSession`) per change type. It EDITS the
/// workspace it is given, so it refuses a workspace inside a git checkout
/// (the pinned CDO baseline is one): run it on a copy.
///
/// Steps, each one refresh, printed as one JSON line: `cold` (P, the first
/// build), `noop`, `body` (a comment line inside the first procedure body of
/// the chosen file, B) and its restore, `newline` (two lines prepended to the
/// file, B) and its restore, `decl` (a procedure added to the file, S) and its
/// restore, `add` (a new file, F) and its removal, `appjson` (a trailing
/// newline on `app.json`, P) and its restore. Each line has the class, the
/// files parsed and resolved, whether the dependency bodies were resolved
/// again, `model.copied_bytes`, the program and whole-refresh wall times, and
/// the resident set. With `--verify`, each step also builds the model cold
/// and compares the two models' text fingerprints (`write_model`).
///
/// The chosen file (`--file`, else the median-sized codeunit file with a
/// procedure body) is restored after each pair, and the session's model
/// needs are `AssemblyNeeds::BASE` (what a default command builds).
fn session_refresh_cmd(args: &[String]) -> ExitCode {
    use al_sem::program::inputs::WorkspaceInputs;
    use al_sem::program::model::workspace::AssemblyNeeds;
    use al_sem::program::session::WorkspaceSession;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    const USAGE: &str =
        "usage: aldump --session-refresh <workspace-copy> [--file <src-relative .al>] [--verify]";
    let (mut ws, mut file, mut verify) = (None::<PathBuf>, None::<String>, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--verify" => verify = true,
            "--file" => file = it.next().cloned(),
            _ if ws.is_none() && !a.starts_with("--") => ws = Some(PathBuf::from(a)),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(ws) = ws else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let Ok(ws) = ws.canonicalize() else {
        eprintln!("aldump --session-refresh: no workspace at {}", ws.display());
        return ExitCode::FAILURE;
    };
    if ws.ancestors().any(|d| d.join(".git").exists()) {
        eprintln!(
            "aldump --session-refresh: {} is inside a git checkout; it edits the workspace, run it on a copy",
            ws.display()
        );
        return ExitCode::FAILURE;
    }
    fn status_kib(key: &str) -> Option<u64> {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = s.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }
    fn al_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    al_files(&p, out);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("al")) {
                out.push(p);
            }
        }
    }
    let target: PathBuf = match file {
        Some(f) => ws.join(f),
        None => {
            let mut all = Vec::new();
            al_files(&ws, &mut all);
            let mut cands: Vec<(u64, PathBuf)> = all
                .into_iter()
                .filter_map(|p| {
                    let t = std::fs::read_to_string(&p).ok()?;
                    (t.contains("\n    begin\n")
                        && t.trim_start().to_lowercase().starts_with("codeunit"))
                    .then_some((t.len() as u64, p))
                })
                .collect();
            cands.sort();
            match cands.get(cands.len() / 2) {
                Some((_, p)) => p.clone(),
                None => {
                    eprintln!("aldump --session-refresh: no codeunit file with a procedure body");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    let Ok(original) = std::fs::read_to_string(&target) else {
        eprintln!("aldump --session-refresh: cannot read {}", target.display());
        return ExitCode::FAILURE;
    };
    let app_json = ws.join("app.json");
    let Ok(app_original) = std::fs::read(&app_json) else {
        eprintln!(
            "aldump --session-refresh: cannot read {}",
            app_json.display()
        );
        return ExitCode::FAILURE;
    };
    let probe = target
        .parent()
        .unwrap_or(&ws)
        .join("SessionProbe.Codeunit.al");
    eprintln!("aldump --session-refresh: editing {}", target.display());

    let mut session = WorkspaceSession::new();
    let mut failed = false;
    let mut step = |name: &str, session: &mut WorkspaceSession| {
        let inputs = WorkspaceInputs::read(&ws, &Default::default());
        let out = session.refresh(&inputs, false, AssemblyNeeds::BASE);
        let r = session.last_report().cloned();
        let mut row = json!({
            "step": name,
            "rssKiB": status_kib("VmRSS:"),
            "peakRssKiB": status_kib("VmHWM:"),
        });
        if let Some(r) = &r {
            row["class"] = json!(format!("{:?}", r.class));
            row["changed"] = json!(r.changed.len());
            row["parsed"] = json!(r.parsed);
            row["resolved"] = json!(r.resolved);
            row["dependencyBodiesResolved"] = json!(r.dependency_bodies_resolved);
            row["copiedBytes"] = json!(r.copied_bytes);
            row["programMs"] = json!(r.program_ms);
            row["refreshMs"] = json!(r.wall_ms);
        }
        match out {
            Ok(model) => {
                if verify {
                    let session_fp = al_sem::program::session::workspace::model_fingerprint(&model);
                    drop(model);
                    let t = std::time::Instant::now();
                    let cold = al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
                        &inputs,
                        false,
                        AssemblyNeeds::BASE,
                    );
                    row["coldMs"] = json!(t.elapsed().as_millis());
                    let equal = cold.as_ref().is_ok_and(|c| {
                        al_sem::program::session::workspace::model_fingerprint(c) == session_fp
                    });
                    row["equalToCold"] = json!(equal);
                    failed |= !equal;
                }
            }
            Err(e) => {
                row["error"] = json!(format!("{e:?}"));
                failed = true;
            }
        }
        println!("{row}");
    };
    let write = |p: &Path, t: &str| std::fs::write(p, t).expect("write the workspace copy");
    step("cold", &mut session);
    step("noop", &mut session);
    let body = original.replacen(
        "\n    begin\n",
        "\n    begin\n        // session probe\n",
        1,
    );
    write(&target, &body);
    step("body", &mut session);
    write(&target, &original);
    step("body-restore", &mut session);
    write(&target, &format!("\n\n{original}"));
    step("newline", &mut session);
    write(&target, &original);
    step("newline-restore", &mut session);
    let decl = match original.rfind('}') {
        Some(i) => format!(
            "{}\n    procedure SessionProbeAdded()\n    begin\n    end;\n{}",
            &original[..i],
            &original[i..]
        ),
        None => original.clone(),
    };
    write(&target, &decl);
    step("decl", &mut session);
    write(&target, &original);
    step("decl-restore", &mut session);
    write(
        &probe,
        "codeunit 99977 \"Session Probe\"\n{\n    procedure Run()\n    begin\n    end;\n}\n",
    );
    step("add", &mut session);
    std::fs::remove_file(&probe).expect("remove the probe file");
    step("add-restore", &mut session);
    let mut app = app_original.clone();
    app.push(b'\n');
    std::fs::write(&app_json, &app).expect("write app.json");
    step("appjson", &mut session);
    std::fs::write(&app_json, &app_original).expect("restore app.json");
    step("appjson-restore", &mut session);
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `--dump-dependency-source <workspace> <out-dir>`: write the AL source the
/// engine reads from inside each dependency `.app` to
/// `<out-dir>/<app guid>/<virtual path>`: embedded source, and the AL synthesized
/// for a runtime package that ships none (its statements end in `// cs <line>`,
/// the transpiled C# line). A dependency citation `dep:<guid>:<path>` then names a
/// file there, with the same lines. Source read from a folder on disk is not
/// written; it is already a file.
fn dump_dependency_source_cmd(args: &[String]) -> ExitCode {
    let [ws, out] = args else {
        return usage();
    };
    let snapshot = match (al_sem::snapshot::SnapshotBuilder {
        workspace_root: std::path::PathBuf::from(ws),
        local_providers: Vec::new(),
    })
    .build()
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aldump: error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let out = std::path::Path::new(out);
    for app in snapshot.apps.iter().skip(1) {
        let Some(root) = &app.source else {
            continue;
        };
        if root.tier != al_sem::snapshot::TrustTier::EmbeddedSource {
            continue;
        }
        let dir = out.join(&app.id.guid);
        for f in root.files.iter() {
            // A path inside the package: never absolute, never `..`.
            let rel = std::path::Path::new(&f.virtual_path);
            if !rel
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                eprintln!(
                    "aldump: error: {}: unsafe path {:?}",
                    app.id.short(),
                    f.virtual_path
                );
                return ExitCode::FAILURE;
            }
            let path = dir.join(rel);
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, f.text.as_bytes()));
            if let Err(e) = written {
                eprintln!("aldump: error: writing {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        println!(
            "{} {}: {} files",
            app.id.guid,
            app.id.short(),
            root.files.len()
        );
    }
    ExitCode::SUCCESS
}

/// `--switch-dump <workspace> <out-dir>`: write the engine-switch dump.
fn switch_dump_cmd(args: &[String]) -> ExitCode {
    let [ws, out] = args else {
        return usage();
    };
    let dump = al_sem::engine::switch_dump::dump_lines(std::path::Path::new(ws));
    if let Err(e) = al_sem::engine::switch_dump::write_dump(&dump, std::path::Path::new(out)) {
        eprintln!("aldump: error: writing {out}: {e}");
        return ExitCode::FAILURE;
    }
    let rows: usize = dump.values().map(Vec::len).sum();
    println!("wrote {} files, {rows} rows to {out}", dump.len());
    ExitCode::SUCCESS
}

/// `--switch-compare <A> <B> [--sample N] [--width N]`: diff two dumps. Exit 0 when
/// identical, 1 when they differ, 2 on a usage or read error.
fn switch_compare_cmd(args: &[String]) -> ExitCode {
    let (mut sample, mut width) = (5usize, 400usize);
    let mut dirs: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let slot = match a.as_str() {
            "--sample" => &mut sample,
            "--width" => &mut width,
            _ => {
                dirs.push(a);
                continue;
            }
        };
        match it.next().and_then(|v| v.parse().ok()) {
            Some(v) => *slot = v,
            None => {
                eprintln!("aldump: error: {a} needs a number");
                return ExitCode::from(2);
            }
        }
    }
    let [a, b] = dirs[..] else {
        usage();
        return ExitCode::from(2);
    };
    let read = |d: &String| {
        al_sem::engine::switch_dump::read_dump(std::path::Path::new(d))
            .map_err(|e| eprintln!("aldump: error: reading {d}: {e}"))
    };
    let (Ok(da), Ok(db)) = (read(a), read(b)) else {
        return ExitCode::from(2);
    };
    let diffs = al_sem::engine::switch_dump::compare(&da, &db);
    print!(
        "{}",
        al_sem::engine::switch_dump::render(&diffs, sample, width)
    );
    if diffs.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `aldump --compiler-oracle <graph.jsonl> <workspace> [--all-apps]`
/// (engine-switch S9.0): compare the program resolver (a `ResolvedProgram`:
/// `ProgramScope::CrossApp` with `--all-apps`, else `Workspace`) with the AL
/// compiler's call graph from `altool graph extract-whole`, site by site. Writes
/// the JSON report to stdout; the summary goes to stderr. Default scope: callers
/// in the workspace app; `--all-apps` compares every app's callers.
fn compiler_oracle_cmd(args: &[String]) -> ExitCode {
    use al_sem::program::resolve::compiler_oracle::{CompilerGraph, compare, program_sites};
    use al_sem::program::resolve::differential::project_fresh;
    use al_sem::program::resolved::{ProgramScope, ResolvedProgram};
    let (Some(graph_path), Some(ws)) = (args.first(), args.get(1)) else {
        eprintln!("usage: aldump --compiler-oracle <graph.jsonl> <workspace> [--all-apps]");
        return ExitCode::FAILURE;
    };
    let all_apps = args.iter().any(|a| a == "--all-apps");
    let compiler = match CompilerGraph::read(std::path::Path::new(graph_path)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("aldump: error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(
        std::path::Path::new(ws),
        &al_sem::program::inputs::WorkspaceNeeds::default(),
    );
    // `--all-apps` also compares the dependency call sites, each resolved from
    // its own app (as the cross-app model does); otherwise the workspace's.
    let scope = if all_apps {
        ProgramScope::CrossApp
    } else {
        ProgramScope::Workspace
    };
    let built = match ResolvedProgram::build(&inputs, scope) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("aldump: error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let ctx = &built.body().ctx;
    let mut edges: Vec<_> = built.body().all_edges().map(|ce| ce.edge.clone()).collect();
    al_sem::program::resolve::compiler_oracle::reclassify_entry_runs(&mut edges, ctx.graph());
    let program = program_sites(&project_fresh(&edges, &ctx.graph().apps));
    let mut apps = std::collections::BTreeSet::new();
    if !all_apps {
        apps.insert(ctx.snapshot().workspace_app.guid.to_ascii_lowercase());
    }
    let r = compare(&compiler.sites, &program, &apps);
    eprintln!(
        "compiler edges {:?}, unmapped {}; callers {}; pairs agree {}, compiler-only {}, program-only {}",
        compiler.edge_kinds,
        compiler.unmapped_edges,
        r.callers,
        r.pairs_agree,
        r.pairs_compiler_only,
        r.pairs_program_only
    );
    println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    ExitCode::SUCCESS
}

/// `aldump --snapshot-memory <workspace> [--single-app] [--hash]`: measure
/// `compose_snapshot` (the cone base, then the snapshot from it) on the model
/// `alsem analyze` builds, cross-app by default (the full model: workspace plus
/// the dependency code it demands) or the workspace alone with `--single-app`.
/// Re-measures the cone-memory targets
/// (`docs/superpowers/specs/2026-10-31-cone-memory-design.md`, "Re-measuring").
///
/// Prints one JSON object on stdout: per-phase wall time and resident memory,
/// the build's census marks, the counts that size the structures (inherited
/// and direct facts, their shared tables, snapshot rows and tables, the
/// producer-solve arena) and their row sizes. With `--hash`, also BLAKE3 of
/// every serialized snapshot section, of the whole document (the sections
/// concatenated in order) and of the producer-solve arena, which the document
/// omits: a memory change that must keep the output byte-identical compares
/// them before and after. `--pd-dump <file>` writes the cone's producer-solve
/// arena as `ConeArena::logical_lines` (internal ids, every index replaced by
/// its identity), so a change to the solve's order can be compared as a
/// reordering.
///
/// Memory is the process's resident set (`VmRSS`/`VmHWM` from
/// `/proc/self/status`; `null` where that file does not exist), not live heap:
/// a counting allocator would be `aldump`'s global allocator for every mode,
/// adding two atomic operations to each allocation of every run, the timed
/// north-star command included. Live heap by phase stays with
/// `tools/census-probe`. `--hash` streams ~9.5 GB of JSON through BLAKE3 on
/// CDO and raises peak RSS a little; take the reference peak from a run
/// without it, under `/usr/bin/time -v`.
///
/// Needs (the `declared-needs` skill): the scope fixes the build profile
/// (cross-app → `FULL`, single-app → summary bodies) and the assembly needs
/// are `AssemblyNeeds::BASE`, what a default `analyze` builds, under the
/// workspace's gate model-instance id, so the counts and hashes are those of
/// the model a real run composes from.
fn snapshot_memory_cmd(args: &[String]) -> ExitCode {
    use al_sem::engine::l4::capability_cone::{CapabilityFact, build_r3a3_source_only_base};
    use al_sem::engine::l5::snapshot::{SnapshotCapabilityFact, compose_snapshot_from_base};
    use serde_json::{Value, json};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    const USAGE: &str =
        "usage: aldump --snapshot-memory <workspace> [--single-app] [--hash] [--pd-dump <file>]";
    let (mut single_app, mut hash, mut ws) = (false, false, None);
    let mut pd_dump: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--single-app" => single_app = true,
            "--hash" => hash = true,
            "--pd-dump" => match it.next() {
                Some(p) => pd_dump = Some(PathBuf::from(p)),
                None => {
                    eprintln!("aldump: error: --pd-dump needs a file\n{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            _ if ws.is_none() && !a.starts_with("--") => ws = Some(PathBuf::from(a)),
            _ => {
                eprintln!("aldump: error: unexpected argument {a:?}\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(ws) = ws else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    /// A `/proc/self/status` field in KiB (`None` off Linux).
    fn status_kib(key: &str) -> Option<u64> {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = s.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }
    static START: OnceLock<Instant> = OnceLock::new();
    static MARKS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
    fn on_mark(name: &'static str) {
        let secs = START.get().map_or(0.0, |t| t.elapsed().as_secs_f64());
        let row = json!({ "name": name, "secs": secs, "rssKiB": status_kib("VmRSS:") });
        MARKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(row);
    }
    START.get_or_init(Instant::now);
    // The hook is set only in this mode; every other run pays one atomic load
    // per mark, as before.
    let _ = al_sem::census_hook::HOOK.set(on_mark);
    let mut phases: Vec<Value> = Vec::new();
    let mut end_phase = |name: &str, t: Instant| {
        phases.push(json!({
            "name": name,
            "secs": t.elapsed().as_secs_f64(),
            "rssKiB": status_kib("VmRSS:"),
            "peakRssKiB": status_kib("VmHWM:"),
        }));
    };

    let t = Instant::now();
    // The presence check `analyze` makes (a root `app.json` with an `id`); the
    // id itself no longer enters the routine ids.
    let Some(_) = al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id(&ws)
    else {
        eprintln!(
            "aldump: error: {}: no app.json or unreadable workspace",
            ws.display()
        );
        return ExitCode::FAILURE;
    };
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
    let needs = al_sem::program::model::workspace::AssemblyNeeds::BASE;
    let model = if single_app {
        al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
            &inputs, false, needs,
        )
    } else {
        al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
            &inputs, false, needs,
        )
        .ok()
        .map(|p| p.resolved)
    };
    let Some(model) = model else {
        eprintln!("aldump: error: {}: the model did not build", ws.display());
        return ExitCode::FAILURE;
    };
    end_phase("model", t);

    let t = Instant::now();
    let base = build_r3a3_source_only_base(&model);
    end_phase("coneBase", t);

    let t = Instant::now();
    let snap = compose_snapshot_from_base(&model, &base, None);
    end_phase("composeSnapshot", t);

    if let Some(path) = &pd_dump {
        // Untimed and after the timed phases: the dump is an inspection aid.
        let mut text = base.pd_solution.logical_lines().join("\n");
        text.push('\n');
        if let Err(e) = std::fs::write(path, text) {
            eprintln!("aldump: error: writing {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }

    let tables = base.raw.tables();
    let snap_tables = snap
        .capability_facts
        .first()
        .map_or((0, 0, 0), SnapshotCapabilityFact::table_lens);
    let pd = &snap.pd_solution;
    let counts = json!({
        "modelRoutines": model.workspace.routines.len(),
        "modelCalls": model.calls.edges.len(),
        "coneTypedEdges": base.graph.typed_edges.len(),
        "routinesWithInherited": base.raw.len(),
        "inheritedFacts": base.raw.total_facts(),
        "inheritedTables": {
            "subjects": tables.subjects.len(),
            "reps": tables.reps.len(),
            "hops": tables.hops.len(),
        },
        "directFacts": base.direct_full.values().map(Vec::len).sum::<usize>(),
        "coneCoverage": base.coverage.len(),
        "snapshot": {
            "capabilityFacts": snap.capability_facts.len(),
            "tables": {
                "subjects": snap_tables.0,
                "reps": snap_tables.1,
                "hops": snap_tables.2,
            },
            "typedEdges": snap.typed_edges.len(),
            "operationIndex": snap.operation_index.len(),
            "callsiteIndex": snap.callsite_index.len(),
            "callsiteResolutions": snap.callsite_resolutions.len(),
            "coverage": snap.coverage.len(),
        },
        "pdArena": {
            "states": pd.states.len(),
            "links": pd.links.len(),
            "routines": pd.by_routine.len(),
        },
    });

    let hashes = if hash {
        let t = Instant::now();
        /// A BLAKE3 sink that counts the bytes it is fed.
        struct B3(blake3::Hasher, u64);
        impl std::io::Write for B3 {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.update(b);
                self.1 += b.len() as u64;
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut doc = B3(blake3::Hasher::new(), 0);
        let mut sections: Vec<Value> = Vec::new();
        let mut section = |name: &str, v: &dyn erased::Ser| -> Result<(), serde_json::Error> {
            let mut one = B3(blake3::Hasher::new(), 0);
            v.write_json(&mut one)?;
            v.write_json(&mut doc)?;
            sections.push(json!({
                "name": name,
                "bytes": one.1,
                "blake3": one.0.finalize().to_hex().to_string(),
            }));
            Ok(())
        };
        // The document's sections, in its order (the cone-memory spec §9).
        let written = (|| {
            section("identities", &snap.identities)?;
            section("capabilityFacts", &snap.capability_facts)?;
            section("typedEdges", &snap.typed_edges)?;
            section("operationIndex", &snap.operation_index)?;
            section("callsiteIndex", &snap.callsite_index)?;
            section("callsiteResolutions", &snap.callsite_resolutions)?;
            section("analysisGaps", &snap.analysis_gaps)?;
            section("coverage", &snap.coverage)?;
            section("eventDeclarations", &snap.event_declarations)?;
            section("rootClassifications", &snap.root_classifications)?;
            section("routineOrderFrames", &snap.routine_order_frames)
        })();
        if let Err(e) = written {
            eprintln!("aldump: error: serializing the snapshot: {e}");
            return ExitCode::FAILURE;
        }
        // The producer-solve arena (not serialized): each state, the links,
        // then the per-routine membership in key order. Its Debug forms are
        // part of the hash, so a changed derive moves it: compare it only
        // across a change that keeps those types.
        let mut ph = blake3::Hasher::new();
        for s in &pd.states {
            ph.update(
                format!(
                    "{}|{:?}|{}|{}|{:?}\n",
                    s.producer_fact, s.temp_state, s.routine, s.seed, s.links
                )
                .as_bytes(),
            );
        }
        ph.update(format!("{:?}\n", pd.links).as_bytes());
        let by: std::collections::BTreeMap<_, _> = pd.by_routine.iter().collect();
        ph.update(format!("{by:?}").as_bytes());
        end_phase("hash", t);
        json!({
            "sections": sections,
            "document": { "bytes": doc.1, "blake3": doc.0.finalize().to_hex().to_string() },
            "pdArena": { "blake3": ph.finalize().to_hex().to_string() },
        })
    } else {
        Value::Null
    };

    let marks = std::mem::take(
        &mut *MARKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let out = json!({
        "scope": if single_app { "singleApp" } else { "crossApp" },
        "phases": phases,
        "marks": marks,
        "peakRssKiB": status_kib("VmHWM:"),
        "counts": counts,
        "rowBytes": {
            "capabilityFact": std::mem::size_of::<CapabilityFact>(),
            "snapshotCapabilityFact": std::mem::size_of::<SnapshotCapabilityFact>(),
        },
        "hash": hashes,
    });
    match serde_json::to_string_pretty(&out) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("aldump: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Object-safe JSON writing for `--snapshot-memory`'s per-section hashing
/// (`serde::Serialize` itself is not object safe).
mod erased {
    pub trait Ser {
        fn write_json(&self, w: &mut dyn std::io::Write) -> Result<(), serde_json::Error>;
    }
    impl<T: serde::Serialize> Ser for T {
        fn write_json(&self, w: &mut dyn std::io::Write) -> Result<(), serde_json::Error> {
            serde_json::to_writer(w, self)
        }
    }
}

fn main() -> ExitCode {
    let code = run_main();
    // The process-cumulative cone census, once, after every cone the mode
    // composed (#160 §4.1.7). One OnceLock read when off.
    al_sem::engine::l5::detector_context::report_cones_census();
    code
}

fn run_main() -> ExitCode {
    // Warnings go to stderr (a dropped dependency was once only a `warn!` that
    // nothing printed); `RUST_LOG` overrides the level. stdout is unchanged.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // Engine-switch harness (`engine::switch_dump`): own argument shapes.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match raw.first().map(String::as_str) {
        Some("--switch-dump") => return switch_dump_cmd(&raw[1..]),
        Some("--dump-dependency-source") => return dump_dependency_source_cmd(&raw[1..]),
        Some("--switch-compare") => return switch_compare_cmd(&raw[1..]),
        Some("--compiler-oracle") => return compiler_oracle_cmd(&raw[1..]),
        Some("--snapshot-memory") => return snapshot_memory_cmd(&raw[1..]),
        Some("--session-refresh") => return session_refresh_cmd(&raw[1..]),
        _ => {}
    }
    let mut l2 = false;
    let mut program_call_graph_stats = false;
    let mut graphify_export = false;
    let mut graphify_export_fragments = false;
    let mut integration_points = false;
    let mut r3a1_combined_graph = false;
    let mut r3a2_summary_core = false;
    let mut r3a3_cone_coverage = false;
    let mut r3a4_dep_hooks = false;
    let mut r3a5_cross_app_summary = false;
    let mut r4_findings = false;
    let mut r4_findings_cross_app = false;
    let mut dependency_bodies_stats = false;
    let mut sites = false;
    let mut r4f_root_classifications = false;
    let mut r4f_return_summaries = false;
    let mut r4f_snapshot = false;
    let mut r4f_digest_effects = false;
    let mut r4f_scoped_guarantees = false;
    let mut r4f_ordering_facts = false;
    let mut workspace_arg: Option<std::ffi::OsString> = None;

    for arg in std::env::args_os().skip(1) {
        // Mode flags (anywhere); else the single positional.
        if arg == "--l2" {
            l2 = true;
            continue;
        }
        if arg == "--graphify-export" {
            graphify_export = true;
            continue;
        }
        if arg == "--graphify-export-fragments" {
            graphify_export_fragments = true;
            continue;
        }
        if arg == "--integration-points" {
            integration_points = true;
            continue;
        }
        if arg == "--r3a1-combined-graph" {
            r3a1_combined_graph = true;
            continue;
        }
        if arg == "--r3a2-summary-core" {
            r3a2_summary_core = true;
            continue;
        }
        if arg == "--r3a3-cone-coverage" {
            r3a3_cone_coverage = true;
            continue;
        }
        if arg == "--r3a4-dep-hooks" {
            r3a4_dep_hooks = true;
            continue;
        }
        if arg == "--r3a5-cross-app-summary" {
            r3a5_cross_app_summary = true;
            continue;
        }
        if arg == "--r4-findings" {
            r4_findings = true;
            continue;
        }
        if arg == "--r4-findings-cross-app" {
            r4_findings_cross_app = true;
            continue;
        }
        if arg == "--dependency-bodies-stats" {
            dependency_bodies_stats = true;
            continue;
        }
        if arg == "--sites" {
            sites = true;
            continue;
        }
        if arg == "--r4f-root-classifications" {
            r4f_root_classifications = true;
            continue;
        }
        if arg == "--r4f-return-summaries" {
            r4f_return_summaries = true;
            continue;
        }
        if arg == "--r4f-snapshot" {
            r4f_snapshot = true;
            continue;
        }
        if arg == "--r4f-digest-effects" {
            r4f_digest_effects = true;
            continue;
        }
        if arg == "--r4f-scoped-guarantees" {
            r4f_scoped_guarantees = true;
            continue;
        }
        if arg == "--r4f-ordering-facts" {
            r4f_ordering_facts = true;
            continue;
        }
        if arg == "--program-call-graph-stats" {
            program_call_graph_stats = true;
            continue;
        }
        if workspace_arg.is_some() {
            eprintln!("aldump: error: more than one workspace argument");
            return usage();
        }
        workspace_arg = Some(arg);
    }

    if [
        l2,
        r3a1_combined_graph,
        r3a2_summary_core,
        r3a3_cone_coverage,
        r3a4_dep_hooks,
        r3a5_cross_app_summary,
        r4_findings,
        r4_findings_cross_app,
        dependency_bodies_stats,
        r4f_root_classifications,
        r4f_return_summaries,
        r4f_snapshot,
        r4f_digest_effects,
        r4f_scoped_guarantees,
        r4f_ordering_facts,
        program_call_graph_stats,
        // T4-B: these three each guard their own dedicated `if`-block (like every
        // flag above) but were missing from this array — a combo like
        // `--graphify-export --l2` silently ran whichever block's `if`
        // happened to come first in source order and dropped the other flag.
        graphify_export,
        graphify_export_fragments,
        integration_points,
    ]
    .iter()
    .filter(|f| **f)
    .count()
        > 1
    {
        eprintln!(
            "aldump: error: --l2 / \
             --r3a1-combined-graph / --r3a2-summary-core / --r3a3-cone-coverage / \
             --r3a4-dep-hooks / --r3a5-cross-app-summary / --r4f-return-summaries / \
             --program-call-graph-stats / \
             --graphify-export / --graphify-export-fragments / --integration-points are mutually exclusive"
        );
        return usage();
    }

    let Some(workspace_arg) = workspace_arg else {
        return usage();
    };
    let workspace = PathBuf::from(workspace_arg);

    if r3a4_dep_hooks {
        // R3a-4 DEP-HOOK PROJECTION: read the workspace's `.alpackages` dep `.app`(s),
        // build each dep's embedded-source PRODUCER artifact, drive the CONSUMER hooks
        // (inject_intra_app_call_edges / collect_cited_dep_evidence /
        // collect_dep_order_index) over a merged model whose routine membership =
        // workspace own routines + every dep's own routines, then STABLE-PROJECT every
        // id-bearing field (appGuid:Type:Num#sigHash — cache/modelInstanceId-independent)
        // and emit the producer payloads + consumed effect in the SAME stable shape /
        // key-order as the al-sem `cross-app-dep-hooks.r3a4.golden.json`. CAPTURE POINT:
        // post-inject/collect hooks; the R3a-5 cross-app cone is NOT projected here.
        //
        // `project_r3a4_from_workspace` itself is "engine-never-throws" (a missing dep
        // ledger is a legitimate empty answer, and several differential/oracle tests
        // call it directly expecting that always-succeeds shape) — it has no signal
        // for "the workspace itself is unusable". Task T0.1: gate that ONE genuine
        // failure mode at the CLI boundary with the program-backed model (S9.1),
        // without touching the library function's tested contract.
        if program_model(&workspace).is_none() {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-4 dep-hook projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        }
        let projection = al_sem::engine::deps::r3a4_projection::project_r3a4_from_workspace(
            &workspace,
            "cross-app-dep-hooks",
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-4 dep-hook projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r3a5_cross_app_summary {
        // R3a-5 CROSS-APP FULL SUMMARY (the final R3a sub-gate): run the FULL
        // cross-app L4 path over the workspace + its `.alpackages` dep `.app`(s) WITH
        // the R3a-4 dep hooks — merged index → buildCombinedGraph →
        // injectIntraAppCallEdges → computeSummaries → the cone — and project EVERY
        // routine's FULL RoutineSummary (R3a-2 core + R3a-3 cone/coverage) in the SAME
        // stable shape/key-order as the al-sem `cross-app-full-summary.r3a5.golden.json`.
        // The dep routines arrive EMPTY-featured with a RETAINED summary + direct facts;
        // the injected intra-app typed edges let the cone propagate the dep's Insert
        // capabilityFactsDirect to the PRIMARY caller's capabilityFactsInherited.
        // CAPTURE POINT: post-computeSummaries WITH dep hooks. Fail-closed → empty.
        //
        // Same T0.1 gate as `--r3a4-dep-hooks` above: `project_r3a5_cross_app` itself
        // stays engine-never-throws (zero deps is legitimate, and its `empty` fallback
        // is exercised directly by differential/oracle tests), so the ONE genuine
        // failure — an unbuildable primary workspace — is caught at the CLI boundary
        // with the program-backed model (S9.1).
        if program_model(&workspace).is_none() {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-5 cross-app summary",
                workspace.display()
            );
            return ExitCode::FAILURE;
        }
        let projection = al_sem::engine::l4::capability_cone::project_r3a5_cross_app(
            &workspace,
            "cross-app-full-summary",
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!(
                    "aldump: error: failed to serialize R3a-5 cross-app summary projection: {e}"
                );
                ExitCode::FAILURE
            }
        };
    }

    if r3a2_summary_core {
        // R3a-2 SUMMARY CORE: run the SOURCE-ONLY L0→L3 pipeline → buildCombinedGraph
        // → tarjanScc → computeSummaries (the JACOBI fixed point), then project the
        // RoutineSummary CORE (dbEffects / uncertainties / parameterRoles /
        // inRecursiveCycle / hasUnresolvedCalls) in the SAME stable shape/key-order as
        // the al-sem `<fixture>.r3a2.golden.json`. CAPTURE POINT: POST-computeSummaries;
        // NO dep hooks (R3a-4); the cone/coverage (R3a-3) are never declared on the
        // projected types.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure, not a
        // legitimate empty answer — exits non-zero with no stdout output.
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-2 summary-core projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::l4::summary::project_r3a2(&resolved);
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-2 summary-core projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r3a3_cone_coverage {
        // R3a-3 CAPABILITY CONE + COVERAGE: run the SOURCE-ONLY L0→L3 pipeline, then
        // the cone/coverage pass (direct capability extraction over the resolved
        // features + the publisher-fact injection → composeInheritedCones), and emit
        // the stable projection (capabilityFactsDirect / capabilityFactsInherited /
        // coverage per routine) in the SAME shape/key-order as the al-sem
        // `<fixture>.r3a3.golden.json`. CAPTURE POINT: POST-computeSummaries cone pass;
        // NO dep hooks (R3a-4).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-3 cone+coverage projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::l4::capability_cone::project_r3a3(&resolved);
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-3 cone+coverage projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r4_findings {
        // R4 FINDINGS: run the SOURCE-ONLY L0→L3 pipeline, then the L5 harness
        // (build_detector_context → run_detectors over the registered detectors →
        // stable projection) and emit the R4FindingsProjection in the SAME
        // shape/key-order as the al-sem `<fixture>.r4.golden.json`. Only the ported
        // detectors are registered, so the projection carries their subset.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let detectors = al_sem::engine::l5::detectors::registered_detectors();
        let detector_names: Vec<String> = detectors.iter().map(|d| d.name.clone()).collect();
        let Some(resolved) = program_model_for(&workspace, &detectors) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4 findings projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::l5::finding::project_r4_findings(
            &resolved,
            &detectors,
            &fixture_name,
            &detector_names,
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R4 findings projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if dependency_bodies_stats {
        // Engine-switch S7.1: the dependency bodies' call sites, resolved from each
        // dependency's own view. Kept out of `--program-call-graph-stats`.
        use al_sem::program::resolve::edge::{Histogram, unknown_reason_breakdown};
        let Some(ctx) = al_sem::program::resolve::full::build_context(&workspace) else {
            eprintln!(
                "aldump: error: snapshot build failed at {}",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let res = ctx.resolve_dependency_bodies();
        let graph = ctx.graph();
        let mut per_app: std::collections::BTreeMap<
            String,
            Vec<al_sem::program::resolve::edge::Edge>,
        > = std::collections::BTreeMap::new();
        for ce in res.edges.iter() {
            per_app
                .entry(graph.apps.resolve(ce.edge.from.object.app).name.clone())
                .or_default()
                .push(ce.edge.clone());
        }
        let hist = |edges: &[al_sem::program::resolve::edge::Edge]| {
            let h = Histogram::of_edges(edges);
            let reasons: std::collections::BTreeMap<String, usize> =
                unknown_reason_breakdown(edges.iter())
                    .into_iter()
                    .map(|(r, n)| (r.as_str().to_string(), n))
                    .collect();
            serde_json::json!({
                "total": h.total,
                "resolvedSource": h.resolved_source,
                "resolvedCatalog": h.resolved_catalog,
                "resolvedAbiExternal": h.resolved_abi_external,
                "conditionalResolved": h.conditional_resolved,
                "honestDynamic": h.honest_dynamic,
                "honestEmpty": h.honest_empty,
                "unknown": h.unknown,
                "ambiguousResolved": h.ambiguous_resolved,
                "unknownByReason": reasons,
            })
        };
        let all: Vec<_> = res.edges.iter().map(|ce| ce.edge.clone()).collect();
        let apps: serde_json::Map<String, serde_json::Value> =
            per_app.iter().map(|(n, e)| (n.clone(), hist(e))).collect();
        let mut out = serde_json::json!({ "all": hist(&all), "perApp": apps });
        if sites {
            // `--sites`: every unknown route's site with its source line, for triage.
            use al_sem::program::resolve::edge::Evidence;
            let mut texts: std::collections::HashMap<(String, &str), &str> =
                std::collections::HashMap::new();
            for u in ctx.dep_bodies().unwrap_or_default() {
                for pf in &u.files {
                    texts.insert(
                        (u.app.guid.to_ascii_lowercase(), &pf.virtual_path),
                        &pf.text,
                    );
                }
            }
            use al_sem::program::resolve::edge::{ObligationOutcome, RouteTarget};
            let mut rows = Vec::new();
            let mut ambiguous = Vec::new();
            for ce in res.edges.iter() {
                let app = graph.apps.resolve(ce.edge.from.object.app);
                let span = &ce.edge.site.span;
                let line_text = || {
                    texts
                        .get(&(app.guid.to_ascii_lowercase(), span.unit.as_str()))
                        .and_then(|t| t.lines().nth(span.start.line as usize))
                        .unwrap_or("")
                        .trim()
                };
                // The edges `unknown` counts, each with its first unknown reason
                // (`unknown_reason_breakdown`'s rule), and the `ambiguousResolved`
                // ones with their candidates (`name/arity`).
                match al_sem::program::resolve::edge::classify_obligation(&ce.edge) {
                    ObligationOutcome::Unknown => {}
                    ObligationOutcome::AmbiguousResolved => {
                        let candidates: Vec<String> = ce
                            .edge
                            .routes
                            .iter()
                            .filter_map(|r| match &r.target {
                                RouteTarget::Routine(rid) => {
                                    Some(format!("{}/{}", rid.name_lc, rid.params_count))
                                }
                                _ => None,
                            })
                            .collect();
                        ambiguous.push(serde_json::json!({
                            "app": app.name,
                            "file": span.unit,
                            "line": span.start.line + 1,
                            "routine": ce.edge.from.name_lc,
                            "candidates": candidates,
                            "text": line_text(),
                        }));
                        continue;
                    }
                    _ => continue,
                }
                if let Some(reason) = ce.edge.routes.iter().find_map(|r| match &r.evidence {
                    Evidence::Unknown(reason) => Some(reason),
                    _ => None,
                }) {
                    let line = line_text();
                    let receiver = res
                        .site_facts
                        .get(&ce.obligation_id)
                        .and_then(|f| f.receiver.as_ref())
                        .map(|r| format!("{:?} {}", r.ty, r.type_text.as_deref().unwrap_or("")));
                    rows.push(serde_json::json!({
                        "receiver": receiver,
                        "app": app.name,
                        "file": span.unit,
                        "line": span.start.line + 1,
                        "routine": ce.edge.from.name_lc,
                        "reason": reason.as_str(),
                        "text": line,
                    }));
                }
            }
            out["unknownSites"] = serde_json::Value::Array(rows);
            out["ambiguousSites"] = serde_json::Value::Array(ambiguous);
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return ExitCode::SUCCESS;
    }

    if r4_findings_cross_app {
        // Every registered detector in CROSS-APP mode over the workspace and the
        // dependency `.app`s it finds (engine-switch S7.0: the before/after surface for
        // the cross-app replacement; `project_r4_findings_cross_app`).
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let detectors = al_sem::engine::l5::detectors::registered_detectors();
        let detector_names: Vec<String> = detectors.iter().map(|d| d.name.clone()).collect();
        let projection = al_sem::engine::l5::finding::project_r4_findings_cross_app(
            &workspace,
            &detectors,
            &fixture_name,
            &detector_names,
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize cross-app R4 findings: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r4f_return_summaries {
        // R4-F RETURN SUMMARIES: run the SOURCE-ONLY L0→L3 pipeline, then compute
        // per-routine returnability summaries (spec §J5), and emit the stable
        // projection in the SAME shape/key-order as the al-sem
        // `<fixture>.returnsummary.golden.json`.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F return-summary projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection =
            al_sem::engine::return_summary::project_r4f_return_summaries(&resolved, &fixture_name);
        return match serde_json::to_string_pretty(&projection) {
            Ok(mut json) => {
                json.push('\n');
                print!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R4-F return-summary projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r4f_snapshot {
        // R4-F SNAPSHOT (Stage-2b): run the SOURCE-ONLY L0→L3 pipeline, then compose
        // + project the CapabilitySnapshot CONSUMED-CORE (composeSnapshot's
        // ordering-facts subset) in the SAME shape/key-order as the al-sem
        // `<fixture>.snapshot.golden.json`. The projection re-projects the R3a
        // source-only base (cone facts / typed edges / event graph / coverage /
        // root classifications).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F snapshot projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json = al_sem::engine::l5::snapshot::project_r4f_snapshot(&resolved, &fixture_name);
        // `project_r4f_snapshot` already appends a trailing newline.
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_digest_effects {
        // R4-F DIGEST EFFECTS (Stage-3b): run the SOURCE-ONLY L0→L3 pipeline, compose
        // the CapabilitySnapshot, then run the digest witness + effects + occurrence-build
        // path per reportable root, emitting the per-root DigestEffectResult[] (each with a
        // stable occurrenceId = factId) in the SAME shape/key-order as the al-sem
        // `<fixture>.digest.golden.json`.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F digest-effects projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json = al_sem::engine::l5::digest::project_r4f_digest_effects(&resolved, &fixture_name);
        // `project_r4f_digest_effects` already appends a trailing newline.
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_scoped_guarantees {
        // R4-F SCOPED GUARANTEES (Stage-4): run the SOURCE-ONLY L0→L3 pipeline, compose
        // the CapabilitySnapshot, compute return summaries + isolated event ids, run the
        // digest + ORDERING-ENGINE path, and emit the per-root per-effect scopedGuarantees
        // (filtered to the 5 RELEVANT labels) in the al-sem `<fixture>.scoped.golden.json`
        // shape.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F scoped-guarantees projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json =
            al_sem::engine::l5::digest::project_r4f_scoped_guarantees(&resolved, &fixture_name);
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_ordering_facts {
        // R4-F ORDERING FACTS (Stage-5b, M5): run the SOURCE-ONLY L0→L3 pipeline, then
        // the ordering-facts facade (compute_ordering_facts: composeSnapshot → return
        // summaries → isolated events → digest+ordering → resolve each scopedGuarantee
        // to its IO/write/commit anchors) and emit the per-routine resolved OrderingFact[]
        // in the al-sem `<fixture>.orderingfacts.golden.json` shape.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F ordering-facts projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json = al_sem::engine::l5::ordering_facts::project_r4f_ordering_facts(
            &resolved,
            &fixture_name,
        );
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_root_classifications {
        // R4-F ROOT CLASSIFICATIONS: run the SOURCE-ONLY L0→L3 pipeline (which now
        // classifies AST roots + overlays `<workspace>/roots.config.json`), then
        // emit the STABLE RootClassification projection in the SAME shape/key-order
        // as the al-sem `<fixture>.rootclass.golden.json`.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F root-classification projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::root_classification::project_r4f_root_classifications(
            &resolved,
            &fixture_name,
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!(
                    "aldump: error: failed to serialize R4-F root-classification projection: {e}"
                );
                ExitCode::FAILURE
            }
        };
    }

    if r3a1_combined_graph {
        // R3a-1 L4 GRAPH SUBSTRATE: run the SOURCE-ONLY L0→L3 pipeline, then
        // buildCombinedGraph → tarjanScc → projectR3a1, and emit the stable R3a-1
        // projection (combinedEdges + uncertaintyEdges + typedEdges + the
        // reverse-topo SCC list) in the SAME shape/key-order as the al-sem
        // `<fixture>.r3a1.golden.json`. CAPTURE POINT: POST-buildCombinedGraph /
        // POST-tarjanScc / PRE-computeSummaries — NO dep hooks, NO summaries (R3a-2+).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = program_model(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-1 projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = resolved.project_r3a1_combined_graph();
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-1 projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if program_call_graph_stats {
        // 1B.3a Task 3: self-reported north-star metric.
        //
        // Runs `resolve_full_program` (clean-room, no L3 oracle) over the
        // workspace and prints:
        //   - Taxonomy'd Histogram for the whole program + primary-scoped variant
        //   - Coverage result (obligation SET equality)
        //   - ABI ingestion integrity summary
        //
        // Fully independent of L3.
        use al_sem::program::resolve::edge::{
            unknown_reason_breakdown, unknown_receiver_tier_breakdown,
        };
        use al_sem::program::resolve::full::{
            coverage_holds, is_primary_scope, resolve_full_program,
        };

        let Some(r) = resolve_full_program(&workspace) else {
            eprintln!("aldump: error: resolve_full_program failed (snapshot build error)");
            return ExitCode::FAILURE;
        };

        let h = &r.histogram;
        let ph = &r.primary_histogram;
        let cov = &r.coverage;
        let abi = r.abi_integrity();

        // Task 3: stratified `Unknown`-reason breakdown (charter §8). Purely
        // diagnostic — never changes `h`/`ph`/`cov` above. Rendered via
        // `UnknownReason::as_str()` (stable camelCase keys), never `Debug`.
        let whole_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_reason_breakdown(r.edges.iter().map(|ce| &ce.edge))
                .into_iter()
                .map(|(reason, count)| (reason.as_str().to_string(), count))
                .collect();
        let primary_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_reason_breakdown(
                r.edges
                    .iter()
                    .filter(|ce| is_primary_scope(ce, r.primary_app_ref))
                    .map(|ce| &ce.edge),
            )
            .into_iter()
            .map(|(reason, count)| (reason.as_str().to_string(), count))
            .collect();

        // Reason-split Task 2: ADDITIVE `receiver_tier` diagnostic, keyed
        // `"<reason>:<tier|none>"` — sibling of `unknownByReason` above, never
        // a replacement. Only `memberNotFound` routes ever carry `Some(tier)`
        // today (see `Route::receiver_tier`'s doc); every other reason
        // reports under its own `:none` key.
        fn tier_reason_key(
            reason: al_sem::program::resolve::edge::UnknownReason,
            tier: Option<al_sem::snapshot::TrustTier>,
        ) -> String {
            match tier {
                Some(t) => format!("{}:{}", reason.as_str(), t.as_str()),
                None => format!("{}:none", reason.as_str()),
            }
        }
        let whole_tier_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_receiver_tier_breakdown(r.edges.iter().map(|ce| &ce.edge))
                .into_iter()
                .map(|((reason, tier), count)| (tier_reason_key(reason, tier), count))
                .collect();
        let primary_tier_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_receiver_tier_breakdown(
                r.edges
                    .iter()
                    .filter(|ce| is_primary_scope(ce, r.primary_app_ref))
                    .map(|ce| &ce.edge),
            )
            .into_iter()
            .map(|((reason, tier), count)| (tier_reason_key(reason, tier), count))
            .collect();

        let value = serde_json::json!({
            // ── Whole-program histogram ──────────────────────────────────────
            "wholeProgram": {
                "total": h.total,
                "resolvedSource": h.resolved_source,
                "resolvedCatalog": h.resolved_catalog,
                "resolvedAbiExternal": h.resolved_abi_external,
                "conditionalResolved": h.conditional_resolved,
                "honestDynamic": h.honest_dynamic,
                "honestEmpty": h.honest_empty,
                "unknown": h.unknown,
                // Task 3 (sigfp-and-ambiguous-reclassification plan): closed
                // same-object overload-ambiguity candidate sets, honestly
                // excluded from `unknown`/`realUnknownRate` — see
                // `ObligationOutcome::AmbiguousResolved`'s doc. Wired by a
                // real producer (`resolve_in_object`) as of Task 4.
                "ambiguousResolved": h.ambiguous_resolved,
                "realUnknownRate": h.real_unknown_rate(),
                // Task 4 both-ways reporting (round-1 addendum, BINDING): the
                // LEGACY/advisory rate under the PRE-Task-4 metric definition
                // (counts `ambiguousResolved` as unknown too) — additive,
                // side-by-side with `realUnknownRate` so the metric-definition
                // change is never stat-juked. See `Histogram::legacy_
                // unknown_rate_including_ambiguous`'s doc.
                "realUnknownRateLegacyIncludingAmbiguous":
                    h.legacy_unknown_rate_including_ambiguous(),
                "unknownByReason": whole_by_reason,
                "unknownReceiverTier": whole_tier_by_reason,
            },
            // ── Primary-scoped histogram (workspace edges only) ──────────────
            "primaryScoped": {
                "total": ph.total,
                "resolvedSource": ph.resolved_source,
                "resolvedCatalog": ph.resolved_catalog,
                "resolvedAbiExternal": ph.resolved_abi_external,
                "conditionalResolved": ph.conditional_resolved,
                "honestDynamic": ph.honest_dynamic,
                "honestEmpty": ph.honest_empty,
                "unknown": ph.unknown,
                "ambiguousResolved": ph.ambiguous_resolved,
                "realUnknownRate": ph.real_unknown_rate(),
                "realUnknownRateLegacyIncludingAmbiguous":
                    ph.legacy_unknown_rate_including_ambiguous(),
                "unknownByReason": primary_by_reason,
                "unknownReceiverTier": primary_tier_by_reason,
            },
            // ── Coverage contract ────────────────────────────────────────────
            "coverage": {
                "parsedObligations": cov.parsed_obligations,
                "classifiedEdges": cov.classified_edges,
                "holds": coverage_holds(cov),
                "missingCount": cov.missing.len(),
                "extraCount": cov.extra.len(),
            },
            // ── ABI ingestion integrity ──────────────────────────────────────
            "abiIntegrity": {
                "abiRoutesTotal": abi.abi_routes_total,
                "abiMapped": abi.abi_mapped,
                "abiUnmapped": abi.abi_unmapped,
            },
            // ── Collision-guard observability (Task 1) ───────────────────────
            // Publisher EventFlow edges skipped by the dual-publisher
            // source-overload-alias guard (`resolver::emit_event_flow_edges`).
            // Expected 0 outside the CDO-measured known dual-publisher pairs.
            "eventFlowDualPublisherAliasSkips": r.event_flow_dual_publisher_alias_skips,
            // Task 3 (preprocessor foundations plan): additive, non-gating
            // ParseStatus::Recovered diagnostic — see `recovered_files`'s doc
            // on `ProgramReport`. Expected `count: 0` on a well-formed
            // workspace; a nonzero count means that many files' IR may be
            // missing content tree-sitter could not parse.
            "recoveredFiles": {
                "count": r.recovered_files.len(),
                "paths": r.recovered_files,
            },
        });

        return match serde_json::to_string_pretty(&value) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize program-call-graph-stats: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if graphify_export {
        // graphify ADAPTER: project the whole-program resolved call graph into a
        // graphify node-link extraction document (`{ nodes, edges, hyperedges }`),
        // consumed by graphify's `build_from_json` (see `graphify_export.rs` +
        // `U:\Git\graphify\adapter.md`). Fail-closed → snapshot build error.
        let Some(doc) = al_sem::program::graphify_export::export_workspace(&workspace) else {
            eprintln!("aldump: error: graphify export failed (snapshot build error)");
            return ExitCode::FAILURE;
        };
        return match serde_json::to_string_pretty(&doc) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize graphify export: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if graphify_export_fragments {
        // graphify INCREMENTAL: the graphify document partitioned into per-object
        // fragments + a content-hash manifest (`{ manifest, fragments, shared }`).
        // Diff the manifest across runs → only re-process the objects whose output
        // changed (see `program::graphify_export::FragmentSet`). Fail-closed.
        let Some(fs) = al_sem::program::graphify_export::export_workspace_fragments(&workspace)
        else {
            eprintln!("aldump: error: graphify fragment export failed (snapshot build error)");
            return ExitCode::FAILURE;
        };
        return match serde_json::to_string_pretty(&fs) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize graphify fragments: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if integration_points {
        // INTEGRATION-POINTS REPORT: the resolved event wiring as a "who-reacts-to-
        // what" slice scoped to the workspace's integration surface (see
        // `program::integration_report`). Fail-closed → snapshot build error.
        let Some(report) = al_sem::program::integration_report::report_workspace(&workspace) else {
            eprintln!("aldump: error: integration-points report failed (snapshot build error)");
            return ExitCode::FAILURE;
        };
        return match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize integration-points report: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l2 {
        let projection = match project_workspace(&workspace) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("aldump: error: {e:#}");
                return ExitCode::FAILURE;
            }
        };
        match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize L2 projection: {e}");
                ExitCode::FAILURE
            }
        }
    } else {
        let snapshot = match snapshot_workspace(&workspace) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("aldump: error: {e:#}");
                return ExitCode::FAILURE;
            }
        };
        // Pretty-print with 2-space indent to mirror the goldens (the differ
        // parses structurally, so pretty-printing is a convenience).
        match serde_json::to_string_pretty(&snapshot) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize snapshot: {e}");
                ExitCode::FAILURE
            }
        }
    }
}
