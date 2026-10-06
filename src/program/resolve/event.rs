//! Event attribute parsing primitives — Plan 1B.2 Phase 4b Task 1.
//!
//! Reads `[EventSubscriber(...)]` attribute arguments from the IR, detects
//! publisher routines by attribute name, and reads the
//! `EventSubscriberInstance = Manual` codeunit property.
//!
//! Clean-room: no L3 `event_graph` imports.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AttributeIr, ExprId, ExprKind, Ir, Literal, ObjectDecl, RoutineDecl};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Subscriber argument parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Typed result of parsing an `[EventSubscriber(…)]` attribute's positional args.
///
/// All string fields are lowercased and unquoted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedSubscriberArgs {
    /// Publisher object type, lowercased (e.g. `"codeunit"`).
    pub publisher_object_type: String,
    /// Publisher object name, unquoted and lowercased. Empty when the attribute
    /// names the publisher by number (`publisher_id`).
    pub publisher_name: String,
    /// The publisher's object number when the attribute names it that way
    /// (`[EventSubscriber(ObjectType::Codeunit, 50, …)]`; engine-switch S4.3b).
    pub publisher_id: Option<i64>,
    /// Event procedure name, unquoted and lowercased.
    pub event_name: String,
    /// Optional element filter — `None` when absent or when the arg is an empty
    /// string literal.
    pub element: Option<String>,
    pub skip_on_missing_license: bool,
    pub skip_on_missing_permission: bool,
}

/// Parse `[EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterX',
/// 'Element', SkipLicense, SkipPermission)]` from the IR expression arena.
///
/// Arg mapping:
/// - 0 `QualifiedEnum.value`                      → `publisher_object_type` (lc)
/// - 1 `DatabaseReference` / `Member.member` / …  → `publisher_name` (unquoted, lc)
/// - 2 `Literal::Text`                            → `event_name` (stripped, lc)
/// - 3 `Literal::Text`                            → `element` (`None` if absent/empty)
/// - 4 `Literal::Bool`                            → `skip_on_missing_license` (absent → false)
/// - 5 `Literal::Bool`                            → `skip_on_missing_permission` (absent → false)
///
/// Returns `None` when arg 0/1/2 is missing or of an unrecognised kind.
pub fn parse_event_subscriber_ir(attr: &AttributeIr, ir: &Ir) -> Option<ParsedSubscriberArgs> {
    if attr.args.len() < 3 {
        return None;
    }

    // Arg 0: `ObjectType::Codeunit` → QualifiedEnum { value: "Codeunit" }
    let publisher_object_type = match &ir.expr(attr.args[0]).kind {
        ExprKind::QualifiedEnum { value, .. } => value.to_ascii_lowercase(),
        _ => return None,
    };

    // Arg 1: `Codeunit::"Pub"` — several IR shapes depending on grammar parse
    // path — or the object number (`50`), which AL also accepts.
    let (publisher_name, publisher_id) = match &ir.expr(attr.args[1]).kind {
        ExprKind::Literal(Literal::Int(n)) => (String::new(), Some(n.trim().parse().ok()?)),
        _ => (resolve_publisher_name(ir, attr.args[1])?, None),
    };

    // Arg 2: the event name, as a text literal (`'OnAfterX'`) or as an
    // identifier (`OnAfterX`, `"On After X"`); AL accepts both, and real code
    // uses both (35 of CDO's 96 subscriptions name the event as an identifier).
    let event_name = name_arg_text(ir, attr.args[2])?;
    if event_name.is_empty() {
        return None;
    }

    // Arg 3 (optional): element filter, in the same two forms — absent or empty
    // → None.
    let element = attr
        .args
        .get(3)
        .and_then(|&id| name_arg_text(ir, id))
        .filter(|v| !v.is_empty());

    // Arg 4 (optional): skip_on_missing_license; absent → false.
    let skip_on_missing_license = attr
        .args
        .get(4)
        .is_some_and(|&id| matches!(&ir.expr(id).kind, ExprKind::Literal(Literal::Bool(true))));

    // Arg 5 (optional): skip_on_missing_permission; absent → false.
    let skip_on_missing_permission = attr
        .args
        .get(5)
        .is_some_and(|&id| matches!(&ir.expr(id).kind, ExprKind::Literal(Literal::Bool(true))));

    Some(ParsedSubscriberArgs {
        publisher_object_type,
        publisher_name,
        publisher_id,
        event_name,
        element,
        skip_on_missing_license,
        skip_on_missing_permission,
    })
}

/// The text of an event-name or element argument, unquoted and folded: a text
/// literal (`'OnAfterX'`) or an identifier (`OnAfterX` / `"On After X"`).
fn name_arg_text(ir: &Ir, id: ExprId) -> Option<String> {
    match &ir.expr(id).kind {
        ExprKind::Literal(Literal::Text(s))
        | ExprKind::Identifier(s)
        | ExprKind::QuotedIdentifier(s) => Some(strip_al_string(s).fold_identifier()),
        _ => None,
    }
}

/// Resolve arg 1 of an `[EventSubscriber]` attribute to the publisher object
/// name (unquoted, lowercased).
///
/// Handles:
/// - `DatabaseReference("Codeunit::\"Pub\"")` — split on `::`, strip quotes on RHS
/// - `Member { member: "\"Pub\"", .. }`        — strip quotes from member text
/// - `QualifiedEnum { value, .. }`              — already unquoted by `ident_text`
/// - `Identifier` / `QuotedIdentifier`          — strip quotes, lowercase
fn resolve_publisher_name(ir: &Ir, id: ExprId) -> Option<String> {
    match &ir.expr(id).kind {
        ExprKind::DatabaseReference(t) => {
            let name_part = match t.split_once("::") {
                Some((_, n)) => n,
                None => t.as_str(),
            };
            Some(strip_al_string(name_part).fold_identifier())
        }
        ExprKind::Member { member, .. } => Some(strip_al_string(member).fold_identifier()),
        ExprKind::QualifiedEnum { value, .. } => Some(value.fold_identifier()),
        ExprKind::Identifier(s) | ExprKind::QuotedIdentifier(s) => {
            Some(strip_al_string(s).fold_identifier())
        }
        _ => None,
    }
}

/// Strip exactly ONE layer of surrounding single or double quotes from a raw AL
/// string / identifier token.  Returns the inner slice (already trimmed).
fn strip_al_string(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 {
        let b = s.as_bytes();
        let first = b[0];
        let last = b[s.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Publisher kind detection
// ─────────────────────────────────────────────────────────────────────────────

/// The event-publisher kind encoded by a routine's attribute.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum PublisherKind {
    Integration,
    Business,
    Internal,
    /// A platform-generated table event (`OnAfter*Event` / `OnBefore*Event` +
    /// field validate) with NO publisher routine in source. Carried by a
    /// SYNTHETIC publisher routine injected on the table so that subscribers to
    /// the platform's implicit DB-trigger / validate events (a large class of
    /// real integration wiring) resolve instead of orphaning. See
    /// [`is_platform_table_event`] and `build::inject_platform_event_publishers`.
    Platform,
}

/// True when `name_lc` is a platform-generated TABLE event that AL raises
/// implicitly on a DB operation (insert/modify/delete/rename) or a field
/// validate. These have NO publisher routine in source — a `[EventSubscriber(
/// ObjectType::Table, Database::X, 'OnAfterDeleteEvent', …)]` targeting one binds
/// to a synthetic [`PublisherKind::Platform`] publisher on the table.
pub fn is_platform_table_event(name_lc: &str) -> bool {
    matches!(
        name_lc,
        "onbeforeinsertevent"
            | "onafterinsertevent"
            | "onbeforemodifyevent"
            | "onaftermodifyevent"
            | "onbeforedeleteevent"
            | "onafterdeleteevent"
            | "onbeforerenameevent"
            | "onafterrenameevent"
            | "onbeforevalidateevent"
            | "onaftervalidateevent"
    )
}

/// True when `name_lc` is a platform-generated PAGE event that AL raises
/// implicitly (page lifecycle, record navigation, page-level record CRUD, field
/// validate, or action). These have NO publisher routine in source — a
/// `[EventSubscriber(ObjectType::Page, Page::X, 'OnOpenPageEvent'/
/// 'OnModifyRecordEvent'/'OnAfterValidateEvent'/'OnAfterActionEvent'/…)]` binds
/// to a synthetic [`PublisherKind::Platform`] publisher on the page.
pub fn is_platform_page_event(name_lc: &str) -> bool {
    matches!(
        name_lc,
        "onopenpageevent"
            | "onclosepageevent"
            | "onqueryclosepageevent"
            | "onaftergetrecordevent"
            | "onaftergetcurrrecordevent"
            | "onnewrecordevent"
            | "oninsertrecordevent"
            | "onmodifyrecordevent"
            | "ondeleterecordevent"
            | "onbeforevalidateevent"
            | "onaftervalidateevent"
            | "onbeforeactionevent"
            | "onafteractionevent"
    )
}

/// Canonical PascalCase display name for a platform table/page event; `name_lc`
/// must satisfy [`is_platform_table_event`] or [`is_platform_page_event`]. Falls
/// back to a generic label otherwise.
pub fn platform_event_display_name(name_lc: &str) -> &'static str {
    match name_lc {
        // Table DB triggers.
        "onbeforeinsertevent" => "OnBeforeInsertEvent",
        "onafterinsertevent" => "OnAfterInsertEvent",
        "onbeforemodifyevent" => "OnBeforeModifyEvent",
        "onaftermodifyevent" => "OnAfterModifyEvent",
        "onbeforedeleteevent" => "OnBeforeDeleteEvent",
        "onafterdeleteevent" => "OnAfterDeleteEvent",
        "onbeforerenameevent" => "OnBeforeRenameEvent",
        "onafterrenameevent" => "OnAfterRenameEvent",
        // Shared table/page field validate.
        "onbeforevalidateevent" => "OnBeforeValidateEvent",
        "onaftervalidateevent" => "OnAfterValidateEvent",
        // Page lifecycle / record / action.
        "onopenpageevent" => "OnOpenPageEvent",
        "onclosepageevent" => "OnClosePageEvent",
        "onqueryclosepageevent" => "OnQueryClosePageEvent",
        "onaftergetrecordevent" => "OnAfterGetRecordEvent",
        "onaftergetcurrrecordevent" => "OnAfterGetCurrRecordEvent",
        "onnewrecordevent" => "OnNewRecordEvent",
        "oninsertrecordevent" => "OnInsertRecordEvent",
        "onmodifyrecordevent" => "OnModifyRecordEvent",
        "ondeleterecordevent" => "OnDeleteRecordEvent",
        "onbeforeactionevent" => "OnBeforeActionEvent",
        "onafteractionevent" => "OnAfterActionEvent",
        _ => "PlatformEvent",
    }
}

/// Classify a routine as an event publisher from its lowercased `attributes`
/// list.  Returns the first matching kind; `None` when the routine carries no
/// publisher attribute.
pub fn is_event_publisher(decl: &RoutineDecl) -> Option<PublisherKind> {
    for attr in &decl.attributes {
        match attr.as_str() {
            "integrationevent" => return Some(PublisherKind::Integration),
            "businessevent" => return Some(PublisherKind::Business),
            "internalevent" => return Some(PublisherKind::Internal),
            _ => {}
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// IncludeSender — Task 1 (applicability-checker fix, round-2 grounding)
// ─────────────────────────────────────────────────────────────────────────────

/// Read `IncludeSender` — the FIRST positional arg of a publisher routine's
/// `[IntegrationEvent]` / `[BusinessEvent]` / `[InternalEvent]` attribute —
/// from its raw IR. Verified against Microsoft Learn (2026-07-02): all three
/// attributes carry `IncludeSender` at argument index 0:
/// `[IntegrationEvent(IncludeSender: Boolean, GlobalVarAccess: Boolean [,
/// Isolated: Boolean])]`, `[BusinessEvent(IncludeSender: Boolean [, Isolated:
/// Boolean])]`, `[InternalEvent(IncludeSender: Boolean [, Isolated:
/// Boolean])]`. When `true`, the compiler prepends an implicit `Sender`
/// parameter that subscriber signatures may (but need not) capture — see
/// [`subscriber_arity_bound`] for the tolerance rule this feeds.
///
/// Returns `None` (tri-state UNKNOWN) when the routine carries no publisher
/// attribute at all, or the attribute's first arg is absent / not a boolean
/// literal — defensive fail-closed, since [`subscriber_arity_bound`] treats
/// `None` identically to `Some(false)` (no `+1` tolerance without positive
/// evidence). In practice a real publisher attribute always carries a
/// literal boolean here: source-tier because the attribute is parsed
/// directly from the call site's own text (this function); ABI-tier per
/// `abi_ingest::abi_publisher_include_sender`'s doc (a 13,581-entry probe of
/// a real Microsoft Base Application `SymbolReference.json` found 100%
/// coverage — every publisher attribute's `arg[0]` was present and
/// parseable, zero `None`s).
pub fn publisher_include_sender(decl: &RoutineDecl, ir: &Ir) -> Option<bool> {
    let attr = decl.attributes_parsed.iter().find(|a| {
        matches!(
            a.name.to_ascii_lowercase().as_str(),
            "integrationevent" | "businessevent" | "internalevent"
        )
    })?;
    match attr.args.first().map(|&id| &ir.expr(id).kind) {
        Some(ExprKind::Literal(Literal::Bool(b))) => Some(*b),
        _ => None,
    }
}

/// The Sender-tolerant subscriber arity bound for a publisher: the
/// publisher's own explicit arity, plus exactly ONE additional parameter —
/// but ONLY when the publisher's `include_sender == Some(true)`. A blanket
/// `+1` regardless of `IncludeSender` is SYNCHRONIZED WRONGNESS: the extra
/// parameter is illegal AL unless the publisher attribute actually declares
/// it. `include_sender == None` (unknown) and `Some(false)` both yield NO
/// tolerance — fail-closed, no `+1` without positive evidence.
///
/// SINGLE SOURCE OF TRUTH for this bound — consumed by BOTH
/// `ResolveIndex::build`'s subscriber-candidate wiring
/// (`crate::program::resolve::index`) and `verify_event_subscriber_route`'s
/// independent re-check (`crate::program::resolve::differential`). Any
/// future change to the tolerance rule belongs HERE, not duplicated at
/// either call site — the two must never drift (Task 1).
///
/// Residual (round-2 addendum): this is an ARITY bound only — the Sender
/// parameter's declared TYPE is not cross-checked against the publisher's
/// own object type.
pub fn subscriber_arity_bound(
    publisher_params_count: usize,
    include_sender: Option<bool>,
) -> usize {
    publisher_params_count + usize::from(include_sender == Some(true))
}

// ─────────────────────────────────────────────────────────────────────────────
// EventSubscriberInstance property
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` when the object's `EventSubscriberInstance` property is set
/// to `Manual` (case-insensitive).  Accepts both the bare form (`Manual`) and
/// the qualified enum form (`EventSubscriberInstance::Manual`).
pub fn read_event_subscriber_instance(obj: &ObjectDecl) -> bool {
    obj.properties.iter().any(|p| {
        if p.name != "eventsubscriberinstance" {
            return false;
        }
        // Strip an optional `Enum::` qualifier from the raw value text.
        let v = match p.value.rfind("::") {
            Some(i) => &p.value[i + 2..],
            None => p.value.as_str(),
        };
        v.trim().eq_ignore_ascii_case("manual")
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // All tests parse real AL source via `al_syntax::parse` to build a genuine IR
    // rather than constructing arena nodes by hand.  This validates the full
    // lowerer → IR → event-parser pipeline.

    // ── parse_event_subscriber_ir ─────────────────────────────────────────────

    #[test]
    fn full_six_args_empty_element_license_true() {
        let src = r#"codeunit 50100 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterX', '', true, false)]
    local procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let attr = &af.objects[0].routines[0].attributes_parsed[0];
        assert_eq!(
            parse_event_subscriber_ir(attr, &af.ir),
            Some(ParsedSubscriberArgs {
                publisher_object_type: "codeunit".into(),
                publisher_name: "pub".into(),
                publisher_id: None,
                event_name: "onafterx".into(),
                element: None,
                skip_on_missing_license: true,
                skip_on_missing_permission: false,
            })
        );
    }

    #[test]
    fn element_present_is_returned() {
        let src = r#"codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterX', 'MyElement', false, false)]
    local procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let attr = &af.objects[0].routines[0].attributes_parsed[0];
        let result = parse_event_subscriber_ir(attr, &af.ir).expect("should parse");
        assert_eq!(result.element, Some("myelement".into()));
    }

    #[test]
    fn missing_optional_args_default_to_false() {
        // Only 4 args: element present, args 4+5 absent → both false.
        let src = r#"codeunit 50102 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterX', '')]
    local procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let attr = &af.objects[0].routines[0].attributes_parsed[0];
        let result = parse_event_subscriber_ir(attr, &af.ir).expect("should parse");
        assert!(!result.skip_on_missing_license, "absent arg 4 → false");
        assert!(!result.skip_on_missing_permission, "absent arg 5 → false");
    }

    // Engine-switch S4.3a: AL also accepts the event name and the element as
    // identifiers (real CDO shapes: `OnOpenPageEvent`, and
    // `OnAfterValidateEvent, EnabledFeature`). Neither engine read them before.
    #[test]
    fn identifier_event_name_and_element_parse() {
        let src = r#"codeunit 50106 Sub
{
    [EventSubscriber(ObjectType::Page, Page::"Pub Page", OnAfterValidateEvent, EnabledFeature, true, true)]
    local procedure A()
    begin
    end;

    [EventSubscriber(ObjectType::Page, Page::"Pub Page", OnOpenPageEvent, '', true, true)]
    local procedure B()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", "On After X", "My Field", false, false)]
    local procedure C()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let got: Vec<(String, Option<String>)> = af.objects[0]
            .routines
            .iter()
            .map(|r| {
                let a = parse_event_subscriber_ir(&r.attributes_parsed[0], &af.ir)
                    .expect("identifier forms parse");
                (a.event_name, a.element)
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "onaftervalidateevent".to_string(),
                    Some("enabledfeature".to_string())
                ),
                ("onopenpageevent".to_string(), None),
                ("on after x".to_string(), Some("my field".to_string())),
            ]
        );
    }

    // Engine-switch S4.3b: AL also accepts the publisher's object number (4 of
    // CDO's 96 subscriptions, e.g. `ObjectType::Codeunit, 80`).
    #[test]
    fn numeric_publisher_parses_to_an_id() {
        let src = r#"codeunit 50107 Sub
{
    [EventSubscriber(ObjectType::Codeunit, 80, 'OnAfterX', '', false, false)]
    local procedure A()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let a = parse_event_subscriber_ir(&af.objects[0].routines[0].attributes_parsed[0], &af.ir)
            .expect("numeric publisher parses");
        assert_eq!((a.publisher_name.as_str(), a.publisher_id), ("", Some(80)));
    }

    #[test]
    fn malformed_too_few_args_returns_none() {
        let src = r#"codeunit 50103 Sub
{
    [EventSubscriber(ObjectType::Codeunit)]
    local procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let attr = &af.objects[0].routines[0].attributes_parsed[0];
        assert_eq!(parse_event_subscriber_ir(attr, &af.ir), None);
    }

    #[test]
    fn two_event_subscriber_attributes_both_parsed() {
        let src = r#"codeunit 50104 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub1", 'OnAfterX', '', false, false)]
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub2", 'OnAfterY', '', false, false)]
    local procedure MultiSub()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let attrs = &af.objects[0].routines[0].attributes_parsed;
        assert_eq!(
            attrs.len(),
            2,
            "both EventSubscriber attrs attached to the routine"
        );
        let r1 = parse_event_subscriber_ir(&attrs[0], &af.ir).expect("first attr parses");
        let r2 = parse_event_subscriber_ir(&attrs[1], &af.ir).expect("second attr parses");
        assert_eq!(r1.publisher_name, "pub1");
        assert_eq!(r1.event_name, "onafterx");
        assert_eq!(r2.publisher_name, "pub2");
        assert_eq!(r2.event_name, "onaftery");
    }

    // Task T1.4 (deep-review-t1-4), H-8: a comment interleaved between
    // `[EventSubscriber(...)]` args is a legal named child (a `comment`/
    // `multiline_comment` can appear almost anywhere) — pre-fix, `lower_routine`
    // pushed it into `AttributeIr.args` as a positional slot, shifting every
    // later arg (event name / element / skip flags) by one and silently
    // unregistering the WHOLE subscriber (`parse_event_subscriber_ir` rejects
    // an arg-0/1/2 kind mismatch). The fix lives entirely in `al-syntax`
    // (`structural_children`, applied to the attribute-arg lowering loop) — this
    // engine-side test proves the whole `al_syntax::parse` → `attributes_parsed`
    // → `parse_event_subscriber_ir` pipeline still resolves the subscriber
    // correctly with the comment present.
    #[test]
    fn comment_between_attribute_args_does_not_shift_positional_parsing() {
        let src = r#"codeunit 50105 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterX', /* comment */ '', false, false)]
    local procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let attr = &af.objects[0].routines[0].attributes_parsed[0];
        assert_eq!(
            parse_event_subscriber_ir(attr, &af.ir),
            Some(ParsedSubscriberArgs {
                publisher_object_type: "codeunit".into(),
                publisher_name: "pub".into(),
                publisher_id: None,
                event_name: "onafterx".into(),
                element: None,
                skip_on_missing_license: false,
                skip_on_missing_permission: false,
            }),
            "the subscriber must still be registered despite the inline comment"
        );
    }

    // ── is_event_publisher ────────────────────────────────────────────────────

    #[test]
    fn integration_event_attribute_detected() {
        let src = r#"codeunit 50200 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(is_event_publisher(r), Some(PublisherKind::Integration));
    }

    #[test]
    fn business_event_attribute_detected() {
        let src = r#"codeunit 50201 Pub
{
    [BusinessEvent(false)]
    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(is_event_publisher(r), Some(PublisherKind::Business));
    }

    #[test]
    fn no_publisher_attribute_returns_none() {
        let src = r#"codeunit 50202 Plain
{
    procedure Plain()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(is_event_publisher(r), None);
    }

    // ── publisher_include_sender ─────────────────────────────────────────────

    #[test]
    fn include_sender_true_on_integration_event() {
        let src = r#"codeunit 50210 Pub
{
    [IntegrationEvent(true, false)]
    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(publisher_include_sender(r, &af.ir), Some(true));
    }

    #[test]
    fn include_sender_false_on_integration_event() {
        let src = r#"codeunit 50211 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(publisher_include_sender(r, &af.ir), Some(false));
    }

    #[test]
    fn include_sender_true_on_business_event() {
        let src = r#"codeunit 50212 Pub
{
    [BusinessEvent(true)]
    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(publisher_include_sender(r, &af.ir), Some(true));
    }

    #[test]
    fn include_sender_true_on_internal_event() {
        let src = r#"codeunit 50213 Pub
{
    [InternalEvent(true)]
    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(publisher_include_sender(r, &af.ir), Some(true));
    }

    #[test]
    fn include_sender_none_when_no_publisher_attribute() {
        let src = r#"codeunit 50214 Plain
{
    procedure Plain()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(publisher_include_sender(r, &af.ir), None);
    }

    // ── subscriber_arity_bound ───────────────────────────────────────────────

    #[test]
    fn arity_bound_adds_one_when_include_sender_true() {
        assert_eq!(subscriber_arity_bound(0, Some(true)), 1);
        assert_eq!(subscriber_arity_bound(3, Some(true)), 4);
    }

    #[test]
    fn arity_bound_no_tolerance_when_include_sender_false() {
        assert_eq!(subscriber_arity_bound(0, Some(false)), 0);
        assert_eq!(subscriber_arity_bound(3, Some(false)), 3);
    }

    #[test]
    fn arity_bound_no_tolerance_when_include_sender_unknown() {
        assert_eq!(subscriber_arity_bound(0, None), 0);
        assert_eq!(subscriber_arity_bound(3, None), 3);
    }

    // ── read_event_subscriber_instance ───────────────────────────────────────

    #[test]
    fn event_subscriber_instance_manual_true() {
        let src = r#"codeunit 50300 Sub
{
    EventSubscriberInstance = Manual;

    procedure OnAfterX()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        assert!(
            read_event_subscriber_instance(&af.objects[0]),
            "Manual → true"
        );
    }

    #[test]
    fn event_subscriber_instance_absent_false() {
        let src = r#"codeunit 50301 Plain
{
    procedure Plain()
    begin
    end;
}"#;
        let af = al_syntax::parse(src);
        assert!(
            !read_event_subscriber_instance(&af.objects[0]),
            "absent → false"
        );
    }
}
