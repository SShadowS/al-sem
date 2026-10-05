//! Port of al-sem `src/index/capability/table.ts` + `commit.ts`.
//!
//! - `extract_table` — iterate `recordOperations`, map each `RecordOpType` to a
//!   `CapabilityOp` (state-only/filter ops are skipped), emit one fact carrying a
//!   `TableExtra` (opSubtype + recordVariableId + tempState). `resourceId`
//!   (tableId) is L3 — STRUCTURALLY ABSENT here; confidence is "unresolved" at L2
//!   (the TS uses "static" only when tableId is resolved, which never happens
//!   pre-resolve).
//! - `extract_commit` — one fact per `operationSites` entry of kind "commit".

use super::super::features::PRoutine;
use super::{CapabilityExtra, CapabilityFact, CoverageReason, ExtractionContext, TableExtra};

/// Map an AL `RecordOpType` (string) to a `CapabilityOp`, or `None` for
/// state-only / filter ops (SetRange, SetFilter, Init, SetLoadFields, ...).
/// Mirrors `table.ts` `mapOp`.
fn map_op(op: &str) -> Option<&'static str> {
    match op {
        "Get" | "Find" | "FindFirst" | "FindLast" | "FindSet" | "IsEmpty" | "Count"
        | "CountApprox" | "Next" | "CalcFields" | "CalcSums" | "TestField" => Some("read"),
        "Modify" | "ModifyAll" | "Validate" | "Copy" | "TransferFields" => Some("modify"),
        "Insert" => Some("insert"),
        "Delete" | "DeleteAll" => Some("delete"),
        // Init / SetRange / SetFilter / SetLoadFields / AddLoadFields /
        // SetCurrentKey / Reset / LockTable → not capability-relevant.
        // Rename IS a write, but which TableData permission it needs is not
        // measured yet (#9: Permissions Mock did not enforce in the test
        // container), so it is deliberately unmapped here and in
        // `capability_cone::map_table_op`, and so absent from the cone's writes.
        _ => None,
    }
}

pub fn extract_table(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for op in &ctx.features.record_operations {
        let Some(cap_op) = map_op(&op.op) else {
            continue;
        };

        let extra = TableExtra {
            kind: "table",
            record_variable_id: op.record_variable_id.clone(),
            temp_state: Some(op.temp_state.clone()),
            op_subtype: Some(op.op.clone()),
        };

        facts.push(CapabilityFact {
            op: cap_op.to_string(),
            resource_kind: "table".to_string(),
            // tableId is L3-resolved (absent at L2) → confidence is "unresolved".
            confidence: "unresolved".to_string(),
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: None,
            witness_operation_id: Some(op.id.clone()),
            witness_callsite_id: None,
            extra: Some(CapabilityExtra::Table(extra)),
        });
    }

    (facts, vec![])
}

pub fn extract_commit(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for op in &ctx.features.operation_sites {
        if op.kind == "commit" {
            facts.push(CapabilityFact {
                op: "commit".to_string(),
                resource_kind: "transaction".to_string(),
                confidence: "static".to_string(),
                provenance: "direct".to_string(),
                via: "self".to_string(),
                resource_arg_source: None,
                witness_operation_id: Some(op.id.clone()),
                witness_callsite_id: None,
                extra: None,
            });
        }
    }

    (facts, vec![])
}
