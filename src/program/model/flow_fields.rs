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
//!   bare `SetAutoCalcFields(F)` is a call on the record of its innermost
//!   implicit record frame (`PCallSite::implicit_receiver`): the `X` of an
//!   enclosing `with X do`, else the object's implicit `Rec`. That is the same
//!   frame a bare `FindSet()` takes its record from, so a bare call never
//!   reaches another variable's retrieval;
//! - and, when that record is object-level, every `Get` / `Find*` / `Next` on
//!   it in every routine that shares it. The implicit `Rec` (any name the
//!   routine does not declare) is shared by the routines of the object AND of
//!   every extension of the same base object: a page extension's triggers run
//!   on the extended page's `Rec`, a table extension's procedures on the
//!   table's. So `SetAutoCalcFields` in a page's `OnOpenPage` and `Rec.FindSet`
//!   in an action a page extension adds is one record. An object global is
//!   shared by the routines of its own object; a base object's global also by
//!   its extensions (a `protected var`). A routine that declares a local or
//!   parameter of that name is not touched. The sharing is keyed by the base
//!   object's type and NAME (an extension names its target by name), so two
//!   same-named pages of different apps share too: a may-read, never a lost
//!   one;
//! - a RecordRef: `RecRef.SetAutoCalcFields(N, ...)` names fields by NUMBER
//!   (Learn, `RecordRef.SetAutoCalcFields([Fields: Integer,...])`), looked up
//!   by number in the RecordRef's table;
//! - a `FieldRef` (Learn: `FieldRef.CalcField()` "Updates FlowFields in a
//!   record", like `Record.CalcFields`): `FR.CalcField()` is a `CalcFields`
//!   of the FieldRef's field on its RecordRef's record, and
//!   `FR.SetFilter`/`SetRange` a filter on it. These are not record
//!   operations, so they become [`ModelRoutine::field_ref_operations`] (the
//!   detectors do not see them). The FieldRef's field is read off
//!   `FR := RecRef.Field(<number>|'<name>'|Rec.FieldNo(<field>))` and its
//!   table off the RecordRef's resolved operations or provable bindings. A
//!   `CalcField` whose table is not known is a read of a table not resolved
//!   ("unknown"); a field that is not known on a table with FlowFields is
//!   "not decided". A filter reads what every binding of the FieldRef reads
//!   (two assignments on two tables: both); a binding whose table is not
//!   known, or a FieldRef whose bindings are not readable (a parameter, a
//!   global, a local passed to a call), is "not decided". So a FieldRef
//!   filter is never dropped unless every binding is decided and none names a
//!   FlowField.
//!
//! A field argument written with its record (`X.SetFilter(X."Total
//! Amount", ...)`, `X.CalcFields(X."F")`) names the field after the last dot.
//! An argument that names no field of a table that HAS FlowFields is not
//! decided; on a table without FlowFields nothing can be calculated, so it
//! reads nothing more.
//!
//! Not covered (engine limits L19): a record passed to another routine. A
//! caller's `SetAutoCalcFields` or filter on a FlowField followed by a callee's
//! `Find*` on the record it received gives no read in either routine, so the
//! callee's own answer, and a caller's answer through a `SetAutoCalcFields` it
//! makes, can still be a "no". A filter in the caller is placed at the filter
//! (above), so the caller's answer is right in that case. A FieldRef received
//! as a parameter or from a call is not linked to the caller's either: its
//! table and field are not known, so a `CalcField` or a filter on it answers
//! "unknown" (never "no").
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

use al_syntax::IdentifierFoldExt;

use super::workspace::{
    ModelEntities, ModelField, ModelObject, ModelRecordOperation, ModelRoutine, ModelTable,
};
use crate::program::body::features::{PAnchor, PCallSite, PCallee, PTempState};
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
pub(crate) const HOST_NOT_LOADED: &str = "the record's table is not loaded";
pub(crate) const FIELD_REF_FIELD_UNKNOWN: &str =
    "the FieldRef's field is not known, and its table has FlowFields";
pub(crate) const FIELD_REF_TABLE_UNKNOWN: &str =
    "the FieldRef's table is not known, so its field may be a FlowField";

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

/// How an argument names a field.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldSel {
    /// By name (logical: quotes stripped, `""` read as `"`).
    Name(String),
    /// By number (a RecordRef's `SetAutoCalcFields(4)`, `Field(4)`).
    Number(i64),
    /// Not readable from the text.
    Unknown,
}

/// The dotted segments of `s` outside double quotes (`X."No."` -> `X`,
/// `"No."`), each trimmed.
fn dotted_segments(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '.' if !in_quotes => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// A name as written, logical: quotes stripped, `""` read as `"`.
fn logical_name(s: &str) -> String {
    strip_quotes(s.trim()).replace("\"\"", "\"")
}

/// `Recv.Method(Args)` as written: the receiver text, the method folded and
/// the argument text, or `None` for any other shape.
fn member_call(text: &str) -> Option<(&str, String, &str)> {
    let t = text.trim();
    let body = t.strip_suffix(')')?;
    // The `(` that opens the final argument list, outside quotes and nested
    // parentheses.
    let mut depth = 0usize;
    let mut in_dq = false;
    let mut in_sq = false;
    let mut open = None;
    for (i, c) in body.char_indices().rev() {
        match c {
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            ')' if !in_dq && !in_sq => depth += 1,
            '(' if !in_dq && !in_sq => {
                if depth == 0 {
                    open = Some(i);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let open = open?;
    let head = &body[..open];
    let segments = dotted_segments(head);
    if segments.len() < 2 {
        return None;
    }
    let method = segments[segments.len() - 1];
    let recv_len = head.rfind(method)?;
    let recv = head[..recv_len].trim_end().strip_suffix('.')?.trim();
    Some((recv, logical_name(method).to_lowercase(), &body[open + 1..]))
}

/// The field an argument names. A record-qualified field (`X."Total
/// Amount"`) names the field after its last dot; `Rec.FieldNo(F)` names `F`;
/// an integer literal is a field number; a text literal (`RecRef.Field('No.')`)
/// a field name.
fn field_sel(arg: &str) -> FieldSel {
    let a = arg.trim();
    if let Ok(n) = a.parse::<i64>() {
        return FieldSel::Number(n);
    }
    if let Some(inner) = a.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return FieldSel::Name(inner.replace("''", "'"));
    }
    if a.ends_with(')') {
        // A bare `FieldNo(F)` on the implicit record.
        if let Some(head) = a.get(..8)
            && head.eq_ignore_ascii_case("fieldno(")
        {
            return field_sel(&a[8..a.len() - 1]);
        }
        return match member_call(a) {
            Some((_, method, inner)) if method == "fieldno" => field_sel(inner),
            _ => FieldSel::Unknown,
        };
    }
    let segments = dotted_segments(a);
    let last = segments.last().copied().unwrap_or_default();
    // A name ending in a dot outside quotes (`"No."` stripped elsewhere) is
    // not split: read the whole text.
    let name = if last.is_empty() { a } else { last };
    if name.is_empty() {
        return FieldSel::Unknown;
    }
    FieldSel::Name(logical_name(name))
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
    /// Real tables by number (a RecordRef's `Open(18)`); `None` when two
    /// apps' tables share the number.
    real_by_number: HashMap<i64, Option<&'a ModelTable>>,
}

impl<'a> Tables<'a> {
    fn new(tables: &'a [ModelTable]) -> Self {
        let mut by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let mut real_by_name: HashMap<String, Option<&ModelTable>> = HashMap::new();
        let mut stub_by_name: HashMap<String, Option<&ModelTable>> = HashMap::new();
        let mut real_by_number: HashMap<i64, Option<&ModelTable>> = HashMap::new();
        for t in tables {
            if !t.is_extension_stub {
                real_by_number
                    .entry(t.table_number)
                    .and_modify(|seen| {
                        if seen.is_some_and(|p| p.id != t.id) {
                            *seen = None;
                        }
                    })
                    .or_insert(Some(t));
            }
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
            real_by_number,
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

    /// What reading field argument `arg` of table `table_id` reads through a
    /// FlowField ([`field_sel`]).
    fn reads_of(&self, table_id: &str, arg: &str, calculates: bool, out: &mut Vec<FlowFieldRead>) {
        self.sel_reads(
            table_id,
            &field_sel(arg),
            arg.trim(),
            calculates,
            FIELD_NOT_FOUND,
            out,
        );
    }

    /// What reading the field `sel` of table `table_id` reads through a
    /// FlowField. A field that is not found (or not known) is not decided
    /// (`not_found`) when the table has a FlowField, and reads nothing more
    /// when it has none. `calculates`: the call computes the field it names
    /// (`CalcFields`, `SetAutoCalcFields`, `CalcField`), so a table that is not
    /// loaded is not decided; a filter on such a table adds nothing (its
    /// retrieval is the read).
    fn sel_reads(
        &self,
        table_id: &str,
        sel: &FieldSel,
        written: &str,
        calculates: bool,
        not_found: &'static str,
        out: &mut Vec<FlowFieldRead>,
    ) {
        let Some(host) = self.by_id.get(table_id) else {
            if calculates {
                push(out, written, FlowFieldSource::Undecided(HOST_NOT_LOADED));
            }
            return;
        };
        let field = match sel {
            FieldSel::Name(name) => host
                .fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(name)),
            FieldSel::Number(n) => host.fields.iter().find(|f| f.field_number == *n),
            FieldSel::Unknown => None,
        };
        match field {
            Some(field) => self.field_reads(field, out),
            None if host.fields.iter().any(|f| f.field_class == "FlowField") => {
                let label = match sel {
                    FieldSel::Name(name) => name.clone(),
                    _ => written.to_string(),
                };
                push(out, &label, FlowFieldSource::Undecided(not_found));
            }
            None => {}
        }
    }

    /// The table a RecordRef `Open(<literal>)` binding names.
    fn by_reference(&self, reference: &str, is_name: bool) -> Option<&'a ModelTable> {
        if is_name {
            self.by_name(reference).ok()
        } else {
            reference
                .trim()
                .parse::<i64>()
                .ok()
                .and_then(|n| self.real_by_number.get(&n).copied().flatten())
        }
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
        "Get" | "GetBySystemId" | "Find" | "FindFirst" | "FindLast" | "FindSet" | "Next"
    )
}

fn var_key(raw: &str) -> String {
    strip_quotes(raw.trim())
        .replace("\"\"", "\"")
        .to_lowercase()
}

/// A record receiver's key, spelled as a retrieval's receiver is
/// (`record_ops::receiver_key_text`: `Recs[ 1 ]` is `Recs[1]`, `this.G` is `G`).
/// Not for a declared name: its spaces are the name's (`Line [Temp]`).
fn receiver_key(raw: &str) -> String {
    var_key(&crate::record_ops::receiver_key_text(raw))
}

/// Whether a receiver is written through `this` (`this.G`): the object's
/// global, whatever local shadows its name.
fn through_this(raw: &str) -> bool {
    raw.trim_start()
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("this."))
}

/// One routine's `SetAutoCalcFields` calls: (record key, field argument,
/// written through `this`). A bare call is keyed by its implicit record frame
/// (`PCallSite::implicit_receiver`: a `with` record, else `Rec`), the frame a
/// bare retrieval takes its record from; a bare call with no record frame is
/// not a record method.
fn auto_calc_calls(routine: &ModelRoutine) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for cs in &routine.call_sites {
        let (key, this) = match &cs.callee {
            PCallee::Member { receiver, method }
                if method.eq_ignore_ascii_case("setautocalcfields") =>
            {
                (receiver_key(receiver), through_this(receiver))
            }
            PCallee::Bare { name } if name.eq_ignore_ascii_case("setautocalcfields") => {
                match &cs.implicit_receiver {
                    Some(r) => (receiver_key(r), false),
                    None => continue,
                }
            }
            _ => continue,
        };
        for arg in &cs.argument_texts {
            out.push((key.clone(), arg.clone(), this));
        }
    }
    out
}

/// What an object's routines share an implicit record with (module doc):
/// the sharing groups (`page:<base name>` for a page and its extensions,
/// likewise `table:` and `report:`; `id:<object id>` for any other object),
/// and whether the object is a base object (its globals are visible to its
/// extensions as `protected var`).
struct Sharing<'a> {
    by_object: HashMap<&'a str, (Vec<String>, bool)>,
}

impl<'a> Sharing<'a> {
    fn new(objects: &'a [ModelObject]) -> Self {
        let mut by_object: HashMap<&'a str, (Vec<String>, bool)> = HashMap::new();
        for o in objects {
            let (groups, base) = object_groups(o);
            // An `#if`-split object has a row per arm under one id: their
            // groups add up.
            let entry = by_object.entry(o.id.as_str()).or_default();
            for g in groups {
                if !entry.0.contains(&g) {
                    entry.0.push(g);
                }
            }
            entry.1 |= base;
        }
        Sharing { by_object }
    }

    /// The scopes record `key` is shared in, seen from `routine`: none for
    /// its own local or parameter; the object for a global (and the group
    /// for a base object's global); the group for a name the routine does
    /// not declare (the implicit `Rec`).
    fn scopes(&self, routine: &ModelRoutine, key: &str, this: bool) -> Vec<String> {
        let own = self.by_object.get(routine.object_id.as_str());
        let group_scopes = || -> Vec<String> {
            match own {
                Some((groups, _)) => groups.iter().map(|g| format!("{g}|{key}")).collect(),
                None => vec![format!("id:{}|{key}", routine.object_id)],
            }
        };
        // Whether the routine sees `key` as a global (`None`: it declares no
        // such name). `this.G` is the global whatever local shadows it; an
        // array element (`recs[1]`) is shared as its array variable is, a
        // name that itself holds `[` (`"Line [Temp]"`) as written.
        let declared = if this {
            Some(true)
        } else {
            let find = |k: &str| routine.variables.iter().find(|v| var_key(&v.name) == k);
            find(key)
                .or_else(|| find(strip_quotes(key.split('[').next().unwrap_or(key))))
                .map(|v| v.scope.as_deref() == Some("global"))
        };
        match declared {
            Some(true) => {
                let mut s = vec![format!("obj:{}|{key}", routine.object_id)];
                if own.is_some_and(|(_, base)| *base) {
                    s.extend(group_scopes());
                }
                s
            }
            Some(false) => Vec::new(),
            None => group_scopes(),
        }
    }
}

/// The sharing groups of one object row and whether it is a base object.
fn object_groups(o: &ModelObject) -> (Vec<String>, bool) {
    let family = match o.object_type.as_str() {
        "Page" | "PageExtension" => "page",
        "Table" | "TableExtension" => "table",
        "Report" | "ReportExtension" => "report",
        _ => return (vec![format!("id:{}", o.id)], false),
    };
    if !o.object_type.ends_with("Extension") {
        return (vec![format!("{family}:{}", o.name.fold_identifier())], true);
    }
    let Some(target) = o.extends_target_name.as_deref() else {
        return (vec![format!("id:{}", o.id)], false);
    };
    // The target as written, and its last dotted segment when it is
    // namespace-qualified (`Microsoft.Sales."Customer Card"`).
    let mut groups = vec![format!("{family}:{}", target.fold_identifier())];
    let segments = dotted_segments(target);
    if segments.len() > 1
        && let Some(last) = segments.last().filter(|s| !s.is_empty())
    {
        let g = format!("{family}:{}", logical_name(last).fold_identifier());
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    (groups, false)
}

/// The table and temp state a RecordRef `key` holds in `routine`: the one
/// table its resolved operations name, else the one table its provable
/// bindings name (`ModelRoutine::recordref_bindings`); `None` when neither
/// decides it.
fn record_ref_table(
    routine: &ModelRoutine,
    key: &str,
    tables: &Tables<'_>,
) -> (Option<String>, Option<PTempState>) {
    let mut table: Option<&str> = None;
    let mut seen = false;
    let mut decided = true;
    let mut temp: Option<&PTempState> = None;
    let mut temp_decided = true;
    for op in &routine.record_operations {
        if var_key(&op.record_variable_name) != key {
            continue;
        }
        seen = true;
        match (op.table_id.as_deref(), table) {
            (Some(t), None) => table = Some(t),
            (Some(t), Some(x)) if t == x => {}
            _ => decided = false,
        }
        match (op.temp_state.as_ref(), temp) {
            (Some(t), None) => temp = Some(t),
            (Some(t), Some(x)) if t == x => {}
            _ => temp_decided = false,
        }
    }
    let temp = temp.filter(|_| temp_decided).cloned();
    if seen {
        return (table.filter(|_| decided).map(str::to_string), temp);
    }
    let Some((_, bindings)) = routine.recordref_bindings.iter().find(|(v, _)| v == key) else {
        return (None, None);
    };
    let mut table: Option<String> = None;
    for b in bindings {
        let t = match b {
            crate::program::recordref_table::Binding::Table { reference, is_name } => tables
                .by_reference(reference, *is_name)
                .map(|t| t.id.clone()),
            crate::program::recordref_table::Binding::GetTable(v) => routine
                .record_variables
                .iter()
                .find(|rv| rv.name.eq_fold_identifier(v))
                .and_then(|rv| rv.table_id.clone()),
        };
        match (t, &table) {
            (Some(t), None) => table = Some(t),
            (Some(t), Some(x)) if t == *x => {}
            _ => return (None, None),
        }
    }
    (table, None)
}

/// True when `receiver` is a FieldRef: a variable declared `FieldRef`, or a
/// `RecRef.Field(...)` / `RecRef.FieldIndex(...)` written in place.
fn is_field_ref(routine: &ModelRoutine, receiver: &str) -> bool {
    if let Some((_, m, _)) = member_call(receiver) {
        return m == "field" || m == "fieldindex";
    }
    let key = var_key(receiver);
    routine
        .variables
        .iter()
        .find(|v| var_key(&v.name) == key)
        .is_some_and(|v| v.declared_type.trim().eq_ignore_ascii_case("fieldref"))
}

fn within(inner: &PAnchor, outer: &PAnchor) -> bool {
    (inner.start_line, inner.start_column) >= (outer.start_line, outer.start_column)
        && (inner.end_line, inner.end_column) <= (outer.end_line, outer.end_column)
}

/// Where a FieldRef receiver comes from: each (RecordRef key, field) it can
/// be, or `None` when that is not readable. A `RecRef.Field(n)` in place is
/// itself; a LOCAL FieldRef variable is every `FR := RecRef.Field(...)` /
/// `FieldIndex(...)` assigned to it, provided nothing else binds it (another
/// assignment, or passing it to a call, which a `var` parameter could
/// rebind). A parameter or global is bound outside the routine.
fn field_ref_sources(routine: &ModelRoutine, receiver: &str) -> Option<Vec<(String, FieldSel)>> {
    let from_call = |text: &str| -> Option<(String, FieldSel)> {
        let (recv, m, inner) = member_call(text)?;
        match m.as_str() {
            "field" => Some((var_key(recv), field_sel(inner))),
            "fieldindex" => Some((var_key(recv), FieldSel::Unknown)),
            _ => None,
        }
    };
    if receiver.trim().ends_with(')') {
        return from_call(receiver).map(|s| vec![s]);
    }
    let key = var_key(receiver);
    let var = routine.variables.iter().find(|v| var_key(&v.name) == key)?;
    if var.scope.as_deref() != Some("local") {
        return None;
    }
    let passed = routine
        .call_sites
        .iter()
        .any(|cs| cs.argument_texts.iter().any(|a| var_key(a) == key));
    if passed {
        return None;
    }
    let mut out = Vec::new();
    for a in routine
        .var_assignments
        .iter()
        .filter(|a| var_key(&a.lhs_name) == key)
    {
        // The assignment's whole right-hand side is a `RecRef.Field(...)` /
        // `FieldIndex(...)` call: a call site inside the statement that ends
        // where the statement ends.
        let cs: &PCallSite = routine.call_sites.iter().find(|cs| {
            within(&cs.source_anchor, &a.source_anchor)
                && (cs.source_anchor.end_line, cs.source_anchor.end_column)
                    == (a.source_anchor.end_line, a.source_anchor.end_column)
        })?;
        let PCallee::Member { receiver, method } = &cs.callee else {
            return None;
        };
        let sel = match method.to_ascii_lowercase().as_str() {
            "field" => cs
                .argument_texts
                .first()
                .map_or(FieldSel::Unknown, |arg| field_sel(arg)),
            "fieldindex" => FieldSel::Unknown,
            _ => return None,
        };
        out.push((var_key(receiver), sel));
    }
    (!out.is_empty()).then_some(out)
}

/// The routine's FieldRef operations (module doc,
/// [`ModelRoutine::field_ref_operations`]).
fn field_ref_operations(routine: &ModelRoutine, tables: &Tables<'_>) -> Vec<ModelRecordOperation> {
    let mut out = Vec::new();
    for cs in &routine.call_sites {
        if cs.control_context.as_deref() == Some("unreachable") {
            continue;
        }
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        let op = match method.to_ascii_lowercase().as_str() {
            "calcfield" => "CalcFields",
            "setfilter" => "SetFilter",
            "setrange" => "SetRange",
            _ => continue,
        };
        if !is_field_ref(routine, receiver) {
            continue;
        }
        let sources = field_ref_sources(routine, receiver);
        let (mut table_id, mut temp_state, mut field) = (None, None, FieldSel::Unknown);
        if let Some(sources) = &sources {
            let mut tables_of = sources
                .iter()
                .map(|(recref, _)| record_ref_table(routine, recref, tables));
            let first = tables_of.next().unwrap_or((None, None));
            if tables_of.all(|t| t == first) {
                (table_id, temp_state) = first;
            }
            if sources.iter().all(|(_, f)| *f == sources[0].1) {
                field = sources[0].1.clone();
            }
        }
        let calculates = op == "CalcFields";
        let mut reads = Vec::new();
        if calculates {
            // A `CalcField` is a read of its record (an unresolved one when
            // the bindings do not agree on one table), and of the field's
            // FlowField table.
            if let Some(t) = &table_id {
                tables.sel_reads(
                    t,
                    &field,
                    receiver,
                    true,
                    FIELD_REF_FIELD_UNKNOWN,
                    &mut reads,
                );
            }
        } else {
            // A filter is no read of its own: it reads what a FlowField it
            // names reads. That is the reads of every (table, field) the
            // FieldRef can be bound to (a may-read); a binding whose table is
            // not known, or a FieldRef whose bindings are not readable (a
            // parameter, a global, one passed to a call), may be a FlowField
            // and is not decided. Only when every binding is decided and none
            // is a FlowField is the filter no effect.
            match &sources {
                Some(sources) => {
                    for (recref, sel) in sources {
                        match record_ref_table(routine, recref, tables).0 {
                            Some(t) => tables.sel_reads(
                                &t,
                                sel,
                                receiver,
                                false,
                                FIELD_REF_FIELD_UNKNOWN,
                                &mut reads,
                            ),
                            None => push(
                                &mut reads,
                                receiver,
                                FlowFieldSource::Undecided(FIELD_REF_TABLE_UNKNOWN),
                            ),
                        }
                    }
                }
                None => push(
                    &mut reads,
                    receiver,
                    FlowFieldSource::Undecided(FIELD_REF_TABLE_UNKNOWN),
                ),
            }
            if reads.is_empty() {
                continue;
            }
        }
        out.push(ModelRecordOperation {
            id: format!("{}{}", cs.id, super::workspace::FIELD_REF_OP_SUFFIX),
            op: op.to_string(),
            record_variable_name: receiver.clone(),
            record_variable_id: None,
            table_id,
            temp_state: Some(temp_state.unwrap_or_else(|| PTempState {
                kind: "unknown".to_string(),
                value: None,
                parameter_index: None,
            })),
            field_arguments: None,
            source_anchor: cs.source_anchor.clone(),
            loop_stack: cs.loop_stack.clone(),
            field_argument_infos: None,
            in_until_condition: false,
            in_statement_position: cs.in_statement_position,
            run_trigger: None,
            modify_all_field: None,
            transfer_fields_source: None,
            receiver_is_record_ref: true,
            flow_field_reads: reads,
            dataset_reason: None,
        });
    }
    out
}

/// Write every record operation's [`ModelRecordOperation::flow_field_reads`]
/// and every routine's [`ModelRoutine::field_ref_operations`] (module doc).
/// Runs after record types are resolved and extension fields merged.
pub fn attach_flow_field_reads(workspace: &mut ModelEntities) {
    let pass = Pass::new(&workspace.tables, &workspace.objects, &workspace.routines);
    for routine in &mut workspace.routines {
        let mut ops = std::mem::take(&mut routine.record_operations);
        pass.attach(routine, &mut ops);
        routine.record_operations = ops;
        routine.field_ref_operations = field_ref_operations(routine, &pass.tables);
    }
}

/// ENG-12: [`attach_flow_field_reads`]'s rules for the operations appended
/// after it ran (a call result's, `GetRef().CalcFields(F)`; a shadowed
/// `this.G.FindFirst()`'s): for each `(routine, first appended op)`.
pub fn attach_appended_flow_field_reads(
    workspace: &mut ModelEntities,
    appended: &HashMap<usize, usize>,
) {
    let needs = |op: &ModelRecordOperation| {
        op.table_id.is_some() && (op.op == "CalcFields" || is_retrieval(&op.op))
    };
    let any = appended.iter().any(|(&ri, &from)| {
        workspace.routines[ri].record_operations[from..]
            .iter()
            .any(needs)
    });
    if !any {
        return;
    }
    let pass = Pass::new(&workspace.tables, &workspace.objects, &workspace.routines);
    for (&ri, &from) in appended {
        let routine = &mut workspace.routines[ri];
        let mut ops = routine.record_operations.split_off(from);
        pass.attach(routine, &mut ops);
        routine.record_operations.extend(ops);
    }
}

/// What every FlowField rule reads: the tables, the sharing groups, and each
/// shared record's `SetAutoCalcFields` (scope, module doc -> field arguments;
/// every routine that retrieves that record in that scope gets them).
struct Pass<'a> {
    tables: Tables<'a>,
    sharing: Sharing<'a>,
    shared_auto: HashMap<String, Vec<String>>,
}

impl<'a> Pass<'a> {
    fn new(
        tables: &'a [ModelTable],
        objects: &'a [ModelObject],
        routines: &[ModelRoutine],
    ) -> Self {
        let sharing = Sharing::new(objects);
        let mut shared_auto: HashMap<String, Vec<String>> = HashMap::new();
        for routine in routines {
            for (key, arg, this) in auto_calc_calls(routine) {
                for scope in sharing.scopes(routine, &key, this) {
                    shared_auto.entry(scope).or_default().push(arg.clone());
                }
            }
        }
        Pass {
            tables: Tables::new(tables),
            sharing,
            shared_auto,
        }
    }

    /// Write the FlowField reads of `ops`, operations of `routine`.
    fn attach(&self, routine: &ModelRoutine, ops: &mut [ModelRecordOperation]) {
        let auto = auto_calc_calls(routine);
        let mut scopes_of: HashMap<(String, bool), Vec<String>> = HashMap::new();
        for op in ops {
            let Some(table_id) = op.table_id.as_deref() else {
                op.flow_field_reads = Vec::new();
                continue;
            };
            let mut reads = Vec::new();
            match op.op.as_str() {
                "CalcFields" => {
                    for arg in op.field_arguments.iter().flatten() {
                        self.tables.reads_of(table_id, arg, true, &mut reads);
                    }
                }
                "SetRange" | "SetFilter" => {
                    if let Some(arg) = op.field_arguments.as_ref().and_then(|a| a.first()) {
                        self.tables.reads_of(table_id, arg, false, &mut reads);
                    }
                }
                o if is_retrieval(o) => {
                    let key = receiver_key(&op.record_variable_name);
                    for (k, arg, _) in &auto {
                        if *k == key {
                            self.tables.reads_of(table_id, arg, true, &mut reads);
                        }
                    }
                    let this = through_this(&op.record_variable_name);
                    let scopes = scopes_of
                        .entry((key.clone(), this))
                        .or_insert_with(|| self.sharing.scopes(routine, &key, this));
                    for scope in scopes.iter() {
                        for arg in self.shared_auto.get(scope).into_iter().flatten() {
                            self.tables.reads_of(table_id, arg, true, &mut reads);
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

    /// A `FieldRef` operation has no operation site, so the full snapshot's
    /// operation evidence (`derive_operation_evidence`) takes its control
    /// context and `asserterror` flag from its call site. Precondition, stated:
    /// each routine holds exactly one `FieldRef` operation, and its call site
    /// carries the context asserted (a conditional `if`, an `asserterror`).
    #[test]
    fn field_ref_operation_evidence_cites_its_call_site() {
        let src = r#"
table 50100 "H"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(4; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("L" where("No." = field("No.")));
        }
    }
}
table 50101 "L"
{
    fields { field(1; "No."; Code[20]) { } }
}
codeunit 50102 "C"
{
    procedure CondFr(B: Boolean)
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"H");
        FR := RR.Field(4);
        if B then
            FR.CalcField();
    end;

    procedure AsErrFr()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"H");
        FR := RR.Field(4);
        asserterror FR.CalcField();
    end;
}
"#;
        let m = crate::program::model::program_calls::assemble_and_resolve_inline_program_default(
            &[("src/C.al".to_string(), src.to_string())],
            "33333333-0000-0000-0000-0000000000b5",
        );
        let snap = crate::engine::l5::snapshot::compose_snapshot(&m);
        for (name, line, cond, asserterror) in [
            ("CondFr", 28, true, None),
            ("AsErrFr", 38, false, Some(true)),
        ] {
            let r = m
                .workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap();
            assert_eq!(r.field_ref_operations.len(), 1, "precondition: {name}");
            let op = &r.field_ref_operations[0];
            let site = super::super::workspace::field_ref_op_call_site(&op.id).unwrap();
            let cs = r.call_sites.iter().find(|c| c.id == site).unwrap();
            assert_eq!(
                cs.control_context.as_deref() != Some("top-level"),
                cond,
                "precondition: {name}'s call site context {:?}",
                cs.control_context
            );
            let ev = snap
                .operation_index
                .iter()
                .find(|e| e.operation_id == op.id)
                .unwrap_or_else(|| panic!("{name}: no evidence for {}", op.id));
            assert_eq!(ev.start_line + 1, line, "{name}");
            assert_eq!(ev.control_context, cs.control_context, "{name}");
            assert_eq!(ev.under_asserterror, asserterror, "{name}");
        }
    }

    #[test]
    fn field_arguments_name_their_field() {
        let cases = [
            ("\"Total Amount\"", FieldSel::Name("Total Amount".into())),
            ("X.\"Total Amount\"", FieldSel::Name("Total Amount".into())),
            ("Rec.\"No.\"", FieldSel::Name("No.".into())),
            ("\"No.\"", FieldSel::Name("No.".into())),
            ("Amount", FieldSel::Name("Amount".into())),
            ("4", FieldSel::Number(4)),
            ("'Line Count'", FieldSel::Name("Line Count".into())),
            (
                "H.FieldNo(\"Total Amount\")",
                FieldSel::Name("Total Amount".into()),
            ),
            ("FieldNo(Amount)", FieldSel::Name("Amount".into())),
            ("GetNo()", FieldSel::Unknown),
        ];
        for (arg, want) in cases {
            assert_eq!(field_sel(arg), want, "{arg}");
        }
        assert_eq!(
            member_call("RR.Field(4)"),
            Some(("RR", "field".to_string(), "4"))
        );
        assert_eq!(
            member_call("\"My Ref\".FieldIndex(F(1))"),
            Some(("\"My Ref\"", "fieldindex".to_string(), "F(1)"))
        );
        assert_eq!(member_call("Field(4)"), None);
    }

    fn table(id: &str, fields: Vec<(i64, &str, &str)>) -> ModelTable {
        ModelTable {
            id: id.to_string(),
            app_guid: "app".to_string(),
            table_number: 1,
            name: id.to_string(),
            fields: fields
                .into_iter()
                .map(|(n, name, class)| ModelField {
                    id: format!("{id}/{n}"),
                    physical_table_id: id.to_string(),
                    declaring_object_id: id.to_string(),
                    declaring_app_id: "app".to_string(),
                    field_number: n,
                    name: name.to_string(),
                    field_class: class.to_string(),
                    data_type: "Decimal".to_string(),
                    is_blob_like: false,
                    captions: Vec::new(),
                    calc_formula: Vec::new(),
                })
                .collect(),
            keys: Vec::new(),
            is_temporary: false,
            is_extension_stub: false,
        }
    }

    /// Hand-stated: a table WITH a FlowField and one without. A field that
    /// is not found is "not decided" only where a FlowField exists; a
    /// calculation on a record whose table is not loaded is "not decided".
    #[test]
    fn a_field_that_is_not_found_is_undecided_only_where_flowfields_exist() {
        let tables = [
            table("flow", vec![(1, "No.", "Normal"), (2, "F", "FlowField")]),
            table("plain", vec![(1, "No.", "Normal")]),
        ];
        let t = Tables::new(&tables);
        let mut out = Vec::new();
        t.reads_of("flow", "Missing", false, &mut out);
        assert_eq!(
            out.iter().map(|r| r.source.clone()).collect::<Vec<_>>(),
            vec![FlowFieldSource::Undecided(FIELD_NOT_FOUND)]
        );
        let mut out = Vec::new();
        t.reads_of("plain", "Missing", true, &mut out);
        assert!(out.is_empty(), "{out:?}");
        let mut out = Vec::new();
        t.reads_of("gone", "F", true, &mut out);
        assert_eq!(
            out.iter().map(|r| r.source.clone()).collect::<Vec<_>>(),
            vec![FlowFieldSource::Undecided(HOST_NOT_LOADED)]
        );
        let mut out = Vec::new();
        t.reads_of("gone", "F", false, &mut out);
        assert!(out.is_empty(), "a filter's retrieval is the read: {out:?}");
    }

    #[test]
    fn extensions_share_their_base_objects_group() {
        let obj = |id: &str, ty: &str, name: &str, ext: Option<&str>| ModelObject {
            id: id.to_string(),
            app_guid: "app".to_string(),
            object_type: ty.to_string(),
            object_number: 1,
            object_key: "1".to_string(),
            name: name.to_string(),
            source_table_name: None,
            extends_target_name: ext.map(str::to_string),
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            inherent_commit_behavior_conditional: false,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
            declaration_only_routines: Vec::new(),
            run_actions: None,
            enum_impls: None,
        };
        assert_eq!(
            object_groups(&obj("p", "Page", "Q Card", None)),
            (vec!["page:q card".to_string()], true)
        );
        assert_eq!(
            object_groups(&obj("e", "PageExtension", "Q Ext", Some("Q Card"))),
            (vec!["page:q card".to_string()], false)
        );
        assert_eq!(
            object_groups(&obj(
                "e",
                "PageExtension",
                "Q Ext",
                Some("Microsoft.Sales.\"Q Card\"")
            )),
            (
                vec![
                    "page:microsoft.sales.\"q card\"".to_string(),
                    "page:q card".to_string()
                ],
                false
            )
        );
        assert_eq!(
            object_groups(&obj("t", "TableExtension", "T Ext", Some("Cust."))),
            (vec!["table:cust.".to_string()], false)
        );
        assert_eq!(
            object_groups(&obj("c", "Codeunit", "C", None)),
            (vec!["id:c".to_string()], false)
        );
    }

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
