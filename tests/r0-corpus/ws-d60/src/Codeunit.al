codeunit 50934 "D60 Upgrade"
{
    Subtype = Upgrade;

    // FLAGGED: row-by-row rewrite in an upgrade codeunit — DataTransfer territory.
    trigger OnUpgradePerCompany()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: the loop body CALLS a routine per row — DataTransfer cannot
    // invoke code per row.
    procedure UpgradeWithCall()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Name := Compute();
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: the loop body reads ANOTHER record (cross-table lookup) —
    // not a set-based copy DataTransfer can express.
    procedure UpgradeWithOtherRecord()
    var
        Item: Record "D60 Item";
        Ref: Record "D60 Ref";
    begin
        if Item.FindSet() then
            repeat
                Ref.Get(Item."No.");
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: the loop body COMPUTES the value under a parenthesized,
    // quoted-field `if` — a conditional DataTransfer cannot express. The parens +
    // quoted field mirror the real DO shape that the identifier-only
    // condition_references collection misses; the structural statement-tree walk
    // still catches it.
    procedure UpgradeWithConditional()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                if (Item."No." = '') then
                    Item.Name := 'default'
                else
                    Item.Name := 'set';
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: the loop body branches on a `case` over a quoted field — same
    // structural reason (mirrors the DO UpgradeSendCode shape).
    procedure UpgradeWithCase()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                case Item."No." of
                    '':
                        Item.Name := 'empty';
                    else
                        Item.Name := 'other';
                end;
                Item.Modify();
            until Item.Next() = 0;
    end;

    local procedure Compute(): Text
    begin
        exit('x');
    end;

    // ── issue #21: d60's traversal gate ────────────────────────────────────
    // Until #21, d60 inspected calls, other records and if/case in the loop
    // body but NOTHING about ops on the loop's own driver, so every shape
    // below was reported as a DataTransfer candidate. These must not be.
    //
    // They live here rather than beside d5's cases because d60 returns before
    // its loop unless the workspace has an Upgrade/Install object at all
    // (Codeunit.al has Subtype = Upgrade; a plain codeunit is invisible to it).
    // The d5 fixture is a plain codeunit, so it pins nothing about d60.

    // NOT FLAGGED: two rows per step -- the loop skips rows.
    procedure UpgradeMultiStepAdvance()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next(2) = 0;
    end;

    // NOT FLAGGED: a second unit-step advance in the BODY. Each advance is
    // eligible alone; together they visit every other row.
    procedure UpgradeBodyAdvanceSkipsRows()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Next();
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: the selected set is narrowed to the current row mid-loop.
    procedure UpgradeInLoopSetRange()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.SetRange("No.", Item."No.");
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: traversal order changes mid-iteration.
    procedure UpgradeInLoopSetCurrentKey()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.SetCurrentKey(Name);
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: `Reset` clears the filters AND the key. d5 rejects Reset by
    // NAME; d60 has no such list, so before #21 nothing stopped it -- this is
    // the shape the implementation review (I2) found still open.
    procedure UpgradeInLoopReset()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Name := 'migrated';
                Item.Modify();
                Item.Reset();
            until Item.Next() = 0;
    end;

    // NOT FLAGGED: a second retrieval on the driver repositions the cursor.
    procedure UpgradeInLoopGet()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Get('X');
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next() = 0;
    end;

    // STILL FLAGGED: an explicit unit step is an ordinary advance. This row
    // matters as much as the ones above -- the change must not cost a true
    // positive.
    procedure UpgradeUnitStepAdvance()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Name := 'migrated';
                Item.Modify();
            until Item.Next(1) = 0;
    end;
}

codeunit 50935 "D60 Normal"
{
    // NOT FLAGGED: same loop outside an upgrade/install codeunit (d5/d10 territory).
    procedure RegularLoop()
    var
        Item: Record "D60 Item";
    begin
        if Item.FindSet() then
            repeat
                Item.Name := 'x';
                Item.Modify();
            until Item.Next() = 0;
    end;
}
