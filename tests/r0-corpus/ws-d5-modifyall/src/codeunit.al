codeunit 50105 D5ModifyAll
{
	procedure BadLoop()
	var Customer: Record Customer;
	begin
		Customer.SetRange("Buy-from No.", '');
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next() = 0;
	end;

	procedure SafeLoop()
	var Customer: Record Customer; Helper: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Helper.Get(Customer."Buy-from No.");
				Helper.Modify();
			until Customer.Next() = 0;
	end;

	// ── issue #21: the companion-op list is argument-blind ──────────────
	// Each routine below is one acceptance row. The ones that must STILL be
	// reported matter as much as the ones that must not: this change only
	// removes findings, so a lost true positive is the regression to watch.

	// A2: an explicit unit step is still an ordinary advance. REPORTED.
	procedure UnitStepAdvance()
	var Customer: Record Customer;
	begin
		Customer.SetRange("Buy-from No.", '');
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next(1) = 0;
	end;

	// A3: two rows per step -- the loop does not visit every row. NOT reported.
	procedure MultiStepAdvance()
	var Customer: Record Customer;
	begin
		Customer.SetRange("Buy-from No.", '');
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next(2) = 0;
	end;

	// A4a: zero step. NOT reported.
	procedure ZeroStepAdvance()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next(0) = 0;
	end;

	// A4b: a signed literal is a unary expression, not an integer literal,
	// so it is not the literal 1. NOT reported.
	procedure SignedStepAdvance()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next(-1) = 0;
	end;

	// A4c: a variable step is unknowable here. NOT reported.
	procedure VariableStepAdvance(Step: Integer)
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next(Step) = 0;
	end;

	// A5: TWO unit-step advances -- each is eligible on its own, and together
	// they skip every other row. No argument predicate catches this; only
	// cardinality plus terminator ownership does. NOT reported.
	procedure BodyAdvanceSkipsRows()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.Next();
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next() = 0;
	end;

	// A6: an in-loop filter narrowed to the CURRENT row. NOT reported.
	procedure InLoopSetRange()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.SetRange("No.", Customer."No.");
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next() = 0;
	end;

	// A7: same, via SetFilter. NOT reported.
	procedure InLoopSetFilter()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.SetFilter("No.", '>%1', Customer."No.");
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next() = 0;
	end;

	// A8: re-keying mid-iteration changes what "the rest of the set" means.
	// NOT reported.
	procedure InLoopSetCurrentKey()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.SetCurrentKey(Blocked);
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next() = 0;
	end;

	// A9: companions the NAME gate already rejected must stay rejected. The
	// new predicate returns no veto for these, so if it ever became the sole
	// gate they would start being reported. NOT reported -- before or after.
	procedure InLoopDelete()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
				Customer.Delete();
			until Customer.Next() = 0;
	end;

	procedure InLoopValidate()
	var Customer: Record Customer;
	begin
		if Customer.FindSet() then
			repeat
				Customer.Validate(Blocked);
				Customer.Modify();
			until Customer.Next() = 0;
	end;

	// A10: load-field setters change which COLUMNS are fetched, not which
	// rows or in what order. REPORTED.
	procedure InLoopLoadFields()
	var Customer: Record Customer;
	begin
		Customer.SetLoadFields(Blocked);
		if Customer.FindSet() then
			repeat
				Customer.Blocked := Customer.Blocked::All;
				Customer.Modify();
			until Customer.Next() = 0;
	end;
}

table 18 Customer
{
	fields {
		field(1; "No."; Code[20]) { }
		field(2; Blocked; Option) { OptionMembers = " ",All; }
		field(60; "Buy-from No."; Code[20]) { }
	}
}
