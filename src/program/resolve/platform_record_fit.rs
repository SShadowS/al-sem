//! ENG-173: whether the PLATFORM Record method of a name can take a call, from its overloads'
//! parameters ([`super::platform_record_sigs`], generated from the AL compiler's own symbols).
//!
//! A table procedure named like a record built-in OVERLOADS it, and alc binds each call by its
//! arguments (BC 28.4 and alc 18.0.41, `/work/.al-sem/engine-notes/eng173/{alc-bind,joker}/
//! RESULTS.md`). The platform method cannot take a call when:
//! - the call's arity is outside every overload (`E.Next(RR, true)`: `Next(Steps?)` takes at most
//!   one argument; `E.Count(RR)`: `Count()` takes none), A1 and A4;
//! - a `Field` parameter gets something that is no field of the record: a literal, a variable, an
//!   expression (`E.FieldNo('K')`, `E.FieldNo(S)`, `E.Validate(1, 2)`), J1, J3 and J6;
//! - a scalar or a record parameter gets a value of another kind no conversion reaches (a
//!   RecordRef or an object handle for a scalar, a scalar or a RecordRef for a record).
//!
//! Then only the table's procedure binds. Otherwise the platform MAY bind. When the table's
//! procedure may take the call too, alc ranks the conversions (`Find('-')` runs the platform over
//! a user `Find(Text)`, A8; `Get(I: Integer)` runs a user `Get(Variant)`, J9), which this module
//! does not model: the caller keeps both.

#[path = "platform_record_sigs.rs"]
mod sigs;

/// A platform parameter's kind (the generator's mapping of the compiler's `NavTypeKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamKind {
    /// A `Joker` named `Field…`: a field of the record.
    Field,
    /// Any other `Joker`: a value.
    Value,
    Boolean,
    Integer,
    BigInteger,
    Decimal,
    Text,
    Guid,
    /// A record (`Copy(FromRecord)`, `TransferFields`).
    Record,
    Enum,
    /// Any other type: never ruled out here.
    Other,
}

/// One parameter of a platform overload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlatformParam {
    pub kind: ParamKind,
    pub optional: bool,
    /// The last parameter repeats (`Get(Value…)`).
    pub variadic: bool,
    /// The AL type keyword a declared parameter spells for this type (`boolean`, `integer`, `text`,
    /// …); `None` for a type no declaration spells the same way (`String`, `Joker`, `Table`).
    pub al: Option<&'static str>,
}

/// Whether a table procedure of `method_lc` whose parameters have the AL types `user` (each a
/// declared type, folded: `code[20]`, `boolean`) HIDES the platform method: some overload has
/// exactly those types (alc warns AL0524 "the base type already defines a method … with the same
/// parameter types"; BC 28.4 ran the table's `GetFilters()` and `Insert()`, ENG-173's zeroarg
/// probe). A type no declaration can spell as the platform does (`Find(Which: String)` against a
/// user `Find(Which: Text)`) is no duplicate: the two overload (BC ran the platform's `Find('-')`).
pub(crate) fn platform_hidden_by(method_lc: &str, user: &[String]) -> bool {
    let sigs = sigs::PLATFORM_RECORD_SIGS;
    let Ok(i) = sigs.binary_search_by(|(n, _)| (*n).cmp(method_lc)) else {
        return false;
    };
    let keyword = |t: &str| t.split(['[', ' ']).next().unwrap_or("").to_string();
    sigs[i].1.iter().any(|params| {
        let same_types = params.len() == user.len()
            && params
                .iter()
                .zip(user)
                .all(|(p, u)| p.al.is_some_and(|al| al == keyword(u)));
        // ENG-178 (Fable r2 on ENG-173): a procedure with no parameters takes the call with no
        // arguments from an overload whose parameters are all optional (`Delete(RunTrigger?)`,
        // `Next(Steps?)`, `Modify(RunTrigger?)`, `FindSet(ForUpdate?)`): alc gives no AL0524 but
        // binds the table's procedure (`D := O.Delete()` compiles only so, the procedure
        // returning a record). Measured for zero parameters, against every all-optional
        // overload in the table (23 methods, `Get(Value…)` and `LockTable(Wait?, …)` among them;
        // engine-notes/eng178/alloptional, sol r1).
        let zero_over_optional = user.is_empty() && params.iter().all(|p| p.optional || p.variadic);
        same_types || zero_over_optional
    })
}

/// What the resolver knows of one argument, as [`platform_can_take`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArgFacts {
    /// Whether the argument is a field of the receiver's table: `Some(false)` for a literal, an
    /// expression, or a name no field of the table scope has; `None` when it may be one.
    pub field: Option<bool>,
    pub class: ArgClass,
}

/// The argument's type, as far as it rules a parameter out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgClass {
    RecordRef,
    /// A record of a known table.
    Record,
    /// Another object handle (a codeunit, a page, an interface …).
    Object,
    /// A simple value (Integer, Text, Code, Boolean, Date, Option …).
    Scalar,
    Unknown,
}

/// Whether the platform method can take a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fit {
    /// No overload can: only a table procedure of the name binds.
    Cannot,
    /// Some overload may.
    May,
}

/// [`Fit`] of the platform Record method `method_lc` for a call with `args`; `None` when no
/// platform Record method has that name.
pub(crate) fn platform_can_take(method_lc: &str, args: &[ArgFacts]) -> Option<Fit> {
    let sigs = sigs::PLATFORM_RECORD_SIGS;
    let i = sigs.binary_search_by(|(n, _)| (*n).cmp(method_lc)).ok()?;
    let any = sigs[i]
        .1
        .iter()
        .any(|params| overload_may_take(params, args));
    Some(if any { Fit::May } else { Fit::Cannot })
}

fn overload_may_take(params: &[PlatformParam], args: &[ArgFacts]) -> bool {
    let required = params.iter().filter(|p| !p.optional && !p.variadic).count();
    let variadic = params.last().is_some_and(|p| p.variadic);
    if args.len() < required || (!variadic && args.len() > params.len()) {
        return false;
    }
    args.iter().enumerate().all(|(k, a)| {
        let p = params
            .get(k)
            .or_else(|| params.last())
            .expect("arity checked");
        !certain_clash(p.kind, a)
    })
}

/// Whether no argument of these facts can be passed to a parameter of kind `p` (measured or a
/// conversion AL has not).
fn certain_clash(p: ParamKind, a: &ArgFacts) -> bool {
    use ArgClass as C;
    match p {
        ParamKind::Field => a.field == Some(false),
        // A value Joker takes a RecordRef too (Fable on ENG-173: with no table procedure,
        // `PL.Rename(RR)` compiles, `PL.Get(RR)` with AL0697 only): `E.Get(RR)` running the
        // table's `Get(RecordRef)` (A6, J4) is alc's ranking, not the platform refusing it.
        ParamKind::Value => false,
        ParamKind::Boolean
        | ParamKind::Integer
        | ParamKind::BigInteger
        | ParamKind::Decimal
        | ParamKind::Text
        | ParamKind::Guid
        | ParamKind::Enum => matches!(a.class, C::RecordRef | C::Record | C::Object),
        ParamKind::Record => matches!(a.class, C::RecordRef | C::Scalar | C::Object),
        ParamKind::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELD: ArgFacts = ArgFacts {
        field: None,
        class: ArgClass::Unknown,
    };
    const fn arg(field: Option<bool>, class: ArgClass) -> ArgFacts {
        ArgFacts { field, class }
    }
    const RR: ArgFacts = arg(Some(false), ArgClass::RecordRef);
    const BOOL_LIT: ArgFacts = arg(Some(false), ArgClass::Scalar);
    const INT_LIT: ArgFacts = arg(Some(false), ArgClass::Scalar);
    const TEXT_LIT: ArgFacts = arg(Some(false), ArgClass::Scalar);
    const TEXT_VAR: ArgFacts = arg(Some(false), ArgClass::Scalar);
    const INT_VAR: ArgFacts = arg(Some(false), ArgClass::Scalar);
    const VARIANT: ArgFacts = arg(Some(false), ArgClass::Unknown);

    /// Every call measured on BC 28.4 (`eng173/alc-bind` A1-A9, `eng173/joker` J1-J9): where the
    /// table's procedure ran and no platform overload can take the call, `Cannot`; where the
    /// platform ran, `May`. Where the table's procedure ran though the platform could take the
    /// call too (A8's opposite: J8, J9, and A6 — a value Joker takes a RecordRef: with no table
    /// procedure `PL.Rename(RR)` compiles, Fable on ENG-173 — by alc's conversion ranking), `May`:
    /// the caller keeps both.
    ///
    /// Discrimination (2026-11-23): a `Field` parameter taking anything (`certain_clash` false for
    /// `Field`) answers `May` for J1, J3, J6; the arity check off answers `May` for A1 and A4:
    /// each fails; restored, passes.
    #[test]
    fn every_measured_call_is_read_as_bc_binds_it() {
        use Fit::*;
        let rows: &[(&str, &str, &[ArgFacts], Fit)] = &[
            // A: table procedures Next(var RecordRef; Boolean), Count(RecordRef), Get(RecordRef),
            // Find(Text).
            (
                "A1 E.Next(RR, true) -> USER",
                "next",
                &[RR, BOOL_LIT],
                Cannot,
            ),
            ("A2 E.Next() -> PLATFORM", "next", &[], May),
            ("A3 E.Next(1) -> PLATFORM", "next", &[INT_LIT], May),
            ("A4 E.Count(RR) -> USER", "count", &[RR], Cannot),
            ("A5 E.Count() -> PLATFORM", "count", &[], May),
            (
                "A6 E.Get(RR) -> USER (ranked; both kept)",
                "get",
                &[RR],
                May,
            ),
            ("A7 E.Get('X') -> PLATFORM", "get", &[TEXT_LIT], May),
            (
                "A8 E.Find('-') -> PLATFORM (user Find(Text) fits too)",
                "find",
                &[TEXT_LIT],
                May,
            ),
            ("A9 E.Find() -> PLATFORM", "find", &[], May),
            // J: table procedures FieldNo(Text), SetRange(RecordRef), Validate(Integer; Integer),
            // Get(Variant); K is a field of the table.
            ("J1 E.FieldNo('K') -> USER", "fieldno", &[TEXT_LIT], Cannot),
            ("J2 E.FieldNo(K) -> PLATFORM", "fieldno", &[FIELD], May),
            (
                "J3 E.FieldNo(S: Text) -> USER",
                "fieldno",
                &[TEXT_VAR],
                Cannot,
            ),
            // A RecordRef is no field: `SetRange(Field, …)` cannot take it.
            ("J4 E.SetRange(RR) -> USER", "setrange", &[RR], Cannot),
            ("J5 E.SetRange(K) -> PLATFORM", "setrange", &[FIELD], May),
            (
                "J6 E.Validate(1, 2) -> USER",
                "validate",
                &[INT_LIT, INT_LIT],
                Cannot,
            ),
            (
                "J7 E.Validate(K, 2) -> PLATFORM",
                "validate",
                &[FIELD, INT_LIT],
                May,
            ),
            (
                "J8 E.Get(V: Variant) -> USER (ranked; both kept)",
                "get",
                &[VARIANT],
                May,
            ),
            (
                "J9 E.Get(I: Integer) -> USER (ranked; both kept)",
                "get",
                &[INT_VAR],
                May,
            ),
        ];
        for (case, method, args, want) in rows {
            assert_eq!(platform_can_take(method, args), Some(*want), "{case}");
        }
        assert_eq!(
            platform_can_take("myproc", &[]),
            None,
            "no platform method of the name"
        );
    }
}
