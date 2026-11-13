//! `alsem query` — the db-effect query CLI differential (Task 6).
//!
//! ## Why this file is the point of the task, not a formality
//!
//! `ReverseEffectIndex` shipped with **zero production callers** — every
//! `build()` sat inside its own `#[cfg(test)]` module, so it had never executed
//! against real data. Its intended consumer (a VSCode hover) is months away.
//! Wiring it to a CLI would have parked it again the moment nobody ran that
//! CLI, so this family exists to make "nobody runs it" impossible: the golden
//! dir `tests/cli-query-goldens/` is named in `scripts/check-goldens`, which
//! `scripts/git-hooks/pre-commit` blocks commits on. Every golden run therefore
//! executes `ReverseEffectIndex::build`, both up-queries, the ancestor BFS, the
//! `DbEffectQuery` facade and the `SolveIx -> ModelRoutine` join — forever, with
//! no discipline required.
//!
//! ## Driven through the SHIPPED BINARY, deliberately
//!
//! Every case below spawns `CARGO_BIN_EXE_alsem`, not the library entry point.
//! A library-level test would pin `run_query_touches_pipeline` while leaving
//! `alsem.rs`'s clap arm silently deletable — the exact "test pins a helper,
//! call site rots" shape this arc has already hit four times. Going through
//! `main` means deleting the subcommand, its flags, its format validation or
//! its exit-code plumbing breaks these goldens.
//!
//! ## Corpus coverage (scope §4.2 level 3)
//!
//! - a table the routine touches DIRECTLY (`via: direct`)
//! - a table it touches only through a CALLEE (`via: inherited`)
//! - a table it does NOT touch while an ANCESTOR does, through another branch —
//!   the whole reason the ancestor-scoped query was built
//! - the `"unknown"` bucket (a real population: `Missing.Get()` on
//!   `Record "No Such Table"`), asserted to be LABELLED, not shown as a table
//! - an absent table selector, and `--direction down` with no `--from` (both
//!   exit 2 with a well-formed answer rather than a silent empty result)
//! - the workspace-global list (count first, uncapped)
//! - `query effects`, including an `implicit-trigger` and an `event-subscriber`
//!   `via` — provenance `ConeDerivedStore` does not carry at all
//!
//! ## Refresh
//!
//! Rust-owned baselines: `REGEN_TEMP_GOLDENS=1 cargo test --test cli
//! cli_query_differential::` — then INSPECT the diff. Never a blind bless.

use std::path::PathBuf;
use std::process::Command;

use crate::regen;

/// Pinned so `alsemVersion` cannot drift the goldens on a version bump. Passed
/// per-Command (not via `std::env::set_var`), so this file needs no `ENV_LOCK`.
const VERSION_OVERRIDE: &str = "cli-query-v1";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("cli-query-goldens")
}

fn regen_golden(golden_path: &std::path::Path, got: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    if let Some(parent) = golden_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", parent.display()));
    }
    regen::write_golden(golden_path, got)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    true
}

/// `autocrlf=true` checkouts: the golden dir is `eol=lf`-pinned in
/// `.gitattributes`, but strip `\r` defensively (the same guard
/// `l4_summary_differential` uses).
fn strip_cr(s: &str) -> String {
    s.replace('\r', "")
}

/// Run `alsem <args...> --format <fmt> --deterministic` and return
/// `(stdout, exit_code)`.
fn run_alsem(args: &[&str], format: &str) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .args(["--format", format, "--deterministic"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// One golden case: a slug, the argv after the workspace path, and the exit
/// code the run must produce.
pub(crate) struct Case {
    slug: &'static str,
    pub(crate) workspace: &'static str,
    /// Argv AFTER `query <sub> <workspace>` — flags only.
    pub(crate) args: &'static [&'static str],
    pub(crate) sub: &'static str,
    exit_code: i32,
}

/// The W2-3 touches fixture, relative to `tests/r0-corpus` (it lives under
/// `tests/fixtures/` so it moves no r0-corpus golden family, and under `ws/` so
/// `ids_parent_baseline`'s fixture-app set does not change).
const QT_WS: &str = "../fixtures/query-touches/ws";

pub(crate) const CORPUS: &[Case] = &[
    // A table the routine touches DIRECTLY, both directions. `ModifyHelper`'s
    // own `Cust.Modify()` is `via: direct` with `temp: parameter-dependent(0)`
    // (a `var Cust: Record` parameter), and because it touches the table
    // itself every one of its 3 callers trivially does too — which the `up`
    // block must SAY rather than present as a finding.
    Case {
        slug: "d1-multi-caller.modifyhelper.mc-customer.both",
        workspace: "ws-d1-multi-caller",
        sub: "touches",
        args: &["--table", "MC Customer", "--from", "ModifyHelper"],
        exit_code: 0,
    },
    // The same table from a CALLER: `CallerA`'s own `FindSet` is `direct`,
    // while the `Modify` it reaches through `ModifyHelper` is `inherited` —
    // and the origin join must point at `ModifyHelper`'s line, not `CallerA`'s.
    // 98.8% of real memberships are `inherited`, so this is the common shape.
    Case {
        slug: "d1-multi-caller.callera.mc-customer.down",
        workspace: "ws-d1-multi-caller",
        sub: "touches",
        args: &[
            "--table",
            "MC Customer",
            "--from",
            "CallerA",
            "--direction",
            "down",
        ],
        exit_code: 0,
    },
    // THE ancestor-scoped payoff. `HandlePosted` (an event subscriber) touches
    // `Audit Entry`, never `Sales Line`. Its transitive caller `PostSalesDoc`
    // reaches `Sales Line` through a DIFFERENT branch (`Line.ModifyAll`), two
    // condensation hops up via the event-dispatch edge. This answer is
    // unobtainable from `up_table` alone (workspace-global) and from `down`
    // alone (empty) — it exists only because the ancestor BFS was implemented.
    Case {
        slug: "d8.handleposted.sales-line.up",
        workspace: "ws-d8-commit-in-tx",
        sub: "touches",
        args: &[
            "--table",
            "Sales Line",
            "--from",
            "HandlePosted",
            "--direction",
            "up",
        ],
        exit_code: 0,
    },
    // The workspace-global list: count first, complete (never truncated).
    Case {
        slug: "d1-multi-caller.mc-customer.global",
        workspace: "ws-d1-multi-caller",
        sub: "touches",
        args: &["--table", "MC Customer"],
        exit_code: 0,
    },
    // The `"unknown"` bucket. `ws-r2a-record-types` declares `Missing: Record
    // "No Such Table"` and calls `Missing.Get()`, so the effect is REAL and its
    // target table is unresolved. It must be answerable AND labelled as the
    // bucket it is — never rendered as though `unknown` were a table id.
    Case {
        slug: "r2a-record-types.unknown-bucket.global",
        workspace: "ws-r2a-record-types",
        sub: "touches",
        args: &["--table", "unknown"],
        exit_code: 0,
    },
    // An absent table selector: a well-formed exit-2 document, not a silent
    // empty result that reads like "nothing touches it".
    Case {
        slug: "d1-multi-caller.absent-table",
        workspace: "ws-d1-multi-caller",
        sub: "touches",
        args: &["--table", "No Such Table At All"],
        exit_code: 2,
    },
    // `--direction down` with no `--from`: a down query is about one routine's
    // cone, so this asks for something that does not exist. Rejected by
    // `render_query_touches` itself (one rule, one place — the clap arm does not
    // duplicate it), with a document that says why rather than a bare usage error.
    Case {
        slug: "d1-multi-caller.down-without-from",
        workspace: "ws-d1-multi-caller",
        sub: "touches",
        args: &["--table", "MC Customer", "--direction", "down"],
        exit_code: 2,
    },
    // `query effects`: the complete down-list for the unknown-bucket routine —
    // 6 effects spanning `direct`, `implicit-trigger` (the table's own OnInsert
    // trigger, origin in Tables.al) and the `unknown` bucket, all with `via`,
    // which is exactly what `ConeDerivedStore` cannot carry.
    Case {
        slug: "r2a-record-types.declaredvars.effects",
        workspace: "ws-r2a-record-types",
        sub: "effects",
        args: &["--routine", "DeclaredVars"],
        exit_code: 0,
    },
    // `query effects` on an event PUBLISHER with an empty body: its only effect
    // is its subscriber's `Insert`, `via: event-subscriber`, with the origin
    // anchored in the SUBSCRIBER's file. Data-is-control-flow, made visible.
    Case {
        slug: "d8.onafterpostsalesdoc.effects",
        workspace: "ws-d8-commit-in-tx",
        sub: "effects",
        args: &["--routine", "OnAfterPostSalesDoc"],
        exit_code: 0,
    },
    // ---- MCP output wave 2, W2-3 (spec §4.6, T-W2-10): `tests/fixtures/query-touches`.
    // Indices 9..=16; the tests below name them by slug (`case`).
    // A parameterized writer: its own Insert is caller-dependent (possibly
    // persistent), and because it touches the table every caller does: the
    // trivial `up` is one counted line with no witness built.
    Case {
        slug: "qt.writeitem.qt-item.both",
        workspace: QT_WS,
        sub: "touches",
        args: &["--table", "QT Item", "--from", "WriteItem"],
        exit_code: 0,
    },
    // The same with `--all`: every caller's witnesses, as before W2-3.
    Case {
        slug: "qt.writeitem.qt-item.both.all",
        workspace: QT_WS,
        sub: "touches",
        args: &["--table", "QT Item", "--from", "WriteItem", "--all"],
        exit_code: 0,
    },
    // A temporary-only root: every touch is on a temporary record, and it is
    // still a touch (counted and folded, never dropped).
    Case {
        slug: "qt.temponly.qt-item.both",
        workspace: QT_WS,
        sub: "touches",
        args: &["--table", "QT Item", "--from", "TempOnly"],
        exit_code: 0,
    },
    // Caller-frame temp rebinding: WriteItem's caller-dependent Insert is a
    // temporary-record touch in this caller's frame.
    Case {
        slug: "qt.callwithtemp.qt-item.down",
        workspace: QT_WS,
        sub: "touches",
        args: &[
            "--table",
            "QT Item",
            "--from",
            "CallWithTemp",
            "--direction",
            "down",
        ],
        exit_code: 0,
    },
    // Persistent, rebound-temporary and temporary touches in one cone.
    Case {
        slug: "qt.mixed.qt-item.down",
        workspace: QT_WS,
        sub: "touches",
        args: &[
            "--table",
            "QT Item",
            "--from",
            "Mixed",
            "--direction",
            "down",
        ],
        exit_code: 0,
    },
    // The informative case: Leaf does not touch QT Item; Top1, Top2 and Entry
    // do through other branches, Top3 does not. One line per caller with its
    // first witness and count.
    Case {
        slug: "qt.leaf.qt-item.up",
        workspace: QT_WS,
        sub: "touches",
        args: &["--table", "QT Item", "--from", "Leaf", "--direction", "up"],
        exit_code: 0,
    },
    // The informative case with `--all`: every caller's witnesses.
    Case {
        slug: "qt.leaf.qt-item.up.all",
        workspace: QT_WS,
        sub: "touches",
        args: &[
            "--table",
            "QT Item",
            "--from",
            "Leaf",
            "--direction",
            "up",
            "--all",
        ],
        exit_code: 0,
    },
    // The unscoped (workspace-global) list on the fixture.
    Case {
        slug: "qt.qt-item.global",
        workspace: QT_WS,
        sub: "touches",
        args: &["--table", "QT Item"],
        exit_code: 0,
    },
    // Round 2 G8: `--op-family write`, every record-write site with its
    // temp state.
    Case {
        slug: "qt.qt-item.global.writes",
        workspace: QT_WS,
        sub: "touches",
        args: &["--table", "QT Item", "--op-family", "write"],
        exit_code: 0,
    },
    // ---- Agent-tasks milestone D (design §7): `tests/fixtures/field-touches`.
    // Every consumer of "FT Item".Amount, by use kind (the fixture's header
    // holds the hand counts; `cli_query_field` asserts them).
    Case {
        slug: "ft.ft-item.amount.global",
        workspace: FT_WS,
        sub: "touches",
        args: &["--table", "FT Item", "--field", "Amount", "--all"],
        exit_code: 0,
    },
    // The same field by number, folded (the default human shape).
    Case {
        slug: "ft.ft-item.2.global",
        workspace: FT_WS,
        sub: "touches",
        args: &["--table", "FT Item", "--field", "2"],
        exit_code: 0,
    },
    // `--from`: Kinds' cone (it and the OnValidate its Validate runs) and
    // its callers (none).
    Case {
        slug: "ft.kinds.ft-item.amount.both",
        workspace: FT_WS,
        sub: "touches",
        args: &["--table", "FT Item", "--field", "Amount", "--from", "Kinds"],
        exit_code: 0,
    },
    // `--from Kinds --direction up`: its callers by R0's reverse walk (two
    // hops and an event subscriber), with each up site's tier.
    Case {
        slug: "ft.kinds.ft-item.unit-price.up",
        workspace: FT_WS,
        sub: "touches",
        args: &[
            "--table",
            "FT Item",
            "--field",
            "Unit Price",
            "--from",
            "Kinds",
            "--direction",
            "up",
        ],
        exit_code: 0,
    },
    // A field with no code consumer: the answer says what was not searched,
    // never "no".
    Case {
        slug: "ft.ft-nomatch.unused.global",
        workspace: FT_WS,
        sub: "touches",
        args: &["--table", "FT NoMatch", "--field", "Unused"],
        exit_code: 0,
    },
    // An absent field: a well-formed exit-2 document.
    Case {
        slug: "ft.ft-item.absent-field",
        workspace: FT_WS,
        sub: "touches",
        args: &["--table", "FT Item", "--field", "No Such Field"],
        exit_code: 2,
    },
];

/// The milestone D fixture, beside `QT_WS` (under `ws/` for the same reason).
pub(crate) const FT_WS: &str = "../fixtures/field-touches/ws";

fn check_case(c: &Case) {
    let ws = fixtures_dir().join(c.workspace);
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());
    let ws_str = ws.to_string_lossy().to_string();

    let mut argv: Vec<&str> = vec!["query", c.sub, &ws_str];
    argv.extend_from_slice(c.args);

    for (format, ext) in [("json", "json"), ("human", "human.txt")] {
        let (got, code) = run_alsem(&argv, format);
        assert_eq!(
            code, c.exit_code,
            "[{}/{format}] exit code — stdout was:\n{got}",
            c.slug
        );
        let golden = goldens_dir().join(format!("{}.{ext}", c.slug));
        if regen_golden(&golden, &got) {
            continue;
        }
        let expected = std::fs::read_to_string(&golden).unwrap_or_else(|e| {
            panic!(
                "[{}/{format}] read golden {} failed: {e} — run \
                 REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_query_differential:: \
                 to (re)capture",
                c.slug,
                golden.display()
            )
        });
        assert_eq!(
            strip_cr(&got),
            strip_cr(&expected),
            "[{}/{format}] diverged from {}",
            c.slug,
            golden.display()
        );
    }
}

#[test]
fn query_touches_direct_hit_both_directions() {
    check_case(&CORPUS[0]);
}

#[test]
fn query_touches_inherited_via_a_callee() {
    check_case(&CORPUS[1]);
}

#[test]
fn query_touches_ancestor_scoped_through_another_branch() {
    check_case(&CORPUS[2]);
}

#[test]
fn query_touches_workspace_global_list() {
    check_case(&CORPUS[3]);
}

#[test]
fn query_touches_unknown_table_bucket() {
    check_case(&CORPUS[4]);
}

#[test]
fn query_touches_absent_table_selector_exits_2() {
    check_case(&CORPUS[5]);
}

#[test]
fn query_touches_down_without_from_is_rejected() {
    check_case(&CORPUS[6]);
}

#[test]
fn query_effects_full_down_list_with_via() {
    check_case(&CORPUS[7]);
}

#[test]
fn query_effects_event_subscriber_via() {
    check_case(&CORPUS[8]);
}

/// The corpus case named `slug`.
fn case(slug: &str) -> &'static Case {
    CORPUS
        .iter()
        .find(|c| c.slug == slug)
        .unwrap_or_else(|| panic!("no corpus case {slug}"))
}

/// W2-3: every `tests/fixtures/query-touches` golden.
#[test]
fn query_touches_w2_3_fixture_goldens() {
    let qt: Vec<&Case> = CORPUS.iter().filter(|c| c.workspace == QT_WS).collect();
    assert_eq!(qt.len(), 9);
    for c in qt {
        check_case(c);
    }
}

/// The lines of `text` from the one starting with `start` (trimmed) up to
/// the next family header (a two-space line), the header excluded.
fn family_block<'t>(text: &'t str, start: &str) -> Vec<&'t str> {
    let mut lines = text
        .lines()
        .skip_while(|l| !l.trim_start().starts_with(start));
    let header = lines
        .next()
        .unwrap_or_else(|| panic!("no `{start}` line in:\n{text}"));
    let _ = header;
    lines.take_while(|l| l.starts_with("    ")).collect()
}

/// Round 2 G8: table mode lists the record writes as their own family,
/// first, with their own count; the fixture's `Validate` is a validation,
/// counted and sampled apart, never among the record writes.
#[test]
fn query_touches_table_mode_families() {
    let ws = fixtures_dir().join(QT_WS);
    let ws = ws.to_string_lossy().to_string();
    let (human, code) = run_alsem(&["query", "touches", &ws, "--table", "QT Item"], "human");
    assert_eq!(code, 0, "{human}");
    let writes_line = human
        .lines()
        .find(|l| l.trim_start().starts_with("record writes:"))
        .unwrap_or_else(|| panic!("no record writes line:\n{human}"));
    assert!(
        writes_line.contains("record writes: 4 sites in 3 routines"),
        "the record writes are Insert 2, Modify 1, Delete 1 (no Validate): {writes_line}"
    );
    let writes = family_block(&human, "record writes:");
    assert!(
        writes.iter().any(|l| l.contains("[Modify]")),
        "the Modify is sampled under record writes:\n{human}"
    );
    assert!(
        writes
            .iter()
            .all(|l| !l.contains("[Validate]") && !l.contains("[Get]")),
        "no validation or read under record writes:\n{human}"
    );
    let validations = family_block(&human, "validations:");
    assert_eq!(validations.len(), 1, "{human}");
    assert!(validations[0].contains("[Validate]"), "{human}");
    // Record writes come before every other family.
    let pos = |needle: &str| human.find(needle).unwrap_or(usize::MAX);
    assert!(pos("record writes:") < pos("validations:"));
    assert!(pos("validations:") < pos("key-relevant"));
    let (json, _) = run_alsem(&["query", "touches", &ws, "--table", "QT Item"], "json");
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json");
    let fam = &doc["payload"]["up"]["directSites"]["byFamily"];
    assert_eq!(fam["recordWrites"]["sites"], 4, "{fam}");
    assert_eq!(
        fam["recordWrites"]["temporary"], 1,
        "TempOnly's Insert: {fam}"
    );
    assert_eq!(fam["validations"]["sites"], 1, "{fam}");
    // `--op-family write`: every record-write site, the temporary one
    // included and tagged, no validation.
    let (json, code) = run_alsem(
        &[
            "query",
            "touches",
            &ws,
            "--table",
            "QT Item",
            "--op-family",
            "write",
        ],
        "json",
    );
    assert_eq!(code, 0, "{json}");
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json");
    let sites = doc["payload"]["up"]["writeSites"]
        .as_array()
        .unwrap_or_else(|| panic!("writeSites: {doc:#}"));
    let ops: Vec<&str> = sites.iter().map(|s| s["op"].as_str().unwrap()).collect();
    assert_eq!(ops.len(), 4, "{ops:?}");
    assert!(
        ops.iter().all(|o| [
            "Insert",
            "Modify",
            "ModifyAll",
            "Delete",
            "DeleteAll",
            "Rename"
        ]
        .contains(o)),
        "{ops:?}"
    );
    assert_eq!(
        sites
            .iter()
            .filter(|s| s["tempState"] == "known(true)")
            .count(),
        1,
        "TempOnly's Insert is listed and tagged: {sites:#?}"
    );
    let (_, code) = run_alsem(
        &[
            "query",
            "touches",
            &ws,
            "--table",
            "QT Item",
            "--op-family",
            "read",
        ],
        "human",
    );
    assert_ne!(code, 0, "only `write` ships");
}

/// Output validation r3 §7 R6: `--op-family write` leads with the writes the
/// task asks about. The fixture (`tests/fixtures/write-order`) states the
/// write sites in the reverse of the wanted order: two temporary writes, a
/// RecordRef Insert (temp state not resolved), a caller-dependent Modify,
/// then a persistent Delete, Modify and Insert. The listing (human and
/// `writeSites`) is persistent → caller-dependent → not resolved →
/// temporary, Insert before Modify before Delete within a class, and the
/// heading states the split. Without the mode the heading keeps its neutral
/// form.
///
/// DISCRIMINATION (2026-11-08): the write-only listing left in file order
/// (the `v.sort_by(… write_order …)` line removed) fails the `writeSites`
/// order assert; the class rank of not-resolved and caller-dependent swapped
/// in `WriteClass` fails it too. Each restored, it passes.
#[test]
fn query_touches_write_listing_leads_with_persistent_writes() {
    let ws = repo_root().join("tests/fixtures/write-order/ws");
    let ws = ws.to_string_lossy().to_string();
    let args = [
        "query",
        "touches",
        &ws,
        "--table",
        "WO Item",
        "--op-family",
        "write",
    ];
    let (json, code) = run_alsem(&args, "json");
    assert_eq!(code, 0, "{json}");
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json");
    let up = &doc["payload"]["up"];
    let sites: Vec<(String, String, String)> = up["writeSites"]
        .as_array()
        .unwrap_or_else(|| panic!("writeSites: {doc:#}"))
        .iter()
        .map(|s| {
            (
                s["routine"].as_str().unwrap().to_string(),
                s["op"].as_str().unwrap().to_string(),
                s["tempState"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    // Precondition: every class is present, and the file order is the
    // reverse of the wanted one (the first site in the file is temporary).
    let first_in_file = up["writeSites"]
        .as_array()
        .unwrap()
        .iter()
        .min_by_key(|s| s["line"].as_u64())
        .unwrap();
    assert_eq!(first_in_file["tempState"], "known(true)", "{doc:#}");
    let rows: Vec<(&str, &str)> = sites
        .iter()
        .map(|(r, o, _)| (r.rsplit("::").next().unwrap_or(r.as_str()), o.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("Real", "Insert"),
            ("Real", "Modify"),
            ("Real", "Delete"),
            ("ByCaller", "Modify"),
            ("ViaRecRef", "Insert"),
            ("TempFirst", "Insert"),
            ("TempFirst", "Delete"),
        ],
        "persistent, caller-dependent, not resolved, temporary; Insert, Modify, Delete: \
         {sites:#?}"
    );
    assert_eq!(
        up["writeSitesByTempState"],
        serde_json::json!({"persistent": 3, "callerDependent": 1, "notResolved": 1,
                           "temporary": 2}),
        "{doc:#}"
    );
    let (human, code) = run_alsem(&args, "human");
    assert_eq!(code, 0, "{human}");
    assert!(
        human.contains(
            "record writes: 7 sites in 4 routines — persistent 3, caller-dependent 1, \
             not resolved 1, temporary 2; by op: "
        ),
        "{human}"
    );
    let listed: Vec<&str> = family_block(&human, "record writes:")
        .into_iter()
        .filter(|l| l.trim_start().starts_with('['))
        .collect();
    assert_eq!(listed.len(), 7, "{human}");
    assert!(
        listed[0].contains("[Insert]") && listed[0].contains("Real"),
        "{human}"
    );
    assert!(
        listed[6].contains("[Delete]") && listed[6].contains("(temporary record)"),
        "{human}"
    );
    // Without the mode: the neutral heading.
    let (human, _) = run_alsem(&["query", "touches", &ws, "--table", "WO Item"], "human");
    assert!(
        human.contains("record writes: 7 sites in 4 routines — ")
            && !human.contains("persistent 3"),
        "{human}"
    );
}

/// Output validation r3 §7 R6, field mode: in-memory assignments and
/// validations stay apart from record persistence. The fixture's
/// `Item.ModifyAll(Amount, 0)` (FTFields.al:79) writes the field to the
/// database; its row and every count call it `record write (ModifyAll)`,
/// never `field assignment`, while the four assignments (`Tmp.Amount := 1`
/// and the others) keep that word and the `Validate` stays `validate`.
///
/// DISCRIMINATION (2026-11-08): `field_use_word` returning `u.human()`
/// for every site fails "the ModifyAll row is a record write"; restored,
/// passes.
#[test]
fn query_touches_field_mode_keeps_record_persistence_apart() {
    let ws = repo_root().join("tests/fixtures/field-touches/ws");
    let ws = ws.to_string_lossy().to_string();
    let (human, code) = run_alsem(
        &[
            "query", "touches", &ws, "--table", "FT Item", "--field", "Amount", "--all",
        ],
        "human",
    );
    assert_eq!(code, 0, "{human}");
    let row = human
        .lines()
        .find(|l| l.contains("ws:src/FTFields.al:79 "))
        .unwrap_or_else(|| panic!("the ModifyAll row: {human}"));
    assert!(
        row.contains(":79 record write (ModifyAll) [") && row.contains("ModifyAll]"),
        "the ModifyAll row is a record write: {row}"
    );
    assert!(
        human.contains("field assignment 4 (1 also read), record write (ModifyAll) 1, validate 1"),
        "the counts keep them apart: {human}"
    );
    assert!(
        !human.contains("record writes:"),
        "field mode takes no record-write heading: {human}"
    );
}

/// Agent-tasks milestone D: every `tests/fixtures/field-touches` golden
/// (`query touches --field`).
#[test]
fn query_touches_field_fixture_goldens() {
    let ft: Vec<&Case> = CORPUS.iter().filter(|c| c.workspace == FT_WS).collect();
    assert_eq!(ft.len(), 6);
    for c in ft {
        check_case(c);
    }
}

// ---------------------------------------------------------------------------
// W2-3 (spec §4.6, T-W2-10): the touches counts, on the fixture.
// ---------------------------------------------------------------------------

fn num(v: &serde_json::Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a number: {v}"))
}

/// `knownPersistent + possiblyPersistent + temporary == witnessCount`, and the
/// class counts equal the witnesses' own `touchClass`es.
fn assert_down_partition(down: &serde_json::Value) -> [u64; 4] {
    let c = &down["counts"];
    let classes = [
        num(&c["knownPersistent"]),
        num(&c["possiblyPersistent"]["tempStateUnknown"]),
        num(&c["possiblyPersistent"]["callerDependent"]),
        num(&c["temporary"]),
    ];
    let ws = down["witnesses"]
        .as_array()
        .expect("down keeps every witness");
    assert_eq!(classes.iter().sum::<u64>(), num(&down["witnessCount"]));
    assert_eq!(ws.len() as u64, num(&down["witnessCount"]));
    for (k, name) in [
        "knownPersistent",
        "tempStateUnknown",
        "callerDependent",
        "temporary",
    ]
    .iter()
    .enumerate()
    {
        let n = ws.iter().filter(|w| w["touchClass"] == *name).count() as u64;
        assert_eq!(n, classes[k], "{name}: {down}");
    }
    assert_eq!(down["beyond"], serde_json::json!([]), "reserved for wave 3");
    classes
}

/// `callersTouching + callersNotTouching + callersUnknown == transitiveCallers`,
/// with the named lists matching their counts.
fn assert_up_partition(up: &serde_json::Value) {
    let (t, n, u) = (
        num(&up["callersTouching"]),
        num(&up["callersNotTouching"]),
        num(&up["callersUnknown"]),
    );
    assert_eq!(t + n + u, num(&up["transitiveCallers"]), "{up}");
    assert_eq!(up["notTouching"].as_array().unwrap().len() as u64, n);
    assert_eq!(up["unknown"].as_array().unwrap().len() as u64, u);
}

/// The trivial case (the routine touches the table itself): `up` is counted,
/// carries no witness and prints one line; the down witness is the
/// caller-dependent Insert. `--all` restores the caller witnesses.
#[test]
fn touches_trivial_up_is_counted_not_enumerated() {
    let v = json_of(case("qt.writeitem.qt-item.both"));
    let down = &v["payload"]["down"];
    assert_eq!(assert_down_partition(down), [0, 0, 1, 0]);
    let up = &v["payload"]["up"];
    assert_up_partition(up);
    assert_eq!(up["informative"], serde_json::json!(false));
    assert_eq!(num(&up["callersTouching"]), 5);
    assert!(up.get("witnesses").is_none(), "built never: {up}");
    assert!(up.get("callers").is_none(), "{up}");

    let ws = fixtures_dir().join(QT_WS).to_string_lossy().to_string();
    let (human, _) = run_alsem(
        &[
            "query",
            "touches",
            &ws,
            "--table",
            "QT Item",
            "--from",
            "WriteItem",
        ],
        "human",
    );
    // The solve graph's `up` is one line; the callers through dependency
    // code (query schema 3.5.0) follow as their own section.
    let up_lines: Vec<&str> = human
        .lines()
        .skip_while(|l| !l.starts_with("  up:"))
        .take_while(|l| !l.starts_with("  through dependency code"))
        .collect();
    assert_eq!(up_lines.len(), 1, "one `up` line: {human}");

    let all = json_of(case("qt.writeitem.qt-item.both.all"));
    let w = all["payload"]["up"]["witnesses"].as_array().expect("--all");
    assert_eq!(w.len(), 14, "every caller witness under --all");
}

/// A temporary-only root: every touch is temporary, and it is still a touch
/// (`touches: true`, counted, the callers counted as touching it).
#[test]
fn touches_temporary_only_root_is_a_touch() {
    let v = json_of(case("qt.temponly.qt-item.both"));
    let down = &v["payload"]["down"];
    assert_eq!(down["touches"], serde_json::json!(true));
    assert_eq!(assert_down_partition(down), [0, 0, 0, 2]);
    let up = &v["payload"]["up"];
    assert_up_partition(up);
    assert_eq!(num(&up["callersTouching"]), 2, "Mixed and Entry");
}

/// Caller-frame temp rebinding: WriteItem's caller-dependent Insert is a
/// temporary-record touch in CallWithTemp's frame, and the mixed cone counts
/// each class.
#[test]
fn touches_counts_in_the_callers_frame() {
    let v = json_of(case("qt.callwithtemp.qt-item.down"));
    let down = &v["payload"]["down"];
    assert_eq!(assert_down_partition(down), [0, 0, 0, 1]);
    assert_eq!(down["witnesses"][0]["op"], serde_json::json!("Insert"));

    let m = json_of(case("qt.mixed.qt-item.down"));
    // Get (persistent), WriteItem's Insert rebound to temporary, TempOnly's
    // Insert and FindFirst (temporary).
    assert_eq!(assert_down_partition(&m["payload"]["down"]), [1, 0, 0, 3]);
    assert_eq!(
        m["payload"]["down"]["counts"]["byOp"],
        serde_json::json!({"FindFirst": 1, "Get": 1, "Insert": 2})
    );
}

/// The informative case: Leaf does not touch QT Item; Top1, Top2, Entry and
/// the round-3 Publisher, OnTop2 and RunTop2OnEvent do through other
/// branches, Top3 does not (and is named). One `callers[]` row per touching
/// caller; `--all` adds every witness.
#[test]
fn touches_informative_up_aggregates_per_caller() {
    let v = json_of(case("qt.leaf.qt-item.up"));
    let up = &v["payload"]["up"];
    assert_up_partition(up);
    assert_eq!(up["informative"], serde_json::json!(true));
    assert_eq!(num(&up["callersTouching"]), 6);
    assert_eq!(num(&up["callersNotTouching"]), 1);
    assert_eq!(
        up["notTouching"][0]["routine"]["display"],
        serde_json::json!("Top3")
    );
    let callers = up["callers"].as_array().unwrap();
    assert_eq!(callers.len(), 6);
    assert!(up.get("witnesses").is_none());
    let all = json_of(case("qt.leaf.qt-item.up.all"));
    let w = all["payload"]["up"]["witnesses"].as_array().unwrap();
    let total: u64 = callers.iter().map(|c| num(&c["witnessCount"])).sum();
    assert_eq!(w.len() as u64, total, "the counts are the --all witnesses");
    for c in callers {
        let counts = &c["counts"];
        let sum = num(&counts["knownPersistent"])
            + num(&counts["possiblyPersistent"]["tempStateUnknown"])
            + num(&counts["possiblyPersistent"]["callerDependent"])
            + num(&counts["temporary"]);
        assert_eq!(sum, num(&c["witnessCount"]), "{c}");
    }
}

/// T-W2-10's work check, through the shipped renderer: in the default trivial
/// case no caller witness is built; the informative case builds one per
/// touching caller; only `--all` builds the product. The counter is the
/// query's own (`DbEffectQuery::ancestor_witnesses_built`).
///
/// Step 1b discrimination (2026-11-05): skipping the trivial case's
/// `--callers` list fails at "--callers lists them"; restored, passes.
#[test]
fn touches_default_never_builds_the_caller_witness_product() {
    use al_sem::engine::l4::effect_query_cli::{
        Direction, QuerySubstrate, query_view, render_query_touches,
    };
    let ws = fixtures_dir().join(QT_WS);
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(&ws, &crate::fingerprint_needs());
    let view = query_view(&inputs, true).expect("query view");
    let substrate = QuerySubstrate::of(&view);
    let render = |from: &str, direction: Direction, all: bool, callers: bool| {
        let r = render_query_touches(
            &substrate,
            "QT Item",
            Some(from),
            direction,
            all,
            callers,
            "t",
            true,
        );
        assert_eq!(r.exit_code, 0);
        r
    };
    let built = |from: &str, direction: Direction, all: bool| {
        render(from, direction, all, false).ancestor_witnesses_built
    };
    assert_eq!(built("WriteItem", Direction::Both, false), 0);
    assert_eq!(built("WriteItem", Direction::Up, false), 0);
    assert_eq!(built("WriteItem", Direction::Both, true), 14);
    assert_eq!(built("Leaf", Direction::Up, false), 6);
    assert!(built("Leaf", Direction::Up, true) > 6);
    // Step 1b: `--callers` names every caller of the trivial case (each
    // with its outcome) and still builds no caller witness.
    let named = render("WriteItem", Direction::Up, false, true);
    assert_eq!(named.ancestor_witnesses_built, 0);
    let doc: serde_json::Value = serde_json::from_str(&named.json_text).unwrap();
    let up = &doc["payload"]["up"];
    let listed = up["callers"].as_array().expect("--callers lists them");
    assert_eq!(
        listed.len() as u64,
        up["transitiveCallers"].as_u64().unwrap()
    );
    assert!(listed.iter().all(|c| c["outcome"] == "touches"), "{up}");
    assert!(up.get("witnesses").is_none(), "{up}");
}

/// Step 1b (astra Task 4, top 1): a touches `down` that finds no indexed
/// operation on the table says `no` only when nothing leaves room for a
/// touch the per-operation index cannot see. QT's `Leaf` stays in workspace
/// code with every call resolved and no table left unresolved: `no`. The
/// MCP-output fixture's `IssueDoc` vs `Out Item` (a table it never names)
/// has two RecordRef operations, enters dependency code at two entry
/// points and holds calls not resolved: `unknown`, each reason stated,
/// and never the old `no — this routine's cone does not touch the table`.
///
/// Discrimination (2026-11-05): `absence_reasons` returning an empty list
/// fails at the `IssueDoc` answer (`no`, reasons `[]`); the `cone-not-read`
/// reason pushed for a read cone too fails at the `Leaf` answer (`unknown`);
/// each restored, passes.
#[test]
fn touches_absence_is_no_only_when_nothing_is_unseen() {
    let down = |ws: &str, table: &str, from: &str| {
        let ws = fixtures_dir().join(ws).to_string_lossy().into_owned();
        let argv = [
            "query",
            "touches",
            &ws,
            "--table",
            table,
            "--from",
            from,
            "--direction",
            "down",
        ];
        let (json, code) = run_alsem(&argv, "json");
        assert_eq!(code, 0, "{json}");
        let (human, _) = run_alsem(&argv, "human");
        let doc: serde_json::Value = crate::as_v3(serde_json::from_str(&json).unwrap());
        (doc["payload"]["down"].clone(), human)
    };
    let (leaf, human) = down(QT_WS, "QT Item", "Leaf");
    assert_eq!(leaf["touches"], false, "{leaf}");
    assert_eq!(leaf["absence"]["answer"], "no", "{leaf}");
    assert_eq!(leaf["absence"]["reasons"], serde_json::json!([]), "{leaf}");
    assert!(
        // Output validation §5.2: the result line is the header's answer.
        human.contains("\nanswer: down: no (every candidate decided) — "),
        "{human}"
    );

    let (doc, human) = down("../fixtures/mcp-output/ws", "Out Item", "IssueDoc");
    assert_eq!(doc["touches"], false, "{doc}");
    assert_eq!(doc["absence"]["answer"], "unknown", "{doc}");
    let kinds: Vec<&str> = doc["absence"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "table-not-resolved",
            "dependency-depth",
            "calls-not-resolved"
        ],
        "{doc}"
    );
    assert!(
        human.contains("\nanswer: down: unknown — no indexed touch observed"),
        "{human}"
    );
    assert!(!human.contains("does not touch the table"), "{human}");
}

// ---------------------------------------------------------------------------
// Semantic assertions — the goldens pin BYTES, these pin MEANING.
//
// A golden diff tells you something moved; it does not tell you whether the
// answer is still right. These read the JSON and assert the facts the whole
// query exists to deliver, so a rebaseline that silently inverts an answer
// cannot pass review as "just a formatting change".
// ---------------------------------------------------------------------------

fn json_of(c: &Case) -> serde_json::Value {
    let ws = fixtures_dir().join(c.workspace);
    let ws_str = ws.to_string_lossy().to_string();
    let mut argv: Vec<&str> = vec!["query", c.sub, &ws_str];
    argv.extend_from_slice(c.args);
    let (got, _code) = run_alsem(&argv, "json");
    crate::as_v3(serde_json::from_str(&got).expect("query JSON parses"))
}

/// The ancestor-scoped answer is INFORMATIVE here — `HandlePosted` does not
/// touch `Sales Line` and exactly one transitive caller does — and the witness
/// is anchored at the caller's OWN `ModifyAll`, two condensation hops up.
#[test]
fn ancestor_scoped_answer_is_informative_and_anchored() {
    let v = json_of(&CORPUS[2]);
    let up = &v["payload"]["up"];
    assert_eq!(up["scoped"], serde_json::json!(true));
    assert_eq!(
        up["informative"],
        serde_json::json!(true),
        "the routine itself must NOT touch the table — that is what makes the \
         ancestor list a finding rather than a restatement"
    );
    assert_eq!(up["callersTouching"], serde_json::json!(1));
    assert_eq!(up["transitiveCallers"], serde_json::json!(2));

    // W2-3: the informative default carries one row per touching caller, its
    // first witness and its count (`--all` adds `witnesses`).
    let c = &up["callers"][0];
    assert_eq!(c["depth"], serde_json::json!(2));
    assert_eq!(c["routine"]["display"], serde_json::json!("PostSalesDoc"));
    assert_eq!(c["witnessCount"], serde_json::json!(1));
    let w = &c["firstWitness"];
    assert_eq!(w["op"], serde_json::json!("ModifyAll"));
    assert_eq!(
        w["origin"]["routine"]["display"],
        serde_json::json!("PostSalesDoc"),
        "a `direct` effect originates in the routine itself"
    );
    assert!(
        w["origin"]["anchor"]["line"].as_u64().is_some(),
        "every witness must carry a real source anchor"
    );
}

/// The `inherited` case: `CallerA` reaches `Modify` through `ModifyHelper`, and
/// the origin join must name `ModifyHelper` — pointing at `CallerA` would make
/// the 98.8%-inherited majority of all answers useless.
#[test]
fn inherited_witness_is_anchored_at_the_callee_that_performs_it() {
    let v = json_of(&CORPUS[1]);
    let down = &v["payload"]["down"];
    assert_eq!(down["touches"], serde_json::json!(true));

    let witnesses = down["witnesses"].as_array().expect("witness array");
    let modify = witnesses
        .iter()
        .find(|w| w["op"] == serde_json::json!("Modify"))
        .expect("CallerA transitively reaches a Modify");
    assert_eq!(modify["via"], serde_json::json!("inherited"));
    assert_eq!(
        modify["origin"]["routine"]["display"],
        serde_json::json!("ModifyHelper"),
        "the inherited Modify lives in the CALLEE, not in CallerA"
    );

    let findset = witnesses
        .iter()
        .find(|w| w["op"] == serde_json::json!("FindSet"))
        .expect("CallerA does its own FindSet");
    assert_eq!(findset["via"], serde_json::json!("direct"));
    assert_eq!(
        findset["origin"]["routine"]["display"],
        serde_json::json!("CallerA")
    );
}

/// The `"unknown"` bucket must be answerable AND flagged. `isUnknownBucket`
/// exists precisely so no consumer can render it as a table id by accident.
#[test]
fn unknown_bucket_is_flagged_not_disguised_as_a_table() {
    let v = json_of(&CORPUS[4]);
    let table = &v["payload"]["table"];
    assert_eq!(table["id"], serde_json::json!("unknown"));
    assert_eq!(table["isUnknownBucket"], serde_json::json!(true));
    // ⟨final-branch-review-l3.md M-11⟩ `serialize_document_value` drops every
    // explicit JSON null (`sort_and_drop_nulls`), and `serde_json::Value`'s
    // `Index` yields `Value::Null` for an absent key too — so `table["name"] ==
    // Value::Null` would pass identically whether the key were null or simply
    // missing, and cannot tell "correctly omitted" from "typo'd/renamed key
    // that never got populated". Assert the key's actual absence instead.
    assert!(
        table.get("name").is_none(),
        "the bucket has no name because it is not a table"
    );
    assert_eq!(v["payload"]["up"]["routineCount"], serde_json::json!(1));

    // And in `query effects`, the same effect is flagged per-row.
    let e = json_of(&CORPUS[7]);
    let effects = e["payload"]["effects"].as_array().expect("effects array");
    let unknown_rows: Vec<_> = effects
        .iter()
        .filter(|r| r["isUnknownBucket"] == serde_json::json!(true))
        .collect();
    assert_eq!(
        unknown_rows.len(),
        1,
        "ws-r2a-record-types' `Missing.Get()` on Record \"No Such Table\" is the \
         one unresolved-table effect in this fixture"
    );
    assert_eq!(unknown_rows[0]["op"], serde_json::json!("Get"));
    // ⟨final-branch-review-l3.md M-11⟩ same reasoning as above: assert the key
    // is absent, not that an indexed lookup happens to equal `Value::Null`.
    assert!(unknown_rows[0].get("tableName").is_none());
    // Every OTHER row resolved to a real, named table.
    for r in effects.iter().filter(|r| r["isUnknownBucket"] == false) {
        assert!(
            r["tableName"].is_string(),
            "a resolved effect must render its table NAME, not just an internal id: {r}"
        );
    }
}

/// A table the routine touches itself makes the ancestor list a restatement,
/// not a finding — and the payload must say so instead of implying otherwise.
#[test]
fn self_touching_routine_reports_its_ancestors_as_non_informative() {
    let v = json_of(&CORPUS[0]);
    assert_eq!(v["payload"]["down"]["touches"], serde_json::json!(true));
    let up = &v["payload"]["up"];
    assert_eq!(up["informative"], serde_json::json!(false));
    assert_eq!(
        up["callersTouching"], up["transitiveCallers"],
        "summaries are transitive-down, so EVERY caller trivially touches it"
    );
}

/// An unmatched selector produces a well-formed exit-2 document that names the
/// failure — not an empty result that reads as "nothing touches this table".
#[test]
fn absent_table_selector_reports_unmatched_rather_than_empty() {
    let v = json_of(&CORPUS[5]);
    assert_eq!(
        v["payload"]["table"]["resolution"],
        serde_json::json!("unmatched")
    );
    // ⟨final-branch-review-l3.md M-11⟩ `.is_null()` on an indexed lookup cannot
    // tell an absent key from an explicit null (which `sort_and_drop_nulls`
    // makes impossible here anyway) — assert the key's actual absence.
    assert!(v["payload"].get("up").is_none(), "no answer was computed");
}

/// ⟨final-branch-review-l3.md M-3⟩ `VALID_QUERY_FORMATS` (`src/bin/alsem.rs`)
/// had zero coverage — every golden case above passes only `json` or `human`,
/// so deleting the check entirely is silent: `emit_query` falls through to
/// `human` for any non-`"json"` string, and no golden's exit code would move.
/// Pin the rejection directly, for both subcommands, without needing a golden
/// (the check runs before workspace assembly, so this only asserts an exit
/// code — no output to pin).
#[test]
fn invalid_query_format_is_rejected_with_exit_1() {
    let ws = fixtures_dir().join("ws-d1-multi-caller");
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());
    let ws_str = ws.to_string_lossy().to_string();

    let (_out, touches_code) = run_alsem(
        &["query", "touches", &ws_str, "--table", "MC Customer"],
        "xml",
    );
    assert_eq!(
        touches_code, 1,
        "query touches: an invalid --format must exit 1, not fall through to human"
    );

    let (_out, effects_code) = run_alsem(
        &["query", "effects", &ws_str, "--routine", "ModifyHelper"],
        "xml",
    );
    assert_eq!(
        effects_code, 1,
        "query effects: an invalid --format must exit 1, not fall through to human"
    );
}

/// #9: `Record.Rename()` is a write. Before the fix `record_op_type` had no
/// "rename" arm, so a Rename produced no row in the db-effect store and every
/// consumer under-reported it. A scratch workspace (no corpus fixture, so no
/// golden family moves) driven through the shipped binary: the effect must name
/// the renamed table with op `Rename`.
#[test]
fn query_effects_reports_a_rename_as_a_write_on_its_table() {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-query-rename-")
        .tempdir()
        .expect("scratch ws");
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"99999999-0000-0000-0000-000000000009","name":"Rename Probe","publisher":"PT","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(
        ws.join("src").join("rename.al"),
        r#"table 50300 "Rn Item"
{
    fields
    {
        field(1; "Code"; Code[20]) { }
    }
    keys
    {
        key(PK; "Code") { }
    }
}

codeunit 50301 "Rn Runner"
{
    procedure DoRename()
    var
        Item: Record "Rn Item";
    begin
        Item.Get('A');
        Item.Rename('B');
    end;
}
"#,
    )
    .unwrap();
    let ws_str = ws.to_string_lossy().to_string();
    let (out, code) = run_alsem(
        &["query", "effects", &ws_str, "--routine", "DoRename"],
        "json",
    );
    assert_eq!(code, 0, "query effects failed: {out}");
    let doc: serde_json::Value = crate::as_v3(serde_json::from_str(&out).expect("json"));
    let effects = doc["payload"]["effects"].as_array().expect("effects array");
    let ops: Vec<(&str, &str)> = effects
        .iter()
        .map(|e| {
            (
                e["op"].as_str().unwrap_or(""),
                e["tableName"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert!(
        ops.contains(&("Rename", "Rn Item")),
        "expected a Rename effect on Rn Item, got {ops:?}"
    );
}

/// S8: the L4 db-effect solver carries a subscriber's `var` record parameter
/// across the event-dispatch edge into the PUBLISHER's frame by name
/// (`summary_runner::event_param_temp_state`), so the raiser's argument decides
/// the temp state of the subscriber's write. A PUBLIC event, so no closed-world
/// proof applies: only the edge substitution can decide. `temporary` raise ->
/// `known(true)`; physical raise -> `known(false)`.
///
/// Discrimination (2026-10-07): returning `Unknown` for `event-dispatch` in
/// `substitute_pd_temp_state` turns the temporary case into `unknown`
/// (`left: "unknown"`); restored, both pass.
#[test]
fn query_effects_event_raise_decides_the_subscribers_temp_state() {
    for (decl, want) in [
        ("\"Q Log\" temporary", "known(true)"),
        ("\"Q Log\"", "known(false)"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"id":"aaaa1111-0000-0000-0000-0000000000a8","name":"Q","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50200,"to":50299}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/All.al"),
            format!(
                "table 50200 \"Q Log\"\n{{\n    fields {{ field(1; Code; Code[20]) {{ }} }}\n    keys {{ key(PK; Code) {{ Clustered = true; }} }}\n}}\ncodeunit 50201 \"Q Pub\"\n{{\n    procedure Raise()\n    var\n        Tmp: Record {decl};\n    begin\n        OnFill(Tmp);\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnFill(var Log: Record \"Q Log\")\n    begin\n    end;\n}}\ncodeunit 50202 \"Q Sub\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Q Pub\", 'OnFill', '', false, false)]\n    local procedure Fill(var Log: Record \"Q Log\")\n    begin\n        Log.Insert();\n    end;\n}}\n"
            ),
        )
        .unwrap();
        let ws = dir.path().to_string_lossy().to_string();
        let (out, code) = run_alsem(&["query", "effects", &ws, "--routine", "Raise"], "json");
        assert_eq!(code, 0, "{out}");
        let v: serde_json::Value = crate::as_v3(serde_json::from_str(&out).unwrap());
        let effects = v["payload"]["effects"].as_array().unwrap();
        assert_eq!(effects.len(), 1, "{out}");
        assert_eq!(effects[0]["tempState"], want, "{decl}");
    }
}

/// The scope text of `alsem <argv>`'s human header (after " · scope: ";
/// `None` when a failed answer prints no analysis line) and of its JSON
/// header (`payload.result.scope.scope`).
fn scope_both_ways(argv: &[&str]) -> (Option<String>, String) {
    let (human, _) = run_alsem(argv, "human");
    let (json, _) = run_alsem(argv, "json");
    let h = human
        .lines()
        .find_map(|l| l.split(" · scope: ").nth(1))
        .map(str::to_string);
    let v: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    let j = v["payload"]["result"]["scope"]["scope"]
        .as_str()
        .unwrap_or_else(|| panic!("no payload.result.scope.scope:\n{json}"))
        .to_string();
    (h, j)
}

/// TLS-49: the human and JSON headers state one scope, in every answer the
/// query corpus holds (no dependency loaded: effects whole cone, touches from
/// a routine, `up`, workspace-wide, and the failures: absent table, `down`
/// without `--from`) and on the output fixture, whose cone enters a
/// dependency (effects and touches entering dependency code). Both come from
/// one home, the lines the body prints (`push_scope`). Not covered: effects
/// whose cone was not read (no CLI path gives one).
///
/// DISCRIMINATION (2026-11-13): making `query_scope_json` ignore the recorded
/// lines fails here at the first case
/// (`d1-multi-caller.modifyhelper.mc-customer.both`: human "dependency code:
/// none reached; dependencies: 0 apps", JSON "dependencies: 0 apps");
/// restored, passes.
#[test]
fn query_scope_is_one_fact_in_both_formats() {
    let fixture = crate::output_fixture::workspace()
        .to_string_lossy()
        .into_owned();
    let q = crate::output_fixture::ISSUE_DOC_Q;
    let entering: [(&str, Vec<&str>); 2] = [
        (
            "issuedoc.effects",
            vec!["query", "effects", &fixture, "--routine", q],
        ),
        (
            "issuedoc.base-register.touches",
            vec![
                "query",
                "touches",
                &fixture,
                "--table",
                "Base Register",
                "--from",
                q,
            ],
        ),
    ];
    for (slug, argv) in &entering {
        let (h, j) = scope_both_ways(argv);
        assert_eq!(h.as_deref(), Some(j.as_str()), "[{slug}] headers differ");
        assert!(
            !j.contains("no dependency code entered") && !j.contains("none reached"),
            "[{slug}] the fixture's cone enters a dependency: {j}"
        );
    }
    for c in CORPUS {
        let ws = fixtures_dir().join(c.workspace);
        let ws_str = ws.to_string_lossy().to_string();
        let mut argv: Vec<&str> = vec!["query", c.sub, &ws_str];
        argv.extend_from_slice(c.args);
        let (h, j) = scope_both_ways(&argv);
        match h {
            Some(h) => assert_eq!(h, j, "[{}] human and JSON headers differ", c.slug),
            // A failed answer states no scope in human form; the JSON then
            // states no depth either, only what was loaded.
            None => assert!(
                j.starts_with("dependencies: ") && !j.contains(';'),
                "[{}] the JSON states a scope the human answer does not: {j}",
                c.slug
            ),
        }
    }
}

/// TLS-49 (e): a cone that enters no dependency code but holds a call the
/// engine could not resolve says what it did not search, in both headers.
///
/// DISCRIMINATION (2026-11-13): dropping the clause's condition
/// (`if false && call_not_resolved + open_world > 0`) fails here (no "not
/// searched"); restored, passes.
#[test]
fn query_effects_whole_cone_with_an_unresolved_call_says_not_searched() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"aaaa1111-0000-0000-0000-0000000000b9","name":"Q","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50300,"to":50399}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/All.al"),
        "table 50300 \"Q Row\"\n{\n    fields { field(1; Code; Code[20]) { } }\n    keys { key(PK; Code) { Clustered = true; } }\n}\ncodeunit 50301 \"Q Caller\"\n{\n    procedure Run()\n    var\n        R: Record \"Q Row\";\n        C: Codeunit \"No Such Codeunit\";\n    begin\n        R.Insert();\n        C.DoIt();\n    end;\n}\n",
    )
    .unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let (h, j) = scope_both_ways(&["query", "effects", &ws, "--routine", "Run"]);
    assert_eq!(
        h.as_deref(),
        Some(j.as_str()),
        "human and JSON headers differ"
    );
    assert!(
        j.starts_with(
            "reads, locks, validate, writes: the whole cone, no dependency code entered (not \
             searched: calls not resolved);"
        ),
        "{j}"
    );
}

/// TLS-49: touches' machine token for a miss (`down.absence.index`) states
/// the scope line's fact. A routine whose cone enters no dependency code,
/// asked about a table it does not touch: the header says "dependency code:
/// none reached", the token `whole-cone`, and the human miss line names no
/// depth.
///
/// DISCRIMINATION (2026-11-13): `index_depth` answering the one-routine-deep
/// pair for both arms fails here at the token; restored, passes.
#[test]
fn query_touches_miss_index_states_the_scope_fact() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"aaaa1111-0000-0000-0000-0000000000ba","name":"Q","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50400,"to":50499}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/All.al"),
        "table 50400 \"Q One\"\n{\n    fields { field(1; Code; Code[20]) { } }\n    keys { key(PK; Code) { Clustered = true; } }\n}\ntable 50401 \"Q Two\"\n{\n    fields { field(1; Code; Code[20]) { } }\n    keys { key(PK; Code) { Clustered = true; } }\n}\ncodeunit 50402 \"Q Writer\"\n{\n    procedure Run()\n    var\n        R: Record \"Q One\";\n    begin\n        R.Insert();\n    end;\n}\n",
    )
    .unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let argv = [
        "query",
        "touches",
        &ws,
        "--table",
        "Q Two",
        "--from",
        "Q Writer.Run",
        "--direction",
        "down",
    ];
    let (h, j) = scope_both_ways(&argv);
    assert_eq!(h.as_deref(), Some(j.as_str()));
    assert!(j.starts_with("dependency code: none reached;"), "{j}");
    let (json, _) = run_alsem(&argv, "json");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["payload"]["down"]["absence"]["index"], "whole-cone",
        "{}",
        v["payload"]["down"]
    );
    let (human, _) = run_alsem(&argv, "human");
    assert!(
        !human.contains("one routine into dependency code"),
        "{human}"
    );
}
