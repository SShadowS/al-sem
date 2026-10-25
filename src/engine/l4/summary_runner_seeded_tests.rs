//! #222 Task 1 (spec §5.1 T0a, §5.3 T2/T2b): the seeded solve alone. Every
//! test runs over every `tests/r0-corpus/` app dir (no precondition) and the
//! curated fixtures C1-C4 (each with its precondition asserted first).

use super::*;
use crate::engine::l4::analysis::field_index;
use crate::engine::l4::exec_relation::ExecRelation;
use crate::program::model::program_calls::{
    assemble_and_resolve_inline_program_default, assemble_and_resolve_workspace_program,
};
use crate::program::model::workspace::{MODEL_INSTANCE_ID_DEFAULT, Model};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// §4.7's projection, lifted to the host in Task 3.
pub(crate) use crate::engine::incremental::{L4Projection as Projection, l4_projection};

/// One solve input set: a model plus the hand-stated cross-app maps and the
/// roles round cap.
struct Case {
    name: String,
    model: Model,
    leaves: HashMap<String, RoutineSummary>,
    fixed: HashSet<String>,
    cap: Option<usize>,
}

/// A case's solve inputs, built once.
struct Prepared<'m> {
    case: &'m Case,
    rel: ExecRelation,
    fields: FieldIndex,
}

impl<'m> Prepared<'m> {
    fn new(case: &'m Case) -> Self {
        Prepared {
            case,
            rel: ExecRelation::single_app(&case.model),
            fields: field_index(&case.model.workspace),
        }
    }

    fn inputs(&self) -> V2Inputs<'_> {
        V2Inputs {
            routines: &self.case.model.workspace.routines,
            graph: self.rel.primary(),
            scc: self.rel.scc(),
            upgraded_bindings: &self.case.model.calls.upgraded_bindings,
            fields: &self.fields,
            leaf_summaries: &self.case.leaves,
            fixed_roles: &self.case.fixed,
        }
    }

    fn capped<R>(&self, f: impl FnOnce() -> R) -> R {
        match self.case.cap {
            Some(c) => with_roles_iteration_cap(c, f),
            None => f(),
        }
    }

    /// Today's command entry.
    fn fresh(&self) -> SolvedV2<()> {
        let i = self.inputs();
        let (b, m, d) = self.capped(|| {
            compute_summaries_v2_bundle_with_leaves(
                i.routines,
                i.graph,
                i.scc,
                i.upgraded_bindings,
                i.fields,
                i.leaf_summaries,
                i.fixed_roles,
            )
        });
        (b, m, d, ())
    }

    fn seeded(&self, reuse: Option<L4Reuse<'_>>) -> SeededSolve {
        let never = || false;
        let control = L4Control {
            cancel: &never,
            reuse,
        };
        self.capped(|| compute_summaries_v2_bundle_seeded(&self.inputs(), &control))
            .expect("never cancelled")
    }

    fn projection<O>(&self, s: &SolvedV2<O>) -> Projection {
        l4_projection(&s.0, &s.1, &s.2, self.rel.scc())
    }
}

const APP: &str = "22222222-0000-0000-0000-000000000222";

fn inline_model(src: &str) -> Model {
    let tables = r#"
table 50100 Cust
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
    }
    keys { key(PK; "No.") { } }
}

table 50101 Zeta
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;
    assemble_and_resolve_inline_program_default(
        &[
            ("src/Tab.al".to_string(), tables.to_string()),
            ("src/Code.al".to_string(), src.to_string()),
        ],
        APP,
    )
}

fn id_of(m: &Model, name: &str) -> String {
    let hits: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(hits.len(), 1, "precondition: one routine {name}");
    hits[0].id.clone()
}

fn plain(name: &str, model: Model) -> Case {
    Case {
        name: name.to_string(),
        model,
        leaves: HashMap::new(),
        fixed: HashSet::new(),
        cap: None,
    }
}

const C1_SRC: &str = r#"
codeunit 50120 C1
{
    procedure P(var R: Record Cust)
    begin
        Q(R);
    end;

    procedure Q(var R: Record Cust)
    begin
        R.Name := 'x';
        R.Modify();
        P(R);
    end;

    procedure U()
    var
        I: Record Zeta;
    begin
        I.Insert();
    end;
}
"#;

/// C1 capped roles: `{P, Q}` under a one-round cap. Precondition: a
/// non-convergence diagnostic and a `fixpoint-capped` uncertainty on both.
fn c1() -> Case {
    let mut c = plain("C1", inline_model(C1_SRC));
    c.cap = Some(1);
    let p = Prepared::new(&c);
    let (_, map, diags, ()) = p.fresh();
    assert!(
        diags.iter().any(|d| d.message.contains("did not converge")),
        "C1 precondition: a non-convergence diagnostic, got {diags:?}"
    );
    for name in ["P", "Q"] {
        let id = id_of(&c.model, name);
        assert!(
            map[&id]
                .uncertainties
                .iter()
                .any(|u| u.kind == "fixpoint-capped"),
            "C1 precondition: {name} carries fixpoint-capped"
        );
    }
    drop(p);
    c
}

const C2_SRC: &str = r#"
codeunit 50121 C2
{
    procedure S(var R: Record Cust)
    var
        Z: Record Zeta;
    begin
        Z.Insert();
        R.Insert();
    end;

    procedure C()
    var
        T: Record Cust temporary;
    begin
        S(T);
    end;

    procedure U()
    var
        I: Record Zeta;
    begin
        I.Modify();
    end;
}
"#;

/// C2 seeded callee, re-solved caller, different numbering. Preconditions:
/// `S` is recomputed with a parameter-dependent effect; `C` holds the
/// substituted `Known(true)` effect; `S`'s effects were interned in an order
/// other than their `key_rank` order.
fn c2() -> Case {
    let c = plain("C2", inline_model(C2_SRC));
    let p = Prepared::new(&c);
    let fresh = p.fresh();
    let (s, cid) = (id_of(&c.model, "S"), id_of(&c.model, "C"));
    let proj = p.projection(&fresh);
    let s_effects = proj.per_id[&s]
        .1
        .as_ref()
        .expect("C2 precondition: S has a row");
    assert!(
        s_effects
            .iter()
            .any(|e| matches!(e.temp_state, TempState::ParameterDependent(_))),
        "C2 precondition: S holds a parameter-dependent effect, got {s_effects:?}"
    );
    let c_effects = proj.per_id[&cid]
        .1
        .as_ref()
        .expect("C2 precondition: C has a row");
    assert!(
        c_effects
            .iter()
            .any(|e| e.temp_state == TempState::Known(true)),
        "C2 precondition: C holds a Known(true) effect, got {c_effects:?}"
    );
    let bundle = &fresh.0;
    let universe = bundle.effects().universe();
    let ix = bundle.routine_ix(&s).unwrap();
    let ids_in_key_order: Vec<u32> = bundle
        .db_effects(ix)
        .map(|e| {
            let identity = EffectIdentity {
                op: e.op.to_string(),
                table_id: e.table_id.to_string(),
                operation_id: e.operation_id.to_string(),
                temp: e.temp_state.clone(),
            };
            universe.get(&identity).unwrap().0
        })
        .collect();
    assert!(
        !ids_in_key_order.is_sorted(),
        "C2 precondition: S's discovery order differs from its key_rank order, ids {ids_in_key_order:?}"
    );
    drop(p);
    c
}

const C3_SRC: &str = r#"
codeunit 50122 C3
{
    procedure L(var R: Record Cust)
    begin
        R.Insert();
    end;

    procedure F()
    var
        X: Record Cust;
    begin
        L(X);
        X.Modify();
    end;

    procedure G()
    begin
        F();
    end;

    procedure U()
    var
        I: Record Zeta;
    begin
        I.Insert();
    end;
}
"#;

/// C3 leaf and fixed role: `L` (no out-edge) a hand-stated leaf with its own
/// summary, `F` (an out-edge) a fixed role.
fn c3() -> Case {
    let model = inline_model(C3_SRC);
    let (l, f) = (id_of(&model, "L"), id_of(&model, "F"));
    let rel = ExecRelation::single_app(&model);
    let graph = rel.primary();
    assert!(
        graph.edges_by_from.get(&l).is_none_or(|e| e.is_empty()),
        "C3 precondition: L has no out-edge"
    );
    assert!(
        graph.edges_by_from.get(&f).is_some_and(|e| !e.is_empty()),
        "C3 precondition: F has an out-edge"
    );
    let fields = field_index(&model.workspace);
    let (map, _) = compute_summaries_v2(
        &model.workspace.routines,
        graph,
        rel.scc(),
        &model.calls.upgraded_bindings,
        &fields,
    );
    let leaf = map[&l].clone();
    assert!(
        !leaf.db_effects.is_empty(),
        "C3 precondition: the leaf summary carries a db effect"
    );
    drop(rel);
    Case {
        name: "C3".to_string(),
        model,
        leaves: HashMap::from([(l, leaf)]),
        fixed: HashSet::from([f]),
        cap: None,
    }
}

const C4_SRC: &str = r#"
codeunit 50123 C4
{
    procedure A(N: Integer)
    var
        X: Record Cust;
    begin
        X.Insert();
        if N > 0 then
            B(N - 1);
    end;

    procedure B(N: Integer)
    var
        Z: Record Zeta;
    begin
        Z.Modify();
        A(N);
    end;

    procedure U()
    var
        I: Record Zeta;
    begin
        I.Delete();
    end;
}
"#;

/// C4 multi-member SCC `{A, B}`, each member with a db op of its own.
fn c4() -> Case {
    let c = plain("C4", inline_model(C4_SRC));
    let p = Prepared::new(&c);
    let (a, b) = (id_of(&c.model, "A"), id_of(&c.model, "B"));
    assert!(
        p.rel
            .scc()
            .sccs
            .iter()
            .any(|s| s.members.len() == 2 && s.members.contains(&a) && s.members.contains(&b)),
        "C4 precondition: {{A, B}} is one SCC"
    );
    let proj = p.projection(&p.fresh());
    for (name, id) in [("A", &a), ("B", &b)] {
        let row = c
            .model
            .workspace
            .routines
            .iter()
            .find(|r| &r.id == id)
            .unwrap();
        assert!(
            !row.record_operations.is_empty(),
            "C4 precondition: {name} has a db op of its own"
        );
        assert!(
            proj.per_id[id].1.as_ref().is_some_and(|e| !e.is_empty()),
            "C4 precondition: {name} has db effects"
        );
    }
    drop(p);
    c
}

/// Sorted dirs under `root`, at any depth, holding an `app.json`.
fn app_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// C1-C4 (first, so their failures are the ones shown), then every r0 corpus
/// dir (unrestricted).
fn all_cases() -> Vec<Case> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let dirs = app_dirs(&root);
    assert!(!dirs.is_empty(), "precondition: r0 dirs under {root:?}");
    let mut cases = vec![c1(), c2(), c3(), c4()];
    cases.extend(dirs.iter().map(|dir| {
        let name = dir.strip_prefix(&root).unwrap().display().to_string();
        let inputs = crate::program::inputs::WorkspaceInputs::read(dir, &Default::default());
        let model =
            assemble_and_resolve_workspace_program(&inputs, MODEL_INSTANCE_ID_DEFAULT, false)
                .unwrap_or_else(|| panic!("{name}: no single-app model built"));
        plain(&name, model)
    }));
    cases
}

fn assert_no_failures(failures: &[String]) {
    let shown: Vec<&str> = failures.iter().take(10).map(String::as_str).collect();
    assert!(
        failures.is_empty(),
        "{} failure(s), first {}:\n{}",
        failures.len(),
        shown.len(),
        shown.join("\n")
    );
}

/// T0a: with nothing to reuse, the seeded entry equals today's entry, and
/// re-solves every SCC.
#[test]
fn t0a_seeded_entry_without_reuse_equals_today() {
    let mut failures = Vec::new();
    for case in &all_cases() {
        let p = Prepared::new(case);
        let fresh = p.projection(&p.fresh());
        let seeded = p.seeded(None);
        let o = &seeded.3;
        if p.projection(&seeded) != fresh {
            failures.push(format!("{}: projection differs", case.name));
        }
        if !o.full || o.seeded != 0 || o.resolved.len() != o.total {
            failures.push(format!(
                "{}: full {}, seeded {}, resolved {} of {}",
                case.name,
                o.full,
                o.seeded,
                o.resolved.len(),
                o.total
            ));
        }
    }
    assert_no_failures(&failures);
}

/// T2: the baseline reused whole re-solves nothing and projects equal. At
/// least one SCC is seeded across the cases, so a corpus with no SCC cannot
/// pass it trivially (Task 1 review R1-2).
#[test]
fn t2_all_seeded_equals_fresh() {
    let mut failures = Vec::new();
    let mut seeded_total = 0;
    for case in &all_cases() {
        let p = Prepared::new(case);
        let first = p.seeded(None);
        let reuse = L4Reuse {
            solve: &first.3.baseline,
            bundle: &first.0,
            core: &first.1,
        };
        let second = p.seeded(Some(reuse));
        seeded_total += second.3.seeded;
        if !second.3.resolved.is_empty() || second.3.full {
            failures.push(format!(
                "{}: re-solved {:?} (full {})",
                case.name, second.3.resolved, second.3.full
            ));
        }
        if p.projection(&second) != p.projection(&first) {
            failures.push(format!("{}: projection differs", case.name));
        }
    }
    assert!(seeded_total > 0, "T2 seeded no SCC across the cases");
    assert_no_failures(&failures);
}

/// T2b: for every SCC `k`, the baseline with `k`'s row digests deleted
/// re-solves exactly `k` and its one-hop callers, seeds the rest, and
/// projects equal to a fresh solve. On C2 with `k = {C}` the mixed path is
/// asserted: `S` seeded, `C` re-solved, and an `EffectId` that differs.
#[test]
fn t2b_mixed_equals_fresh_for_every_k() {
    let mut failures = Vec::new();
    let mut c2_checked = false;
    for case in &all_cases() {
        let p = Prepared::new(case);
        let fresh = p.seeded(None);
        let fresh_proj = p.projection(&fresh);
        let graph = p.rel.primary();
        for k in &p.rel.scc().sccs {
            // The routine names of `k`, for the failure lines.
            let k_names: Vec<&str> = k
                .members
                .iter()
                .map(|id| {
                    case.model
                        .workspace
                        .routines
                        .iter()
                        .find(|r| &r.id == id)
                        .map_or(id.as_str(), |r| r.name.as_str())
                })
                .collect();
            let mut baseline = fresh.3.baseline.clone();
            let changed: HashSet<&String> = k
                .members
                .iter()
                .filter(|m| baseline.rows.remove(*m).is_some())
                .collect();
            let reuse = L4Reuse {
                solve: &baseline,
                bundle: &fresh.0,
                core: &fresh.1,
            };
            let mixed = p.seeded(Some(reuse));
            let want: BTreeSet<Vec<String>> = if changed.is_empty() {
                BTreeSet::new()
            } else {
                p.rel
                    .scc()
                    .sccs
                    .iter()
                    .filter(|s| {
                        s.members == k.members
                            || s.members.iter().any(|m| {
                                graph
                                    .edges_by_from
                                    .get(m)
                                    .into_iter()
                                    .flatten()
                                    .any(|e| changed.contains(&e.to))
                            })
                    })
                    .map(|s| s.members.clone())
                    .collect()
            };
            let got: BTreeSet<Vec<String>> = mixed.3.resolved.iter().cloned().collect();
            if got != want {
                failures.push(format!(
                    "{} k={k_names:?}: re-solved {got:?}, want {want:?}",
                    case.name
                ));
            }
            if p.projection(&mixed) != fresh_proj {
                failures.push(format!("{} k={k_names:?}: projection differs", case.name));
            }
            if case.name == "C2" && k.members == [id_of(&case.model, "C")] {
                c2_checked = true;
                let s = id_of(&case.model, "S");
                if got.contains(&vec![s]) || !got.contains(&k.members) {
                    failures.push(format!(
                        "C2 k={{C}}: want S seeded and C re-solved, got {got:?}"
                    ));
                }
                let (fu, mu) = (fresh.0.effects().universe(), mixed.0.effects().universe());
                let renumbered = (0..fu.len() as u32).any(|i| {
                    let id = crate::engine::l4::effect_universe::EffectId(i);
                    mu.get(fu.identity(id)) != Some(id)
                });
                if !renumbered {
                    failures.push("C2 k={C}: no EffectId differs between mixed and fresh".into());
                }
            }
        }
    }
    assert!(c2_checked, "precondition: C2 ran with k = {{C}}");
    assert_no_failures(&failures);
}
