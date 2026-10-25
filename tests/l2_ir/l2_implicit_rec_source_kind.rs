//! #272 A2b: which bare `Rec`/`xRec` argument is the implicit record, and what a
//! declared `Rec` of any type does to it (spec: `## Design` revision 3, change 2,
//! with the O15 amendment, in the issue ledger).
//!
//! An argument is `implicit-rec` only when (a) the object exposes an implicit record
//! (Table, TableExtension, PageExtension, or a Page with a SourceTable) and (b) no
//! parameter, local or object global of ANY type is named `rec`/`xrec`. Otherwise
//! the chain is `parameter` -> `local` -> `unknown`. A declared `Rec` also removes
//! the synthetic `Rec` record row, so an object-global record `Rec` is promoted at
//! L3 and binds `global` with its own table.
//!
//! Fixtures: `tests/fixtures/issue-272/ws-shapes` and `ws-gshadow` (the latter's
//! global `Rec` is rejected by alc, AL0155: a robustness fixture).
//!
//! The L2 test pins `sourceKind` per site, because no finding moves when the local
//! shadow check is dropped: only the label catches that break.

use std::path::{Path, PathBuf};

use al_sem::program::body::l2_workspace::project_workspace;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/issue-272")
        .join(name)
}

/// One row per call-site argument whose text is a bare `Rec`/`xRec`:
/// `"<object>\t<routine>\t<callee text>\t<sourceKind>\t<sourceVariableName or ->"`,
/// sorted (a multiset).
fn rec_argument_kinds(ws: &Path) -> Vec<String> {
    let l2 = project_workspace(ws).expect("L2 projection");
    let mut out = Vec::new();
    for r in &l2.routines {
        let object_id = r.stable_routine_id.split('#').next().unwrap();
        let object = l2
            .objects
            .iter()
            .find(|o| o.stable_object_id == object_id)
            .unwrap_or_else(|| panic!("no object {object_id}"));
        for cs in &r.features.call_sites {
            for b in &cs.argument_bindings {
                let text = cs.argument_texts[b.parameter_index as usize]
                    .trim()
                    .to_ascii_lowercase();
                if text != "rec" && text != "xrec" {
                    continue;
                }
                out.push(format!(
                    "{}\t{}\t{}\t{}\t{}",
                    object.name,
                    r.name,
                    cs.callee_text,
                    b.source_kind,
                    b.source_variable_name.as_deref().unwrap_or("-")
                ));
            }
        }
    }
    out.sort();
    out
}

fn sorted(expected: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = expected.iter().map(|r| r.replace(" | ", "\t")).collect();
    v.sort();
    v
}

/// `ws-shapes`: every implicit `Rec`/`xRec` on a table, tableext, page with
/// SourceTable and pageext is `implicit-rec`. The shadows: a local scalar `Rec`
/// (`ShadowScalar`) is `unknown`; a declared local `xRec` (`ShadowDeclaredXRec`) is
/// `local`; a declared local record `Rec` leaves the undeclared `xRec`
/// (`ShadowRecFwdXRec`) `implicit-rec`. A codeunit global `Rec` (`ViaGlobalRec`;
/// a codeunit has no implicit record) is `unknown` at L2 (L3 promotes the global).
///
/// Today `ShadowScalar` and `ViaGlobalRec` are labelled `implicit-rec`.
#[test]
fn shapes_rec_argument_source_kinds() {
    assert_eq!(
        rec_argument_kinds(&fixture("ws-shapes")),
        sorted(&[
            // page S Card (SourceTable = "S Cust")
            "S Card | OnValidate | H.Dirty | implicit-rec | rec",
            "S Card | OnAction | H.ResetIt | implicit-rec | rec",
            "S Card | OnAction | H.Dirty | implicit-rec | rec",
            "S Card | OnAction | H.ReadsAddress | implicit-rec | rec",
            "S Card | PageProcDirty | H.Dirty | implicit-rec | rec",
            "S Card | PageProcViaRec | CurrPage.SetSelectionFilter | implicit-rec | rec",
            "S Card | PageProcViaRec | H.ResetIt | implicit-rec | rec",
            "S Card | ShadowScalar | H.Num | unknown | -",
            "S Card | ShadowRecFwdXRec | H.ResetIt | implicit-rec | xrec",
            "S Card | ShadowDeclaredXRec | H.ResetIt | local | xrec",
            "S Card | OnModifyRecord | H.Dirty | implicit-rec | rec",
            // pageextensions
            "S Card Ext | OnAction | H.ResetIt | implicit-rec | rec",
            "S Card Ext2 | OnAfterAction | H.Dirty | implicit-rec | rec",
            "S RO List Ext | OnAction | H.Dirty | implicit-rec | rec",
            // other pages
            "S RO List | OnAction | H.Dirty | implicit-rec | rec",
            "S AGR Card | OnAfterGetRecord | H.Dirty | implicit-rec | rec",
            "S NoMod Card | OnAction | H.Dirty | implicit-rec | rec",
            // codeunit with a global record Rec
            "S Global Rec | ViaGlobalRec | H.ResetIt | unknown | -",
            // table S Cust
            "S Cust | OnValidate | H.Dirty | implicit-rec | rec",
            "S Cust | OnValidate | H.ResetIt | implicit-rec | xrec",
            "S Cust | OnModify | H.Dirty | implicit-rec | rec",
            "S Cust | D41ViaRec | H.ResetIt | implicit-rec | rec",
            "S Cust | D41ViaBareRec | H.ResetIt | implicit-rec | rec",
            "S Cust | D39ViaRec | H.Dirty | implicit-rec | rec",
            "S Cust | D42ViaRec | H.ReadsAddress | implicit-rec | rec",
            "S Cust | D40ViaRec | H.NeedsLoaded | implicit-rec | rec",
            // tableextensions
            "S Cust Ext | ExtD41ViaRec | H.ResetIt | implicit-rec | rec",
            "S Cust Ext | OnAfterModify | H.Dirty | implicit-rec | rec",
            "S Cust Ext | OnBeforeModify | H.Dirty | implicit-rec | rec",
            "S Cust Ext2 | ExtReloadAfter | H.Dirty | implicit-rec | rec",
            "S Cust Ext2 | OnAfterRename | H.Dirty | implicit-rec | rec",
            "S Cust Ext2 | OnBeforeRename | H.Dirty | implicit-rec | rec",
            "S Cust Ext2 | OnAfterDelete | H.Dirty | implicit-rec | rec",
            "S Cust Ext2 | OnAfterInsert | H.Dirty | implicit-rec | rec",
            "S Cust Ext2 | ExtXRec | H.Dirty | implicit-rec | xrec",
        ])
    );
}

/// `ws-gshadow`: a table or page object global named `Rec`, scalar or record, is
/// not the implicit record: all four sites are `unknown` at L2.
#[test]
fn gshadow_rec_argument_source_kinds() {
    assert_eq!(
        rec_argument_kinds(&fixture("ws-gshadow")),
        sorted(&[
            "G TScalar | ViaScalar | H.Num | unknown | -",
            "G TRecord | ViaRecord | H.ResetIt | unknown | -",
            "G PScalar | ViaScalar | H.Num | unknown | -",
            "G PRecord | ViaRecord | H.ResetIt | unknown | -",
        ])
    );
}

/// `ws-gshadow`, after L3: the object-global record `Rec: Record "G Other"` on table
/// `G TRecord` (50304) and page `G PRecord` (50306, SourceTable `G Cust` 50300) is
/// promoted and binds `global` on table 50301, the global's own table, not the
/// object's (50304 / 50300). Breaking the global shadow check gives `local` on the
/// object's own table instead.
#[test]
fn gshadow_global_record_rec_binds_its_own_table() {
    let model = assemble_and_resolve_workspace_with_program_calls(&fixture("ws-gshadow"))
        .expect("model of ws-gshadow");
    let mut seen = Vec::new();
    for r in model
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "ViaRecord")
    {
        let cs = r
            .call_sites
            .iter()
            .find(|cs| cs.callee_text == "H.ResetIt")
            .unwrap_or_else(|| panic!("{}: no H.ResetIt call", r.object_id));
        let b = &cs.argument_bindings[0];
        assert_eq!(b.source_kind, "global", "{}: {b:#?}", r.object_id);
        let rv_id = b
            .source_record_variable_id
            .as_deref()
            .unwrap_or_else(|| panic!("{}: binding has no record variable", r.object_id));
        let rv = r
            .record_variables
            .iter()
            .find(|rv| rv.id == rv_id)
            .unwrap_or_else(|| panic!("{}: no record variable {rv_id}", r.object_id));
        assert_eq!(
            rv.table_id.as_deref(),
            Some("11111111-0000-0000-0000-000000272003/table/50301"),
            "{}: {rv:#?}",
            r.object_id
        );
        seen.push(r.object_type.clone());
    }
    seen.sort();
    assert_eq!(
        seen,
        ["Page", "Table"],
        "precondition: both ViaRecord routines"
    );
}
