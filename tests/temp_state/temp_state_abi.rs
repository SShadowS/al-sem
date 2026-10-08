//! Task 6 (temp-state-tracking, G7 / RV-4): ABI (dependency) side reads the temp
//! markers — `TypeDefinition.Temporary` on record params, `TableType = Temporary`
//! on tables — and the ABI→L3 projection synthesizes per-param `record_variables`
//! with the SAME `Known(true)/ParameterDependent(i)/Known(false)` temp shapes the
//! native source path produces (the native+ABI shape-parity rule).
//!
//! (a)/(b) assert at the `parse_symbol_reference` (AbiTable/AbiParameter) level.
//! (c) asserts on the production cross-app model of a workspace that depends on
//! the synthetic `.app`: each dependency record param exposes a record var whose
//! `temp_state` matches the native rule, and a param typed on a
//! `TableType=Temporary` ABI table resolves to `Known(true)` via the table-level
//! override that `resolve()` runs.

use al_sem::engine::deps::symbol_reference::parse_symbol_reference;

/// A SymbolReference.json with:
///   - Table 50000 "Temp Buffer" carrying property {"Name":"TableType","Value":"Temporary"}
///   - Table 50001 "Normal Table" with no TableType property
///   - Codeunit 50100 with procedures whose record params carry various temp markers
const SYMBOL_REFERENCE: &str = r#"{
  "AppId": "11111111-2222-3333-4444-555555555555",
  "Name": "TempDep",
  "Publisher": "P",
  "Version": "1.0.0.0",
  "Tables": [
    {
      "Id": 50000,
      "Name": "Temp Buffer",
      "Properties": [ { "Name": "TableType", "Value": "Temporary" } ],
      "Fields": [ { "Id": 1, "Name": "Entry No.", "TypeDefinition": { "Name": "Integer" } } ]
    },
    {
      "Id": 50001,
      "Name": "Normal Table",
      "Fields": [ { "Id": 1, "Name": "Entry No.", "TypeDefinition": { "Name": "Integer" } } ]
    }
  ],
  "Codeunits": [
    {
      "Id": 50100,
      "Name": "Dep Proc",
      "Methods": [
        {
          "Name": "TempMarkedParam",
          "Parameters": [
            { "Name": "Rec", "IsVar": true, "TypeDefinition": { "Name": "Record \"Normal Table\"", "Subtype": { "Name": "Normal Table" }, "Temporary": true } }
          ]
        },
        {
          "Name": "ByVarUnmarked",
          "Parameters": [
            { "Name": "Rec", "IsVar": true, "TypeDefinition": { "Name": "Record \"Normal Table\"" } }
          ]
        },
        {
          "Name": "ByValueUnmarked",
          "Parameters": [
            { "Name": "Rec", "IsVar": false, "TypeDefinition": { "Name": "Record \"Normal Table\"" } }
          ]
        },
        {
          "Name": "TempTableParam",
          "Parameters": [
            { "Name": "Rec", "IsVar": true, "TypeDefinition": { "Name": "Record \"Temp Buffer\"" } }
          ]
        }
      ]
    }
  ]
}"#;

// --- (a) table-level TableType=Temporary -----------------------------------

#[test]
fn abi_table_reads_tabletype_temporary() {
    let abi = parse_symbol_reference(SYMBOL_REFERENCE);
    let temp = abi
        .tables
        .iter()
        .find(|t| t.name == "Temp Buffer")
        .expect("Temp Buffer table");
    assert!(
        temp.is_temporary,
        "Table with TableType=Temporary property → AbiTable.is_temporary == true"
    );
    let normal = abi
        .tables
        .iter()
        .find(|t| t.name == "Normal Table")
        .expect("Normal Table");
    assert!(
        !normal.is_temporary,
        "Table without TableType property → AbiTable.is_temporary == false"
    );
}

// --- (b) param TypeDefinition.Temporary ------------------------------------

#[test]
fn abi_param_reads_typedefinition_temporary() {
    let abi = parse_symbol_reference(SYMBOL_REFERENCE);
    let cu = abi
        .objects
        .iter()
        .find(|o| o.name == "Dep Proc")
        .expect("Dep Proc codeunit");

    let marked = cu
        .routines
        .iter()
        .find(|r| r.name == "TempMarkedParam")
        .expect("TempMarkedParam");
    assert!(
        marked.parameters[0].is_temporary,
        "param with TypeDefinition.Temporary=true → AbiParameter.is_temporary == true"
    );

    let unmarked = cu
        .routines
        .iter()
        .find(|r| r.name == "ByVarUnmarked")
        .expect("ByVarUnmarked");
    assert!(
        !unmarked.parameters[0].is_temporary,
        "param without TypeDefinition.Temporary → AbiParameter.is_temporary == false"
    );
}

// --- (c) ABI→L3 projection per-param record-var temp shapes -----------------

/// Build the production cross-app model of a workspace that depends on the
/// synthetic symbol-only `.app`, and return its rows (the dependency's ABI rows
/// after `resolve()` over the merged whole, so the table-level override has run).
/// The workspace calls every dependency procedure, so each is in the model.
fn project_and_resolve() -> al_sem::program::model::workspace::ModelEntities {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    let dep = "11111111-2222-3333-4444-555555555555";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        format!(
            r#"{{"id":"aaaaaaaa-0000-0000-0000-000000000001","name":"TempWs","publisher":"P","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{dep}","name":"TempDep","publisher":"P","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Main.al"),
        "codeunit 50200 \"Ws Main\"\n{\n    procedure Go()\n    var\n        D: Codeunit \"Dep Proc\";\n        N: Record \"Normal Table\";\n        T: Record \"Temp Buffer\";\n    begin\n        D.TempMarkedParam(N);\n        D.ByVarUnmarked(N);\n        D.ByValueUnmarked(N);\n        D.TempTableParam(T);\n    end;\n}\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join(".alpackages")).unwrap();
    crate::symbol_app::write_symbol_app(
        &dir.path().join(".alpackages/P_TempDep_1.0.0.0.app"),
        dep,
        "TempDep",
        "1.0.0.0",
        SYMBOL_REFERENCE,
    );
    assemble_and_resolve_cross_app_program(dir.path(), MI, false)
        .expect("cross-app model")
        .resolved
        .workspace
}

fn temp_kind(
    ws: &al_sem::program::model::workspace::ModelEntities,
    routine_name: &str,
) -> (String, Option<bool>, Option<u32>) {
    let r = ws
        .routines
        .iter()
        .find(|r| r.name == routine_name)
        .unwrap_or_else(|| panic!("routine {routine_name}"));
    let rv = r
        .record_variables
        .iter()
        .find(|v| v.is_parameter)
        .unwrap_or_else(|| panic!("record var param on {routine_name}"));
    (
        rv.temp_state.kind.clone(),
        rv.temp_state.value,
        rv.temp_state.parameter_index,
    )
}

#[test]
fn abi_temp_marked_param_projects_known_true() {
    let ws = project_and_resolve();
    let (kind, value, _) = temp_kind(&ws, "TempMarkedParam");
    assert_eq!(kind, "known");
    assert_eq!(
        value,
        Some(true),
        "Temporary:true record param → Known(true)"
    );

    // "Both markers active" path: Temporary:true AND a resolvable table_name (the
    // type text `Record "Normal Table"` a real ABI param carries). The synthesized
    // var must keep its table_name (so the table-level override could also apply),
    // not drop it — covers the realistic shape, not the degenerate `Record`-only one.
    let rv = ws
        .routines
        .iter()
        .find(|r| r.name == "TempMarkedParam")
        .unwrap()
        .record_variables
        .iter()
        .find(|v| v.is_parameter)
        .unwrap();
    assert_eq!(
        rv.table_name.as_deref(),
        Some("Normal Table"),
        "record_table_name_of resolves the param's table from the full type text"
    );
}

#[test]
fn abi_by_var_unmarked_param_projects_parameter_dependent() {
    let ws = project_and_resolve();
    let (kind, _, idx) = temp_kind(&ws, "ByVarUnmarked");
    assert_eq!(
        kind, "parameter-dependent",
        "by-var unmarked record param → ParameterDependent(index)"
    );
    assert_eq!(idx, Some(0), "the param's positional index");
}

#[test]
fn abi_by_value_unmarked_param_projects_known_false() {
    let ws = project_and_resolve();
    let (kind, value, _) = temp_kind(&ws, "ByValueUnmarked");
    assert_eq!(kind, "known");
    assert_eq!(value, Some(false), "by-value record param → Known(false)");
}

#[test]
fn abi_param_typed_on_temp_table_projects_known_true() {
    let ws = project_and_resolve();
    let (kind, value, _) = temp_kind(&ws, "TempTableParam");
    assert_eq!(kind, "known");
    assert_eq!(
        value,
        Some(true),
        "param typed on TableType=Temporary ABI table → Known(true) (table-level override)"
    );
}
