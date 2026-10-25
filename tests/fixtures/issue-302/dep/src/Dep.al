table 70300 "Dep Cust"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
    }
    keys
    {
        key(PK; "No.") { }
    }
}

page 70301 "Dep RO List"
{
    PageType = List;
    SourceTable = "Dep Cust";
    Editable = false;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
}

page 70302 "Dep NoMod Card"
{
    PageType = List;
    SourceTable = "Dep Cust";
    ModifyAllowed = false;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
}

page 70303 "Dep Card"
{
    PageType = List;
    SourceTable = "Dep Cust";
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
}

page 70304 "Dep Editable Card"
{
    PageType = List;
    SourceTable = "Dep Cust";
    Editable = true;
    ModifyAllowed = true;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
}
