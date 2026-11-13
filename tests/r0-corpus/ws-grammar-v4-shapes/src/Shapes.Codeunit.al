// TLS-41: the grammar-v4 changes that alter what a program means, one routine per
// shape, so `l2_features.snapshot` pins the engine's view of each. The exact IR
// shapes are pinned in `crates/al-syntax/src/lower/grammar_v4_tests.rs`.
codeunit 50410 "G4 Shapes"
{
    // The `else` belongs to `if B`: when A is false, neither call runs.
    procedure DanglingElse(A: Boolean; B: Boolean)
    begin
        if A then
            if B then
                OnInner()
            else
                OnInnerElse();
    end;

    // `and`/`or`/`xor` bind tighter than `=` and `<`; `-X * Y` is `(-X) * Y`.
    procedure Precedence(A: Boolean; B: Boolean; C: Boolean; X: Integer; Y: Integer): Integer
    begin
        if A = B and C then
            OnInner();
        if X < Y xor C then
            OnInnerElse();
        if A or B and C then
            OnInner();
        exit(-X * Y);
    end;

    // `..` only inside a list or a case label.
    procedure Ranges(X: Integer)
    begin
        if X in [1, 3..5] then
            OnInner();
        case X of
            1..5:
                OnInnerElse();
        end;
    end;

    local procedure OnInner()
    begin
    end;

    local procedure OnInnerElse()
    begin
    end;
}
