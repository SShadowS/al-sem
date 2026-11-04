codeunit 50117 "Out Overloads"
{
    // T-W2-24: two overloads with one display on one root's paths.
    procedure RunBoth()
    begin
        Note('a');
        Note('a', 1);
    end;

    local procedure Note(Msg: Text)
    var
        Bin: Record "Out Bin";
    begin
        Bin.Code := 'A';
        Bin.Insert();
    end;

    local procedure Note(Msg: Text; Level: Integer)
    var
        Item: Record "Out Item";
    begin
        Item."No." := 'Z';
        Item.Insert();
    end;
}
