//! `alsem query touches --table T --field F`: every code consumer of one
//! field, by use kind (agent-tasks design §7, milestone D, narrowed v1).
//!
//! Read from the model alone, over every routine with a body (workspace and
//! the dependency code the model loaded), and built only when `--field` is
//! given: the query command owns its pipeline, so no other command pays.
//!
//! ## Extraction per operation (design §7.2)
//!
//! Each kind names the argument it reads; membership in
//! [`FIELD_ARGS_OPS`](crate::program::body::record_op::FIELD_ARGS_OPS) decides
//! nothing (`Next(2)` is a step, `Get('X')` a key value). The per-method table
//! is [`field_method`]:
//!
//! | Source | Use |
//! |---|---|
//! | a field access `R.F` (`field_accesses`), or a bare `F` through the implicit record (`implicit_field_accesses`) | read, write, or both for `R.F += 1` (one site, two uses) |
//! | `Validate` arg 0 | validate |
//! | `SetRange` / `SetFilter` / `GetFilter` / `GetRangeMin` / `GetRangeMax` / `CopyFilter` arg 0 | filter |
//! | `SetCurrentKey` every arg | key-use |
//! | `SetLoadFields` / `AddLoadFields` every arg | load |
//! | `CalcFields` / `CalcSums` / `SetAutoCalcFields` every arg | calc |
//! | `TestField` / `FieldError` arg 0 | rule |
//! | `ModifyAll` arg 0 (`modify_all_field`) | write |
//! | `FieldNo` / `FieldCaption` / `FieldName` / `FieldActive` arg 0 | metadata |
//! | `TransferFields` (receiver and `transfer_fields_source`) | copy-candidate |
//! | `RecordRef.Field(<literal>)` | by-number |
//!
//! Record operations carry their arguments (the body walk keeps them for
//! `FIELD_ARGS_OPS`); the other methods are call sites, which keep every
//! argument text.
//!
//! ## Matching
//!
//! A site matches when its record's table is `T` and the field it names is
//! `F` (by name, compared the AL way; by number for `RecordRef.Field`). A
//! site whose record's table is not resolved, and which names a field called
//! `F`, is a **candidate**, never a match and never left out.
//! `TransferFields` copies fields by number: with the other table known, the
//! site is listed only when that table has a field with `F`'s number; with
//! it unknown, it is a candidate. `RecordRef.Field(n)` counts only with the
//! RecordRef's table known in the routine ([`record_ref_tables`]), else a
//! candidate; a number that is not a literal is counted, never matched.
//! `FieldIndex(n)` is the n-th field, not field n: never matched.
//!
//! ## What is not searched (v1)
//!
//! Declaration-level consumers (CalcFormula, TableRelation, page / report /
//! query / XMLport columns) and dependency routines the model holds without a
//! body. [`FieldAnswer::not_searched`] states them; an answer with no site
//! says "no indexed consumer observed", never "no".

use std::collections::{BTreeMap, HashMap};

use al_syntax::IdentifierFoldExt;

use crate::program::body::features::{FieldAccessKind, PAnchor, PCallSite, PCallee, PTempState};
use crate::program::body::record_op::{FieldArgs, FieldUse, field_method};
use crate::program::model::role::Role;
use crate::program::model::workspace::{
    Model, ModelField, ModelRecordVariable, ModelRoutine, ModelTable, table_by_id_preferring_real,
};

/// A field name as the body walk records it, compared the AL way:
/// surrounding quotes dropped, `""` read as `"`, case folded.
#[must_use]
pub fn field_key(name: &str) -> String {
    let t = name.trim();
    let inner = t
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t);
    inner.replace("\"\"", "\"").fold_identifier()
}

/// The temporariness of the record a site uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TempClass {
    Temporary,
    Persistent,
    /// Decided by the caller (a record parameter, a table method's `Rec`).
    CallerDependent,
    Unknown,
}

impl TempClass {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TempClass::Temporary => "temporary",
            TempClass::Persistent => "persistent",
            TempClass::CallerDependent => "caller-dependent",
            TempClass::Unknown => "unknown",
        }
    }

    fn of(ts: Option<&PTempState>) -> Self {
        let Some(ts) = ts else {
            return TempClass::Unknown;
        };
        match ts.known_value() {
            Some(v) if crate::program::body::features::known_temp_suppresses(Some(v)) => {
                TempClass::Temporary
            }
            Some(_) => TempClass::Persistent,
            None if ts.kind == "parameter-dependent" => TempClass::CallerDependent,
            None => TempClass::Unknown,
        }
    }
}

/// Why a site is a candidate rather than a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Candidate {
    /// The record's table is not resolved; the field name is `F`'s.
    TableNotResolved,
    /// `TransferFields` with the other side's table not resolved.
    CopyTableNotResolved,
    /// `RecordRef.Field(<F's number>)` with the RecordRef's table not known
    /// in the routine.
    RecordRefTableUnknown,
}

impl Candidate {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Candidate::TableNotResolved => "table-not-resolved",
            Candidate::CopyTableNotResolved => "copy-table-not-resolved",
            Candidate::RecordRefTableUnknown => "recordref-table-unknown",
        }
    }
}

/// Which side of a `TransferFields` the field is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopySide {
    /// `T.TransferFields(Other)`: `F` may be written.
    Receiver,
    /// `Other.TransferFields(T)`: `F` may be read.
    Source,
}

/// One consumer site: one source location, its uses (two for `R.F += 1`).
#[derive(Debug, Clone)]
pub struct FieldSite<'m> {
    /// The routine's position in `model.workspace.routines`.
    pub routine_ix: usize,
    pub anchor: &'m PAnchor,
    pub uses: Vec<FieldUse>,
    pub temp: TempClass,
    pub candidate: Option<Candidate>,
    /// What the site is, as written: `access`, `implicit-access`, or the
    /// method name (`SetRange`, `FieldNo`, `TransferFields`, `Field`).
    pub source: String,
    /// The record variable as written (`Item`, `Rec`).
    pub record: String,
    pub copy_side: Option<CopySide>,
    pub role: Role,
}

/// What the scan read and what it could not.
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    /// Routines with a body, by role.
    pub workspace_bodies: usize,
    pub dependency_bodies: usize,
    /// Dependency routines the model holds without a body (symbols only).
    pub dependency_without_body: usize,
    /// `RecordRef.Field(x)` sites whose number is not a literal: any field.
    pub by_number_not_literal: usize,
    /// `FieldIndex(n)` sites (the n-th field; never matched).
    pub field_index_sites: usize,
}

/// The answer for one field.
#[derive(Debug, Clone)]
pub struct FieldAnswer<'m> {
    pub table: &'m ModelTable,
    pub field: &'m ModelField,
    pub sites: Vec<FieldSite<'m>>,
    /// The table's keys that contain the field (declaration, `declared-key`).
    pub declared_keys: Vec<usize>,
    pub coverage: Coverage,
}

/// The kinds no v1 answer searches, with why (design §7.4, D-8).
pub const NOT_SEARCHED: &[(&str, &str)] = &[
    (
        "calcformula",
        "CalcFormula of FlowFields (declaration-level, D2)",
    ),
    (
        "tablerelation",
        "TableRelation properties (declaration-level, D2)",
    ),
    (
        "columns",
        "page, report, query and XMLport columns and controls (declaration-level, D2)",
    ),
];

/// The searched / not-searched sentence of a field answer (output
/// validation V2, §4.2, §9.3): one helper for `query touches --field` and
/// `explain --field`, so the two say the same. `cross_app`: dependency code
/// is loaded (only what the workspace calls). Returns `(searched, not
/// searched)`, for `searched: …; not searched: …` and for the §8.1
/// `none observed (searched: …; not searched: …)` of an empty inventory.
#[must_use]
pub fn search_words(cov: &Coverage, cross_app: bool) -> (String, String) {
    let n = |k: usize, one: &str, many: &str| {
        format!(
            "{} {}",
            crate::engine::l5::routine_rollup::thousands(k),
            if k == 1 { one } else { many }
        )
    };
    let searched = format!(
        "{}, {}",
        n(
            cov.workspace_bodies,
            "workspace routine body",
            "workspace routine bodies"
        ),
        n(
            cov.dependency_bodies,
            "dependency routine body",
            "dependency routine bodies"
        )
    );
    let mut not = vec![
        "CalcFormula, TableRelation, page/report/query/XMLport columns (declaration-level)"
            .to_string(),
    ];
    not.push(if cross_app {
        "dependency code the workspace does not call (not loaded)".to_string()
    } else {
        "dependency code (none loaded)".to_string()
    });
    if cov.dependency_without_body > 0 {
        not.push(format!(
            "{} known by symbols only",
            n(
                cov.dependency_without_body,
                "dependency routine",
                "dependency routines"
            )
        ));
    }
    if cov.by_number_not_literal > 0 {
        not.push(format!(
            "dynamic field numbers ({} not matched)",
            n(cov.by_number_not_literal, "site", "sites")
        ));
    }
    (searched, not.join("; "))
}

/// Resolve `selector` to one field of `table`: a field number, or a name
/// compared the AL way. `Err` lists the candidates (empty when none).
pub fn resolve_field<'m>(
    table: &'m ModelTable,
    selector: &str,
) -> Result<&'m ModelField, Vec<String>> {
    let sel = selector.trim();
    let number: Option<i64> = (!sel.is_empty() && sel.bytes().all(|b| b.is_ascii_digit()))
        .then(|| sel.parse().ok())
        .flatten();
    let key = field_key(sel);
    let hits: Vec<&ModelField> = table
        .fields
        .iter()
        .filter(|f| number.is_some_and(|n| f.field_number == n) || field_key(&f.name) == key)
        .collect();
    match hits.as_slice() {
        [one] => Ok(one),
        many => Err(many
            .iter()
            .map(|f| format!("{} \"{}\"", f.field_number, f.name))
            .collect()),
    }
}

/// A routine's record variable by name (the AL way), or `None`.
fn record_var<'r>(r: &'r ModelRoutine, name: &str) -> Option<&'r ModelRecordVariable> {
    let key = field_key(name);
    r.record_variables
        .iter()
        .find(|rv| field_key(&rv.name) == key)
}

/// The record a call site is made on: the member receiver's variable, or the
/// implicit `Rec` for a bare call.
fn call_receiver<'r>(
    r: &'r ModelRoutine,
    cs: &'r PCallSite,
) -> Option<(&'r str, Option<&'r ModelRecordVariable>)> {
    match &cs.callee {
        PCallee::Member { receiver, .. } => Some((receiver.as_str(), record_var(r, receiver))),
        PCallee::Bare { .. } => {
            let rec = r.record_variables.iter().find(|rv| rv.is_implicit_rec)?;
            Some(("Rec", Some(rec)))
        }
        _ => None,
    }
}

fn call_method(cs: &PCallSite) -> Option<&str> {
    match &cs.callee {
        PCallee::Member { method, .. } => Some(method.as_str()),
        PCallee::Bare { name } => Some(name.as_str()),
        _ => None,
    }
}

/// Is `name` a variable the routine declares with type `RecordRef`?
fn is_record_ref_var(r: &ModelRoutine, name: &str) -> bool {
    r.variables.iter().any(|v| {
        v.name.eq_fold_identifier(name) && v.declared_type.trim().eq_ignore_ascii_case("recordref")
    })
}

/// The table each RecordRef variable of `r` holds, when the routine alone
/// decides it: a LOCAL RecordRef (not a parameter, not a global) whose every
/// `Open(<table>)` / `GetTable(<record>)` in the routine names one resolved
/// table, that is assigned nowhere and passed to no call. Anything else is
/// unknown (`None` in the map, or absent).
fn record_ref_tables<'m>(
    r: &ModelRoutine,
    by_number: &HashMap<i64, Option<&'m str>>,
    by_name: &HashMap<String, Option<&'m str>>,
) -> HashMap<String, Option<String>> {
    let mut out: HashMap<String, Option<String>> = HashMap::new();
    let locals: Vec<&str> = r
        .variables
        .iter()
        .filter(|v| {
            v.scope.as_deref() == Some("local")
                && v.declared_type.trim().eq_ignore_ascii_case("recordref")
        })
        .map(|v| v.name.as_str())
        .collect();
    for name in locals {
        let key = field_key(name);
        let mut table: Option<Option<String>> = None;
        let mut agree = |t: Option<String>| {
            table = Some(match (&table, t) {
                (None, t) => t,
                (Some(Some(a)), Some(b)) if *a == b => Some(b),
                _ => None,
            });
        };
        let mut escapes = r
            .var_assignments
            .iter()
            .any(|a| field_key(&a.lhs_name) == key);
        for cs in &r.call_sites {
            let on_it = matches!(&cs.callee, PCallee::Member { receiver, .. } if field_key(receiver) == key);
            if !on_it {
                if cs.argument_texts.iter().any(|t| field_key(t) == key) {
                    escapes = true;
                }
                continue;
            }
            let method = call_method(cs).unwrap_or("").to_ascii_lowercase();
            let arg = cs.argument_texts.first().map(String::as_str);
            match method.as_str() {
                "open" => agree(arg.and_then(|a| table_of_ref(a, by_number, by_name))),
                "gettable" => agree(
                    arg.and_then(|a| record_var(r, a))
                        .and_then(|rv| rv.table_id.clone()),
                ),
                _ => {}
            }
        }
        let decided = if escapes { None } else { table.flatten() };
        out.insert(key, decided);
    }
    out
}

/// A table reference as written: a number, `Database::"Name"`, or a name.
fn table_of_ref<'m>(
    text: &str,
    by_number: &HashMap<i64, Option<&'m str>>,
    by_name: &HashMap<String, Option<&'m str>>,
) -> Option<String> {
    let t = text.trim();
    if let Ok(n) = t.parse::<i64>() {
        return by_number.get(&n).copied().flatten().map(str::to_string);
    }
    let name = t.rsplit("::").next().unwrap_or(t);
    by_name
        .get(&field_key(name))
        .copied()
        .flatten()
        .map(str::to_string)
}

/// Every consumer site of `field` of `table` in every routine with a body
/// that `in_scope` admits (by position in `model.workspace.routines`; `--from`
/// passes the routine's cone and callers). The coverage counts the same
/// routines.
#[must_use]
pub fn field_touches<'m>(
    model: &'m Model,
    table: &'m ModelTable,
    field: &'m ModelField,
    in_scope: &dyn Fn(usize) -> bool,
) -> FieldAnswer<'m> {
    let tables = table_by_id_preferring_real(&model.workspace.tables);
    // Unique table numbers and names (an ambiguity is `None`: unknown).
    let mut by_number: HashMap<i64, Option<&str>> = HashMap::new();
    let mut by_name: HashMap<String, Option<&str>> = HashMap::new();
    for t in tables.values().filter(|t| !t.is_extension_stub) {
        by_number
            .entry(t.table_number)
            .and_modify(|e| *e = None)
            .or_insert(Some(t.id.as_str()));
        by_name
            .entry(field_key(&t.name))
            .and_modify(|e| *e = None)
            .or_insert(Some(t.id.as_str()));
    }
    let fkey = field_key(&field.name);
    let names_f = |text: &str| field_key(text) == fkey;
    let has_number = |table_id: &str| {
        tables.get(table_id).is_some_and(|t| {
            t.fields
                .iter()
                .any(|f| f.field_number == field.field_number)
        })
    };
    let mut coverage = Coverage::default();
    let mut sites: Vec<FieldSite<'m>> = Vec::new();

    for (ix, r) in model.workspace.routines.iter().enumerate() {
        if !in_scope(ix) {
            continue;
        }
        let role = model.routine_role(r);
        let has_body = r.body_available;
        match (role, has_body) {
            (Role::Primary, true) => coverage.workspace_bodies += 1,
            (Role::Dependency, true) => coverage.dependency_bodies += 1,
            (Role::Dependency, false) => coverage.dependency_without_body += 1,
            (Role::Primary, false) => {}
        }
        if !has_body {
            continue;
        }
        // The match for a record of `table_id`: a match, a candidate, or no.
        let judge = |table_id: Option<&str>| -> Option<Option<Candidate>> {
            match table_id {
                Some(t) if t == table.id => Some(None),
                Some(_) => None,
                None => Some(Some(Candidate::TableNotResolved)),
            }
        };
        let mut push = |anchor: &'m PAnchor,
                        uses: Vec<FieldUse>,
                        temp: TempClass,
                        candidate: Option<Candidate>,
                        source: &str,
                        record: &str,
                        copy_side: Option<CopySide>| {
            sites.push(FieldSite {
                routine_ix: ix,
                anchor,
                uses,
                temp,
                candidate,
                source: source.to_string(),
                record: record.to_string(),
                copy_side,
                role,
            });
        };

        // Field accesses, explicit and through the implicit record (the one
        // definition the field writers read too).
        for (fa, source) in r.field_access_sites() {
            if !names_f(&fa.field_name) {
                continue;
            }
            // A member of a RecordRef / RecordId / FieldRef is not a field.
            let Some(rv) = record_var(r, &fa.record_variable_name) else {
                continue;
            };
            let Some(candidate) = judge(rv.table_id.as_deref()) else {
                continue;
            };
            let uses = match fa.access {
                FieldAccessKind::Read => vec![FieldUse::Read],
                FieldAccessKind::Write => vec![FieldUse::Write],
                FieldAccessKind::ReadWrite => vec![FieldUse::Read, FieldUse::Write],
            };
            push(
                &fa.source_anchor,
                uses,
                TempClass::of(Some(&rv.temp_state)),
                candidate,
                source,
                &fa.record_variable_name,
                None,
            );
        }

        // Record operations.
        for op in &r.record_operations {
            if op.receiver_is_record_ref {
                continue;
            }
            let temp = TempClass::of(op.temp_state.as_ref());
            if op.op == "TransferFields" {
                let other = op
                    .transfer_fields_source
                    .as_deref()
                    .and_then(|s| record_var(r, s));
                let other_table = other.and_then(|rv| rv.table_id.as_deref());
                let mut side = |this: Option<&str>, that: Option<&str>, s: CopySide, rec: &str| {
                    if this != Some(table.id.as_str()) {
                        return;
                    }
                    let candidate = match that {
                        Some(t) if has_number(t) => None,
                        Some(_) => return,
                        None => Some(Candidate::CopyTableNotResolved),
                    };
                    push(
                        &op.source_anchor,
                        vec![FieldUse::CopyCandidate],
                        temp,
                        candidate,
                        "TransferFields",
                        rec,
                        Some(s),
                    );
                };
                side(
                    op.table_id.as_deref(),
                    other_table,
                    CopySide::Receiver,
                    &op.record_variable_name,
                );
                if let Some(src) = op.transfer_fields_source.as_deref() {
                    let src_temp =
                        other.map_or(TempClass::Unknown, |rv| TempClass::of(Some(&rv.temp_state)));
                    let this = other_table;
                    if this == Some(table.id.as_str()) {
                        let candidate = match op.table_id.as_deref() {
                            Some(t) if has_number(t) => Some(None),
                            Some(_) => None,
                            None => Some(Some(Candidate::CopyTableNotResolved)),
                        };
                        if let Some(candidate) = candidate {
                            push(
                                &op.source_anchor,
                                vec![FieldUse::CopyCandidate],
                                src_temp,
                                candidate,
                                "TransferFields",
                                src,
                                Some(CopySide::Source),
                            );
                        }
                    }
                }
                continue;
            }
            let Some((use_, at)) = field_method(&op.op.to_ascii_lowercase()) else {
                continue;
            };
            let args: Vec<&str> = if op.op == "ModifyAll" {
                op.modify_all_field.as_deref().into_iter().collect()
            } else {
                op.field_arguments
                    .iter()
                    .flatten()
                    .map(String::as_str)
                    .collect()
            };
            let named = match at {
                FieldArgs::First => args.first().is_some_and(|a| names_f(a)),
                FieldArgs::All => args.iter().any(|a| names_f(a)),
            };
            if !named {
                continue;
            }
            let Some(candidate) = judge(op.table_id.as_deref()) else {
                continue;
            };
            push(
                &op.source_anchor,
                vec![use_],
                temp,
                candidate,
                &op.op,
                &op.record_variable_name,
                None,
            );
        }

        // Call sites: record methods that are not record operations, and
        // RecordRef.Field / FieldIndex.
        let mut ref_tables: Option<HashMap<String, Option<String>>> = None;
        for cs in &r.call_sites {
            let Some(method) = call_method(cs) else {
                continue;
            };
            let method_lc = method.to_ascii_lowercase();
            if let PCallee::Member { receiver, .. } = &cs.callee
                && is_record_ref_var(r, receiver)
            {
                match method_lc.as_str() {
                    "fieldindex" => coverage.field_index_sites += 1,
                    "field" => {
                        let Some(arg) = cs.argument_texts.first() else {
                            continue;
                        };
                        let Ok(n) = arg.trim().parse::<i64>() else {
                            coverage.by_number_not_literal += 1;
                            continue;
                        };
                        if n != field.field_number {
                            continue;
                        }
                        let decided = ref_tables
                            .get_or_insert_with(|| record_ref_tables(r, &by_number, &by_name))
                            .get(&field_key(receiver))
                            .cloned()
                            .flatten();
                        let candidate = match decided.as_deref() {
                            Some(t) if t == table.id => None,
                            Some(_) => continue,
                            None => Some(Candidate::RecordRefTableUnknown),
                        };
                        push(
                            &cs.source_anchor,
                            vec![FieldUse::ByNumber],
                            TempClass::Unknown,
                            candidate,
                            "Field",
                            receiver,
                            None,
                        );
                    }
                    _ => {}
                }
                continue;
            }
            // Record operations are counted above, never twice.
            if crate::program::body::record_op::record_op_type(&method_lc).is_some() {
                continue;
            }
            let Some((use_, at)) = field_method(&method_lc) else {
                continue;
            };
            let named = match at {
                FieldArgs::First => cs.argument_texts.first().is_some_and(|a| names_f(a)),
                FieldArgs::All => cs.argument_texts.iter().any(|a| names_f(a)),
            };
            if !named {
                continue;
            }
            let Some((record, rv)) = call_receiver(r, cs) else {
                continue;
            };
            let Some(rv) = rv else {
                continue;
            };
            let Some(candidate) = judge(rv.table_id.as_deref()) else {
                continue;
            };
            push(
                &cs.source_anchor,
                vec![use_],
                TempClass::of(Some(&rv.temp_state)),
                candidate,
                method,
                record,
                None,
            );
        }
    }
    let field_id = &field.id;
    let declared_keys = table
        .keys
        .iter()
        .enumerate()
        .filter(|(_, k)| k.fields.iter().any(|f| f == field_id))
        .map(|(i, _)| i)
        .collect();
    FieldAnswer {
        table,
        field,
        sites,
        declared_keys,
        coverage,
    }
}

/// Uses counted by kind, and sites by temp class, over `sites`.
#[must_use]
pub fn use_counts(sites: &[FieldSite<'_>]) -> BTreeMap<FieldUse, usize> {
    let mut m = BTreeMap::new();
    for s in sites {
        for u in &s.uses {
            *m.entry(*u).or_insert(0) += 1;
        }
    }
    m
}

/// D1 census (agent-tasks design §7.3): the field facts the model holds, by
/// role, for `aldump --field-census`. Counts only; a probe, not an answer.
#[must_use]
pub fn census(model: &Model) -> serde_json::Value {
    #[derive(Default, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Side {
        routines: usize,
        with_body: usize,
        with_implicit_rec: usize,
        with_field_accesses: usize,
        field_accesses: usize,
        field_accesses_read_write: usize,
        implicit_field_accesses: usize,
        implicit_field_accesses_matching_a_field: usize,
        field_access_bytes: usize,
        implicit_field_access_bytes: usize,
        record_ops_by_field_kind: BTreeMap<&'static str, usize>,
        transfer_fields: usize,
        transfer_fields_with_source: usize,
        transfer_fields_source_table_resolved: usize,
        field_method_calls_by_kind: BTreeMap<&'static str, usize>,
        record_ref_field_literal: usize,
        record_ref_field_literal_table_known: usize,
        record_ref_field_not_literal: usize,
        record_ref_field_index: usize,
    }
    fn bytes(fa: &crate::program::body::features::PFieldAccess) -> usize {
        std::mem::size_of_val(fa)
            + fa.record_variable_name.capacity()
            + fa.field_name.capacity()
            + fa.source_anchor.source_unit_id.capacity()
            + fa.source_anchor.syntax_kind.capacity()
    }
    let tables = table_by_id_preferring_real(&model.workspace.tables);
    let mut by_number: HashMap<i64, Option<&str>> = HashMap::new();
    let mut by_name: HashMap<String, Option<&str>> = HashMap::new();
    for t in tables.values().filter(|t| !t.is_extension_stub) {
        by_number
            .entry(t.table_number)
            .and_modify(|e| *e = None)
            .or_insert(Some(t.id.as_str()));
        by_name
            .entry(field_key(&t.name))
            .and_modify(|e| *e = None)
            .or_insert(Some(t.id.as_str()));
    }
    let mut sides: BTreeMap<&'static str, Side> = BTreeMap::new();
    for r in &model.workspace.routines {
        let s = sides.entry(model.routine_role(r).as_str()).or_default();
        s.routines += 1;
        if !r.body_available {
            continue;
        }
        s.with_body += 1;
        if r.record_variables.iter().any(|rv| rv.is_implicit_rec) {
            s.with_implicit_rec += 1;
        }
        if !r.field_accesses.is_empty() {
            s.with_field_accesses += 1;
        }
        s.field_accesses += r.field_accesses.len();
        s.field_accesses_read_write += r
            .field_accesses
            .iter()
            .filter(|fa| fa.access == FieldAccessKind::ReadWrite)
            .count();
        s.field_access_bytes += r.field_accesses.iter().map(bytes).sum::<usize>();
        s.implicit_field_accesses += r.implicit_field_accesses.len();
        s.implicit_field_access_bytes += r.implicit_field_accesses.iter().map(bytes).sum::<usize>();
        for fa in &r.implicit_field_accesses {
            let hit = record_var(r, &fa.record_variable_name)
                .and_then(|rv| rv.table_id.as_deref())
                .and_then(|t| tables.get(t))
                .is_some_and(|t| {
                    let k = field_key(&fa.field_name);
                    t.fields.iter().any(|f| field_key(&f.name) == k)
                });
            if hit {
                s.implicit_field_accesses_matching_a_field += 1;
            }
        }
        for op in &r.record_operations {
            if op.op == "TransferFields" {
                s.transfer_fields += 1;
                if let Some(src) = op.transfer_fields_source.as_deref() {
                    s.transfer_fields_with_source += 1;
                    if record_var(r, src).is_some_and(|rv| rv.table_id.is_some()) {
                        s.transfer_fields_source_table_resolved += 1;
                    }
                }
            } else if let Some((u, _)) = field_method(&op.op.to_ascii_lowercase()) {
                *s.record_ops_by_field_kind.entry(u.as_str()).or_insert(0) += 1;
            }
        }
        let mut ref_tables = None;
        for cs in &r.call_sites {
            let Some(method) = call_method(cs) else {
                continue;
            };
            let m = method.to_ascii_lowercase();
            if let PCallee::Member { receiver, .. } = &cs.callee
                && is_record_ref_var(r, receiver)
            {
                match m.as_str() {
                    "fieldindex" => s.record_ref_field_index += 1,
                    "field" => match cs.argument_texts.first().map(|a| a.trim().parse::<i64>()) {
                        Some(Ok(_)) => {
                            s.record_ref_field_literal += 1;
                            let known = ref_tables
                                .get_or_insert_with(|| record_ref_tables(r, &by_number, &by_name))
                                .get(&field_key(receiver))
                                .cloned()
                                .flatten()
                                .is_some();
                            if known {
                                s.record_ref_field_literal_table_known += 1;
                            }
                        }
                        _ => s.record_ref_field_not_literal += 1,
                    },
                    _ => {}
                }
                continue;
            }
            if crate::program::body::record_op::record_op_type(&m).is_some() {
                continue;
            }
            if let Some((u, _)) = field_method(&m)
                && call_receiver(r, cs).is_some_and(|(_, rv)| rv.is_some())
            {
                *s.field_method_calls_by_kind.entry(u.as_str()).or_insert(0) += 1;
            }
        }
    }
    serde_json::to_value(sides).expect("census serializes")
}
