//! The #65/#69 temp-state probe workspace, shared by the `cli` umbrella
//! (`cli_digest_temp_state`, the digest acceptance matrix) and, from #69 Task 2,
//! the `r3` umbrella (`r3_temp_class_parity`, the cone/L4 class-parity oracle A5).
//!
//! Every fixture routine but `TwiceUi` is named in the #69 design spec's acceptance matrix
//! (`docs/superpowers/specs/2026-10-14-issue-69-digest-temp-state-coherence-design.md`).
//! `// @marker` comments name call sites the tests locate by LINE.
//!
//! The two deep chains (`Deep0..Deep66`, `DeepF0..DeepF66`) are generated so a
//! path longer than the digest witness's `MAX_DEPTH` (64) exists.

pub const APP_JSON: &str = r#"{
  "id": "65656565-0000-0000-0000-000000000065",
  "name": "Issue 65 Probe",
  "publisher": "RC",
  "version": "1.0.0.0"
}"#;

/// Length of the generated chains: `Deep0..Deep{DEEP_N-1}`, `DeepF0..`.
pub const DEEP_N: usize = 67;

const PROBE_HEAD: &str = r#"table 50300 "Probe Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }

codeunit 50301 "Probe Writer"
{
    procedure TempOnly()
    var Buffer: Record "Probe Row" temporary;
    begin Buffer.Id := 1; Buffer.Insert(); end;

    procedure RealWrite()
    var Row: Record "Probe Row";
    begin Row.Id := 1; Row.Insert(); end;

    procedure Helper(var R: Record "Probe Row")
    begin R.Insert(); end;

    procedure ViaHelperTemp()
    var Buffer: Record "Probe Row" temporary;
    begin Helper(Buffer); end;

    procedure ViaHelperReal()
    var Row: Record "Probe Row";
    begin Helper(Row); end;

    procedure Forward(var X: Record "Probe Row")
    begin Helper(X); end;

    procedure Rec1(var R: Record "Probe Row"; N: Integer)
    begin if N > 0 then Rec2(R, N - 1); R.Insert(); end;

    procedure Rec2(var R: Record "Probe Row"; N: Integer)
    begin Rec1(R, N); end;

    procedure ViaRecursionTemp()
    var Buffer: Record "Probe Row" temporary;
    begin Rec1(Buffer, 2); end;

    procedure ViaRecursionReal()
    var Row: Record "Probe Row";
    begin Rec1(Row, 2); end;

    // ---- #69 fixtures -------------------------------------------------

    procedure HelperUi(var R: Record "Probe Row")
    begin
        R.Insert(); // @helperui-insert
        Message('x');
    end;

    procedure HelperModUi(var R: Record "Probe Row")
    begin
        R.Modify(); // @helpermodui-modify
        Message('x');
    end;

    procedure HelperDelUi(var R: Record "Probe Row")
    begin
        R.Delete(); // @helperdelui-delete
        Message('x');
    end;

    procedure HelperUiTemp()
    var Buffer: Record "Probe Row" temporary;
    begin HelperUi(Buffer); end;

    procedure HelperUiReal()
    var Row: Record "Probe Row";
    begin HelperUi(Row); end;

    procedure HelperModUiTemp()
    var Buffer: Record "Probe Row" temporary;
    begin HelperModUi(Buffer); end;

    procedure HelperDelUiTemp()
    var Buffer: Record "Probe Row" temporary;
    begin HelperDelUi(Buffer); end;

    procedure HelperUiBoth()
    var
        Row: Record "Probe Row";
        Buffer: Record "Probe Row" temporary;
    begin
        HelperUi(Row); // @both-row
        HelperUi(Buffer); // @both-buffer
    end;

    procedure HelperUiBothDeep()
    begin
        BothA(); // @bothdeep-a
    end;

    procedure BothA()
    var
        Row: Record "Probe Row";
        Buffer: Record "Probe Row" temporary;
    begin
        HelperUi(Row); // @botha-row
        HelperUi(Buffer); // @botha-buffer
    end;

    procedure ForwardUi(var X: Record "Probe Row")
    begin HelperUi(X); end;

    procedure ViaForwardUiTemp()
    var Buffer: Record "Probe Row" temporary;
    begin ForwardUi(Buffer); end;

    procedure ViaForwardUiReal()
    var Row: Record "Probe Row";
    begin ForwardUi(Row); end;

    // Not in the acceptance matrix: two unguarded call sites into one callee
    // reach ONE solve state through two links (#69 D2 arena link identity).
    procedure TwiceUi(var X: Record "Probe Row")
    begin
        HelperUi(X);
        HelperUi(X);
    end;

    procedure CondTwice(var X: Record "Probe Row"; Flag: Boolean)
    begin
        if Flag then HelperUi(X); // @condtwice-1
        HelperUi(X); // @condtwice-2
    end;

    procedure ViaCondTwiceTemp()
    var
        Buffer: Record "Probe Row" temporary;
        Flag: Boolean;
    begin CondTwice(Buffer, Flag); end;

    procedure RecUi1(var R: Record "Probe Row"; N: Integer)
    begin
        if N > 0 then RecUi2(R, N - 1);
        R.Insert();
        Message('x');
    end;

    procedure RecUi2(var R: Record "Probe Row"; N: Integer)
    begin RecUi1(R, N); end;

    procedure RecUiTemp()
    var Buffer: Record "Probe Row" temporary;
    begin RecUi1(Buffer, 2); end;

    procedure RecUiReal()
    var Row: Record "Probe Row";
    begin RecUi1(Row, 2); end;

    procedure MixedUi(var R: Record "Probe Row")
    var Local: Record "Probe Row";
    begin
        R.Insert();
        Local.Insert();
        Message('x');
    end;

    procedure MixedUiTemp()
    var Buffer: Record "Probe Row" temporary;
    begin MixedUi(Buffer); end;

    procedure TwoRecUi(var X: Record "Probe Row"; var Y: Record "Probe Row")
    begin
        X.Insert();
        Y.Insert();
        Message('x');
    end;

    procedure TwoRecMix()
    var
        Temp: Record "Probe Row" temporary;
        Real: Record "Probe Row";
    begin TwoRecUi(Temp, Real); end;

    procedure TwoRecTemp()
    var
        Temp: Record "Probe Row" temporary;
        Temp2: Record "Probe Row" temporary;
    begin TwoRecUi(Temp, Temp2); end;

    procedure CycA()
    var B: Record "Probe Row" temporary;
    begin CycB(B, 2); end;

    procedure CycB(var R: Record "Probe Row"; N: Integer)
    begin
        R.Insert();
        Message('x');
        if N > 0 then CycA();
    end;

    procedure CallsCycA()
    begin CycA(); end;

    procedure CycQ1(var R: Record "Probe Row"; N: Integer)
    begin
        R.Insert();
        CycQ2(R, N);
    end;

    procedure CycQ2(var R: Record "Probe Row"; N: Integer)
    var Real: Record "Probe Row";
    begin
        Real.Insert();
        Message('x');
        if N > 0 then CycQ1(R, N - 1);
    end;

    procedure CycQTemp()
    var Buffer: Record "Probe Row" temporary;
    begin CycQ1(Buffer, 2); end;

    procedure RecH(var R: Record "Probe Row"; N: Integer)
    var TempLocal: Record "Probe Row" temporary;
    begin
        HelperUi(TempLocal);
        if N > 0 then RecH(R, N - 1);
    end;

    procedure SibP1(var R: Record "Probe Row"; N: Integer)
    begin
        R.Insert();
        Message('x');
        if N > 0 then SibP2(N);
    end;

    procedure SibP2(N: Integer)
    var T: Record "Probe Row" temporary;
    begin SibP1(T, N - 1); end;

    procedure RootCyc1()
    var T: Record "Probe Row" temporary;
    begin
        HelperUi(T);
        RootCyc2();
    end;

    procedure RootCyc2()
    begin RootCyc1(); end;

    procedure PermUi(var A: Record "Probe Row"; var B: Record "Probe Row"; N: Integer)
    begin
        A.Insert();
        Message('x');
        if N > 0 then PermUi(B, A, N - 1); // @permui-self
    end;

    procedure PermMix()
    var
        Temp: Record "Probe Row" temporary;
        Real: Record "Probe Row";
    begin PermUi(Temp, Real, 2); end;

    procedure SelfRecUi(var X: Record "Probe Row"; N: Integer)
    var TempLocal: Record "Probe Row" temporary;
    begin
        X.Insert();
        Message('x');
        if N > 0 then SelfRecUi(TempLocal, N - 1);
    end;

    procedure SelfRecReal()
    var Real: Record "Probe Row";
    begin SelfRecUi(Real, 1); end;

    procedure GuardFwd(var R: Record "Probe Row"; DoIt: Boolean)
    begin if DoIt then HelperUi(R); end;

    procedure GuardFwdFalse()
    var Real: Record "Probe Row";
    begin GuardFwd(Real, false); end;

    procedure GuardFwdTrue()
    var Real: Record "Probe Row";
    begin GuardFwd(Real, true); end;

    procedure GCyc1(var R: Record "Probe Row"; En: Boolean; N: Integer)
    begin
        if En then HelperUi(R);
        if N > 0 then GCyc2(N - 1, R, En);
    end;

    procedure GCyc2(N: Integer; var R: Record "Probe Row"; En: Boolean)
    begin GCyc1(R, En, N); end;

    procedure GCycFalse()
    var Real: Record "Probe Row";
    begin GCyc2(1, Real, false); end;

    procedure GCycTrue()
    var Real: Record "Probe Row";
    begin GCyc2(1, Real, true); end;

    procedure TempHelperUi()
    var T: Record "Probe Row" temporary;
    begin HelperUi(T); end;

    procedure GT1(En: Boolean; N: Integer)
    begin
        if En then TempHelperUi();
        if N > 0 then GT2(N - 1, En);
    end;

    procedure GT2(N: Integer; En: Boolean)
    begin GT1(En, N); end;

    procedure GTFalse()
    begin GT2(1, false); end;

    procedure GTTrue()
    begin GT2(1, true); end;

    procedure ForwardMix()
    var TempBuf: Record "Probe Row" temporary;
    begin Both(TempBuf); end;

    procedure Both(var X: Record "Probe Row")
    begin
        ForwardUi(X);
        RealWriter();
    end;

    procedure RealWriter()
    var Real: Record "Probe Row";
    begin Real.Insert(); end;

    procedure MixedCarrier()
    var TempBuf: Record "Probe Row" temporary;
    begin Wrap(TempBuf); end;

    procedure Wrap(var X: Record "Probe Row")
    var TempLocal: Record "Probe Row" temporary;
    begin
        HelperUi(X);
        TempLocal.Insert();
    end;

    procedure HelperTwoUi(var R: Record "Probe Row"; Flag: Boolean)
    begin
        if Flag then R.Insert();
        R.Insert();
        Message('x');
    end;

    procedure HelperTwoTemp()
    var
        Buf: Record "Probe Row" temporary;
        F: Boolean;
    begin HelperTwoUi(Buf, F); end;

    procedure CondOnlyTemp()
    var
        Buf: Record "Probe Row" temporary;
        Flag: Boolean;
    begin if Flag then HelperUi(Buf); end;

    procedure ValidateTemp()
    var Buffer: Record "Probe Row" temporary;
    begin Buffer.Validate(Id, 1); end;

    procedure ValidateReal()
    var Row: Record "Probe Row";
    begin Row.Validate(Id, 1); end;

    procedure ValidateRealUi()
    var Row: Record "Probe Row";
    begin
        Row.Validate(Id, 1);
        Message('x');
    end;

    procedure CopyTemp()
    var
        A: Record "Probe Row" temporary;
        B: Record "Probe Row" temporary;
    begin A.Copy(B); end;

    procedure CopyShareTemp()
    var
        A: Record "Probe Row" temporary;
        B: Record "Probe Row" temporary;
    begin A.Copy(B, true); end;

    procedure CopyReal()
    var
        A: Record "Probe Row";
        B: Record "Probe Row";
    begin A.Copy(B); end;

    procedure CopyRealUi()
    var
        A: Record "Probe Row";
        B: Record "Probe Row";
    begin
        A.Copy(B);
        Message('x');
    end;

    procedure TransferTemp()
    var
        A: Record "Probe Row" temporary;
        B: Record "Probe Row";
    begin A.TransferFields(B); end;

    procedure TransferReal()
    var
        A: Record "Probe Row";
        B: Record "Probe Row";
    begin A.TransferFields(B); end;

    procedure IncRoot()
    var Buf: Record "Probe Row" temporary;
    begin IncWrap(Buf); end;

    procedure IncWrap(var X: Record "Probe Row")
    var Flag: Boolean;
    begin
        if Flag then HelperUi(X);
        DeepF0(X);
    end;

    procedure DeepRoot()
    begin Deep0(); end;

    procedure DeepEnd()
    var
        TempBuf: Record "Probe Row" temporary;
        Row: Record "Probe Row";
    begin
        TempBuf.Insert();
        Row.Insert();
    end;
"#;

/// The whole probe source: the hand-written routines plus the two generated
/// chains, closing the codeunit.
pub fn probe_al() -> String {
    let mut s = String::from(PROBE_HEAD);
    for i in 0..DEEP_N {
        let next = if i + 1 < DEEP_N {
            format!("Deep{}()", i + 1)
        } else {
            "DeepEnd()".to_string()
        };
        s.push_str(&format!(
            "\n    procedure Deep{i}()\n    begin {next}; end;\n"
        ));
    }
    for i in 0..DEEP_N {
        let next = if i + 1 < DEEP_N {
            format!("DeepF{}(X)", i + 1)
        } else {
            "HelperUi(X)".to_string()
        };
        s.push_str(&format!(
            "\n    procedure DeepF{i}(var X: Record \"Probe Row\")\n    begin {next}; end;\n"
        ));
    }
    s.push_str("}\n");
    s
}

pub fn write_workspace() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-69-")
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(root.join("src").join("Probe.al"), probe_al()).expect("write al source");
    tmp
}
