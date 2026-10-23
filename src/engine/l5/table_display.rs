//! `describeTable` — port of al-sem `src/detectors/table-display.ts`.
//!
//! Render a record-operation's target table for human consumption. The tiered
//! renderer a detector's `rootCause` byte-depends on; reproduce the EXACT tier
//! order + suffix strings.
//!
//! Four tiers, in order of preference (mirrors al-sem's doc-comment):
//!  1. `op.table_id` resolves in `table_by_id` → return the table's NAME (e.g.
//!     `"Customer"`). This is what the user sees in their IDE.
//!  2. `op.table_id` is None/unresolved AND the receiving record-variable has a
//!     declared `table_name` text → return that text, suffixed with
//!     ` (type not loaded)`. The common case when the type lives in a dependency
//!     we couldn't load.
//!  3. Variable name itself, prefixed with `var `, as a last-resort identity
//!     (e.g. `var DocGroup`).
//!  4. The string `"unknown table"`.

use std::collections::HashMap;

use crate::program::model::workspace::{ModelRoutine, ModelTable};

/// The op fields `describe_table` reads — al-sem's `{ tableId?,
/// recordVariableName }`. A struct-of-references so callers can pass a slice of
/// the real `ModelRecordOperation` without cloning.
pub struct DescribeOp<'a> {
    /// Resolved internal TableId, or `None` when unresolved.
    pub table_id: Option<&'a str>,
    /// The receiving record variable's name (may be empty).
    pub record_variable_name: &'a str,
}

/// Render a record-operation's target table. Mirrors al-sem `describeTable`.
///
/// `table_by_id` is keyed by internal TableId (matching `ModelTable::id`).
/// `routine` is the op's owning routine, or `None` when unavailable (tier 2 is
/// skipped). The variable-name comparison is case-insensitive (`toLowerCase`),
/// exactly as al-sem.
pub fn describe_table(
    op: &DescribeOp,
    routine: Option<&ModelRoutine>,
    table_by_id: &HashMap<&str, &ModelTable>,
) -> String {
    // Tier 1: resolved tableId → the table's NAME.
    if let Some(tid) = op.table_id
        && let Some(table) = table_by_id.get(tid)
    {
        return table.name.clone();
    }
    // Tier 2: declared record-variable type text → `<type> (type not loaded)`.
    if let Some(routine) = routine {
        let lc = op.record_variable_name.to_lowercase();
        if let Some(rv) = routine
            .record_variables
            .iter()
            .find(|v| v.name.to_lowercase() == lc)
            && let Some(table_name) = &rv.table_name
            && !table_name.is_empty()
        {
            return format!("{table_name} (type not loaded)");
        }
    }
    // Tier 3: `var <name>`.
    if !op.record_variable_name.is_empty() {
        return format!("var {}", op.record_variable_name);
    }
    // Tier 4.
    "unknown table".to_string()
}

// ===========================================================================
// Native oracles — the 4 tiers on synthetic input.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::model::workspace::{ModelRecordVariable, ModelRoutine, ModelTable};

    fn table(id: &str, name: &str) -> ModelTable {
        ModelTable {
            id: id.to_string(),
            app_guid: "g".to_string(),
            table_number: 1,
            name: name.to_string(),
            fields: vec![],
            keys: vec![],
            is_temporary: false,
            is_extension_stub: false,
        }
    }

    fn rec_var(name: &str, table_name: Option<&str>) -> ModelRecordVariable {
        ModelRecordVariable {
            id: format!("rv/{name}"),
            name: name.to_string(),
            table_name: table_name.map(str::to_string),
            table_id: None,
            is_parameter: false,
            parameter_index: None,
            temp_state: crate::program::body::features::PTempState {
                kind: "unknown".to_string(),
                value: None,
                parameter_index: None,
            },
            scope: None,
        }
    }

    fn routine_with(vars: Vec<ModelRecordVariable>) -> ModelRoutine {
        // Minimal routine with only record_variables populated — all other fields
        // are defaulted via the test-support builder so describe_table compiles.
        let mut r = crate::engine::l5::test_support::routine("r0/test", "Procedure");
        r.record_variables = vars;
        r
    }

    #[test]
    fn tier1_resolved_table_id_returns_name() {
        let t = table("g/table/18", "Customer");
        let mut by_id: HashMap<&str, &ModelTable> = HashMap::new();
        by_id.insert(t.id.as_str(), &t);
        let routine = routine_with(vec![]);
        let op = DescribeOp {
            table_id: Some("g/table/18"),
            record_variable_name: "Cust",
        };
        assert_eq!(describe_table(&op, Some(&routine), &by_id), "Customer");
    }

    #[test]
    fn tier2_declared_type_with_not_loaded_suffix() {
        let by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let routine = routine_with(vec![rec_var("DocGroup", Some("CDC Document Group"))]);
        // tableId unresolved, but the var has a declared type.
        let op = DescribeOp {
            table_id: None,
            record_variable_name: "DocGroup",
        };
        assert_eq!(
            describe_table(&op, Some(&routine), &by_id),
            "CDC Document Group (type not loaded)"
        );
        // Case-insensitive match on the variable name.
        let op_lc = DescribeOp {
            table_id: None,
            record_variable_name: "docgroup",
        };
        assert_eq!(
            describe_table(&op_lc, Some(&routine), &by_id),
            "CDC Document Group (type not loaded)"
        );
    }

    #[test]
    fn tier2_skipped_when_table_id_resolves() {
        // Even if the var has a declared type, a resolved tableId wins (tier 1).
        let t = table("g/table/18", "Customer");
        let mut by_id: HashMap<&str, &ModelTable> = HashMap::new();
        by_id.insert(t.id.as_str(), &t);
        let routine = routine_with(vec![rec_var("Cust", Some("SomethingElse"))]);
        let op = DescribeOp {
            table_id: Some("g/table/18"),
            record_variable_name: "Cust",
        };
        assert_eq!(describe_table(&op, Some(&routine), &by_id), "Customer");
    }

    #[test]
    fn tier3_var_name_when_no_resolved_or_declared_type() {
        let by_id: HashMap<&str, &ModelTable> = HashMap::new();
        // var has NO declared type → fall through to `var <name>`.
        let routine = routine_with(vec![rec_var("DocGroup", None)]);
        let op = DescribeOp {
            table_id: None,
            record_variable_name: "DocGroup",
        };
        assert_eq!(describe_table(&op, Some(&routine), &by_id), "var DocGroup");
    }

    #[test]
    fn tier3_var_name_when_no_routine() {
        let by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let op = DescribeOp {
            table_id: None,
            record_variable_name: "DocGroup",
        };
        assert_eq!(describe_table(&op, None, &by_id), "var DocGroup");
    }

    #[test]
    fn tier4_unknown_table_when_no_name() {
        let by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let op = DescribeOp {
            table_id: None,
            record_variable_name: "",
        };
        assert_eq!(describe_table(&op, None, &by_id), "unknown table");
    }

    #[test]
    fn tier2_empty_declared_type_falls_to_tier3() {
        // Declared type present but empty string → al-sem's `rv.tableName !== ""`
        // guard skips tier 2, falling to `var <name>`.
        let by_id: HashMap<&str, &ModelTable> = HashMap::new();
        let routine = routine_with(vec![rec_var("DocGroup", Some(""))]);
        let op = DescribeOp {
            table_id: None,
            record_variable_name: "DocGroup",
        };
        assert_eq!(describe_table(&op, Some(&routine), &by_id), "var DocGroup");
    }
}
