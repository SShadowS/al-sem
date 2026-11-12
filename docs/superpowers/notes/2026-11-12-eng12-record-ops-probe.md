# ENG-12 probe: record operations the engine does not see (L35, L36, ENG-06 gap (b))

Master `bdb7aab5`, debug build, 2026-11-12. One workspace (app `e12e12e0-…aa`):

```al
table 52600 "E12 Row"
{
    fields { field(1; K; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; K) { } }
    trigger OnModify()
    begin
        Commit();
    end;
    procedure BareSysId(Id: Guid)
    begin
        if GetBySystemId(Id) then;
    end;
}
codeunit 52601 "E12 Ops"
{
    procedure SysIdMember(Id: Guid)            // var R: Record "E12 Row"
    begin if R.GetBySystemId(Id) then; end;
    procedure RecRefSysId(Id: Guid)            // var RR: RecordRef
    begin RR.Open(Database::"E12 Row"); if RR.GetBySystemId(Id) then; end;
    procedure ArrMember()                      // var Recs: array[2] of Record "E12 Row"
    begin if Recs[1].FindFirst() then; end;
    procedure ArrWith()
    begin with Recs[1] do if FindFirst() then; end;
    procedure ArrModifyTrue()
    begin Recs[1].Modify(true); end;
    procedure RRArrModifyTrue()                // var RRs: array[2] of RecordRef
    begin RRs[1].Open(Database::"E12 Row"); RRs[1].Modify(true); end;
    procedure GetRefModifyTrue()
    begin GetRef().Modify(true); end;
    local procedure GetRef(): Record "E12 Row"
    var R: Record "E12 Row";
    begin exit(R); end;
}
```

(Full source: the session scratchpad `eng12-probe.al`; the fixtures in the ENG-12 tests repeat it.)

| Routine | Question | Answer on master | Right answer |
|---|---|---|---|
| "E12 Row".BareSysId | `query touches --table "E12 Row" --direction down` | no (every candidate decided) | yes (reads the table) |
| SysIdMember | same | no (every candidate decided) | yes |
| RecRefSysId | same | no (every candidate decided) | yes |
| ArrMember | same | no (every candidate decided) | yes |
| ArrWith | same | no (every candidate decided) | yes |
| ArrModifyTrue | `prove … may-commit` | no (every candidate decided) | yes (OnModify commits) |
| RRArrModifyTrue | same | no (every candidate decided) | yes |
| GetRefModifyTrue | same | no (every candidate decided) | yes |

All eight are wrong `no` answers: the operation produces no record operation (L35, L36), or a
trigger-firing operation on a receiver that is not a plain variable has no trigger edge and no
gap (ENG-06 review gap (b)). Not a platform question: the AL semantics are those of a plain
receiver (`GetBySystemId` reads the record like `Get`; an array element is a record; a call's
returned record is a record), so no BC probe was run.
