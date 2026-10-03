//! The dependency pack format's go/no-go gate.
//!
//! Spec: `docs/superpowers/specs/2026-08-07-dependency-pack-cache-design.md` §13.
//! Measures the HIT-PATH shape — per-app packs, parallel decode on the same
//! rayon pool `parse_snapshot` uses, `AppRef` re-intern included — because that
//! is what spec step 5 will actually build. A serial in-memory round trip sails
//! under the threshold and proves nothing.
//!
//! Requires a real workspace: set `PACK_BENCH_WS` to a BC workspace root. The
//! bench exits non-zero with a loud message when unset, never silently passed.
//!
//! # Two artifact shapes, both measured
//!
//! The brief specified ONE synthetic `PackedFile` per app, on the grounds that
//! the gate prices RECORD cost rather than per-file framing. That is a floor:
//! spec §6 stores contributions per SOURCE FILE, so the real artifact carries
//! one `virtual_path` string, one bool and three length prefixes per file —
//! 10,800 dependency source files on DO, of which 7,416 become frames (the
//! rest declare no routines and never bucket); see
//! `docs/2026-08-07-dep-pack-gate-measurement.md` for the count and why it
//! supersedes the unreproducible 11,856 an earlier sizing doc quoted. Since
//! the omission can only ADD cost, this bench builds and times BOTH shapes
//! and the verdict is taken from the larger. That keeps the decision on
//! measured ground: nothing here is adjusted by an estimate of what the
//! framing "would have" cost.
//!
//! The per-file grouping is reconstructed from `RoutineMeta::virtual_path`
//! (the only path any of the three packed record types carries). Routines
//! without a `RoutineMeta` are ABI-ingested from symbol-only deps, which have
//! no source file at all; they and path-less objects go to one synthetic
//! bucket per app. File ORDER differs from extraction order, which matters for
//! the dedup contract in spec §12 but not for a byte count or a decode time.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use al_sem::program::abi_ingest::AbiCache;
use al_sem::program::build::{assemble_program_graph, build_dep_layer};
use al_sem::program::node::{AppRef, AppRegistry, ObjectNodeId, RoutineNodeId};
use al_sem::program::node_extract::{ObjectNode, RoutineNode};
use al_sem::program::pack::{DepPack, PACK_SCHEMA, PackedFile, compute_self_hash, decode, encode};
use al_sem::program::resolve::decl_surface::{DeclSurface, RoutineMeta};
use al_sem::snapshot::{AppId, ParsedUnit, SnapshotBuilder, parse_snapshot};
use rayon::prelude::*;

/// One app's contribution, already bucketed by the file it came from.
#[derive(Default)]
struct FileBucket {
    objects: Vec<ObjectNode>,
    routines: Vec<RoutineNode>,
    routine_meta: Vec<(RoutineNodeId, RoutineMeta)>,
}

/// The bucket name used for records that have no source file to belong to:
/// ABI-ingested routines from symbol-only deps, and objects none of whose
/// routines carry a `RoutineMeta`. Not a real path, and deliberately shaped so
/// it cannot collide with one.
const NO_SOURCE_FILE: &str = "<<no-source-file>>";

fn main() {
    let Some(ws) = std::env::var_os("PACK_BENCH_WS").map(PathBuf::from) else {
        eprintln!(
            "PACK_BENCH_WS is unset — this gate needs a real BC workspace and will not \
             guess. Set it to a workspace root and re-run. NOT a pass."
        );
        std::process::exit(2);
    };

    // ---- Build the real population -------------------------------------
    let t0 = Instant::now();
    let snap = SnapshotBuilder {
        workspace_root: ws.clone(),
        local_providers: vec![],
    }
    .build()
    .expect("build snapshot");

    // `parse_snapshot` + `build_dep_layer` timed separately: together they are
    // exactly what a pack HIT replaces, so this is the saving the load cost has
    // to be weighed against — measured on THIS corpus. Spec §13 quotes ~1,280 ms
    // from `docs/2026-08-02-dep-parse-sizing.md`, but that figure was taken on a
    // corpus whose workspace root was never recorded and which no root in this
    // checkout reproduces, so a ratio against it would cross populations.
    let t_parse = Instant::now();
    let parsed = parse_snapshot(&snap);
    let parse_ms = ms(t_parse.elapsed());
    let abi_cache = AbiCache::new();
    let t_dep = Instant::now();
    let dep_layer = build_dep_layer(&snap, &abi_cache, &parsed);
    let dep_layer_ms = ms(t_dep.elapsed());

    let dep_files: usize = parsed
        .iter()
        .filter(|u| u.app != snap.workspace_app)
        .map(|u| u.files.len())
        .sum();
    let all_files: usize = parsed.iter().map(|u| u.files.len()).sum();

    println!("workspace: {}", ws.display());
    println!(
        "population: {dep_files} dep source files, {} dep objects, {} dep routines",
        dep_layer.dep_objects.len(),
        dep_layer.dep_routines.len()
    );
    println!(
        "WHAT A HIT REPLACES (same corpus): parse_snapshot {parse_ms:.1} ms + build_dep_layer \
         {dep_layer_ms:.1} ms = {:.1} ms",
        parse_ms + dep_layer_ms
    );
    println!(
        "  (whole snapshot, deps + primary: {all_files} source files across {} apps)",
        snap.apps.len()
    );
    // The per-app roster, printed rather than summarized: a gate whose totals
    // disagree with the plan's must be diagnosable as "which app" without a
    // second instrumented run.
    for unit in &snap.apps {
        let files = parsed
            .iter()
            .find(|u| u.app == unit.id)
            .map_or(0, |u| u.files.len());
        let app_ref = dep_layer.apps.find(&unit.id);
        let (objects, routines) = app_ref.map_or((0, 0), |r| {
            (
                dep_layer
                    .dep_objects
                    .iter()
                    .filter(|o| o.id.app == r)
                    .count(),
                dep_layer
                    .dep_routines
                    .iter()
                    .filter(|x| x.id.object.app == r)
                    .count(),
            )
        });
        println!(
            "  {:<40} {:>10} {:>6} files {:>6} objects {:>7} routines{}",
            unit.id.name,
            unit.id.version,
            files,
            objects,
            routines,
            if unit.id == snap.workspace_app {
                "  [PRIMARY — never packed]"
            } else if unit.source.is_none() {
                "  [symbol-only: ABI nodes, no RoutineMeta]"
            } else {
                ""
            }
        );
    }

    // ---- The RoutineMeta tier, which is over half the payload -----------
    // `DeclSurface::build_split` produces exactly the dep-tier `RoutineMeta`
    // map. This is NOT optional: a pack without it measures roughly a third of
    // the real bytes, and it is not derivable on a hit because `from_decl`
    // needs a `RoutineDecl`. A gate run without it is a floor, not a decision.
    //
    // `build_split` wants a `ProgramGraph` (for its `AppRegistry`) and the
    // primary `AppRef`. Assembling from the dep layer we already have avoids
    // the second full `parse_snapshot` that `build_program_graph` would do.
    let empty_ws_unit;
    let ws_unit: &ParsedUnit = match parsed.iter().find(|u| u.app == snap.workspace_app) {
        Some(u) => u,
        None => {
            empty_ws_unit = ParsedUnit {
                app: snap.workspace_app.clone(),
                files: vec![],
            };
            &empty_ws_unit
        }
    };
    let graph = assemble_program_graph(&dep_layer, ws_unit, &snap);
    let primary_app_ref = graph
        .apps
        .find(&snap.workspace_app)
        .expect("workspace app must be interned");
    let (_decl_surface, dep_meta) = DeclSurface::build_split(&graph, &parsed, primary_app_ref);
    println!("routine_meta entries: {}", dep_meta.len());
    assert!(
        !dep_meta.is_empty(),
        "the dep RoutineMeta tier is EMPTY — the pack would measure nodes only and the \
         gate would be invalid. Check that the workspace actually has source-bearing \
         dependencies."
    );
    println!("setup: {:.1} ms", ms(t0.elapsed()));

    // The registry a loader re-interns against. Interned in `snap.apps` order,
    // exactly as `build_dep_layer`'s Step 1 does, so an `AppRef` here is the
    // one the run would really have handed out.
    let mut apps = AppRegistry::default();
    for unit in &snap.apps {
        apps.intern(&unit.id);
    }

    // Which apps actually contribute, and their symbolic identity for the
    // pack header. `resolve` is the registry `build_dep_layer` built.
    let mut app_ids: BTreeMap<u32, AppId> = BTreeMap::new();
    for o in dep_layer.dep_objects.iter() {
        app_ids
            .entry(o.id.app.0)
            .or_insert_with(|| dep_layer.apps.resolve(o.id.app).clone());
    }
    for r in dep_layer.dep_routines.iter() {
        app_ids
            .entry(r.id.object.app.0)
            .or_insert_with(|| dep_layer.apps.resolve(r.id.object.app).clone());
    }

    // ---- Bucket every record by (app, source file) ----------------------
    // A routine's file comes from its own `RoutineMeta`; an object's from the
    // first of its routines that has one.
    let mut routine_path: HashMap<&RoutineNodeId, &str> = HashMap::with_capacity(dep_meta.len());
    let mut object_path: HashMap<&ObjectNodeId, &str> = HashMap::new();
    for (id, meta) in dep_meta.iter() {
        routine_path.insert(id, meta.virtual_path.as_str());
        object_path.entry(&id.object).or_insert(&meta.virtual_path);
    }

    let mut per_app: BTreeMap<u32, BTreeMap<&str, FileBucket>> = BTreeMap::new();
    for o in dep_layer.dep_objects.iter() {
        let path = object_path.get(&o.id).copied().unwrap_or(NO_SOURCE_FILE);
        per_app
            .entry(o.id.app.0)
            .or_default()
            .entry(path)
            .or_default()
            .objects
            .push(o.clone());
    }
    for r in dep_layer.dep_routines.iter() {
        let path = routine_path.get(&r.id).copied().unwrap_or(NO_SOURCE_FILE);
        per_app
            .entry(r.id.object.app.0)
            .or_default()
            .entry(path)
            .or_default()
            .routines
            .push(r.clone());
    }
    for (id, meta) in dep_meta.iter() {
        per_app
            .entry(id.object.app.0)
            .or_default()
            .entry(meta.virtual_path.as_str())
            .or_default()
            .routine_meta
            .push((id.clone(), meta.clone()));
    }

    let packed_files: usize = per_app.values().map(BTreeMap::len).sum();
    println!("pack files (per-file shape): {packed_files}");

    // ---- Encode both artifact shapes ------------------------------------
    let per_file_packs = encode_packs(&per_app, &app_ids, PackShape::PerFile);
    let per_app_packs = encode_packs(&per_app, &app_ids, PackShape::OneFilePerApp);

    let dir = std::env::temp_dir().join("alsem-pack-gate");
    let per_file_paths = write_packs(&dir.join("per-file"), &per_file_packs);
    let per_app_paths = write_packs(&dir.join("per-app"), &per_app_packs);

    // ---- The measurement -------------------------------------------------
    println!(
        "\n=== shape A: one PackedFile per SOURCE FILE (spec §6 shape; {packed_files} files) ==="
    );
    let a = measure(&per_file_paths, &per_file_packs, &apps);
    println!("\n=== shape B: one synthetic PackedFile per APP (the brief's shape) ===");
    let b = measure(&per_app_paths, &per_app_packs, &apps);

    // ---- NOT part of the gate number, measured so it is not left to prose --
    // The seam (spec step 5) must hand `LspSnapshot`/`DeclSurface` an
    // `Arc<DepMetaMap>` — a `HashMap<RoutineNodeId, RoutineMeta>`. A pack
    // stores that tier as a Vec, so the map build is real hit-path work this
    // gate's rounds do NOT contain, and hashing a String-bearing key 120k
    // times is not obviously cheap. Reported separately and excluded from the
    // verdict, because the format decision does not turn on it: a shared
    // string table would not make a HashMap build any faster.
    println!("\n=== seam cost NOT in the gate number: Arc<DepMetaMap> from the packed tier ===");
    let seam = {
        let packs: Vec<DepPack> = per_file_paths
            .iter()
            .map(|p| decode(&std::fs::read(p).expect("read")).expect("decode"))
            .collect();
        let start = Instant::now();
        let mut map: HashMap<RoutineNodeId, RoutineMeta> = HashMap::with_capacity(dep_meta.len());
        for pack in packs {
            for f in pack.files {
                for (id, meta) in f.routine_meta {
                    map.insert(id, meta);
                }
            }
        }
        let e = start.elapsed();
        assert_eq!(
            map.len(),
            dep_meta.len(),
            "the rebuilt tier must be complete"
        );
        std::hint::black_box(&map);
        e
    };
    println!(
        "  {:.1} ms to build the {}-entry map (serial, as the seam would)",
        ms(seam),
        dep_meta.len()
    );

    let worst_a = a.iter().copied().fold(f64::MIN, f64::max);
    let worst_b = b.iter().copied().fold(f64::MIN, f64::max);
    println!(
        "\nslowest round: shape A {worst_a:.1} ms, shape B {worst_b:.1} ms — the verdict \
         takes the larger, {:.1} ms",
        worst_a.max(worst_b)
    );
    println!(
        "GATE: under ~200 ms -> proceed with postcard. \
         Approaching ~600 ms -> switch to the shared string table and re-measure."
    );
}

enum PackShape {
    /// Spec §6: one `PackedFile` per source file.
    PerFile,
    /// The brief's floor: every record of an app in one synthetic file.
    OneFilePerApp,
}

fn encode_packs(
    per_app: &BTreeMap<u32, BTreeMap<&str, FileBucket>>,
    app_ids: &BTreeMap<u32, AppId>,
    shape: PackShape,
) -> Vec<Vec<u8>> {
    per_app
        .iter()
        .map(|(app_ix, files)| {
            let id = &app_ids[app_ix];
            let files = match shape {
                PackShape::PerFile => files
                    .iter()
                    .map(|(path, b)| PackedFile {
                        virtual_path: (*path).to_string(),
                        parse_status_recovered: false,
                        objects: b.objects.clone(),
                        routines: b.routines.clone(),
                        routine_meta: b.routine_meta.clone(),
                    })
                    .collect(),
                PackShape::OneFilePerApp => {
                    let mut one = PackedFile {
                        virtual_path: format!("app-{app_ix}"),
                        parse_status_recovered: false,
                        objects: Vec::new(),
                        routines: Vec::new(),
                        routine_meta: Vec::new(),
                    };
                    for b in files.values() {
                        one.objects.extend(b.objects.iter().cloned());
                        one.routines.extend(b.routines.iter().cloned());
                        one.routine_meta.extend(b.routine_meta.iter().cloned());
                    }
                    vec![one]
                }
            };
            let mut pack = DepPack {
                schema: PACK_SCHEMA,
                app_guid: id.guid.clone(),
                app_name: id.name.clone(),
                app_publisher: id.publisher.clone(),
                app_version: id.version.clone(),
                files,
                self_hash: String::new(),
            };
            pack.self_hash = compute_self_hash(&pack);
            encode(&pack).expect("encode")
        })
        .collect()
}

fn write_packs(dir: &std::path::Path, packs: &[Vec<u8>]) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("mkdir");
    let paths: Vec<PathBuf> = packs
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let p = dir.join(format!("pack-{i}.bin"));
            std::fs::write(&p, bytes).expect("write");
            p
        })
        .collect();
    let total: usize = packs.iter().map(Vec::len).sum();
    println!(
        "artifact {}: {} packs, {:.2} MB total",
        dir.display(),
        packs.len(),
        total as f64 / 1_048_576.0
    );
    paths
}

/// Three rounds of the hit path: parallel read + decode + `AppRef` re-intern,
/// on the same pool `parse_snapshot` uses. Returns each round's wall time in ms.
fn measure(paths: &[PathBuf], packs: &[Vec<u8>], apps: &AppRegistry) -> Vec<f64> {
    // The same LOCAL big-stack pool `parse_snapshot` installs into
    // (`snapshot::parse::parse_snapshot`), not rayon's global pool.
    let pool = al_sem::big_stack::big_stack_pool();

    // The integrity pass on its own. `decode` blake3s the whole body before it
    // parses a field, and at 33 MB that is no longer the free check Task 5
    // measured on a 296 KB pack. Printed because it bounds what a format
    // switch could win: the string table changes the PARSE, not the hash.
    //
    // Runs on the in-memory `packs`, so unlike the read probe below it cannot
    // touch the page cache and cannot affect round 0.
    let hash_only = pool.install(|| {
        let start = Instant::now();
        let n: usize = packs
            .par_iter()
            .map(|b| blake3::hash(b).as_bytes()[0] as usize)
            .sum();
        let e = start.elapsed();
        std::hint::black_box(n);
        e
    });
    println!(
        "  blake3 over every pack (parallel): {:.1} ms",
        ms(hash_only)
    );

    let mut rounds = Vec::new();
    let mut per_pack: Vec<(String, f64, usize)> = Vec::new();
    for round in 0..3 {
        let last = round == 2;
        let elapsed = pool.install(|| {
            let start = Instant::now();
            let decoded: Vec<(DepPack, Duration)> = paths
                .par_iter()
                .map(|p| {
                    let pack_start = Instant::now();
                    let bytes = std::fs::read(p).expect("read");
                    let mut pack = decode(&bytes).expect("decode");
                    // Guid -> AppRef re-intern, which the real hit path pays.
                    //
                    // NOT optional and NOT decoration. `AppRef` is a per-run
                    // interning index (`node.rs` hands out `AppRef(apps.len())`
                    // in encounter order), so every node id loaded from a pack
                    // carries a number that means something different this
                    // run. Spec §13 requires the gate to time this. Miss any of
                    // the THREE id sites per routine and the measurement is a
                    // floor rather than the cost of a hit.
                    let app_ref = resolve_app_ref(apps, &pack);
                    for f in &mut pack.files {
                        for o in &mut f.objects {
                            o.id.app = app_ref;
                        }
                        for r in &mut f.routines {
                            r.id.object.app = app_ref;
                        }
                        // The third site: `routine_meta` is keyed by
                        // `RoutineNodeId`, which embeds `ObjectNodeId.app`.
                        for (id, _meta) in &mut f.routine_meta {
                            id.object.app = app_ref;
                        }
                    }
                    (pack, pack_start.elapsed())
                })
                .collect();
            let e = start.elapsed();
            // Consume the result inside the timed scope's lifetime so nothing
            // above can be optimized away, but AFTER the clock is read so the
            // drop is not charged to the round.
            let files = || decoded.iter().flat_map(|(p, _)| p.files.iter());
            let objects: usize = files().map(|f| f.objects.len()).sum();
            let routines: usize = files().map(|f| f.routines.len()).sum();
            let meta: usize = files().map(|f| f.routine_meta.len()).sum();
            println!(
                "  round {round}: {:.1} ms wall, {objects} objects / {routines} routines / \
                 {meta} routine_meta materialized",
                ms(e)
            );
            if last {
                per_pack = decoded
                    .iter()
                    .map(|(p, d)| {
                        (
                            format!("{} {}", p.app_name, p.app_version),
                            ms(*d),
                            p.files.iter().map(|f| f.routines.len()).sum(),
                        )
                    })
                    .collect();
            }
            e
        });
        rounds.push(ms(elapsed));
    }

    // Per-pack cost, from the last round. The packs decode CONCURRENTLY, one
    // per app, so the wall time is bounded below by the SLOWEST single pack —
    // not by the total divided by core count. On a BC workspace that pack is
    // always Base Application, which is most of the population on its own.
    // Stated because it is the shape of the scaling: more cores do not help,
    // and a bigger Base Application moves the number nearly one-for-one.
    per_pack.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (name, t, routines) in per_pack.iter().take(3) {
        println!("    slowest pack: {name} — {t:.1} ms, {routines} routines");
    }
    if let Some((_, slowest, _)) = per_pack.first() {
        let worst_round = rounds.iter().copied().fold(f64::MIN, f64::max);
        println!(
            "    critical path: the slowest pack is {:.0}% of the slowest round's wall time",
            slowest / worst_round * 100.0
        );
    }

    // The I/O share on its own, so a reader can see how much of a round is the
    // read. Deliberately runs AFTER the rounds: an earlier revision ran it
    // FIRST, which read every pack file into the page cache immediately before
    // round 0 and guaranteed round 0 was warm on any OS, quite apart from
    // eviction behaviour. Moving it here removes the bench's own contribution
    // to that.
    //
    // It does NOT make round 0 cold, and nothing in this bench does: `encode`
    // wrote these files moments earlier, so their pages are resident before
    // any round runs. Spec §13's "cold OS file cache at least once" is
    // therefore NOT met by this bench, and the ledger says so rather than
    // substituting an estimate. Getting a genuinely cold round needs a read
    // that bypasses the cache manager — on Windows a `FILE_FLAG_NO_BUFFERING`
    // handle with sector-aligned buffers — which is the route if a future
    // revision decides the sub-13 ms read share is worth unsafe FFI in a bench.
    let io = pool.install(|| {
        let start = Instant::now();
        let bytes: usize = paths
            .par_iter()
            .map(|p| std::fs::read(p).expect("read").len())
            .sum();
        let e = start.elapsed();
        assert_eq!(bytes, packs.iter().map(Vec::len).sum::<usize>());
        e
    });
    println!(
        "  read only, WARM (not a cold-cache measurement — see the fn comment): {:.1} ms",
        ms(io)
    );

    rounds
}

/// The loader's re-intern: look the pack's SYMBOLIC identity up in this run's
/// registry. Once per pack, exactly as spec step 5 would.
fn resolve_app_ref(apps: &AppRegistry, pack: &DepPack) -> AppRef {
    apps.find(&AppId {
        guid: pack.app_guid.clone(),
        name: pack.app_name.clone(),
        publisher: pack.app_publisher.clone(),
        version: pack.app_version.clone(),
    })
    .expect("a pack's app must be interned in this run's registry")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
