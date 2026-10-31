//! T5 of the cone-memory design (`docs/superpowers/specs/2026-10-31-cone-memory-design.md`
//! §9): a live-heap bound on inherited capability facts, in both stored forms.
//!
//! - The cone's raw inherited facts (step 1): at most 16 bytes of live heap
//!   per fact, tables and per-subject keys included.
//! - The snapshot's capability rows (step 2): at most 32 bytes per row, the
//!   shared tables included.
//!
//! A synthetic corpus makes the facts grow as N x set size: `CALLERS`
//! codeunits each call one shared `Work()` that inserts, modifies and deletes
//! `TABLES` tables, so each caller inherits `3 * TABLES` facts. The bytes are
//! read from a counting `#[global_allocator]` as what dropping each part
//! frees, so the test crate is its own binary (the allocator counts the whole
//! process) and holds ONE test (nothing else allocates beside it).
//!
//! DISCRIMINATION (break with Edit, run, revert, run): a rep interned per
//! fact as its own owned copy (the old per-fact `CapabilityFact`) fails the
//! raw bound; a snapshot rep row projected per row (no memo) fails the row
//! bound.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` with the same arguments; the
// counter only observes the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size(), Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size(), Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            LIVE.fetch_add(new, Relaxed);
            LIVE.fetch_sub(l.size(), Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The shared tables (one rep per distinct callee fact, one hop per caller)
/// are a fixed cost the per-fact bytes amortize, as on a real workspace
/// (CDO cross-app: 27,477 reps and 38,081 hops over 17.0 M facts). At 100
/// callers they were 4.6 B/fact of the raw and 9 B/row of the snapshot
/// (measured); 400 callers keep them under a quarter of each bound, so the
/// bound reads the per-fact cost.
const CALLERS: usize = 400;
const TABLES: usize = 40;

/// The bytes dropping `v` frees.
fn freed<T>(v: T) -> usize {
    let before = LIVE.load(Relaxed);
    drop(v);
    before - LIVE.load(Relaxed)
}

fn corpus() -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut tables = String::new();
    for t in 0..TABLES {
        tables.push_str(&format!(
            "table {} T{t}\n{{\n    fields\n    {{\n        field(1; \"No.\"; Code[20]) {{ }}\n    }}\n    keys {{ key(PK; \"No.\") {{ }} }}\n}}\n\n",
            50100 + t
        ));
    }
    files.push(("src/Tables.al".to_string(), tables));
    let mut vars = String::new();
    let mut body = String::new();
    for t in 0..TABLES {
        vars.push_str(&format!("        R{t}: Record T{t};\n"));
        body.push_str(&format!(
            "        R{t}.Insert();\n        R{t}.Modify();\n        R{t}.Delete();\n"
        ));
    }
    files.push((
        "src/Shared.al".to_string(),
        format!(
            "codeunit 50100 Shared\n{{\n    procedure Work()\n    var\n{vars}    begin\n{body}    end;\n}}\n"
        ),
    ));
    for c in 0..CALLERS {
        files.push((
            format!("src/Caller{c}.al"),
            format!(
                "codeunit {} Caller{c}\n{{\n    procedure Run()\n    var\n        S: Codeunit Shared;\n    begin\n        S.Work();\n    end;\n}}\n",
                50200 + c
            ),
        ));
    }
    files
}

#[test]
fn t5_inherited_facts_and_snapshot_rows_stay_small() {
    use al_sem::engine::l4::capability_cone::build_r3a3_source_only_base;
    use al_sem::engine::l5::snapshot::compose_snapshot_from_base;
    let model = al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default(
        &corpus(),
        "11111111-2222-3333-4444-555555555555",
    );
    let mut base = build_r3a3_source_only_base(&model);
    let facts = base.raw.total_facts();
    let want = CALLERS * 3 * TABLES;
    assert!(
        facts >= want,
        "precondition: every caller inherits the shared set ({facts} facts, want >= {want})"
    );

    // Step 2: the snapshot rows (compose them first; they read the raw).
    let mut snap = compose_snapshot_from_base(&model, &base, None);
    let rows = snap.capability_facts.len();
    assert!(
        rows >= facts,
        "precondition: one row per inherited fact ({rows} rows)"
    );
    let row_bytes = freed(std::mem::take(&mut snap.capability_facts));

    // Step 1: the raw inherited facts.
    let raw_bytes = freed(std::mem::take(&mut base.raw));

    let per_fact = raw_bytes as f64 / facts as f64;
    let per_row = row_bytes as f64 / rows as f64;
    println!(
        "T5: {facts} inherited facts, {raw_bytes} B raw ({per_fact:.1} B/fact); \
         {rows} rows, {row_bytes} B ({per_row:.1} B/row)"
    );
    assert!(
        per_fact <= 16.0,
        "raw inherited facts: {per_fact:.1} B/fact > 16"
    );
    assert!(per_row <= 32.0, "snapshot rows: {per_row:.1} B/row > 32");
}
