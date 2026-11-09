//! FlowField reads (engine limits B5 / L19).
//!
//! A FlowField stores nothing: BC computes it from its `CalcFormula` over
//! another table, the formula's destination table
//! (<https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/devenv-flowfields>,
//! <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/properties/devenv-calcformula-property>).
//! So `Customer.CalcFields("Net Change (LCY)")` reads `Detailed Cust. Ledg.
//! Entry`, though the record operation names `Customer`. Before this module
//! every effect answer saw only the host table, and `query touches --direction
//! down` said a definite "no" for the destination table.
//!
//! [`attach_flow_field_reads`] runs once at L3 resolve, after the extension
//! fields are merged, and writes [`ModelRecordOperation::flow_field_reads`] on:
//!
//! - `CalcFields(F1, F2, ...)`: each argument that is a FlowField;
//! - `SetRange(F, ...)` / `SetFilter(F, ...)` on a FlowField: Learn says a
//!   filter on a FlowField makes BC compute it for the filtered read. The filter
//!   itself issues no query, so this places the read one step early; it is a
//!   may-read, never a definite one;
//! - every `Get` / `Find*` / `Next` on a record the routine calls
//!   `SetAutoCalcFields(F, ...)` on: Learn: the fields are "automatically
//!   calculated when the record is retrieved from the database". The calls are
//!   not ordered here: every read on that record in the routine gets them. A
//!   bare `SetAutoCalcFields(F)` is a call on the implicit record (`Rec`); as
//!   an obsolete `with X do` can make it a call on `X`, it also applies to the
//!   routine's other retrievals whose table has a FlowField of that name;
//! - and, when that record is object-level (an object global, or the implicit
//!   `Rec` a page or table keeps between triggers), every `Get` / `Find*` /
//!   `Next` on it in every routine of the same object: `SetAutoCalcFields` in
//!   `OnOpenPage` and `Rec.FindSet` in an action trigger is one record. A
//!   routine that declares a local or parameter of that name is not touched.
//!
//! Not covered (engine limits L19): a record passed to another routine. A
//! caller's `SetAutoCalcFields` or filter on a FlowField followed by a callee's
//! `Find*` on the record it received gives no read in either routine, so the
//! callee's own answer, and a caller's answer through a `SetAutoCalcFields` it
//! makes, can still be a "no". A filter in the caller is placed at the filter
//! (above), so the caller's answer is right in that case.
//!
//! Each entry is one destination table, or [`FlowFieldSource::Undecided`] with
//! its reason when the engine cannot tell which table the field reads (no
//! formula in the symbols, a formula that does not parse, a destination table
//! that is not loaded or whose name matches more than one loaded table, an
//! argument that names no field of the table). An undecided entry becomes a
//! read of the `unknown` table bucket, so a touches answer says "unknown",
//! never "no".
//!
//! Not FlowField reads: `CalcSums` sums ordinary fields of the record's own
//! table (already a read of that table); `CalcFields` on a Blob loads the
//! record's own column; a FlowFilter field holds a filter value, not data.
//!
//! The `WHERE` part of a formula names only fields of the destination table
//! (left) and constants, filters or fields of the host record (right), so the
//! destination is the only table a formula reads.
//!
//! The destination's temp state: Learn does not say how a FlowField on a
//! TEMPORARY host record is computed. The rule takes the reading that never
//! hides a read: the destination is read as the physical table it is (temporary
//! only when that table is declared `TableType = Temporary`), as d1's FlowField
//! gate already assumes.
//!
//! The record operation list is not changed: detectors see exactly what they
//! saw before. The effect builders (`summary_runner`'s base summary, the
//! capability cone's table family) and `query touches`' table mode read the
//! entries.

use std::collections::HashMap;

use super::workspace::{ModelEntities, ModelField, ModelRecordOperation, ModelRoutine, ModelTable};
use crate::program::body::features::PCallee;
use crate::program::body::node_util::strip_quotes;

/// Which table one FlowField read reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowFieldSource {
    /// The formula's destination table (internal table id) and whether that
    /// table is declared temporary.
    Table { table_id: String, temporary: bool },
    /// The engine cannot tell which table is read; the reason, in words.
    Undecided(&'static str),
}

/// One table a record operation reads through a FlowField.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowFieldRead {
    /// The FlowField's name as declared on the host table (or the argument
    /// text when it names no field).
    pub field: String,
    pub source: FlowFieldSource,
}

impl FlowFieldRead {
    /// The table id an effect row carries: the destination, or the `unknown`
    /// bucket ([`crate::engine::l4::effect_query::UNKNOWN_TABLE_ID`]).
    #[must_use]
    pub fn effect_table_id(&self) -> &str {
        match &self.source {
            FlowFieldSource::Table { table_id, .. } => table_id,
            FlowFieldSource::Undecided(_) => "unknown",
        }
    }

    /// The destination's temp state (module doc): known, and temporary only
    /// for a table declared temporary.
    #[must_use]
    pub fn temporary(&self) -> bool {
        matches!(
            self.source,
            FlowFieldSource::Table {
                temporary: true,
                ..
            }
        )
    }

    /// True when this read is the same read `op` already makes on its own
    /// record: a `CalcFields` of a FlowField over the record's own table, with
    /// the same known temp state. An effect builder emits it once.
    #[must_use]
    pub fn repeats_host_read(&self, op: &ModelRecordOperation) -> bool {
        let FlowFieldSource::Table { table_id, .. } = &self.source else {
            return false;
        };
        op.op == "CalcFields"
            && op.table_id.as_deref() == Some(table_id.as_str())
            && op
                .temp_state
                .as_ref()
                .is_some_and(|t| t.kind == "known" && t.value == Some(self.temporary()))
    }
}

pub(crate) const NO_FORMULA: &str =
    "the FlowField's CalcFormula is not in the loaded source or symbols";
pub(crate) const FORMULA_UNREADABLE: &str = "the FlowField's CalcFormula could not be read";
pub(crate) const TABLE_NOT_LOADED: &str = "the CalcFormula's table is not loaded";
pub(crate) const TABLE_AMBIGUOUS: &str =
    "the CalcFormula's table name matches more than one loaded table";
pub(crate) const FIELD_NOT_FOUND: &str = "the argument names no field of the record's table";

/// The destination table NAME of a `CalcFormula` (logical: quotes stripped,
/// `""` read as `"`), or `None` when the text does not have the documented
/// shape `[-]Kind(<Table>[.<Field>] [where(...)])`. A namespace-qualified
/// table (`Ns.Sub."Table".Field`) gives its table segment: for `exist` and
/// `count` the last segment, for the kinds that name a field the one before.
#[must_use]
pub fn calc_formula_table(formula: &str) -> Option<String> {
    let s = formula.trim().trim_start_matches('-').trim_start();
    let open = s.find('(')?;
    let kind = s[..open].trim().to_ascii_lowercase();
    let names_field = match kind.as_str() {
        "exist" | "count" => false,
        "sum" | "average" | "min" | "max" | "lookup" => true,
        _ => return None,
    };
    // The dotted reference: identifier segments up to whitespace, `(` or `)`
    // outside quotes.
    let rest = &s[open + 1..];
    let mut segments: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = rest.trim_start().chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            '.' => segments.push(std::mem::take(&mut cur)),
            c if c.is_whitespace() || c == '(' || c == ')' => break,
            c => cur.push(c),
        }
    }
    if in_quotes {
        return None;
    }
    segments.push(cur);
    if segments.iter().any(|s| s.is_empty()) {
        return None;
    }
    let table = if names_field {
        if segments.len() < 2 {
            return None;
        }
        &segments[segments.len() - 2]
    } else {
        &segments[segments.len() - 1]
    };
    Some(table.clone())
}

/// A field argument as written, as the field name it names (logical).
fn arg_field_name(arg: &str) -> String {
    strip_quotes(arg.trim()).replace("\"\"", "\"")
}

/// The tables of the model, by id (a real table wins over an extension stub,
/// as `SymbolTable` indexes them) and by name (lowercased). A name is looked up
/// among real tables first, then among stubs; a name two tables with different
/// ids share in the same group is `None`: the model keeps no namespace, so the
/// engine cannot tell which of them a formula names.
struct Tables<'a> {
    by_id: HashMap<&'a str, &'a ModelTable>,
    real_by_name: HashMap<String, Option<&'a ModelTable>>,
    stub_by_name: HashMap<String, Option<&'a ModelTable>>,
}

impl<'a> Tables<'a> {
    fn new(tables: &'a [ModelTable]) -> Self {
        let mut by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let mut real_by_name: HashMap<String, Option<&ModelTable>> = HashMap::new();
        let mut stub_by_name: HashMap<String, Option<&ModelTable>> = HashMap::new();
        for t in tables {
            let keep_id = by_id
                .get(t.id.as_str())
                .is_some_and(|p| !p.is_extension_stub && t.is_extension_stub);
            if !keep_id {
                by_id.insert(t.id.as_str(), t);
            }
            let by_name = if t.is_extension_stub {
                &mut stub_by_name
            } else {
                &mut real_by_name
            };
            by_name
                .entry(t.name.to_lowercase())
                .and_modify(|seen| {
                    if seen.is_some_and(|p| p.id != t.id) {
                        *seen = None;
                    }
                })
                .or_insert(Some(t));
        }
        Tables {
            by_id,
            real_by_name,
            stub_by_name,
        }
    }

    /// The table a formula names, or why it is not decided.
    fn by_name(&self, name: &str) -> Result<&'a ModelTable, &'static str> {
        let key = name.to_lowercase();
        match self
            .real_by_name
            .get(&key)
            .or_else(|| self.stub_by_name.get(&key))
        {
            Some(Some(t)) => Ok(t),
            Some(None) => Err(TABLE_AMBIGUOUS),
            None => Err(TABLE_NOT_LOADED),
        }
    }

    /// What reading field `arg` of table `table_id` reads through a FlowField.
    /// `strict`: a field that is not found is undecided (`CalcFields`,
    /// `SetAutoCalcFields`); otherwise it is not a FlowField read (a filter).
    fn reads_of(&self, table_id: &str, arg: &str, strict: bool, out: &mut Vec<FlowFieldRead>) {
        let Some(host) = self.by_id.get(table_id) else {
            return;
        };
        let name = arg_field_name(arg);
        let Some(field) = host
            .fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(&name))
        else {
            if strict {
                push(out, &name, FlowFieldSource::Undecided(FIELD_NOT_FOUND));
            }
            return;
        };
        self.field_reads(field, out);
    }

    fn field_reads(&self, field: &ModelField, out: &mut Vec<FlowFieldRead>) {
        if field.field_class != "FlowField" {
            return;
        }
        if field.calc_formula.is_empty() {
            push(out, &field.name, FlowFieldSource::Undecided(NO_FORMULA));
            return;
        }
        for formula in &field.calc_formula {
            let source = match calc_formula_table(formula) {
                None => FlowFieldSource::Undecided(FORMULA_UNREADABLE),
                Some(table) => match self.by_name(&table) {
                    Ok(t) => FlowFieldSource::Table {
                        table_id: t.id.clone(),
                        temporary: t.is_temporary,
                    },
                    Err(why) => FlowFieldSource::Undecided(why),
                },
            };
            push(out, &field.name, source);
        }
    }
}

/// Push unless an entry with the same source is already there (one effect row
/// per table per operation).
fn push(out: &mut Vec<FlowFieldRead>, field: &str, source: FlowFieldSource) {
    if out.iter().any(|r| r.source == source) {
        return;
    }
    out.push(FlowFieldRead {
        field: field.to_string(),
        source,
    });
}

fn is_retrieval(op: &str) -> bool {
    matches!(
        op,
        "Get" | "Find" | "FindFirst" | "FindLast" | "FindSet" | "Next"
    )
}

fn var_key(raw: &str) -> String {
    strip_quotes(raw.trim())
        .replace("\"\"", "\"")
        .to_lowercase()
}

/// The implicit record's key: a bare record call is a call on `Rec` (the
/// L2 walk's root implicit frame).
const IMPLICIT_REC: &str = "rec";

/// One routine's `SetAutoCalcFields` calls: (record key, field argument,
/// written bare). A bare call is keyed as [`IMPLICIT_REC`].
fn auto_calc_calls(routine: &ModelRoutine) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for cs in &routine.call_sites {
        let (key, bare) = match &cs.callee {
            PCallee::Member { receiver, method }
                if method.eq_ignore_ascii_case("setautocalcfields") =>
            {
                (var_key(receiver), false)
            }
            PCallee::Bare { name } if name.eq_ignore_ascii_case("setautocalcfields") => {
                (IMPLICIT_REC.to_string(), true)
            }
            _ => continue,
        };
        for arg in &cs.argument_texts {
            out.push((key.clone(), arg.clone(), bare));
        }
    }
    out
}

/// True when `key` names an object-level record in `routine`: an object
/// global, or a name the routine does not declare (the implicit `Rec`). A
/// parameter or local of that name is the routine's own record.
fn is_object_level(routine: &ModelRoutine, key: &str) -> bool {
    match routine.variables.iter().find(|v| v.name == key) {
        Some(v) => v.scope.as_deref() == Some("global"),
        None => true,
    }
}

/// Write every record operation's [`ModelRecordOperation::flow_field_reads`]
/// (module doc). Runs after record types are resolved and extension fields
/// merged.
pub fn attach_flow_field_reads(workspace: &mut ModelEntities) {
    let tables = Tables::new(&workspace.tables);
    // `SetAutoCalcFields` on an object-level record: object id -> record key
    // -> field arguments. Every routine of the object that retrieves that
    // record gets them.
    let mut object_auto: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    for routine in &workspace.routines {
        for (key, arg, _) in auto_calc_calls(routine) {
            if is_object_level(routine, &key) {
                object_auto
                    .entry(routine.object_id.clone())
                    .or_default()
                    .entry(key)
                    .or_default()
                    .push(arg);
            }
        }
    }
    for routine in &mut workspace.routines {
        let auto = auto_calc_calls(routine);
        let shared = object_auto.get(&routine.object_id);
        // The object-level keys this routine's own scope does not shadow.
        let shared: Vec<(&str, &[String])> = shared
            .into_iter()
            .flatten()
            .filter(|(key, _)| is_object_level(routine, key))
            .map(|(key, args)| (key.as_str(), args.as_slice()))
            .collect();
        for op in &mut routine.record_operations {
            let Some(table_id) = op.table_id.as_deref() else {
                continue;
            };
            let mut reads = Vec::new();
            match op.op.as_str() {
                "CalcFields" => {
                    for arg in op.field_arguments.iter().flatten() {
                        tables.reads_of(table_id, arg, true, &mut reads);
                    }
                }
                "SetRange" | "SetFilter" => {
                    if let Some(arg) = op.field_arguments.as_ref().and_then(|a| a.first()) {
                        tables.reads_of(table_id, arg, false, &mut reads);
                    }
                }
                o if is_retrieval(o) => {
                    let key = var_key(&op.record_variable_name);
                    for (k, arg, bare) in &auto {
                        if *k == key {
                            tables.reads_of(table_id, arg, true, &mut reads);
                        } else if *bare {
                            // A bare call under an obsolete `with X do` is a
                            // call on X: a may-read where X's table has the
                            // FlowField.
                            tables.reads_of(table_id, arg, false, &mut reads);
                        }
                    }
                    for (k, args) in &shared {
                        if *k == key {
                            for arg in *args {
                                tables.reads_of(table_id, arg, true, &mut reads);
                            }
                        }
                    }
                }
                _ => {}
            }
            op.flow_field_reads = reads;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_formula_table_reads_every_documented_shape() {
        let cases = [
            (
                r#"sum("Detailed Cust. Ledg. Entry"."Amount (LCY)" where("Customer No." = field("No.")))"#,
                Some("Detailed Cust. Ledg. Entry"),
            ),
            (
                r#"count("DP Line" where("Header No." = field("No.")))"#,
                Some("DP Line"),
            ),
            (
                r#"exist("Approval Comment Line" where("Table ID" = field("Table ID")))"#,
                Some("Approval Comment Line"),
            ),
            (r#"-Sum("G/L Entry".Amount)"#, Some("G/L Entry")),
            (
                r#"lookup(Customer.Name where("No." = field("Sell-to Customer No.")))"#,
                Some("Customer"),
            ),
            (r#"Count(RecordCalcFields)"#, Some("RecordCalcFields")),
            (
                r#"max(Microsoft.Sales.History."Sales Invoice Header"."Posting Date")"#,
                Some("Sales Invoice Header"),
            ),
            (r#"average("A""B".Qty)"#, Some("A\"B")),
            ("sum(Amount)", None),
            ("const(1)", None),
            (r#"sum("Unclosed.Amount)"#, None),
        ];
        for (formula, want) in cases {
            assert_eq!(calc_formula_table(formula).as_deref(), want, "{formula}");
        }
    }
}
