//! `alsem explain --emit-policy` (rules-to-policy spec
//! `docs/superpowers/specs/2026-10-30-rules-to-policy-design.md` §2.5): the
//! majority rules of a table field, written as `al-sem.policy.yaml` pins
//! (version 2), so `alsem policy check` reports every later writer of the
//! field that does not apply them.
//!
//! One key, one bar, one index: each entry is one key d65 forms
//! ([`d65::key_counts`], d65's `rule_key` and `applied`), kept when it passes
//! d65's majority bar ([`d65::KeyCount::is_majority`]), over the writer index
//! d65 reads ([`RulesIndex`]). Unlike d65, a unanimous key is kept: the rule
//! nobody skips yet is the first one worth pinning.
//!
//! - `--table T --field F --emit-policy`: that field; every key below the bar
//!   is also written, commented out, with its counts, so a team can confirm a
//!   rule by hand (the triage on Continia DO found a majority on few fields).
//! - `--emit-policy` alone: every field the primary app writes, majority keys
//!   only.
//!
//! An entry's id hangs on the key alone (a slug, the kind and 8 hex of a
//! SHA-256 of the folded table, field and key), never on the counts, so
//! fixing a bypass and emitting again gives the same ids. The counts go in a
//! comment above the entry.
//!
//! Needs: the model with its rules (`ViewNeeds::rules`, no card text) and no
//! plan at all; the index is built here, as `explain --table/--field` builds
//! it.

use std::fmt::Write as _;

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::ids::sha256_hex;
use crate::engine::l5::detectors::d65::{
    self, KeyCount, MIN_FOLLOWERS, MIN_SHARE, RuleKey, rule_text, table_key,
};
use crate::engine::l5::explain::find_field;
use crate::engine::l5::rules_index::{FieldWriters, RulesIndex};
use crate::engine::session::Plan;
use crate::program::model::declaring::DeclaringObjects;
use crate::program::model::workspace::Model;

/// Which fields to emit.
#[derive(Debug, Clone)]
pub enum EmitScope {
    /// `--table T --field F`: that field, with the keys below the bar
    /// commented out.
    Field { table: String, field: String },
    /// No selector: every field the primary app writes, majority keys only.
    Workspace,
}

/// The command's inputs.
pub struct EmitPolicyRequest<'a> {
    pub inputs: &'a dyn WorkspaceRead,
    pub scope: EmitScope,
    pub single_app: bool,
}

/// `alsem explain --emit-policy` as a batch job.
pub struct EmitPolicyJob<'a> {
    req: EmitPolicyRequest<'a>,
    out: Option<&'a str>,
}

impl<'a> EmitPolicyJob<'a> {
    #[must_use]
    pub fn new(req: EmitPolicyRequest<'a>, out: Option<&'a str>) -> EmitPolicyJob<'a> {
        EmitPolicyJob { req, out }
    }
}

/// The job's side document.
enum Emitted {
    /// The policy document (exit 0).
    Yaml(String),
    /// An unknown table or field: the human diagnostic (exit 2).
    NotFound(String),
}

impl Job for EmitPolicyJob<'_> {
    fn name(&self) -> &'static str {
        "explain"
    }

    fn demands(&self) -> Vec<ViewDemand> {
        // The rules (the writer index is built from them), not the card text.
        ViewDemand::one(
            self.req.inputs,
            self.req.single_app,
            false,
            ViewNeeds {
                rules: true,
                ..ViewNeeds::MODEL
            },
        )
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        // No tree, no run: the index is built in `compute`.
        Ok(PlanPart {
            plan: Plan::merge(std::iter::empty()),
            reads: vec![],
            carry: None,
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let model = cx.view().model();
        let doc = match &self.req.scope {
            EmitScope::Workspace => {
                Emitted::Yaml(emit_policy(model, &RulesIndex::build(model), false))
            }
            EmitScope::Field { table, field } => match find_field(model, table, field) {
                Err((_, human)) => Emitted::NotFound(human),
                Ok((mt, mf)) => {
                    let (tid, fname) = (mt.id.clone(), mf.name.clone());
                    let index = RulesIndex::build_where(model, |t, f| t == tid && f == fname);
                    Emitted::Yaml(emit_policy(model, &index, true))
                }
            },
        };
        Ok(SideDoc::new(Ok::<_, String>(doc)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<Emitted, _>(
            failure.target.message("explain"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        match doc.one::<Result<Emitted, String>>() {
            Err(f) => f,
            Ok(Ok(Emitted::NotFound(line))) => JobOutput::line(&line, 2),
            Ok(r) => JobOutput::text(
                self.out,
                r.map(|e| match e {
                    Emitted::Yaml(text) => (text, 0),
                    Emitted::NotFound(_) => unreachable!("handled above"),
                }),
            ),
        }
    }
}

/// A YAML double-quoted scalar.
fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Text for a YAML comment: one line.
fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

/// `[a-z0-9-]` only: other characters become `-`, runs of `-` collapse.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The longest id the policy loader accepts (`^[a-z][a-z0-9-]{2,80}$`).
const MAX_ID: usize = 81;

/// A pin's id (module doc): stable for a key, whatever the counts.
#[must_use]
pub(crate) fn pin_id(table: &str, field: &str, key: &RuleKey) -> String {
    let hash = sha256_hex(&format!(
        "{}\n{}\n{key}",
        table_key(table),
        table_key(field)
    ));
    let kind = key.kind.to_ascii_lowercase();
    // "pin-" + slug + "-" + kind + "-" + 8 hex.
    let budget = MAX_ID - 4 - 1 - kind.len() - 1 - 8;
    let mut s = slug(&format!("{table} {field}"));
    s.truncate(budget);
    let s = s.trim_end_matches('-');
    let s = if s.is_empty() { "field" } else { s };
    format!("pin-{s}-{kind}-{}", &hash[..8])
}

/// One entry, under `rules:`; `comment`: each line commented out (a key
/// below the bar).
fn entry(out: &mut String, f: &FieldWriters, c: &KeyCount, comment: bool) {
    let p = if comment { "  # " } else { "  " };
    let lines = [
        format!("- id: {}", pin_id(&f.table, &f.field, &c.key)),
        format!(
            "  title: {}",
            q(&format!(
                "Writers of {}.{} apply: {}",
                f.table, f.field, c.key
            ))
        ),
        format!("  description: {}", q(&rule_text(&c.example.rule))),
        "  severity: medium".to_string(),
        "  when:".to_string(),
        format!("    write.table: {}", q(&f.table)),
        format!("    write.field: {}", q(&f.field)),
        "  requires:".to_string(),
        format!("    rule.kind: {}", q(c.key.kind)),
        format!("    rule.atoms: {}", q(&c.key.atoms)),
    ];
    for l in lines {
        let _ = writeln!(out, "{p}{l}");
    }
}

/// The policy document for every field of `index` (module doc).
/// `candidates`: also write each key below the bar, commented out.
#[must_use]
pub fn emit_policy(model: &Model, index: &RulesIndex, candidates: bool) -> String {
    let routines = &model.workspace.routines;
    let declaring = DeclaringObjects::new(&model.workspace.objects);
    let name = |f: &FieldWriters, wi: usize| {
        let r = &routines[f.writers[wi].row];
        let object = declaring.of(r).map_or("", |o| o.name.as_str());
        one_line(&format!("{object}::{}", r.name))
    };
    let names = |f: &FieldWriters, wis: &[usize]| {
        wis.iter()
            .map(|&wi| name(f, wi))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut active = String::new();
    let mut below = String::new();
    for f in &index.fields {
        let n = f.writers.len();
        for c in d65::key_counts(f) {
            let counts = format!(
                "{}.{}: {} of {n} writer(s) apply it",
                one_line(&f.table),
                one_line(&f.field),
                c.followers.len()
            );
            if c.is_majority() {
                let examples: Vec<usize> = c.followers.iter().take(2).copied().collect();
                let _ = writeln!(
                    active,
                    "  # {counts} (for example {}).",
                    names(f, &examples)
                );
                if c.missing.is_empty() {
                    let _ = writeln!(active, "  # Every writer applies it.");
                } else {
                    let _ = writeln!(active, "  # Without it: {}.", names(f, &c.missing));
                }
                entry(&mut active, f, &c, false);
            } else if candidates {
                let _ = writeln!(
                    below,
                    "  # Below the bar: {counts} ({}). To pin it, uncomment the entry.",
                    names(f, &c.followers)
                );
                entry(&mut below, f, &c, true);
            }
        }
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# al-sem policy: rules pinned by `alsem explain --emit-policy`.\n\
         # Each entry is a rule that at least {MIN_FOLLOWERS} writers, and at least {} % of the\n\
         # writers, of a table field apply (d65-guard-bypass's majority rule). `alsem\n\
         # policy check` reports every writer of the field that does not apply it.\n\
         # Keep the entries that are business rules and delete the rest; record a\n\
         # deliberate exception with `except: {{ routine.name: <name> }}`.\n\
         version: 2",
        (MIN_SHARE * 100.0).round()
    );
    if active.is_empty() {
        let _ = writeln!(out, "rules: []");
        if !below.is_empty() {
            let _ = writeln!(
                out,
                "# No rule passes the bar. To pin one below, replace `rules: []` with\n\
                 # `rules:` and uncomment its entry."
            );
        }
    } else {
        let _ = writeln!(out, "rules:");
        out.push_str(&active);
    }
    out.push_str(&below);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_fit_the_loader_and_hang_on_the_key_alone() {
        let key = RuleKey {
            kind: "ConfirmError",
            atoms: "status <> ::open".to_string(),
        };
        let long = "A Very Long Table Name That Goes On And On And On Past Every Limit";
        let id = pin_id(long, "Some Equally Long Field Name", &key);
        assert!(
            id.len() <= MAX_ID && id.starts_with("pin-a-very-long"),
            "{id}"
        );
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
        assert_eq!(id, pin_id(long, "Some Equally Long Field Name", &key));
        let other = RuleKey {
            atoms: "status <> ::released".to_string(),
            ..key.clone()
        };
        assert_ne!(id, pin_id(long, "Some Equally Long Field Name", &other));
        assert_eq!(
            pin_id("\"Sales Header\"", "Status", &key),
            pin_id("sales header", "status", &key),
            "folded names give one id"
        );
    }

    #[test]
    fn quoting_escapes_what_yaml_needs() {
        assert_eq!(q("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }
}
