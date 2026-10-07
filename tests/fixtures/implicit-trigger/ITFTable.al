// 1B.3b Task 1: synthetic ImplicitTrigger target-set fixture.
//
// Table 50500 "ITFTable" declares OnInsert/OnModify/OnDelete triggers.
// TableExtension 50501 "ITFTableExt" ALSO declares OnInsert (fan-out target —
// every insert into ITFTable must fire BOTH the base table's and the
// extension's OnInsert). Codeunit 50502 "ITFCaller" performs one Insert,
// Modify, and Delete on a local `Record ITFTable` variable, each with
// RunTrigger = true: without it no table trigger runs (measured on BC 28,
// engine-switch S9.0c).
//
// Expected fresh ImplicitTrigger resolution (frozen in
// `tests/goldens/semantic-edges/implicit-trigger-fixture.json` — L3-independent,
// no oracle involved):
//   • MyRec.Insert(true) -> {Table ITFTable.OnInsert, TableExtension ITFTableExt.OnInsert}
//   • MyRec.Modify(true) -> {Table ITFTable.OnModify}
//   • MyRec.Delete(true) -> {Table ITFTable.OnDelete}
table 50500 "ITFTable"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }

    trigger OnInsert()
    begin
    end;

    trigger OnModify()
    begin
    end;

    trigger OnDelete()
    begin
    end;
}

tableextension 50501 "ITFTableExt" extends "ITFTable"
{
    trigger OnInsert()
    begin
    end;
}

codeunit 50502 "ITFCaller"
{
    procedure DoStuff()
    var
        MyRec: Record "ITFTable";
    begin
        MyRec.Insert(true);
        MyRec.Modify(true);
        MyRec.Delete(true);
    end;
}
