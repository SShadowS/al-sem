//! r3 E1 (spec `2026-11-08-output-validation-r3.md` §1): Integer, Option and
//! Enum literal values in the literal reach (n10).
//!
//! Continia DO's `DocSave` passes `OutputAction::SaveAsPDF` (a member of a
//! GLOBAL `Option` variable with blank members) into `RunReport`'s Integer
//! parameter, which `RunReportID` reads as `case OutputActionInt of
//! OutputAction::Print: … OutputAction::SaveAsPDF: …`. `EH Handler` is that
//! shape: the arms write different tables, so `prove … writes-table:<T>`
//! says which arms a root can run.
//!
//! Each test states its precondition in `SOURCE` (literal arguments, the
//! parameter's type and uses) and asks `prove`; the controls keep every
//! route that can still run a `yes`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50240 "EH Print" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50241 "EH Save" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50242 "EH Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50243 "EH Not Print" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50244 "EH In List" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50245 "EH Exit Guarded" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50246 "EH Range" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50247 "EH Range First" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50248 "EH Range Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50249 "EH Alpha" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50250 "EH Beta" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50251 "EH Kind Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50252 "EH Other Enum" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50253 "EH Other Beta" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50254 "EH Opt A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50255 "EH Opt B" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50256 "EH Ctl Var" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50257 "EH Ctl Assigned" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50258 "EH Ctl Passed" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50259 "EH Ctl Loop" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50260 "EH Ctl Unknown" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
enum 50261 "EH Kind"
{
    Extensible = true;
    value(0; Alpha) { }
    value(1; Beta) { }
}
enumextension 50262 "EH Kind Ext" extends "EH Kind"
{
    value(50; Gamma) { }
}
enum 50263 "EH Other"
{
    value(0; Alpha) { }
    value(1; Beta) { }
}
codeunit 50264 "EH Handler"
{
    var
        OutputAction: Option Print,EMailOpen,,,SaveAsPDF;

    // ---- an Integer parameter against a global Option's labels (n10) ----
    procedure DocSave()
    begin
        RunReport(OutputAction::SaveAsPDF);
    end;

    procedure DocPrint()
    begin
        RunReport(OutputAction::Print);
    end;

    procedure DocEMail()
    begin
        RunReport(OutputAction::EMailOpen);
    end;

    // 4 is SaveAsPDF's ordinal only when the two blank members count.
    procedure DocFour()
    begin
        RunReport(4);
    end;

    procedure DocEither(A: Integer)
    begin
        RunReport(A);
    end;

    // Two routes into RunReport: a literal one and an unknown one.
    procedure DocJoin(A: Integer)
    begin
        RunReport(OutputAction::SaveAsPDF);
        RunReport(A);
    end;

    procedure DocComputed()
    begin
        RunReport(ComputeAction());
    end;

    local procedure ComputeAction(): Integer
    begin
        exit(OutputAction::Print);
    end;

    local procedure RunReport(OutputActionInt: Integer)
    begin
        RunReportID(OutputActionInt);
    end;

    local procedure RunReportID(OutputActionInt: Integer)
    begin
        case OutputActionInt of
            OutputAction::Print:
                WriteT(1);
            OutputAction::SaveAsPDF:
                WriteT(2);
            else
                WriteT(3);
        end;
        if OutputActionInt <> OutputAction::Print then
            WriteT(4);
        if OutputActionInt in [OutputAction::Print, OutputAction::EMailOpen] then
            WriteT(5);
        RunExitGuard(OutputActionInt);
    end;

    local procedure RunExitGuard(P: Integer)
    begin
        if P <> OutputAction::Print then
            exit;
        WriteT(6);
    end;

    // ---- an unresolved label keeps its arm and the else live ----
    procedure RangePrint()
    begin
        RunRange(OutputAction::Print);
    end;

    procedure RangeSave()
    begin
        RunRange(OutputAction::SaveAsPDF);
    end;

    local procedure RunRange(P: Integer)
    begin
        case P of
            OutputAction::Print:
                WriteT(8);
            10 .. 12:
                WriteT(7);
            else
                WriteT(9);
        end;
    end;

    // ---- an Enum parameter ----
    procedure KindAlpha()
    begin
        RunKind(Enum::"EH Kind"::Alpha);
    end;

    // A known extension member passed explicitly is one value.
    procedure KindGamma()
    begin
        RunKind(Enum::"EH Kind"::Gamma);
    end;

    // An unknown value of an extensible enum is never narrowed to the
    // members loaded today.
    procedure KindUnknown(K: Enum "EH Kind")
    begin
        RunKind(K);
    end;

    local procedure RunKind(Kind: Enum "EH Kind")
    begin
        case Kind of
            Kind::Alpha:
                WriteT(10);
            Kind::Beta:
                WriteT(11);
            else
                WriteT(12);
        end;
    end;

    // A label of ANOTHER enum (alc would reject the comparison): a
    // conversion not modeled, so its arm stays live, though its value name
    // differs from the bound one.
    procedure KindOther()
    begin
        RunKindOther(Enum::"EH Kind"::Alpha);
    end;

    local procedure RunKindOther(Kind: Enum "EH Kind")
    begin
        case Kind of
            Enum::"EH Other"::Beta:
                WriteT(13);
            Kind::Beta:
                WriteT(14);
        end;
    end;

    // ---- an Option parameter, called with a local Option's label ----
    procedure OptCallB()
    var
        M: Option A,B,C;
    begin
        RunOpt(M::B);
    end;

    local procedure RunOpt(Mode: Option A,B,C)
    begin
        if Mode = Mode::A then
            WriteT(15);
        if Mode = Mode::B then
            WriteT(16);
    end;

    // ---- controls: parameters whose value is not the caller's argument ----
    procedure CtlVar()
    var
        X: Integer;
    begin
        X := 2;
        RunVar(X);
    end;

    local procedure RunVar(var P: Integer)
    begin
        if P = 1 then
            WriteT(17);
    end;

    procedure CtlAssigned()
    begin
        RunAssigned(2);
    end;

    local procedure RunAssigned(P: Integer)
    begin
        P += 1;
        if P = 3 then
            WriteT(18);
    end;

    procedure CtlPassed()
    begin
        RunPassed(2);
    end;

    local procedure RunPassed(P: Integer)
    begin
        Bump(P);
        if P = 1 then
            WriteT(19);
    end;

    local procedure Bump(var X: Integer)
    begin
        X := 1;
    end;

    procedure CtlLoop()
    begin
        RunLoop(5);
    end;

    local procedure RunLoop(P: Integer)
    begin
        for P := 1 to 2 do
            WriteNothing();
        if P = 1 then
            WriteT(20);
    end;

    procedure CtlUnknown()
    begin
        RunUnknownCallee(2);
    end;

    local procedure RunUnknownCallee(P: Integer)
    begin
        NoSuchProcedure(P);
        if P = 1 then
            WriteT(21);
    end;

    local procedure WriteNothing()
    begin
    end;

    local procedure WriteT(Which: Integer)
    var
        TPrint: Record "EH Print";
        TSave: Record "EH Save";
        TElse: Record "EH Else";
        TNotPrint: Record "EH Not Print";
        TInList: Record "EH In List";
        TExit: Record "EH Exit Guarded";
        TRange: Record "EH Range";
        TRangeFirst: Record "EH Range First";
        TRangeElse: Record "EH Range Else";
        TAlpha: Record "EH Alpha";
        TBeta: Record "EH Beta";
        TKindElse: Record "EH Kind Else";
        TOther: Record "EH Other Enum";
        TOtherBeta: Record "EH Other Beta";
        TOptA: Record "EH Opt A";
        TOptB: Record "EH Opt B";
        TVar: Record "EH Ctl Var";
        TAssigned: Record "EH Ctl Assigned";
        TPassed: Record "EH Ctl Passed";
        TLoop: Record "EH Ctl Loop";
        TUnknown: Record "EH Ctl Unknown";
    begin
        case Which of
            1:
                TPrint.Insert();
            2:
                TSave.Insert();
            3:
                TElse.Insert();
            4:
                TNotPrint.Insert();
            5:
                TInList.Insert();
            6:
                TExit.Insert();
            7:
                TRange.Insert();
            8:
                TRangeFirst.Insert();
            9:
                TRangeElse.Insert();
            10:
                TAlpha.Insert();
            11:
                TBeta.Insert();
            12:
                TKindElse.Insert();
            13:
                TOther.Insert();
            14:
                TOtherBeta.Insert();
            15:
                TOptA.Insert();
            16:
                TOptB.Insert();
            17:
                TVar.Insert();
            18:
                TAssigned.Insert();
            19:
                TPassed.Insert();
            20:
                TLoop.Insert();
            21:
                TUnknown.Insert();
        end;
    end;
}
table 50266 "EH Verb Error" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50267 "EH Verb Warn" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50268 "EH Verb Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50269 "EH Scope Pub" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50270 "EH Scope All" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
// ENG-30: built-in option types (`Verbosity`, `TelemetryScope`) as value
// domains, compared by member name.
codeunit 50271 "EH Platform"
{
    procedure VerbError()
    begin
        Emit(Verbosity::Error);
    end;

    procedure VerbVerbose()
    begin
        Emit(Verbosity::Verbose);
    end;

    procedure VerbUnknown(V: Verbosity)
    begin
        Emit(V);
    end;

    local procedure Emit(Level: Verbosity)
    var
        E: Record "EH Verb Error";
        W: Record "EH Verb Warn";
        O: Record "EH Verb Else";
    begin
        case Level of
            Verbosity::Error:
                E.Insert();
            Verbosity::Warning:
                W.Insert();
            else
                O.Insert();
        end;
    end;

    // A parameter named like its type: `Verbosity::Warning` then names the
    // parameter's own member.
    procedure NamedWarn()
    begin
        EmitNamed(Verbosity::Warning);
    end;

    local procedure EmitNamed(Verbosity: Verbosity)
    var
        E: Record "EH Verb Error";
        W: Record "EH Verb Warn";
    begin
        if Verbosity = Verbosity::Error then
            E.Insert();
        if Verbosity in [Verbosity::Warning, Verbosity::Normal] then
            W.Insert();
    end;

    procedure ScopeAll()
    begin
        Send(TelemetryScope::All);
    end;

    local procedure Send(Scope: TelemetryScope)
    var
        P: Record "EH Scope Pub";
        A: Record "EH Scope All";
    begin
        if Scope = TelemetryScope::ExtensionPublisher then
            P.Insert()
        else
            A.Insert();
    end;
}
table 50272 "EH Txt Upper" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50273 "EH Txt Lower" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50274 "EH Txt Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50275 "EH Code AB" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50276 "EH Code XY" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50277 "EH Code Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50278 "EH Code If Lower" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50279 "EH Code If Upper" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50280 "EH Code If Other" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50281 "EH Txt Wide" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50282 "EH Txt Wide Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50284 "EH Code Ne Lower" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50285 "EH Txt Idx A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50286 "EH Txt Idx B" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50287 "EH Txt Dict A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50288 "EH Txt Dict Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50289 "EH Txt Read A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50290 "EH Txt Read Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50291 "EH Txt Len A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50292 "EH Txt Len Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50293 "EH Blob" { fields { field(1; Code; Code[20]) { } field(2; Data; Blob) { } } keys { key(PK; Code) { } } }
table 50294 "EH Code In Lower" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50295 "EH Txt Q A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50296 "EH Txt Q Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50297 "EH With Rec" { fields { field(1; Code; Code[20]) { } field(2; T; Text[30]) { } } keys { key(PK; Code) { } } }
table 50298 "EH Txt With A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50299 "EH Txt With Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50300 "EH Code In XY" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50301 "EH Txt WFwd A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50302 "EH Txt WFwd Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50303 "EH Txt WWr A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50304 "EH Txt WWr Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
// ENG-30: Text and Code literals as value domains, by the rules measured on
// BC 28.4 (`literal_values::Domain`).
codeunit 50283 "EH Text"
{
    procedure TxtUpper()
    begin
        OnText('A');
    end;

    procedure TxtLower()
    begin
        OnText('a');
    end;

    local procedure OnText(T: Text)
    var
        U: Record "EH Txt Upper";
        L: Record "EH Txt Lower";
        E: Record "EH Txt Else";
    begin
        case T of
            'A':
                U.Insert();
            'a':
                L.Insert();
            else
                E.Insert();
        end;
    end;

    // A Code parameter holds ' ab ' as 'AB'; a case arm converts too.
    procedure CodeSpaced()
    begin
        OnCode(' ab ');
    end;

    local procedure OnCode(C: Code[20])
    var
        AB: Record "EH Code AB";
        XY: Record "EH Code XY";
        E: Record "EH Code Else";
    begin
        case C of
            'ab':
                AB.Insert();
            'XY':
                XY.Insert();
            else
                E.Insert();
        end;
    end;

    // `=` compares a lowercase literal as written: never equal to a Code
    // value, but the IR cannot tell `=` from `in`, so it is left unresolved.
    procedure CodeIf()
    begin
        OnCodeIf('ab');
    end;

    local procedure OnCodeIf(C: Code[20])
    var
        Lo: Record "EH Code If Lower";
        Up: Record "EH Code If Upper";
        Ot: Record "EH Code If Other";
        Ne: Record "EH Code Ne Lower";
    begin
        if C = 'ab' then
            Lo.Insert();
        if C = 'AB' then
            Up.Insert();
        if C = 'XY' then
            Ot.Insert();
        // Always true for a Code value ('AB' <> 'ab' as written); converting
        // the label would wrongly make it never true.
        if C <> 'ab' then
            Ne.Insert();
    end;

    // A Text parameter written by an element write holds no literal any more.
    procedure TxtIndexed()
    begin
        OnIndexed('A');
    end;

    local procedure OnIndexed(T: Text)
    var
        A: Record "EH Txt Idx A";
        B: Record "EH Txt Idx B";
    begin
        T[1] := 'B';
        case T of
            'A':
                A.Insert();
            'B':
                B.Insert();
        end;
    end;

    // Platform methods that write a Text through a `var`.
    procedure TxtDictGet()
    begin
        OnDictGet('A');
    end;

    local procedure OnDictGet(T: Text)
    var
        D: Dictionary of [Integer, Text];
        A: Record "EH Txt Dict A";
        E: Record "EH Txt Dict Else";
    begin
        D.Add(1, 'B');
        if D.Get(1, T) then;
        case T of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;

    procedure TxtReadText()
    begin
        OnReadText('A');
    end;

    local procedure OnReadText(T: Text)
    var
        Buf: Record "EH Blob" temporary;
        OS: OutStream;
        S: InStream;
        A: Record "EH Txt Read A";
        E: Record "EH Txt Read Else";
    begin
        Buf.Data.CreateOutStream(OS);
        OS.WriteText('B');
        Buf.Data.CreateInStream(S);
        S.ReadText(T);
        case T of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;

    // sol on ENG-30: `in` with a lowercase label on a Code value; as `=`, left unresolved.
    procedure CodeIn()
    begin
        OnCodeIn('ab');
    end;

    local procedure OnCodeIn(C: Code[20])
    var
        Lo: Record "EH Code In Lower";
        Xy: Record "EH Code In XY";
    begin
        if C in ['ab'] then
            Lo.Insert();
        // An uppercase label the value ('AB') can never meet: pruned (sol round 2 on ENG-30:
        // the lone `yes` above would also pass if the guard were ignored).
        if C in ['XY'] then
            Xy.Insert();
    end;

    // sol round 2 on ENG-30: under `with R do`, `T` passed on may be R's field, so the callee
    // gets no value for it.
    procedure TxtWithFwd()
    begin
        OnWithFwd('A');
    end;

    local procedure OnWithFwd(T: Text)
    var
        R: Record "EH With Rec";
    begin
        R.FindFirst();
        with R do
            WithLeaf(T);
    end;

    local procedure WithLeaf(X: Text)
    var
        A: Record "EH Txt WFwd A";
        E: Record "EH Txt WFwd Else";
    begin
        case X of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;

    // And a `var` call under `with` may still write the parameter `T`.
    procedure TxtWithWrite()
    begin
        OnWithWrite('A');
    end;

    local procedure OnWithWrite(T: Text)
    var
        R: Record "EH With Rec";
        A: Record "EH Txt WWr A";
        E: Record "EH Txt WWr Else";
    begin
        with R do
            SetB(T);
        case T of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;

    // sol on ENG-30: a QUOTED argument to a `var` writes the parameter ("T" is T).
    procedure TxtQuoted()
    begin
        OnQuoted('A');
    end;

    local procedure OnQuoted(T: Text)
    var
        A: Record "EH Txt Q A";
        E: Record "EH Txt Q Else";
    begin
        SetB("T");
        case T of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;

    local procedure SetB(var X: Text)
    begin
        X := 'B';
    end;

    // sol on ENG-30: inside `with R do`, `T` is R's field T, not the parameter T.
    procedure TxtWith()
    begin
        OnWith('A');
    end;

    local procedure OnWith(T: Text)
    var
        R: Record "EH With Rec";
        A: Record "EH Txt With A";
        E: Record "EH Txt With Else";
    begin
        R.FindFirst();
        with R do
            case T of
                'A':
                    A.Insert();
                else
                    E.Insert();
            end;
    end;

    // A read-only platform function keeps it usable.
    procedure TxtStrLen()
    begin
        OnStrLen('A');
    end;

    local procedure OnStrLen(T: Text)
    var
        A: Record "EH Txt Len A";
        E: Record "EH Txt Len Else";
    begin
        if StrLen(T) = 0 then
            exit;
        case T of
            'A':
                A.Insert();
            else
                E.Insert();
        end;
    end;

    // Non-ASCII text: the comparison rules are not measured, so unresolved.
    procedure TxtWide()
    begin
        OnWide('Æ');
    end;

    local procedure OnWide(T: Text[10])
    var
        W: Record "EH Txt Wide";
        E: Record "EH Txt Wide Else";
    begin
        case T of
            'Æ':
                W.Insert();
            else
                E.Insert();
        end;
    end;
}
codeunit 50265 "EH Events"
{
    var
        Mode: Option Short,Long;

    // The event is raised two calls down a SHORT route the literal rules out
    // (the witness search lists it) and down a LONG live one.
    procedure EvRoot()
    begin
        EvDispatch(Mode::Long);
    end;

    // Raises the event itself: an extension point.
    procedure EvDirect()
    begin
        OnEhSomething();
    end;

    local procedure EvDispatch(M: Integer)
    begin
        case M of
            Mode::Short:
                EvRaise();
            Mode::Long:
                EvHop1();
        end;
    end;

    local procedure EvHop1()
    begin
        EvHop2();
    end;

    local procedure EvHop2()
    begin
        EvHop3();
    end;

    local procedure EvHop3()
    begin
        EvRaise();
    end;

    local procedure EvRaise()
    begin
        OnEhSomething();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnEhSomething()
    begin
    end;
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    crate::as_v3(serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    }))
}

/// The workspace, and `prove <routine> writes-table:<table>`'s answer.
struct Ws {
    dir: tempfile::TempDir,
    ids: Vec<(String, String)>,
}

impl Ws {
    fn new() -> Ws {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            r#"{"id":"e1e1e1e1-0000-4000-8000-0000000000e1","name":"E1","publisher":"t","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
        let inv = alsem(
            dir.path(),
            &["fingerprint", "ws", "--inventory-only", "--format", "json"],
        );
        let ids = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["routineName"].as_str().unwrap().to_string(),
                    r["stableRoutineId"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        Ws { dir, ids }
    }

    fn answer(&self, routine: &str, table: &str) -> String {
        let id = &self
            .ids
            .iter()
            .find(|(n, _)| n == routine)
            .unwrap_or_else(|| panic!("precondition: routine {routine} is in the inventory"))
            .1;
        let r = alsem(
            self.dir.path(),
            &[
                "prove",
                "ws",
                id,
                &format!("writes-table:{table}"),
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone();
        r["answer"].as_str().unwrap_or_default().to_string()
    }

    /// `explain --format json` of `routine`'s payload.
    fn explain(&self, routine: &str) -> serde_json::Value {
        alsem(
            self.dir.path(),
            &[
                "explain",
                "ws",
                "--routine",
                &format!("EH Events::{routine}"),
                "--format",
                "json",
            ],
        )
    }

    /// Every `(routine, table, answer)` holds; all mismatches in one message.
    fn expect(&self, cases: &[(&str, &str, &str)]) {
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(r, t, want)| {
                let got = self.answer(r, t);
                (got != *want).then(|| format!("{r} writes {t}: got {got}, want {want}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}

/// n10's shape: an Integer parameter, bound from a global Option's label,
/// prunes the `case` arms it does not match, and a forwarded parameter keeps
/// the value (`RunReport` → `RunReportID`). Break: dropping the arm
/// requirement (`DocSave` reaches `EH Print`).
#[test]
fn case_arms_follow_an_option_label_through_an_integer_parameter() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocSave", "EH Print", "no"),
        ("DocSave", "EH Save", "yes"),
        ("DocPrint", "EH Print", "yes"),
        ("DocPrint", "EH Save", "no"),
    ]);
}

/// Option ordinals count blank members: `OutputAction::SaveAsPDF` is 4 (two
/// blank members before it), so the integer literal 4 runs the `SaveAsPDF`
/// arm. Break: skipping blank members (SaveAsPDF becomes 2).
#[test]
fn an_option_ordinal_counts_blank_members() {
    let ws = Ws::new();
    ws.expect(&[("DocFour", "EH Save", "yes"), ("DocFour", "EH Print", "no")]);
}

/// The `else` is dead only when every bound value matches a resolved label
/// of some arm. Break: no requirement on the `else` (`DocSave` reaches
/// `EH Else`).
#[test]
fn case_else_runs_only_for_an_unlisted_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocSave", "EH Else", "no"),
        ("DocPrint", "EH Else", "no"),
        ("DocEMail", "EH Else", "yes"),
    ]);
}

/// `if P <> L`, `if P in [..]` and `if P <> L then exit;` prune as `=` does.
/// Break: flipping the test's polarity.
#[test]
fn if_comparisons_and_an_exit_guard_prune() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocSave", "EH Not Print", "yes"),
        ("DocPrint", "EH Not Print", "no"),
        ("DocSave", "EH In List", "no"),
        ("DocEMail", "EH In List", "yes"),
        ("DocPrint", "EH In List", "yes"),
        ("DocSave", "EH Exit Guarded", "no"),
        ("DocPrint", "EH Exit Guarded", "yes"),
    ]);
}

/// A label the engine cannot resolve (a range) keeps its arm AND the `else`
/// live, even for a value a resolved arm matches; a resolved arm that does not
/// match is still dead. Break: taking the `else` requirement from the resolved
/// arms only (`RangePrint` loses `EH Range Else`).
#[test]
fn an_unresolved_label_keeps_its_arm_and_the_else_live() {
    let ws = Ws::new();
    ws.expect(&[
        ("RangePrint", "EH Range First", "yes"),
        ("RangePrint", "EH Range", "yes"),
        ("RangePrint", "EH Range Else", "yes"),
        ("RangeSave", "EH Range First", "no"),
        ("RangeSave", "EH Range", "yes"),
        ("RangeSave", "EH Range Else", "yes"),
    ]);
}

/// Each route keeps its own binding: `DocJoin` reaches `RunReport` with the
/// `SaveAsPDF` literal and with its own unknown parameter, so the Print arm
/// stays live through the second route; an unknown root parameter or a
/// computed argument binds nothing. Break: keying the search's states by
/// routine only (the unknown route is folded into the literal one).
#[test]
fn an_unknown_route_never_inherits_another_routes_literal() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocJoin", "EH Print", "yes"),
        ("DocEither", "EH Print", "yes"),
        ("DocComputed", "EH Print", "yes"),
    ]);
}

/// An Enum parameter compares by (enum, value): a known value (an extension
/// member included) runs only its arm, or the `else`; an unknown value of an
/// extensible enum runs every arm and the `else`.
#[test]
fn an_enum_parameter_compares_by_enum_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("KindAlpha", "EH Alpha", "yes"),
        ("KindAlpha", "EH Beta", "no"),
        ("KindAlpha", "EH Kind Else", "no"),
        ("KindGamma", "EH Alpha", "no"),
        ("KindGamma", "EH Beta", "no"),
        ("KindGamma", "EH Kind Else", "yes"),
        ("KindUnknown", "EH Alpha", "yes"),
        ("KindUnknown", "EH Beta", "yes"),
        ("KindUnknown", "EH Kind Else", "yes"),
    ]);
}

/// ⟨ENG-30⟩ A built-in option type (`Verbosity`, `TelemetryScope`) is a value
/// domain compared by member name: a bound value runs only its arm (or the
/// `else`, or the `if` branch it meets), also through a parameter named like
/// its type; an unknown value runs every arm. Breaks (2026-11-18, both red):
/// `domain_of` without the built-in option types (every pruned case reads
/// `yes`); `typed_label` without them (`NamedWarn` reaches `EH Verb Error`).
#[test]
fn a_builtin_option_parameter_compares_by_member_name() {
    let ws = Ws::new();
    ws.expect(&[
        ("VerbError", "EH Verb Error", "yes"),
        ("VerbError", "EH Verb Warn", "no"),
        ("VerbError", "EH Verb Else", "no"),
        ("VerbVerbose", "EH Verb Error", "no"),
        ("VerbVerbose", "EH Verb Warn", "no"),
        ("VerbVerbose", "EH Verb Else", "yes"),
        ("VerbUnknown", "EH Verb Error", "yes"),
        ("VerbUnknown", "EH Verb Warn", "yes"),
        ("VerbUnknown", "EH Verb Else", "yes"),
        ("NamedWarn", "EH Verb Error", "no"),
        ("NamedWarn", "EH Verb Warn", "yes"),
        ("ScopeAll", "EH Scope Pub", "no"),
        ("ScopeAll", "EH Scope All", "yes"),
    ]);
}

/// ⟨ENG-30⟩ Text and Code literals as value domains, by the rules measured on
/// BC 28.4: Text compares exactly (case counts); a Code parameter holds a
/// literal trimmed and upper case, and a `case` arm converts the same way; an
/// `if C = '<lower>'` label is unresolved (`=` compares it as written, `in`
/// converts, and the IR does not tell them apart), while `'AB'` and `'XY'`
/// decide; non-ASCII text stays unresolved.
/// Breaks (2026-11-18, all red): `domain_of` without Text/Code (every pruned
/// case reads `yes`); `to_code` without the upper-casing (`CodeIf` drops
/// `EH Code If Upper`, a wrong `no`); `in_domain_test` as `in_domain`
/// (`CodeIf` drops `EH Code Ne Lower`, a wrong `no`).
#[test]
fn text_and_code_parameters_compare_by_the_measured_rules() {
    let ws = Ws::new();
    ws.expect(&[
        ("TxtUpper", "EH Txt Upper", "yes"),
        ("TxtUpper", "EH Txt Lower", "no"),
        ("TxtUpper", "EH Txt Else", "no"),
        ("TxtLower", "EH Txt Upper", "no"),
        ("TxtLower", "EH Txt Lower", "yes"),
        ("CodeSpaced", "EH Code AB", "yes"),
        ("CodeSpaced", "EH Code XY", "no"),
        ("CodeSpaced", "EH Code Else", "no"),
        ("CodeIf", "EH Code If Lower", "yes"),
        ("CodeIf", "EH Code If Upper", "yes"),
        ("CodeIf", "EH Code If Other", "no"),
        ("CodeIf", "EH Code Ne Lower", "yes"),
        ("TxtWide", "EH Txt Wide", "yes"),
        ("TxtWide", "EH Txt Wide Else", "yes"),
    ]);
}

/// ⟨ENG-30 review⟩ A Text parameter holds its literal only while nothing can
/// write it: an element write (`T[1] := 'B'`) or a platform method that writes
/// a text through a `var` (`Dictionary.Get`, `InStream.ReadText`) leaves every
/// arm live; a read-only platform function (`StrLen`) keeps the pruning.
/// Breaks (2026-11-18, both red): `lhs_base_name` without the element write
/// (`TxtIndexed` loses `EH Txt Idx B`, a wrong `no`); `value_passes_by_value`
/// ignoring `text` (the two writers lose their `else`, wrong `no`s).
#[test]
fn a_text_parameter_something_writes_binds_nothing() {
    let ws = Ws::new();
    ws.expect(&[
        ("TxtIndexed", "EH Txt Idx A", "yes"),
        ("TxtIndexed", "EH Txt Idx B", "yes"),
        ("TxtDictGet", "EH Txt Dict A", "yes"),
        ("TxtDictGet", "EH Txt Dict Else", "yes"),
        ("TxtReadText", "EH Txt Read A", "yes"),
        ("TxtReadText", "EH Txt Read Else", "yes"),
        ("TxtStrLen", "EH Txt Len A", "yes"),
        ("TxtStrLen", "EH Txt Len Else", "no"),
        // sol on ENG-30: a quoted `var` argument writes T; `T` under `with R` is R's field.
        ("TxtQuoted", "EH Txt Q A", "yes"),
        ("TxtQuoted", "EH Txt Q Else", "yes"),
        ("TxtWith", "EH Txt With A", "yes"),
        ("TxtWith", "EH Txt With Else", "yes"),
        ("CodeIn", "EH Code In Lower", "yes"),
        ("CodeIn", "EH Code In XY", "no"),
        ("TxtWithFwd", "EH Txt WFwd A", "yes"),
        ("TxtWithFwd", "EH Txt WFwd Else", "yes"),
        ("TxtWithWrite", "EH Txt WWr A", "yes"),
        ("TxtWithWrite", "EH Txt WWr Else", "yes"),
    ]);
}

/// A label of another enum is a conversion not modeled: its arm stays live
/// (its value name `Beta` differs from the bound `Alpha`); the other arm is
/// still judged. Break: dropping the enum's identity from the comparison (the
/// `EH Other` arm becomes dead).
#[test]
fn another_enums_label_is_not_the_same_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("KindOther", "EH Other Enum", "yes"),
        ("KindOther", "EH Other Beta", "no"),
    ]);
}

/// An Option parameter, bound from a LOCAL Option variable's label, compared
/// with its own labels.
#[test]
fn an_option_parameter_binds_from_a_local_options_label() {
    let ws = Ws::new();
    ws.expect(&[
        ("OptCallB", "EH Opt A", "no"),
        ("OptCallB", "EH Opt B", "yes"),
    ]);
}

/// The usable rule: a `var` parameter, an assigned one (compound), one passed
/// `var` to a helper, a `for` variable, and one passed to a call the resolver
/// did not resolve all hold something other than the caller's argument, so
/// nothing is pruned. One break each: removing that clause of the rule.
#[test]
fn a_parameter_whose_value_may_change_binds_nothing() {
    let ws = Ws::new();
    ws.expect(&[
        ("CtlVar", "EH Ctl Var", "yes"),
        ("CtlAssigned", "EH Ctl Assigned", "yes"),
        ("CtlPassed", "EH Ctl Passed", "yes"),
        ("CtlLoop", "EH Ctl Loop", "yes"),
        ("CtlUnknown", "EH Ctl Unknown", "yes"),
    ]);
}

/// An event raised only down calls is no extension point of the root, even
/// when its listed (shortest) path cannot run under the literal arguments and
/// is dropped: the row's empty path is not a zero-hop one. `EvRoot` raises
/// `OnEhSomething` four calls down (the two-call route is literal-dead);
/// `EvDirect` raises it itself. Break: reading an empty path as zero hops
/// (`EvRoot` lists the extension point, the CDO IssueReminder explain grew by
/// 60 such lines).
#[test]
fn an_event_whose_listed_paths_were_dropped_is_no_extension_point() {
    let ws = Ws::new();
    let points = |r: &str| -> Vec<String> {
        let v = ws.explain(r);
        let card = v
            .pointer("/payload/cards/0")
            .or_else(|| v.pointer("/payload"))
            .cloned()
            .unwrap_or(v.clone());
        let mut out = Vec::new();
        let mut stack = vec![&card];
        while let Some(x) = stack.pop() {
            match x {
                serde_json::Value::Object(m) => {
                    if let Some(serde_json::Value::Array(eps)) = m.get("extensionPoints") {
                        out.extend(
                            eps.iter()
                                .filter_map(|e| e["event"].as_str().map(String::from)),
                        );
                    }
                    stack.extend(m.values());
                }
                serde_json::Value::Array(a) => stack.extend(a.iter()),
                _ => {}
            }
        }
        out
    };
    assert_eq!(
        points("EvDirect"),
        ["OnEhSomething"],
        "precondition: a raise of its own is one"
    );
    assert!(points("EvRoot").is_empty(), "{:?}", points("EvRoot"));
}
