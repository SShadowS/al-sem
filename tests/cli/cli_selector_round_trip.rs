//! Output validation V3 (spec 2026-11-06 §2.1, §12.1): the selector round
//! trip. Every `selector` the engine emits, and every display of a covered
//! form (a workspace `Object::Routine`, with a parameter list,
//! `Object."Field"::Trigger`, `Object.action(Name "Caption")::OnAction`),
//! resolves to its own stable id through the one resolution every tool
//! reads (`resolve_changed_roots`: explain, prove, digest, impact, tests).
//!
//! Two scopes:
//! - every workspace routine of the fixture workspaces the CLI goldens run
//!   on (the set of selectors those goldens can hold), and every
//!   `selector` value written in the fixture goldens themselves;
//! - CDO (gated on `CDO_WS`): every workspace routine of the baseline, and
//!   every `selector` in the CDO rendering goldens.
//!
//! Discrimination (2026-11-06): with the member forms removed from the
//! grammar (`resolve_member_selector` answering `None` for every
//! selector), `fixture_selectors_and_displays_resolve_to_their_own_routine`
//! fails on every member trigger (first: `ws-explain`'s
//! `Page "Explain Order Card".action(Post)::OnAction` resolves to `[]`), and
//! `member_displays_paste_back_with_and_without_caption` fails too;
//! restored, both pass (9 member triggers, 41 golden selectors checked).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use al_sem::engine::l4::effect_query_cli::OwnerIndex;
use al_sem::engine::l5::digest_cli::{ChangedInput, DiffRoot, resolve_changed_roots};
use al_sem::engine::l5::routine_rollup::RoutineNames;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
use al_sem::program::model::workspace::{AssemblyNeeds, Model};

/// The fixture workspaces the CLI goldens run on (explain, query, digest,
/// prove, impact, tests), plus the member-trigger and overload fixtures.
const FIXTURES: &[&str] = &[
    "ws-explain",
    "ws-explain-calls",
    "ws-sibling-member-triggers",
    "ws-triggers",
    "ws-implicit-trigger",
    "ws-rules",
    "ws-d8-commit-in-tx",
    "ws-d1-multi-caller",
    "ws-d34",
    "ws-d35",
    "ws-d14-dead-routine",
    "ws-d51-pos",
    "ws-r2a-record-types",
    "ws-perm-temp",
    "ws-policy-api-ui",
];

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus")
}

fn model_of(ws: &Path) -> Model {
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    assemble_and_resolve_workspace_program(&inputs, false, AssemblyNeeds::BASE)
        .unwrap_or_else(|| panic!("model of {}", ws.display()))
}

/// The roots `selector` resolves to (sorted), through the one resolution.
fn resolve(model: &Model, selector: &str) -> Vec<String> {
    let snap = compose_snapshot(model);
    resolve_with(model, &snap.identities, selector)
}

fn resolve_with(
    model: &Model,
    ids: &al_sem::engine::l5::snapshot::SnapshotIdentityTable,
    selector: &str,
) -> Vec<String> {
    let owners = OwnerIndex::build(model);
    let changed = resolve_changed_roots(
        ids,
        model,
        &ChangedInput {
            files: None,
            routines: Some(vec![selector.to_string()]),
            diff_text: None,
            diff_root: DiffRoot::default(),
            workspace_files: &[],
        },
        al_sem::engine::l5::digest_cli::primary_guid_of(model),
        &owners,
    );
    let mut roots: Vec<String> = changed.roots.into_iter().collect();
    roots.sort();
    roots
}

/// Every workspace routine's selector, and its display (with caption), each
/// resolve to the routine's own stable id. Returns the failures and the
/// number of member triggers checked.
fn round_trip_model(label: &str, model: &Model) -> (Vec<String>, usize) {
    let names = RoutineNames::new(model, &[]);
    let snap = compose_snapshot(model);
    let mut failures = Vec::new();
    let mut members = 0usize;
    let ids: BTreeSet<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|r| names.is_primary(&r.stable_routine_id))
        .map(|r| r.stable_routine_id.as_str())
        .collect();
    for id in ids {
        let row = names.row(id).unwrap();
        if row.enclosing_member.is_some() {
            members += 1;
        }
        let selector = names.selector(id);
        let display = names.display(id);
        for (what, text) in [("selector", &selector), ("display", &display)] {
            let got = resolve_with(model, &snap.identities, text);
            if got != [id.to_string()] {
                failures.push(format!(
                    "{label}: {what} `{text}` of {id} resolves to {got:?}"
                ));
            }
        }
    }
    (failures, members)
}

/// Every `{selector, stableId}` pair written anywhere in the JSON files
/// under `dir` (recursively).
fn golden_selectors(dir: &Path) -> BTreeMap<String, BTreeSet<String>> {
    fn walk(v: &Value, out: &mut BTreeMap<String, BTreeSet<String>>) {
        match v {
            Value::Object(m) => {
                let id = m
                    .get("stableId")
                    .or_else(|| m.get("id"))
                    .and_then(Value::as_str);
                if let (Some(sel), Some(id)) = (m.get("selector").and_then(Value::as_str), id)
                    && id.contains('#')
                {
                    out.entry(id.to_string())
                        .or_default()
                        .insert(sel.to_string());
                }
                m.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "json")
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok(v) = serde_json::from_str::<Value>(&text)
            {
                walk(&v, &mut out);
            }
        }
    }
    out
}

#[test]
fn fixture_selectors_and_displays_resolve_to_their_own_routine() {
    let mut failures = Vec::new();
    let mut members = 0usize;
    let mut models: Vec<(String, Model)> = Vec::new();
    for f in FIXTURES {
        let model = model_of(&corpus().join(f));
        let (fails, m) = round_trip_model(f, &model);
        failures.extend(fails);
        members += m;
        models.push((f.to_string(), model));
    }
    assert!(
        members > 0,
        "the fixtures hold member triggers (the forms under test)"
    );
    // Every selector the fixture goldens hold, against the fixture whose
    // workspace holds its routine.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut checked = 0usize;
    for dir in std::fs::read_dir(&root).unwrap().flatten() {
        let name = dir.file_name().to_string_lossy().to_string();
        if !(name.starts_with("cli-") && name.ends_with("-goldens")) {
            continue;
        }
        for (id, selectors) in golden_selectors(&dir.path()) {
            let holders: Vec<&(String, Model)> = models
                .iter()
                .filter(|(_, m)| {
                    m.workspace
                        .routines
                        .iter()
                        .any(|r| r.stable_routine_id == id)
                })
                .collect();
            if holders.is_empty() {
                continue;
            }
            for sel in &selectors {
                checked += 1;
                if !holders.iter().any(|(_, m)| resolve(m, sel) == [id.clone()]) {
                    failures.push(format!(
                        "golden selector `{sel}` of {id} does not resolve to it"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    eprintln!("round trip: {members} member triggers; {checked} golden selectors");
}

/// The member forms the display prints, pasted back (§2.1): with and
/// without the action caption, the field quoted or bare, and a caption on
/// input that the source does not declare (ignored).
#[test]
fn member_displays_paste_back_with_and_without_caption() {
    let model = model_of(&corpus().join("ws-sibling-member-triggers"));
    let names = RoutineNames::new(&model, &[]);
    let mut actions = 0;
    for r in &model.workspace.routines {
        let Some(member) = &r.enclosing_member else {
            continue;
        };
        let id = r.stable_routine_id.as_str();
        let hop = names.display_with(id, false).unwrap();
        assert_eq!(resolve(&model, &hop), [id.to_string()], "{hop}");
        if r.member_detail.as_ref().is_some_and(|d| d.kind == "action") {
            actions += 1;
            let with_caption = hop.replacen(
                &format!("action({member})"),
                &format!("action({member} \"Any Caption\")"),
                1,
            );
            assert_ne!(with_caption, hop, "the action form is printed");
            assert_eq!(
                resolve(&model, &with_caption),
                [id.to_string()],
                "{with_caption}"
            );
        }
    }
    assert!(actions > 0, "the fixture holds page actions");
}

/// The CDO round trip (run by `output_cdo::cdo_selectors_round_trip`, so
/// `scripts/cdo-gate` runs it). One resolution per selector over ~20,000
/// routines is quadratic, so every workspace routine's selector and display
/// go through ONE batched resolution (no diagnostic, and the roots are
/// exactly the routines: a selector that named no routine, two, or a
/// routine no other selector names shows), and each member trigger and each
/// overloaded routine, where a wrong single answer is possible, is resolved
/// alone.
pub(crate) fn cdo_round_trip(ws: &Path) {
    let model = model_of(ws);
    let names = RoutineNames::new(&model, &[]);
    let snap = compose_snapshot(&model);
    let ids: BTreeSet<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|r| names.is_primary(&r.stable_routine_id))
        .map(|r| r.stable_routine_id.as_str())
        .collect();
    let mut failures = Vec::new();
    let mut members = 0usize;
    for what in ["selector", "display"] {
        let selectors: Vec<String> = ids
            .iter()
            .map(|id| match what {
                "selector" => names.selector(id),
                _ => names.display(id),
            })
            .collect();
        let owners = OwnerIndex::build(&model);
        let changed = resolve_changed_roots(
            &snap.identities,
            &model,
            &ChangedInput {
                files: None,
                routines: Some(selectors),
                diff_text: None,
                diff_root: DiffRoot::default(),
                workspace_files: &[],
            },
            al_sem::engine::l5::digest_cli::primary_guid_of(&model),
            &owners,
        );
        for d in &changed.diagnostics {
            failures.push(format!("CDO {what}: {d:?}"));
        }
        let roots: BTreeSet<&str> = changed.roots.iter().map(String::as_str).collect();
        if roots != ids {
            failures.push(format!(
                "CDO {what}s: {} roots for {} routines",
                roots.len(),
                ids.len()
            ));
        }
    }
    for &id in &ids {
        let r = names.row(id).unwrap();
        let alone = r.enclosing_member.is_some() || names.selector(id).ends_with(')');
        if !alone {
            continue;
        }
        if r.enclosing_member.is_some() {
            members += 1;
        }
        for text in [names.selector(id), names.display(id)] {
            let got = resolve_with(&model, &snap.identities, &text);
            if got != [id.to_string()] {
                failures.push(format!("CDO `{text}` of {id} resolves to {got:?}"));
            }
        }
    }
    assert!(members > 100, "CDO holds many member triggers ({members})");
    let goldens = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cli-output-goldens/cdo");
    let mut checked = 0usize;
    for (id, selectors) in golden_selectors(&goldens) {
        if !model
            .workspace
            .routines
            .iter()
            .any(|r| r.stable_routine_id == id)
        {
            continue;
        }
        for sel in &selectors {
            checked += 1;
            if resolve_with(&model, &snap.identities, sel) != [id.clone()] {
                failures.push(format!(
                    "CDO golden selector `{sel}` of {id} does not resolve to it"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    eprintln!(
        "CDO round trip: {} routines; {members} member triggers alone; {checked} golden selectors",
        ids.len()
    );
}
