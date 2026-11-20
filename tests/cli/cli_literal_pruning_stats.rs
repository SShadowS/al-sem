//! `aldump --literal-pruning-stats` (ENG-135's measure, shared with ENG-30 and ENG-132..134):
//! on a fixture whose shapes are stated by hand, every count is what the shapes give.
//!
//! - `GuardCallee(B: Boolean)`: a usable guard parameter; `GuardRoot` passes `false`, so the
//!   `if B then` branch's sites are dead one hop away.
//! - `GuardWrite(B: Boolean)`: `Evaluate(B, ..)` writes it: unusable by a write.
//! - `IntCallee(I: Integer)`: a usable value parameter; `IntRoot` passes `1`, so arm `2`'s sites
//!   are dead one hop away.
//! - `IntWrite(I: Integer)`: `Evaluate(I, ..)` writes it: unusable by a write.
//!
//! Discrimination (2026-11-19): a guard or value frame's write check dropped (its parameter
//! counted usable) or a frame's dead-site predicate dropped each changes a count asserted here.

use std::process::Command;

/// `aldump --literal-pruning-stats` of a one-file workspace holding `src`: its text (two runs
/// print the same bytes) and its JSON.
fn stats(src: &str) -> (String, serde_json::Value) {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"14014014-0000-4000-8000-000000000140","name":"LP","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/LP.al"), src).unwrap();
    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
            .arg("--literal-pruning-stats")
            .arg(t.path())
            .env("RUST_LOG", "off")
            .output()
            .expect("spawn aldump");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let text = run();
    assert_eq!(run(), text, "deterministic");
    let v = serde_json::from_str(&text).expect("json");
    (text, v)
}

/// sol: a routine with two `#if` arms, only the second assigning `I`: the merged value frame
/// keeps `J` only, and `I` is excluded by the ASSIGNMENT (one arm), not by a write, so it is not
/// counted as unusable by a write.
#[test]
fn an_arm_that_assigns_a_parameter_is_no_write() {
    let src = r#"table 51402 "LP Arm" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
codeunit 51411 "LP Arms"
{
#if A
    procedure Arms(I: Integer; J: Integer)
    var
        T: Record "LP Arm";
    begin
        case J of
            1:
                T.Insert();
        end;
    end;
#else
    procedure Arms(I: Integer; J: Integer)
    var
        T: Record "LP Arm";
    begin
        I := 2;
        case J of
            1:
                T.Insert();
        end;
    end;
#endif
}
"#;
    let (text, v) = stats(src);
    let vf = &v["valueFrames"];
    assert_eq!(
        (&vf["paramsUsable"], &vf["paramsUnusableByWrite"]),
        (&1.into(), &0.into()),
        "J usable; I assigned in one arm, not written: {text}"
    );
}

/// sol: one-hop pruning counts a call in ANY arm (here only the second arm calls `Pick`), and a
/// callee site only when the call's arguments make it dead: `Leaf`'s `I = 2` branch is dead
/// under no binding at all, so `Leaf(false)` prunes only the `if B` site. A variable passed twice
/// to one call is one havoc write.
#[test]
fn one_hop_counts_every_arm_and_only_argument_pruning() {
    let src = r#"table 51403 "LP Pick One" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51404 "LP Pick Two" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51405 "LP Own" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51406 "LP Arg" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
codeunit 51412 "LP Hops"
{
#if A
    procedure Caller()
    begin
    end;
#else
    procedure Caller()
    begin
        Pick(1);
    end;
#endif

    local procedure Pick(K: Integer)
    var
        One: Record "LP Pick One";
        Two: Record "LP Pick Two";
    begin
        case K of
            1:
                One.Insert();
            2:
                Two.Insert();
        end;
    end;

    procedure Root()
    begin
        Leaf(false);
    end;

    local procedure Leaf(B: Boolean)
    var
        I: Integer;
        Own: Record "LP Own";
        Arg: Record "LP Arg";
    begin
        I := 1;
        if I = 2 then
            Own.Insert();
        if B then
            Arg.Insert();
    end;

    // sol round 2: Own.Insert is dead under no binding (I = 1), so B = false prunes nothing more
    // there, and no frame's count takes it.
    procedure Nested()
    begin
        NestedLeaf(false);
    end;

    local procedure NestedLeaf(B: Boolean)
    var
        I: Integer;
        Own: Record "LP Own";
    begin
        I := 1;
        if B then
            if I = 2 then
                Own.Insert();
    end;

    // sol round 3: a call binding only a table (G33) counts as binding, and its prune too.
    procedure TabCaller()
    var
        One: Record "LP Pick One";
    begin
        TabLeaf(One);
    end;

    local procedure TabLeaf(V: Variant)
    var
        RecRef: RecordRef;
        Two: Record "LP Pick Two";
    begin
        RecRef.GetTable(V);
        case RecRef.Number of
            Database::"LP Pick Two":
                Two.Insert();
        end;
    end;

    // sol round 6: `C.Run()` is an object run; the search enters OnRun with no bindings, so the
    // counter must credit no prune there, whatever the instance's globals hold.
    // sol round 7: the search never takes a dead edge; this Pick(2) sits under `I = 2` with
    // I = 1, so it brings no call, binding or prune to the counter.
    procedure DeadCaller()
    var
        I: Integer;
    begin
        I := 1;
        if I = 2 then
            Pick(2);
    end;

    procedure RunCaller()
    var
        C: Codeunit "LP Runner";
        One: Record "LP Pick One";
    begin
        C.SetRec(One);
        C.Run();
    end;

    procedure Twice()
    var
        I: Integer;
        Own: Record "LP Own";
    begin
        I := 1;
        Swap(I, I);
        if I = 1 then
            Own.Insert();
    end;

    local procedure Swap(var A: Integer; var B: Integer)
    begin
        A := B;
    end;
}
codeunit 51413 "LP Runner"
{
    var
        R: RecordRef;

    procedure SetRec(V: Variant)
    begin
        R.GetTable(V);
    end;

    trigger OnRun()
    var
        Two: Record "LP Pick Two";
    begin
        if R.Number <> Database::"LP Pick Two" then
            exit;
        Two.Insert();
    end;
}
"#;
    let (text, v) = stats(src);
    let h = &v["oneHop"];
    assert_eq!(
        (&h["deadByGuard"], &h["deadByValue"]),
        (&1.into(), &1.into()),
        "Arg.Insert by guard, Two.Insert by value; Nested's Own.Insert is dead without it: {text}"
    );
    assert_eq!(
        (&h["calls"], &h["callsBinding"], &h["deadSites"]),
        (&7.into(), &5.into(), &3.into()),
        "Pick(1) in the second arm, Leaf(false), NestedLeaf(false), TabLeaf(One) and \
         C.SetRec(One) bind (Swap binds nothing; C.Run() is an object run, entered with no \
         bindings, so OnRun's Batch-only insert is no prune of it; DeadCaller's Pick(2) is a dead \
         edge, never taken); Two.Insert, Arg.Insert and TabLeaf's Two.Insert die by the \
         arguments, the Own.Inserts by their own code: {text}"
    );
    assert_eq!(
        v["flow"]["havocWrites"], 1,
        "Swap(I, I) changes one variable: {text}"
    );
}

const SOURCE: &str = r#"table 51400 "LP One" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 51401 "LP Two" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
codeunit 51410 "LP Shapes"
{
    procedure GuardRoot()
    begin
        GuardCallee(false);
        GuardWrite(false);
    end;

    local procedure GuardCallee(B: Boolean)
    var
        T: Record "LP One";
    begin
        if B then
            T.Insert();
    end;

    local procedure GuardWrite(B: Boolean)
    var
        T: Record "LP One";
    begin
        Evaluate(B, 'true');
        if B then
            T.Insert();
    end;

    procedure IntRoot()
    begin
        IntCallee(1);
        IntWrite(1);
    end;

    local procedure IntCallee(I: Integer)
    var
        A: Record "LP One";
        B: Record "LP Two";
    begin
        case I of
            1:
                A.Insert();
            2:
                B.Insert();
        end;
    end;

    local procedure IntWrite(I: Integer)
    var
        A: Record "LP One";
        B: Record "LP Two";
    begin
        Evaluate(I, '2');
        case I of
            1:
                A.Insert();
            2:
                B.Insert();
        end;
    end;
}
"#;

#[test]
fn the_counts_follow_the_fixtures_shapes() {
    let (text, v) = stats(SOURCE);
    assert_eq!(v["routines"], 6, "{text}");
    let g = &v["guardFrames"];
    assert_eq!(
        (
            &g["paramsUsable"],
            &g["paramsUnusableByWrite"],
            &g["routinesWithFrame"]
        ),
        (&1.into(), &1.into(), &1.into()),
        "guard frames: GuardCallee usable, GuardWrite written: {text}"
    );
    let vf = &v["valueFrames"];
    assert_eq!(
        (
            &vf["paramsUsable"],
            &vf["paramsUnusableByWrite"],
            &vf["routinesWithFrame"]
        ),
        (&1.into(), &1.into(), &1.into()),
        "value frames: IntCallee usable, IntWrite written: {text}"
    );
    assert_eq!(vf["usableByDomain"]["Int"], 1, "{text}");
    // E2: all four callees read their parameter, so each has a flow program binding it; the
    // two `Evaluate`s are its havoc writes.
    let f = &v["flow"];
    assert_eq!(
        (
            &f["routinesWithProgram"],
            &f["paramsBound"],
            &f["havocWrites"]
        ),
        (&4.into(), &2.into(), &2.into()),
        "{text}"
    );
    let h = &v["oneHop"];
    assert_eq!(h["calls"], 4, "four calls, each to one routine: {text}");
    // Every call binds its literal (the written callees through the flow, which the write
    // then havocs), and exactly two sites are dead: the `if B` branch's insert in GuardCallee
    // (the guard frame proves it) and arm `2`'s insert in IntCallee (the value frame). The flow
    // proves both too; nothing in the written callees is dead.
    assert_eq!(h["callsBinding"], 4, "{text}");
    assert_eq!(
        (
            &h["deadSites"],
            &h["deadByGuard"],
            &h["deadByValue"],
            &h["deadByFlow"]
        ),
        (&2.into(), &1.into(), &1.into(), &2.into()),
        "{text}"
    );
}
