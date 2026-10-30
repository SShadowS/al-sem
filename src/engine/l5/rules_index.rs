//! The per-field writer index (`substrate::RULES`, intent-evidence spec
//! `docs/superpowers/specs/2026-10-29-intent-evidence-design.md` §4.5 step 2):
//! for each table field the primary app writes, every writer with the rules in
//! force at each of its writes. `d65-guard-bypass` reads it from the detector
//! context; `alsem explain --table T --field F` builds the same index (it runs
//! no detector) and prints one field's entry, so both answer "which rules does
//! this writer apply" one way.
//!
//! Built once from the model, before the detector loop (the `declared-needs`
//! skill, rule 7), out of two existing parts:
//!
//! - the writers: [`field_writes`] (assignment, `Validate`, `ModifyAll`;
//!   primary-app routines only);
//! - the rules: [`CrossRules`] over the routines' own rules
//!   (`ModelRoutine::rules`, so the model must be built with
//!   `ViewNeeds::rules`: reading a model without them panics, "rules not
//!   built: declare ViewNeeds::rules").
//!
//! **Effective rules** at a write `W` of routine `R`:
//!
//! - `R`'s own rules on the path to `W` (`PRule::guards`);
//! - the rules of each `top-level` call that ends before `W`, with what those
//!   callees run in turn ([`CrossRules::effective_at`]);
//! - the table-trigger rules the write itself runs (a `Validate`'s
//!   `OnValidate`);
//! - the caller rules: the index keeps, per known caller of `R`, the rules it
//!   applies before every call it makes ([`CrossRules::caller_rules`]). A rule
//!   counts when EVERY known caller applies it: [`Writer::caller_rules`] by
//!   rule id, d65 by its rule key (two callers with their own copy of the same
//!   check both apply it). A routine with no known caller has none.
//!
//! Only rules that always stop count: a `MayNotStop` rule is never effective.

use std::collections::{BTreeMap, HashMap};

use crate::engine::l5::cross_rules::{CrossRules, Inh};
use crate::engine::l5::field_writers::{WriteKind, field_writes};
use crate::program::body::features::PAnchor;
use crate::program::body::rules::Stops;
use crate::program::model::workspace::{Model, table_by_id_preferring_real};

/// The index: one entry per written field, sorted by table id, then field.
#[derive(Debug, Default)]
pub struct RulesIndex {
    pub fields: Vec<FieldWriters>,
    /// The cross-call walk stopped at its budget or depth limit
    /// (`CrossRules::truncated`): some effective rules may be missing.
    pub truncated: bool,
}

/// The writers of one table field.
#[derive(Debug)]
pub struct FieldWriters {
    /// The table's internal id.
    pub table_id: String,
    /// The table's name.
    pub table: String,
    /// The field's name as the table declares it.
    pub field: String,
    /// One per routine (stable id), in the order of each one's first write.
    pub writers: Vec<Writer>,
}

/// One routine that writes the field.
#[derive(Debug)]
pub struct Writer {
    pub stable_id: String,
    /// The routine's first row (`model.workspace.routines`).
    pub row: usize,
    /// Its writes of the field, by anchor.
    pub sites: Vec<WriteSite>,
    /// Each known caller (its first row) with the rules it applies before
    /// every call it makes into the routine. Empty: no known caller.
    pub callers: Vec<(usize, Vec<Inh>)>,
}

/// One write of the field.
#[derive(Debug)]
pub struct WriteSite {
    pub row: usize,
    pub kind: WriteKind,
    pub anchor: PAnchor,
    /// The rules in force at the write, before the caller rules.
    pub rules: Vec<Inh>,
}

impl Writer {
    /// The caller rules every known caller applies: the same rule (by id and
    /// rewritten condition) in each one's list. Empty with no known caller.
    #[must_use]
    pub fn caller_rules(&self) -> Vec<&Inh> {
        let Some(((_, first), rest)) = self.callers.split_first() else {
            return Vec::new();
        };
        first
            .iter()
            .filter(|i| {
                rest.iter().all(|(_, rules)| {
                    rules
                        .iter()
                        .any(|j| j.rule.id == i.rule.id && j.rule.when_texts == i.rule.when_texts)
                })
            })
            .collect()
    }
}

fn anchor_key(a: &PAnchor) -> (&str, u32, u32, u32, u32) {
    (
        &a.source_unit_id,
        a.start_line,
        a.start_column,
        a.end_line,
        a.end_column,
    )
}

impl RulesIndex {
    /// Build the index over `model` (see the module doc).
    #[must_use]
    pub fn build(model: &Model) -> RulesIndex {
        Self::build_where(model, |_, _| true)
    }

    /// [`RulesIndex::build`] for the fields `keep(table id, field)` accepts
    /// only (`explain --table/--field` asks for one): the same entries, the
    /// others left out.
    #[must_use]
    pub fn build_where(model: &Model, keep: impl Fn(&str, &str) -> bool) -> RulesIndex {
        let routines = &model.workspace.routines;
        let tables = table_by_id_preferring_real(&model.workspace.tables);
        let cross = CrossRules::new(model);
        let mut rows_of: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in routines.iter().enumerate() {
            rows_of
                .entry(r.stable_routine_id.as_str())
                .or_default()
                .push(i);
        }
        // (table id, field) -> stable id -> its writes.
        type Writes<'m> = BTreeMap<&'m str, Vec<(usize, WriteKind, &'m PAnchor)>>;
        let mut by_field: BTreeMap<(&str, &str), Writes<'_>> = BTreeMap::new();
        for w in field_writes(model) {
            if !keep(w.table_id, w.field) {
                continue;
            }
            let sid = routines[w.routine_ix].stable_routine_id.as_str();
            by_field
                .entry((w.table_id, w.field))
                .or_default()
                .entry(sid)
                .or_default()
                .push((w.routine_ix, w.kind, w.anchor));
        }
        let always = |v: Vec<Inh>| -> Vec<Inh> {
            v.into_iter()
                .filter(|i| i.rule.stops == Stops::Always)
                .collect()
        };
        // Per routine (stable id): each known caller with what it applies.
        let mut callers_of: HashMap<&str, Vec<(usize, Vec<Inh>)>> = HashMap::new();
        let mut fields = Vec::with_capacity(by_field.len());
        for ((table_id, field), writes) in by_field {
            let mut writers: Vec<Writer> = Vec::with_capacity(writes.len());
            for (sid, mut sites) in writes {
                let rows = rows_of.get(sid).cloned().unwrap_or_default();
                let callers = callers_of
                    .entry(sid)
                    .or_insert_with(|| {
                        let rules = cross.caller_rules(&rows);
                        cross
                            .known_callers(&rows)
                            .into_iter()
                            .map(|c| {
                                let applied: Vec<Inh> = rules
                                    .iter()
                                    .filter(|r| r.applied_by.contains(&c))
                                    .map(|r| r.inh.clone())
                                    .collect();
                                (c, always(applied))
                            })
                            .collect()
                    })
                    .clone();
                sites.sort_by(|a, b| anchor_key(a.2).cmp(&anchor_key(b.2)).then(a.1.cmp(&b.1)));
                sites.dedup_by(|a, b| a.1 == b.1 && anchor_key(a.2) == anchor_key(b.2));
                let sites: Vec<WriteSite> = sites
                    .into_iter()
                    .map(|(row, kind, anchor)| WriteSite {
                        row,
                        kind,
                        anchor: anchor.clone(),
                        rules: always(cross.effective_at(row, anchor)),
                    })
                    .collect();
                writers.push(Writer {
                    stable_id: sid.to_string(),
                    row: rows.first().copied().unwrap_or(sites[0].row),
                    sites,
                    callers,
                });
            }
            writers.sort_by(|a, b| {
                (anchor_key(&a.sites[0].anchor), &a.stable_id)
                    .cmp(&(anchor_key(&b.sites[0].anchor), &b.stable_id))
            });
            fields.push(FieldWriters {
                table_id: table_id.to_string(),
                table: tables
                    .get(table_id)
                    .map_or_else(|| table_id.to_string(), |t| t.name.clone()),
                field: field.to_string(),
                writers,
            });
        }
        RulesIndex {
            fields,
            truncated: cross.truncated(),
        }
    }

    /// The entry of one field, if the primary app writes it.
    #[must_use]
    pub fn field(&self, table_id: &str, field: &str) -> Option<&FieldWriters> {
        self.fields
            .iter()
            .find(|f| f.table_id == table_id && f.field == field)
    }
}
