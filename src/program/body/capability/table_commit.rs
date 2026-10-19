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
/// The ONE table op map: L2 (`extract_table`) and the L4 cone's direct table
/// facts (`capability_cone`) both call it, so no second table can drift (#100).
///
/// `Validate`, `Copy` and `TransferFields` are NOT writes (#69 D5): `Copy`
/// copies fields/filters/keys in memory, `TransferFields` assigns field values,
/// `Validate` assigns and runs `OnValidate` (whose own writes are separate
/// effects; its relation check reads the RELATED table). None of them modifies
/// the record's own table.
pub(crate) fn map_op(op: &str) -> Option<&'static str> {
    match op {
        "Get" | "Find" | "FindFirst" | "FindLast" | "FindSet" | "IsEmpty" | "Count"
        | "CountApprox" | "Next" | "CalcFields" | "CalcSums" => Some("read"),
        "Modify" | "ModifyAll" => Some("modify"),
        "Insert" => Some("insert"),
        "Delete" | "DeleteAll" => Some("delete"),
        // Rename writes the table and LockTable locks it, so both are table
        // facts with their own ops: d1/d2 prune callees with no table fact, and
        // both ops are their terminals (`is_db_touching_class`). Rename still
        // maps to NO TableData permission letter: which right it needs is not
        // measured (#9: Permissions Mock did not enforce in the test
        // container). Permission, digest and diff-gain consumers skip ops they
        // do not know, so `rename`/`lock` surface only as facts (#100).
        "Rename" => Some("rename"),
        "LockTable" => Some("lock"),
        // Init / SetRange / SetFilter / SetLoadFields / AddLoadFields /
        // SetCurrentKey / Reset / Validate / Copy / TransferFields / TestField
        // → not capability-relevant.
        // TestField checks the value the record variable already holds in
        // memory and issues no query (`op_classification.rs` classifies it
        // `StateOnly`), so it is not a table read (#100). Residual: under
        // partial records, touching a field that was not loaded makes the
        // platform re-read the row (JIT loading). That holds for ANY field
        // access, and no field access is modelled as a read.
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
