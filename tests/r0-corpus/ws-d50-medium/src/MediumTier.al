// Issue 18 — d50's MEDIUM tier and its medium->info DEMOTION, exercised by the
// corpus for the first time.
//
// d50 escalates a checked-Run finding from `info` to `medium` when some routine
// on the span holds an explicit `Commit()` in its OWN body AND that commit is
// "proven effective" (`is_explicit_commit_proven_effective`, four caps). Cap 2
// rejects a committer whose root classification carries any kind from
// `D50_UNTRUSTED_ROOT_KINDS` — `onrun-codeunit` among them.
//
// Before this fixture NO committed fixture had an explicit `Commit()` anywhere
// near a checked-Run span, so `has_proven_effective_explicit_commit` was false
// by vacuity and the cap-2 list was never consulted: deleting `onrun-codeunit`
// from that list moved ZERO goldens. This fixture makes that edit observable.
//
//   A  local procedure committer   no root classification  -> UNCAPPED -> medium
//   B  trigger OnRun() committer   `onrun-codeunit`        -> CAPPED   -> info
//   C  BOTH committers, independent callers of one manager -> medium (the ANY
//      quantifier at `d50.rs:341-344` short-circuits on the uncapped one)
//
// A AND B ARE ONE-LINE TWINS. `PostDoc` and `RunChecked` are byte-identical in
// both objects; below the object header the ONLY delta is ONE LINE (three
// tokens): `local procedure RunAll()` in A, `trigger OnRun()` in B. Four ways of
// differing at once would make neither case a differential for the other; one
// way means a cap-2 break flips B `info`->`medium` while a span/commit-machinery
// break flips A `medium`->`info`, and neither can rot behind the other.
//
// C IS NOT A CHAIN. `backward_cone` visits a committing caller but never expands
// it (`transaction_spans.rs:105-107`), so `OnRun commits -> Helper commits ->
// seed` truncates at `Helper`, the `OnRun` never enters the span, and C
// measures ZERO findings — silently, because r4's anti-degenerate check is
// >= 1 per FIXTURE and A and B alone satisfy it. C's two committers are therefore
// INDEPENDENT callers of a shared TRANSACTION-MANAGING routine (`PostDocC`) that
// calls the seed. "Independent callers of the SEED" is the same vacuity mode
// again: the span would then hold no manager at all and the seed is skipped.
//
// FIXTURE INVARIANTS — each of these silently changes a verdict while the golden
// still mints, so they are stated rather than left to be inferred:
//   * TABLE-FREE. No table is declared or written anywhere. Every manager here
//     qualifies by the NAME branch (`^(Post|Apply|Release)[A-Z]`) alone. The
//     COUNT branch would give every row a second, unrelated way to move — a
//     temp-state or cone-folding change would then read as a cap regression.
//     `ws-d50-temp-gate` already owns the count branch.
//   * `RunAll` and `HelperC` stay `local`. Drop `local` and the routine becomes
//     `public-procedure`, which is ALSO untrusted, so A would demote to `info`
//     and C's control would collapse.
//   * `PostDoc` (x2) and `PostDocC` are the only `^(Post|Apply|Release)[A-Z]`
//     matches. `manager_id = managers[0]` indexes a list derived from a BTreeSet
//     of hash-bearing ids, so with two managers in one span which one lands in
//     `rootCause` is unpredictable. Exactly one manager per span.
//   * The shared worker's `OnRun` stays EMPTY and calls back into nothing.
//   * A break experiment on this fixture is LINE-COUNT PRESERVING (comment the
//     statement out, never delete the line) — a deleted line moves
//     `primaryLocation.line` in the golden and the test then goes red for a
//     reason that has nothing to do with the guard.

// A — the MEDIUM case. The committer is a `local procedure`, which gets NO root
// classification at all (`root_classification.rs:281` gives `public-procedure`
// only to a `procedure` with no access modifier), so cap 2 passes and the
// explicit Commit() escalates the finding.
codeunit 50230 "D50 Med Local Committer"
{
    local procedure RunAll()
    begin
        Commit();
        PostDoc();
    end;

    local procedure PostDoc()
    begin
        RunChecked();
    end;

    local procedure RunChecked()
    begin
        // checked Run → implicit commit on success → the CheckedRunImplicit seed
        if Codeunit.Run(Codeunit::"D50 Med Worker") then;
    end;
}

// B — the INFO case, and the point of the issue. Byte-identical to A below the
// committer's declaration line. `trigger OnRun()` is classified `onrun-codeunit`
// (`root_classification.rs:234-239`), which is in `D50_UNTRUSTED_ROOT_KINDS`, so
// cap 2 rejects the committer and the finding stays at `info`.
codeunit 50231 "D50 Med OnRun Committer"
{
    trigger OnRun()
    begin
        Commit();
        PostDoc();
    end;

    local procedure PostDoc()
    begin
        RunChecked();
    end;

    local procedure RunChecked()
    begin
        // checked Run → implicit commit on success → the CheckedRunImplicit seed
        if Codeunit.Run(Codeunit::"D50 Med Worker") then;
    end;
}

// C — the CONTROL for the ANY quantifier. Two committers on one span: `OnRun` is
// capped, `HelperC` is not. `any` short-circuits, so the span still escalates.
// Without C, an any->all regression would leave A and B untouched and pass.
codeunit 50232 "D50 Med Mixed Committers"
{
    trigger OnRun()
    begin
        Commit();
        PostDocC();
    end;

    local procedure HelperC()
    begin
        Commit();
        PostDocC();
    end;

    local procedure PostDocC()
    begin
        RunCheckedC();
    end;

    local procedure RunCheckedC()
    begin
        // checked Run → implicit commit on success → the CheckedRunImplicit seed
        if Codeunit.Run(Codeunit::"D50 Med Worker") then;
    end;
}

// The shared checked-Run target. Its OnRun is empty and calls back into nothing,
// so it can never contaminate any of the three spans above.
codeunit 50233 "D50 Med Worker"
{
    trigger OnRun()
    begin
        // intentionally empty
    end;
}
