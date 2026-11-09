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
//! - every `Get` / `Find*` / `Next` on a record variable the same routine calls
//!   `SetAutoCalcFields(F, ...)` on: Learn: the fields are "automatically
//!   calculated when the record is retrieved from the database". The calls are
//!   not ordered here: every read on that variable in the routine gets them.
//!
//! Each entry is one destination table, or [`FlowFieldSource::Undecided`] with
//! its reason when the engine cannot tell which table the field reads (no
//! formula in the symbols, a formula that does not parse, a destination table
//! that is not loaded, an argument that names no field of the table). An
//! undecided entry becomes a read of the `unknown` table bucket, so a touches
//! answer says "unknown", never "no".
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

use super::workspace::{ModelEntities, ModelField, ModelTable};

#[cfg(doc)]
use super::workspace::ModelRecordOperation;
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
}

pub(crate) const NO_FORMULA: &str =
    "the FlowField's CalcFormula is not in the loaded source or symbols";
pub(crate) const FORMULA_UNREADABLE: &str = "the FlowField's CalcFormula could not be read";
pub(crate) const TABLE_NOT_LOADED: &str = "the CalcFormula's table is not loaded";
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

/// The tables of the model, by id (a real table wins over an extension stub)
/// and by name (lowercased, same rule), as `SymbolTable` indexes them.
struct Tables<'a> {
    by_id: HashMap<&'a str, &'a ModelTable>,
    by_name: HashMap<String, &'a ModelTable>,
}

impl<'a> Tables<'a> {
    fn new(tables: &'a [ModelTable]) -> Self {
        let mut by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let mut by_name: HashMap<String, &ModelTable> = HashMap::new();
        for t in tables {
            let keep_id = by_id
                .get(t.id.as_str())
                .is_some_and(|p| !p.is_extension_stub && t.is_extension_stub);
            if !keep_id {
                by_id.insert(t.id.as_str(), t);
            }
            let name = t.name.to_lowercase();
            let keep_name = by_name
                .get(&name)
                .is_some_and(|p| !p.is_extension_stub && t.is_extension_stub);
            if !keep_name {
                by_name.insert(name, t);
            }
        }
        Tables { by_id, by_name }
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
                Some(table) => match self.by_name.get(&table.to_lowercase()) {
                    Some(t) => FlowFieldSource::Table {
                        table_id: t.id.clone(),
                        temporary: t.is_temporary,
                    },
                    None => FlowFieldSource::Undecided(TABLE_NOT_LOADED),
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

/// Write every record operation's [`ModelRecordOperation::flow_field_reads`]
/// (module doc). Runs after record types are resolved and extension fields
/// merged.
pub fn attach_flow_field_reads(workspace: &mut ModelEntities) {
    let tables = Tables::new(&workspace.tables);
    for routine in &mut workspace.routines {
        // `SetAutoCalcFields` calls: receiver variable -> field arguments.
        let mut auto: HashMap<String, Vec<&str>> = HashMap::new();
        for cs in &routine.call_sites {
            if let PCallee::Member { receiver, method } = &cs.callee
                && method.eq_ignore_ascii_case("setautocalcfields")
            {
                auto.entry(var_key(receiver))
                    .or_default()
                    .extend(cs.argument_texts.iter().map(String::as_str));
            }
        }
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
                    if let Some(args) = auto.get(&var_key(&op.record_variable_name)) {
                        for arg in args {
                            tables.reads_of(table_id, arg, true, &mut reads);
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
