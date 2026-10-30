//! D65 — guard bypass (OPT-IN; intent-evidence spec
//! `docs/superpowers/specs/2026-10-29-intent-evidence-design.md` §4.5). A rule
//! that most writers of a table field apply, and one writer does not, is a
//! bug or a deliberate exception; the agent editing the code needs to know
//! either one. (The "deviant behaviour" method of Engler et al., SOSP 2001.)
//!
//! Reads the per-field writer index (`substrate::RULES`,
//! `l5::rules_index`): for each table field `T.F` the primary app writes,
//! each writer with the rules in force at each write (its own rules on the
//! path to the write, the rules of the calls that run before it, the table
//! triggers the write runs, and the caller rules every known caller applies).
//! A `MayNotStop` rule never counts (the index keeps none).
//!
//! **Rule key.** The rule's kind plus its condition atoms on `T`
//! (`FieldCompare` / `FieldEmpty` whose record variable's table is `T`) that
//! come from the check's own condition, an enclosing branch or an early
//! `exit` — not the `not C` an earlier check adds (`PRule::when_earlier`:
//! that check is a rule of its own, counted as one) — with
//! the record variable's name dropped and the table and field kept; an option
//! or enum value is compared by its member name (`Order.Status::Released` and
//! `"Sales Header".Status::Released` both read `::released`). An atom on
//! another table, a parameter, a call result or an `Other` atom (a condition
//! the rules pass did not understand) never forms a key, so a rule whose
//! condition has no atom on `T` has no key and is not counted. That makes the
//! detector quieter, not wrong: a rule written in an unusual shape is not
//! counted as followed or skipped.
//!
//! **Applied.** A writer applies a key when EVERY write of the field it makes
//! has a rule with that key in force, or every known caller applies one (by
//! key: two callers each with their own copy of the check both apply it).
//!
//! **Majority.** A key that at least [`MIN_FOLLOWERS`] writers apply, and at
//! least [`MIN_SHARE`] of all writers of the field, is a majority rule. Each
//! writer that does not apply it is a finding, at its first write of the
//! field, confidence `possible`.
//!
//! **Blind spot** (spec §4.5): writers are counted in the primary app only.
//! Where workspace code writes a dependency's field (a Base Application
//! table), the dependency's own writers are not counted, so a workspace writer
//! that skips the dependency's rule is not found.
//!
//! **Suppression.** `// al-sem-ignore d65-guard-bypass: <reason>` on the line
//! above the write (or at its end) marks the exception (the gate's inline
//! suppression matches the finding's line, the write's).
//!
//! **Triage** (`docs/detector-audit.md`, 2026-10-30): 0 findings on the
//! Continia DO workspace at both scopes; the CDO triage is still owed, so d65
//! stays opt-in.

use std::collections::{BTreeMap, BTreeSet};

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::cross_rules::Inh;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::rules_index::{FieldWriters, Writer};
use crate::program::body::rules::{CmpOp, ConditionAtom, PRule, RuleKind};
use crate::program::model::workspace::Model;

pub(crate) const DETECTOR: &str = "d65-guard-bypass";

/// The fewest writers that must apply a key for it to be a majority rule.
pub const MIN_FOLLOWERS: usize = 3;
/// The smallest share of a field's writers that must apply a key.
pub const MIN_SHARE: f64 = 0.75;

/// The fix text (spec §4.5).
pub const FIX: &str =
    "Add the check, or mark the exception with `// al-sem-ignore d65-guard-bypass` and a reason.";

/// An identifier as compared: quotes dropped, `""` read as `"`, folded.
fn ident(s: &str) -> String {
    let t = s.trim();
    let inner = t
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(t);
    inner.replace("\"\"", "\"").fold_identifier()
}

/// A compared value: an option or enum value by its member name, anything
/// else folded as written.
fn value(v: &str) -> String {
    let v = v.trim();
    match v.rfind("::") {
        Some(i) => format!("::{}", ident(&v[i + 2..])),
        None => v.to_lowercase(),
    }
}

fn op_text(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "<>",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn kind_text(k: RuleKind) -> &'static str {
    match k {
        RuleKind::Error => "Error",
        RuleKind::TestField => "TestField",
        RuleKind::FieldError => "FieldError",
        RuleKind::ConfirmError => "ConfirmError",
        RuleKind::ConfirmExit => "ConfirmExit",
    }
}

/// The atom's key part when it is on table `t` (folded name).
fn atom_key(a: &ConditionAtom, t: &str) -> Option<String> {
    match a {
        ConditionAtom::FieldCompare {
            table: Some(table),
            field,
            op,
            value: v,
            ..
        } if ident(table) == t => Some(format!("{} {} {}", ident(field), op_text(*op), value(v))),
        ConditionAtom::FieldEmpty {
            table: Some(table),
            field,
            negated,
            ..
        } if ident(table) == t => Some(format!(
            "{} {}",
            ident(field),
            if *negated { "is not empty" } else { "is empty" }
        )),
        _ => None,
    }
}

/// A rule's key on table `t` (module doc): `None` when no atom is on `t`.
pub(crate) fn rule_key(r: &PRule, t: &str) -> Option<String> {
    let atoms: BTreeSet<String> = r
        .when
        .iter()
        .enumerate()
        .filter(|(k, _)| !r.when_earlier.get(*k).copied().unwrap_or(false))
        .filter_map(|(_, a)| atom_key(a, t))
        .collect();
    if atoms.is_empty() {
        return None;
    }
    Some(format!(
        "{}: {}",
        kind_text(r.kind),
        atoms.into_iter().collect::<Vec<_>>().join(" and ")
    ))
}

/// The keys (with one rule of each, as an example) of `rules` on `t`.
fn keys<'a>(rules: impl Iterator<Item = &'a Inh>, t: &str) -> BTreeMap<String, &'a Inh> {
    let mut out = BTreeMap::new();
    for i in rules {
        if let Some(k) = rule_key(&i.rule, t) {
            out.entry(k).or_insert(i);
        }
    }
    out
}

/// The keys `w` applies (module doc, "Applied"), each with an example rule.
fn applied<'a>(w: &'a Writer, t: &str) -> BTreeMap<String, &'a Inh> {
    // Every write: the intersection over the sites.
    let mut sites = w.sites.iter();
    let mut own = sites
        .next()
        .map(|s| keys(s.rules.iter(), t))
        .unwrap_or_default();
    for s in sites {
        let here = keys(s.rules.iter(), t);
        own.retain(|k, _| here.contains_key(k));
    }
    // Every known caller: the intersection over the callers.
    let mut callers = w.callers.iter();
    if let Some((_, first)) = callers.next() {
        let mut via = keys(first.iter(), t);
        for (_, rules) in callers {
            let here = keys(rules.iter(), t);
            via.retain(|k, _| here.contains_key(k));
        }
        for (k, i) in via {
            own.entry(k).or_insert(i);
        }
    }
    own
}

/// d65 over one field: `(majority key, example rule, followers, the writers
/// without it)`.
type Majority<'a> = (String, &'a Inh, Vec<usize>, Vec<usize>);

fn majorities<'a>(f: &'a FieldWriters, t: &str) -> Vec<Majority<'a>> {
    let n = f.writers.len();
    let per: Vec<BTreeMap<String, &Inh>> = f.writers.iter().map(|w| applied(w, t)).collect();
    let mut count: BTreeMap<&str, (Vec<usize>, &Inh)> = BTreeMap::new();
    for (wi, keys) in per.iter().enumerate() {
        for (k, i) in keys {
            count
                .entry(k.as_str())
                .or_insert_with(|| (Vec::new(), i))
                .0
                .push(wi);
        }
    }
    let mut out = Vec::new();
    for (k, (followers, example)) in count {
        let share = followers.len() as f64 / n as f64;
        if followers.len() < MIN_FOLLOWERS || share < MIN_SHARE {
            continue;
        }
        let missing: Vec<usize> = (0..n).filter(|wi| !followers.contains(wi)).collect();
        if missing.is_empty() {
            continue;
        }
        out.push((k.to_string(), example, followers, missing));
    }
    out
}

/// The rule as a reader sees it: "rejects when <condition> (<kind>)", the
/// condition without the parts earlier checks add (as the key).
fn rule_text(r: &PRule) -> String {
    let own: Vec<&str> = r
        .when_texts
        .iter()
        .enumerate()
        .filter(|(k, _)| !r.when_earlier.get(*k).copied().unwrap_or(false))
        .map(|(_, t)| t.as_str())
        .collect();
    let when = if own.is_empty() {
        "always".to_string()
    } else {
        own.join(" and ")
    };
    let msg = r
        .message
        .as_ref()
        .and_then(|m| m.text.as_deref())
        .map(|t| format!(": \"{t}\""))
        .unwrap_or_default();
    match r.kind {
        RuleKind::ConfirmExit => format!("exits when {when} (a Confirm answered no){msg}"),
        k => format!("rejects when {when} ({}){msg}", kind_text(k)),
    }
}

pub fn detect_d65(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let index = ctx.get_rules_index();
    let routines = &resolved.workspace.routines;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates = 0usize;
    let mut too_few = 0u64;
    let mut no_majority = 0u64;
    for f in &index.fields {
        let t = ident(&f.table);
        candidates += f.writers.len();
        if f.writers.len() < MIN_FOLLOWERS {
            too_few += 1;
            continue;
        }
        let found = majorities(f, &t);
        if found.is_empty() {
            no_majority += 1;
            continue;
        }
        let name = |wi: usize| {
            let r = &routines[f.writers[wi].row];
            let object = ctx.declaring.of(r).map_or("", |o| o.name.as_str());
            format!("{object}::{}", r.name)
        };
        for (key, example, followers, missing) in found {
            let examples: Vec<String> = followers.iter().take(2).map(|&wi| name(wi)).collect();
            for wi in missing {
                let w = &f.writers[wi];
                let site = &w.sites[0];
                let routine = &routines[site.row];
                let id = format!("d65/{}/{}/{}/{}", routine.id, f.table_id, f.field, key);
                let n = f.writers.len();
                let root_cause = format!(
                    "{} writes {}.{} without the rule that {} of {} writers of this field \
                     apply: {} (for example {}).",
                    routine.name,
                    f.table,
                    f.field,
                    followers.len(),
                    n,
                    rule_text(&example.rule),
                    examples.join(" and "),
                );
                let anchor = anchor_of(&site.anchor, routine);
                let mut finding = Finding {
                    id: id.clone(),
                    root_cause_key: id,
                    detector: DETECTOR.to_string(),
                    title: "Write skips a rule the other writers apply".into(),
                    root_cause,
                    severity: "low".to_string(),
                    confidence: to_confidence(&[], "possible"),
                    primary_location: anchor.clone(),
                    evidence_path: vec![EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor,
                        note: format!(
                            "writes {}.{} ({}) without: {key}",
                            f.table,
                            f.field,
                            site.kind.as_str()
                        ),
                    }],
                    additional_paths: None,
                    affected_objects: vec![routine.object_id.as_str().into()],
                    affected_tables: vec![f.table_id.as_str().into()],
                    fix_options: vec![FixOption {
                        description: FIX.into(),
                        safety: "medium".into(),
                    }],
                    provenance: vec![Evidence {
                        source: "tree-sitter",
                        note: None,
                    }],
                    actionable_anchor: None,
                    fingerprint: None,
                    event_kind: None,
                    cross_extension_subscribers: None,
                    cohort_contexts: None,
                    loose_subject: None,
                };
                finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
                findings.push(finding);
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates, emitted);
    stats.add_skip("tooFewWriters", too_few);
    stats.add_skip("noMajorityRule", no_majority);
    stats.add_skip("crossCallTruncated", u64::from(index.truncated));
    Ok(DetectorOutput::no_diag(findings, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_compare_by_member_name_and_identifiers_fold() {
        assert_eq!(value("Order.Status::Released"), "::released");
        assert_eq!(value("\"Sales Header\".Status::\"Released\""), "::released");
        assert_eq!(value("''"), "''");
        assert_eq!(ident("\"Calls Order\""), ident("calls order"));
    }
}
