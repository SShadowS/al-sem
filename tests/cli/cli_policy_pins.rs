//! Intent evidence step 7: pinning rules (rules-to-policy spec
//! `docs/superpowers/specs/2026-10-30-rules-to-policy-design.md`).
//!
//! `alsem explain --emit-policy` writes the majority rules of a field as
//! `al-sem.policy.yaml` pins (policy version 2); `alsem policy check` then
//! reports every writer of the field that does not apply them.
//!
//! What the tests pin:
//! - the emitted YAML, through the shipped binary (goldens
//!   `tests/cli-explain-goldens/emit.*.yaml`): `ws-d65` whole workspace,
//!   `ws-d65`'s `Ref` (no majority: one commented-out candidate), and the
//!   `tests/fixtures/policy-pins/bypass` fixture, whose golden is also the
//!   policy the check tests read;
//! - `policy check` with that policy (goldens
//!   `tests/cli-c-policy-goldens/pins.*`, in process with the family's pinned
//!   driver version): on `bypass` the plain bypass and the `MayNotStop`
//!   writer are found; on `fixed` (the same code with both checks added)
//!   nothing is;
//! - the round trip on `ws-d65`, through the binary: the pins
//!   `--emit-policy` writes flag exactly the writes d65 flags (compared by
//!   anchor), and emitting `fixed` gives `bypass`'s rule ids;
//! - the declared needs: a policy without a pin builds no rules, one with a
//!   pin builds them (the view `PolicyJob::demands` asks for);
//! - the loader: version 2, and each shape rule of a pin.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use al_sem::engine::batch::job::{Job, ViewKey};
use al_sem::engine::gate::policy::pipeline::{
    PolicyCheckOptions, PolicyJob, policy_request, run_policy_check,
};
use al_sem::engine::gate::policy::policy_loader::{LoadResult, load_policy_from_string};
use al_sem::engine::gate::policy::policy_types::{PolicyDoc, predicate_to_json};
use al_sem::engine::gate::run::build_view_model;
use al_sem::program::inputs::{RequestInputs, WorkspaceInputs};

use crate::regen;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ws_d65() -> PathBuf {
    repo().join("tests/r0-corpus/ws-d65")
}

fn pins_fixture(variant: &str) -> PathBuf {
    repo().join("tests/fixtures/policy-pins").join(variant)
}

fn explain_goldens() -> PathBuf {
    repo().join("tests/cli-explain-goldens")
}

fn policy_goldens() -> PathBuf {
    repo().join("tests/cli-c-policy-goldens")
}

/// The committed pins of `policy-pins/bypass` (an emit golden).
fn pins_policy() -> PathBuf {
    explain_goldens().join("emit.policy-pins.yaml")
}

/// `alsem <args>`: `(stdout, stderr, exit code)`.
fn alsem(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `explain <ws> --emit-policy <args>`: the YAML (exit 0 asserted).
fn emit(ws: &Path, args: &[&str]) -> String {
    let w = path(ws);
    let mut argv = vec!["explain", w.as_str(), "--emit-policy"];
    argv.extend(args);
    let (out, err, code) = alsem(&argv);
    assert_eq!(code, 0, "{argv:?}: {err}");
    out
}

/// Byte-compare `got` with a golden, or rewrite it under regen.
fn check_golden(golden: &Path, got: &str) {
    if regen::regen_mode() {
        std::fs::write(golden, got)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden.display()));
        return;
    }
    let want = std::fs::read_to_string(golden).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} — run REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_policy_pins::",
            golden.display()
        )
    });
    assert_eq!(
        got,
        want.replace('\r', ""),
        "diverged from {}",
        golden.display()
    );
}

fn load(yaml: &str) -> PolicyDoc {
    match load_policy_from_string(yaml) {
        LoadResult::Ok { policy, .. } => policy,
        LoadResult::Err { errors, .. } => panic!("policy does not load: {errors:?}\n{yaml}"),
    }
}

/// Each rule's id, `when` and `requires`, as `policy explain` prints them.
fn shape(p: &PolicyDoc) -> Vec<(String, String, Option<String>)> {
    p.rules
        .iter()
        .map(|r| {
            (
                r.id.clone(),
                predicate_to_json(&r.when),
                r.requires.as_ref().map(predicate_to_json),
            )
        })
        .collect()
}

#[test]
fn emit_policy_goldens() {
    for (slug, ws, args) in [
        ("emit.ws-d65", ws_d65(), &[][..]),
        (
            "emit.ws-d65-ref",
            ws_d65(),
            &["--table", "Guard Order", "--field", "Ref"][..],
        ),
        ("emit.policy-pins", pins_fixture("bypass"), &[][..]),
    ] {
        let got = emit(&ws, args);
        // Every emission is a policy the loader reads.
        let doc = load(&got);
        assert_eq!(doc.version, 2, "{slug}");
        check_golden(&explain_goldens().join(format!("{slug}.yaml")), &got);
    }
}

/// `Ref` has no majority rule: an answer (exit 0) with no pin, and the one
/// key a writer applies, commented out with its counts.
#[test]
fn emit_policy_below_the_bar_is_commented_out() {
    let got = emit(&ws_d65(), &["--table", "Guard Order", "--field", "Ref"]);
    assert!(load(&got).rules.is_empty(), "{got}");
    assert!(
        got.contains("# Below the bar: Guard Order.Ref: 1 of 4 writer(s) apply it"),
        "{got}"
    );
    assert!(
        got.contains("  #     rule.atoms: \"status <> ::open\""),
        "{got}"
    );
}

#[test]
fn emit_policy_unknown_field_exits_2() {
    let w = path(&ws_d65());
    let (out, err, code) = alsem(&[
        "explain",
        &w,
        "--emit-policy",
        "--table",
        "Guard Order",
        "--field",
        "NoSuchField",
    ]);
    assert_eq!(code, 2, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("has no field 'NoSuchField'"), "{err}");
}

#[test]
fn emit_policy_flag_misuse_exits_3() {
    let w = path(&ws_d65());
    for extra in [
        &["--routine", "BypassAmount"][..],
        &["--format", "json"][..],
    ] {
        let mut argv = vec!["explain", w.as_str(), "--emit-policy"];
        argv.extend(extra);
        let (_, err, code) = alsem(&argv);
        assert_eq!(code, 3, "{extra:?}: {err}");
        assert!(err.contains("--emit-policy"), "{err}");
    }
}

/// `policy check <variant> --policy emit.policy-pins.yaml` in process, with
/// the policy family's pinned driver version.
fn check_text(variant: &str, format: &str) -> String {
    let ws = pins_fixture(variant);
    let policy = path(&pins_policy());
    let outcome = run_policy_check(&PolicyCheckOptions {
        inputs: &WorkspaceInputs::read(&ws, &Default::default()),
        request: &RequestInputs::read(&ws, policy_request(Some(&policy), false), None),
        format,
        out: None,
        deterministic: true,
        strict: false,
        driver_version: "cli-c-v1",
        single_app: false,
    });
    assert_eq!(outcome.exit_code, 0, "{:?}", outcome.stderr_lines);
    outcome.text.expect("a report")
}

#[test]
fn policy_check_pins_goldens() {
    for variant in ["bypass", "fixed"] {
        for (format, ext) in [("json", "json"), ("human", "human.txt")] {
            check_golden(
                &policy_goldens().join(format!("pins.{variant}.{ext}")),
                &check_text(variant, format),
            );
        }
    }
}

/// The routine names `policy check --format json` reports.
fn flagged(json: &str) -> BTreeSet<String> {
    let doc: serde_json::Value = serde_json::from_str(json).expect("json");
    doc["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|f| {
            let cause = f["rootCause"].as_str().expect("rootCause");
            cause.split(' ').next().unwrap().to_string()
        })
        .collect()
}

/// The pin is violated by the plain bypass and by the writer whose check
/// may not stop (a `MayNotStop` rule is never applied), and by nothing once
/// both apply the check.
#[test]
fn policy_check_finds_the_bypass_and_passes_when_fixed() {
    let want: BTreeSet<String> = ["Bypass", "Collect"].map(String::from).into();
    assert_eq!(flagged(&check_text("bypass", "json")), want);
    assert!(flagged(&check_text("fixed", "json")).is_empty());
}

/// Fixing a bypass changes the counts, never the pins: `fixed` emits
/// `bypass`'s rules (ids, `when`, `requires`).
#[test]
fn fixing_the_bypass_keeps_the_pins() {
    let bypass = load(&emit(&pins_fixture("bypass"), &[]));
    let fixed = load(&emit(&pins_fixture("fixed"), &[]));
    assert_eq!(bypass.rules.len(), 1, "precondition: one pin");
    assert_eq!(shape(&bypass), shape(&fixed));
}

/// `(file, line, column)` of each finding's primary location.
fn anchors(findings: &[serde_json::Value]) -> BTreeSet<(String, u64, u64)> {
    findings
        .iter()
        .map(|f| {
            let p = &f["primaryLocation"];
            (
                p["sourceUnitId"].as_str().unwrap().to_string(),
                p["range"]["startLine"].as_u64().unwrap(),
                p["range"]["startColumn"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The round trip, through the binary: on `ws-d65` the pins `--emit-policy`
/// writes, checked by `policy check`, flag exactly the writes d65 flags (its
/// r4 projection, before any inline suppression: a policy records an
/// exception with `except`, not d65's comment). Both anchor a finding at the
/// writer's first write of the field.
#[test]
fn pins_round_trip_flags_what_d65_flags() {
    let dir = tempfile::tempdir().expect("temp dir");
    let policy = dir.path().join("pins.yaml");
    std::fs::write(&policy, emit(&ws_d65(), &[])).expect("write policy");
    let (out, err, code) = alsem(&[
        "policy",
        "check",
        &path(&ws_d65()),
        "--policy",
        &path(&policy),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "{err}");
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json");
    let pins = anchors(doc["findings"].as_array().expect("findings"));

    let d65: Vec<_> = al_sem::engine::l5::detectors::registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d65-guard-bypass")
        .collect();
    let model = al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &WorkspaceInputs::read(&ws_d65(), &Default::default()),
        al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
        false,
        al_sem::engine::l5::registry::assembly_needs(&d65),
    )
    .expect("ws-d65 model");
    let projection = al_sem::engine::l5::finding::project_r4_findings(
        &model,
        &d65,
        "ws-d65",
        &["d65-guard-bypass".to_string()],
    );
    let found = serde_json::to_value(&projection.findings).expect("findings json");
    let d65_anchors = anchors(found.as_array().expect("findings"));
    assert_eq!(d65_anchors.len(), 5, "precondition: d65's 5 findings");
    assert_eq!(pins, d65_anchors);
}

/// Whether the view `PolicyJob::demands` asks for, for `policy`, builds the
/// routines' rules (`Some`) on every row of `ws`.
fn view_builds_rules(ws: &Path, policy: Option<&str>) -> bool {
    let inputs = WorkspaceInputs::read(ws, &Default::default());
    let request = RequestInputs::read(ws, policy_request(policy, false), None);
    let job = PolicyJob::new(PolicyCheckOptions {
        inputs: &inputs,
        request: &request,
        format: "json",
        out: None,
        deterministic: true,
        strict: false,
        driver_version: "cli-c-v1",
        single_app: false,
    })
    .unwrap_or_else(|_| panic!("policy loads"));
    let demand = job.demands().remove(0);
    let key = ViewKey::new(ws, false, false);
    let view = build_view_model(&demand.inputs, &key, demand.needs, |_| {})
        .unwrap_or_else(|_| panic!("view"))
        .expect("a model view");
    let rows = &view.target.resolved().workspace.routines;
    assert!(rows.iter().any(|r| r.name == "Bypass"), "precondition");
    let built = rows.iter().filter(|r| r.rules.is_some()).count();
    assert!(
        built == 0 || built == rows.len(),
        "rules built on some rows only"
    );
    built > 0
}

/// The declared needs (spec §2.6): the bundled policy (no pin) builds no
/// rules, a pin policy builds them.
#[test]
fn only_a_pin_policy_builds_rules() {
    let ws = pins_fixture("bypass");
    assert!(
        !view_builds_rules(&ws, None),
        "the bundled policy built rules"
    );
    let pins = path(&pins_policy());
    assert!(
        view_builds_rules(&ws, Some(&pins)),
        "a pin policy built none"
    );
}

// ---- loader: version 2 and a pin's shape ----

fn load_err(yaml: &str) -> String {
    match load_policy_from_string(yaml) {
        LoadResult::Ok { .. } => panic!("expected a load error:\n{yaml}"),
        LoadResult::Err { errors, .. } => errors.join("; "),
    }
}

/// A pin with `when` / `requires` / extra lines filled in.
fn pin_yaml(version: u8, when: &str, requires: &str, extra: &str) -> String {
    format!(
        "version: {version}\nrules:\n  - id: pin-x\n    severity: medium\n    when:\n{when}    requires:\n{requires}{extra}"
    )
}

const WHEN: &str = "      write.table: \"Pin Order\"\n      write.field: Amount\n";
const REQUIRES: &str = "      rule.kind: TestField\n      rule.atoms: \"status <> ::open\"\n";

#[test]
fn loader_reads_a_pin_in_version_2() {
    let doc = load(&pin_yaml(2, WHEN, REQUIRES, ""));
    assert_eq!(doc.version, 2);
    assert!(doc.has_pins());
    // A version 2 file without a pin is fine too, and has none.
    assert!(!load("version: 2\nrules: []\n").has_pins());
}

#[test]
fn loader_refuses_a_bad_pin() {
    for (yaml, want) in [
        (pin_yaml(1, WHEN, REQUIRES, ""), "rules[0].requires: needs policy version 2"),
        (
            pin_yaml(2, WHEN, "      capability.op: commit\n", ""),
            "rules[0].requires: only rule.* fields",
        ),
        (
            pin_yaml(2, "      routine.name: X\n", REQUIRES, ""),
            "rules[0].when: a pin names a write.* field",
        ),
        (
            pin_yaml(2, &format!("{WHEN}      capability.op: commit\n"), REQUIRES, ""),
            "rules[0].when: a pin reads routine and write.* fields only",
        ),
        (
            pin_yaml(2, &format!("{WHEN}      rule.kind: Error\n"), REQUIRES, ""),
            "rules[0].when: a pin reads routine and write.* fields only",
        ),
        (
            pin_yaml(2, WHEN, REQUIRES, "    facts: any\n"),
            "rules[0].facts: does not apply to a pin",
        ),
        (
            pin_yaml(2, WHEN, REQUIRES, "    requireCoverage: complete\n"),
            "rules[0].requireCoverage: does not apply to a pin",
        ),
        (
            "version: 2\nrules:\n  - id: not-a-pin\n    severity: low\n    when:\n      write.field: Amount\n"
                .to_string(),
            "rules[0]: write.* and rule.* fields are for a pin",
        ),
        (
            pin_yaml(2, WHEN, "      rule.kind: Bogus\n", ""),
            "invalid enum value 'Bogus'",
        ),
        ("version: 3\nrules: []\n".to_string(), "policy version must be 1 or 2 (got 3)"),
    ] {
        let e = load_err(&yaml);
        assert!(e.contains(want), "want {want:?}, got {e:?}\n{yaml}");
    }
}
