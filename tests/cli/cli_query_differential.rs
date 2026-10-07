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
//! `DbEffectQuery` facade and the `RoutineIx -> L3Routine` join — forever, with
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
    std::fs::write(golden_path, got)
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
struct Case {
    slug: &'static str,
    workspace: &'static str,
    /// Argv AFTER `query <sub> <workspace>` — flags only.
    args: &'static [&'static str],
    sub: &'static str,
    exit_code: i32,
}

const CORPUS: &[Case] = &[
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
];

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
    serde_json::from_str(&got).expect("query JSON parses")
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

    let w = &up["witnesses"][0];
    assert_eq!(w["depth"], serde_json::json!(2));
    assert_eq!(w["op"], serde_json::json!("ModifyAll"));
    assert_eq!(w["routine"]["display"], serde_json::json!("PostSalesDoc"));
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
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json");
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
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let effects = v["payload"]["effects"].as_array().unwrap();
        assert_eq!(effects.len(), 1, "{out}");
        assert_eq!(effects[0]["tempState"], want, "{decl}");
    }
}
