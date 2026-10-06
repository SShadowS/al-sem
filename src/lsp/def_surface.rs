//! The definition-surface fingerprint (T3 Task 7): a blake3 hash of every
//! piece of ONE file's data that ANOTHER file's call-graph resolution can
//! consult — per the field-list audit,
//! `docs/superpowers/specs/2026-07-12-t3-def-surface-audit.md` §4. The
//! incremental updater (Task 9) compares a file's OLD vs. NEW surface after a
//! re-parse: UNCHANGED means only that file's own edges need re-resolving
//! (rung 1); CHANGED means every file must be re-resolved (rung 2, the safe
//! fallback).
//!
//! A FALSE NEGATIVE here (a real surface change hashing to the SAME value) is
//! a silent stale-resolution bug, so per that audit's own guidance: when in
//! doubt, INCLUDE a field — a false POSITIVE only costs an extra rung-2
//! rebuild, never a soundness violation.
//!
//! Object/routine extraction is NOT re-walked here — it reuses
//! [`crate::program::extract_nodes`], the SAME extractor
//! `crate::program::build` calls to build the real `ProgramGraph`, so this
//! fingerprint's notion of "the surface" can never drift from the graph's own.
//!
//! # Canonical encoding
//!
//! Every string and list is length-prefixed (a `u64` LE count/byte-length,
//! written into the [`blake3::Hasher`] before the payload) so no field's
//! content can be mistaken for an adjacent field's framing (the "Plan-1A
//! content_hash" lesson — never `format!`-glue values into one string before
//! hashing). Every enum/bool is a single tagged byte; every `Option<T>` is a
//! 0/1 tag byte followed by `T`'s encoding when `Some`.
//!
//! # Field order (fixed; matches the audit's §4 list — see below for one item
//! this module's implementation found missing from that list, since folded
//! back into it)
//!
//! For each object, in `ObjectNodeId` order (sorted by `(kind, key)` — a
//! single file's placeholder `AppRef` is constant, so this is effectively
//! `(kind, key)`):
//!
//! 1. `kind` + `key` (the object's [`crate::program::ObjectNodeId`] identity — audit §4 item 1)
//! 2. `name`, lowercased — **found missing from the audit's item list while
//!    implementing this module; the audit doc's §4 item 2.1 now records it
//!    too, see below**
//! 3. `declared_id`
//! 4. `extends_target`
//! 5. `implements` (document order, as extracted)
//! 6. `source_table` + `source_table_temporary`
//! 7. `table_no`
//! 8. `page_controls` (document order)
//! 9. `fields` (document order)
//! 10. `dataitems` (document order)
//! 11. `parse_incomplete` (file-level parse-health flag)
//! 22. `name`, RAW-cased (see "LSP-surface additions" below — added
//!     alongside item 2's lowercased form, not instead of it)
//!
//! Then, for each routine of that object, in `RoutineNodeId` order (sorted by
//! `(name_lc, enclosing_member_lc, params_count, sig_fp)` — audit §4 item 3's
//! "SET of RoutineNodeIds" realized as this per-object sort key):
//!
//! 12. `name_lc`, `enclosing_member_lc`, `params_count`, `sig_fp` (the
//!     routine's [`crate::program::RoutineNodeId`] identity)
//! 13. `access`
//! 14. `event_subscribers` (source order)
//! 15. `subscriber_instance_manual`
//! 16. `publisher_kind`
//! 17. `include_sender`
//! 18. `return_type`
//! 19. `param_sig_key`
//! 20. per-parameter `(ty, by_ref)`, declaration order — read from
//!     `RoutineDecl.params` directly (SOURCE tier only), mirroring the
//!     audit's §3.2 finding that this is the one place the live resolver
//!     itself reads through to `RoutineDecl` rather than `RoutineNode`
//! 21. `parse_incomplete` (routine-level parse-health flag)
//! 23. `is_trigger` (see "LSP-surface additions" below)
//!
//! # Object `name`: a gap found here, now closed in the audit doc too
//!
//! The audit's own §2.2 read-table lists `ObjectNode::name` as a field
//! `graph.resolve_object`'s underlying index consults
//! (`src/program/graph.rs`'s `ObjectIndex::build` keys its by-name lookup on
//! `obj.name.fold_identifier()` for EVERY object, numbered or not) — but
//! §4's derived per-object field list, as first drafted, omitted it (item 1's
//! identity key only covers `declared_id`-or-name-key, which for a NUMBERED
//! object is `ObjKey::Id`, never the display name). Renaming a numbered
//! object (`codeunit 50100 "A"` -> `codeunit 50100 "B"`) therefore changes
//! what `Codeunit "B".Foo()` call sites elsewhere resolve to, without moving
//! item 1's identity key at all — a real false-negative risk. Found while
//! implementing this module (not proposed speculatively), independently
//! reviewed and CONFIRMED, and folded into the audit doc's own §4 item 2.1
//! (see that doc's §6.5) in the same review pass — so this is no longer a
//! deviation from §4, just the paper trail for how item 2.1 got there.
//!
//! # Excluded, per the audit's §4 "Explicitly EXCLUDED" list
//!
//! `decl.origin`/`decl.name_origin` (any span/position — body-extent-only,
//! never resolution-outcome-relevant), `tier` (structurally invariant per
//! file), every ABI-only `RoutineNode` field (always absent for a source
//! file), `abi_overload_collapsed`/`source_overload_aliased` (derived,
//! redundant with `params`), and "enum values" (verified NOT a real resolver
//! read — audit §2.3). Also excluded, not from the audit but by the same
//! reasoning: a parameter's declared NAME (only its `ty`/`by_ref` feed
//! arg-type dispatch).
//!
//! # LSP-surface additions (items 22/23) — a t3 whole-branch review finding,
//! dated 2026-07-13, NOT a correction to the audit's own reasoning
//!
//! The audit (and its §6.2 correction) is still correct on its own terms:
//! `RoutineNode::is_trigger` has zero RESOLUTION-path reads, and item 2's
//! lowercased `name` is sufficient for `graph.resolve_object`'s by-name
//! lookup. Both remain true. What the audit's scope never considered is a
//! SECOND class of reader that exists on this SAME `ProgramGraph`/
//! `ObjectNode`/`RoutineNode` data — the LSP SURFACE's DISPLAY code, added
//! by later T3 tasks after this audit was written:
//! `src/lsp/handlers.rs`'s `symbol_kind_for` reads `RoutineNode::is_trigger`
//! to classify a `CallHierarchyItem`'s `SymbolKind` (FUNCTION vs. EVENT), and
//! `object_name_for` reads the graph's RAW-cased `ObjectNode::name` for a
//! `CallHierarchyItem`'s `detail` text. Since rung 1 NEVER rebuilds `graph`
//! (it reuses `cur.graph` unchanged — see `src/lsp/updater.rs`'s module
//! doc), a change invisible to the fingerprint but visible to one of these
//! DISPLAY reads would leave a stale icon/detail string served until an
//! UNRELATED rung-2/3 happened to refresh the graph. Both fields are added
//! to the fingerprint for this reason — display-staleness prevention, not
//! resolution-soundness — which is why they're NEW items (22/23) appended
//! after the audit's original list, not edits to items 1/2's own reasoning.
//! Cost: an extra rung-2 rebuild on a procedure<->trigger flip or a
//! case-only object rename — both rare edits — is more than paid for by
//! closing this display-correctness gap.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;
use blake3::Hasher;

use crate::program::node_extract::{
    DataitemNode, FieldNode, ObjectRef, PageControlKind, PageControlNode,
};
use crate::program::resolve::event::{ParsedSubscriberArgs, PublisherKind};
use crate::program::{
    Access, AppRef, ObjKey, ObjectNode, ObjectNodeId, RoutineNode, RoutineNodeId, extract_nodes,
};
use crate::snapshot::ParsedFile;

/// A blake3 hash of one file's definition surface — see the module doc for
/// exactly what it covers and the canonical field order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DefSurface(pub [u8; 32]);

/// One routine's raw per-parameter dispatch data plus the routine-level
/// parse-health flag — read directly from `RoutineDecl` (which `RoutineNode`
/// does not carry either of), mirroring the audit's §3.2 finding.
struct RoutineRaw {
    /// `(declared type text, by-ref flag)` per parameter, declaration order.
    params: Vec<(Option<String>, bool)>,
    parse_incomplete: bool,
}

struct RoutineEntry {
    node: RoutineNode,
    raw: RoutineRaw,
}

/// Compute the definition-surface fingerprint of one parsed file.
///
/// See the module doc for the exact field list and order. Nothing here
/// consults a `ProgramGraph`/`ResolveIndex`/`DeclSurface` — the fingerprint is
/// computed from `pf`'s own freshly re-parsed IR only, so comparing an old
/// vs. new fingerprint never needs a rebuild of anything beyond the one file
/// being re-parsed.
#[must_use]
pub fn def_surface_fingerprint(pf: &ParsedFile) -> DefSurface {
    let mut object_nodes: Vec<ObjectNode> = Vec::new();
    let mut routine_nodes: Vec<RoutineNode> = Vec::new();
    extract_nodes(
        AppRef(0), // placeholder — a single file's own AppRef is never hashed (see item 1's kind+key)
        &pf.file,
        pf.provenance.tier,
        &mut object_nodes,
        &mut routine_nodes,
    );

    // `extract_nodes` walks `file.objects` then each object's `.routines` in
    // strict, UNFILTERED nested order — one `ObjectNode` pushed per object,
    // one `RoutineNode` pushed per routine, never skipped (its body has no
    // `continue`/filter anywhere in that nested loop). So re-walking the SAME
    // nested structure here, popping one entry off each flat Vec per step,
    // correlates positionally with zero risk of mismatch — this is how the
    // raw `RoutineDecl.params`/`parse_incomplete` data (which `RoutineNode`
    // doesn't carry) gets recovered without a second, independent extractor.
    let mut object_iter = object_nodes.into_iter();
    let mut routine_iter = routine_nodes.into_iter();

    let mut objects: Vec<(ObjectNode, Vec<RoutineEntry>)> =
        Vec::with_capacity(pf.file.objects.len());
    for obj_ir in &pf.file.objects {
        let object = object_iter
            .next()
            .expect("extract_nodes pushes exactly one ObjectNode per object, unconditionally");
        let mut routines = Vec::with_capacity(obj_ir.routines.len());
        for r_ir in &obj_ir.routines {
            let node = routine_iter.next().expect(
                "extract_nodes pushes exactly one RoutineNode per routine, unconditionally",
            );
            let raw = RoutineRaw {
                params: r_ir
                    .params
                    .iter()
                    .map(|p| (p.ty.clone(), p.by_ref))
                    .collect(),
                parse_incomplete: r_ir.parse_incomplete,
            };
            routines.push(RoutineEntry { node, raw });
        }
        objects.push((object, routines));
    }

    // Stable, deterministic order (independent of source declaration order —
    // see the audit's §4 "e.g. sorted by ObjectNodeId" suggestion).
    objects.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    for (_, routines) in &mut objects {
        routines.sort_by(|a, b| a.node.id.cmp(&b.node.id));
    }

    let mut h = Hasher::new();
    write_list(&mut h, &objects, |h, (obj, routines)| {
        write_object_identity(h, &obj.id); // 1
        write_str(h, &obj.name.fold_identifier()); // 2 (see module doc's "object name" section)
        write_opt_i64(h, obj.declared_id); // 3
        write_opt_str(h, &obj.extends_target); // 4
        write_list(h, &obj.implements, |h, s| write_str(h, s)); // 5
        write_opt_object_ref(h, &obj.source_table); // 6
        write_bool(h, obj.source_table_temporary); // 6 (cont'd)
        write_opt_object_ref(h, &obj.table_no); // 7
        write_list(h, &obj.page_controls, write_page_control); // 8
        write_list(h, &obj.fields, write_field); // 9
        write_list(h, &obj.dataitems, write_dataitem); // 10
        write_bool(h, obj.parse_incomplete); // 11
        write_str(h, &obj.name); // 22 (LSP-surface addition — see module doc)

        write_list(h, routines, |h, entry| {
            write_routine_identity(h, &entry.node.id); // 12
            write_tag(h, access_tag(entry.node.access)); // 13
            write_list(h, &entry.node.event_subscribers, write_subscriber); // 14
            write_bool(h, entry.node.subscriber_instance_manual); // 15
            write_opt_publisher_kind(h, &entry.node.publisher_kind); // 16
            write_opt_bool(h, entry.node.include_sender); // 17
            write_opt_str(h, &entry.node.return_type); // 18
            write_str(h, &entry.node.param_sig_key); // 19
            write_list(h, &entry.raw.params, |h, (ty, by_ref)| {
                // 20
                write_opt_str(h, ty);
                write_bool(h, *by_ref);
            });
            write_bool(h, entry.raw.parse_incomplete); // 21
            write_bool(h, entry.node.is_trigger); // 23 (LSP-surface addition — see module doc)
        });
    });

    DefSurface(*h.finalize().as_bytes())
}

// ---------------------------------------------------------------------------
// Canonical encoding primitives
// ---------------------------------------------------------------------------

fn write_tag(h: &mut Hasher, tag: u8) {
    h.update(&[tag]);
}

fn write_bool(h: &mut Hasher, b: bool) {
    write_tag(h, u8::from(b));
}

fn write_u64(h: &mut Hasher, v: u64) {
    h.update(&v.to_le_bytes());
}

fn write_i64(h: &mut Hasher, v: i64) {
    h.update(&v.to_le_bytes());
}

fn write_str(h: &mut Hasher, s: &str) {
    write_u64(h, s.len() as u64);
    h.update(s.as_bytes());
}

fn write_opt_str(h: &mut Hasher, s: &Option<String>) {
    match s {
        Some(v) => {
            write_tag(h, 1);
            write_str(h, v);
        }
        None => write_tag(h, 0),
    }
}

fn write_opt_i64(h: &mut Hasher, v: Option<i64>) {
    match v {
        Some(n) => {
            write_tag(h, 1);
            write_i64(h, n);
        }
        None => write_tag(h, 0),
    }
}

fn write_opt_bool(h: &mut Hasher, v: Option<bool>) {
    match v {
        Some(b) => {
            write_tag(h, 1);
            write_bool(h, b);
        }
        None => write_tag(h, 0),
    }
}

/// Length-prefix (`u64` LE count) then hash each item via `f`.
fn write_list<T>(h: &mut Hasher, items: &[T], mut f: impl FnMut(&mut Hasher, &T)) {
    write_u64(h, items.len() as u64);
    for item in items {
        f(h, item);
    }
}

fn write_obj_key(h: &mut Hasher, key: &ObjKey) {
    match key {
        ObjKey::Id(n) => {
            write_tag(h, 0);
            write_i64(h, *n);
        }
        ObjKey::Name(s) => {
            write_tag(h, 1);
            write_str(h, s);
        }
    }
}

fn object_kind_tag(k: ObjectKind) -> u8 {
    match k {
        ObjectKind::Codeunit => 0,
        ObjectKind::Table => 1,
        ObjectKind::TableExtension => 2,
        ObjectKind::Page => 3,
        ObjectKind::PageExtension => 4,
        ObjectKind::Report => 5,
        ObjectKind::ReportExtension => 6,
        ObjectKind::Query => 7,
        ObjectKind::XmlPort => 8,
        ObjectKind::Enum => 9,
        ObjectKind::EnumExtension => 10,
        ObjectKind::Interface => 11,
        ObjectKind::ControlAddIn => 12,
        ObjectKind::Entitlement => 13,
        ObjectKind::PermissionSet => 14,
        ObjectKind::PermissionSetExtension => 15,
        ObjectKind::Profile => 16,
        ObjectKind::Other => 17,
    }
}

fn write_object_identity(h: &mut Hasher, id: &ObjectNodeId) {
    write_tag(h, object_kind_tag(id.kind));
    write_obj_key(h, &id.key);
}

fn write_routine_identity(h: &mut Hasher, id: &RoutineNodeId) {
    write_str(h, &id.name_lc);
    write_opt_str(h, &id.enclosing_member_lc);
    write_u64(h, id.params_count as u64);
    write_u64(h, id.sig_fp);
}

fn write_object_ref(h: &mut Hasher, r: &ObjectRef) {
    match r {
        ObjectRef::Id(n) => {
            write_tag(h, 0);
            write_i64(h, *n);
        }
        ObjectRef::Name { raw, normalized_lc } => {
            write_tag(h, 1);
            write_str(h, raw);
            write_str(h, normalized_lc);
        }
    }
}

fn write_opt_object_ref(h: &mut Hasher, r: &Option<ObjectRef>) {
    match r {
        Some(v) => {
            write_tag(h, 1);
            write_object_ref(h, v);
        }
        None => write_tag(h, 0),
    }
}

fn page_control_kind_tag(k: PageControlKind) -> u8 {
    match k {
        PageControlKind::Part => 0,
        PageControlKind::SystemPart => 1,
        PageControlKind::UserControl => 2,
    }
}

fn write_page_control(h: &mut Hasher, pc: &PageControlNode) {
    write_str(h, &pc.name_lc);
    write_tag(h, page_control_kind_tag(pc.kind));
    write_object_ref(h, &pc.target);
}

fn write_field(h: &mut Hasher, f: &FieldNode) {
    write_str(h, &f.name_lc);
    write_str(h, &f.type_text);
}

fn write_dataitem(h: &mut Hasher, d: &DataitemNode) {
    write_str(h, &d.name_lc);
    write_str(h, &d.name);
    write_object_ref(h, &d.source_table);
}

fn access_tag(a: Access) -> u8 {
    match a {
        Access::Public => 0,
        Access::Local => 1,
        Access::Internal => 2,
        Access::Protected => 3,
    }
}

fn publisher_kind_tag(k: PublisherKind) -> u8 {
    match k {
        PublisherKind::Integration => 0,
        PublisherKind::Business => 1,
        PublisherKind::Internal => 2,
        PublisherKind::Platform => 3,
    }
}

fn write_opt_publisher_kind(h: &mut Hasher, k: &Option<PublisherKind>) {
    match k {
        Some(v) => {
            write_tag(h, 1);
            write_tag(h, publisher_kind_tag(*v));
        }
        None => write_tag(h, 0),
    }
}

fn write_subscriber(h: &mut Hasher, s: &ParsedSubscriberArgs) {
    write_str(h, &s.publisher_object_type);
    write_str(h, &s.publisher_name);
    match s.publisher_id {
        Some(n) => {
            write_tag(h, 1);
            write_i64(h, n);
        }
        None => write_tag(h, 0),
    }
    write_str(h, &s.event_name);
    write_opt_str(h, &s.element);
    write_bool(h, s.skip_on_missing_license);
    write_bool(h, s.skip_on_missing_permission);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{AppId, Provenance, TrustTier};

    fn parsed_file(src: &str) -> ParsedFile {
        ParsedFile {
            virtual_path: "Test.al".to_string(),
            file: std::sync::Arc::new(al_syntax::parse(src)),
            provenance: Provenance {
                app: AppId {
                    guid: String::new(),
                    name: "Test".to_string(),
                    publisher: "P".to_string(),
                    version: "1.0.0.0".to_string(),
                },
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            },
            text: src.into(),
        }
    }

    fn fp(src: &str) -> DefSurface {
        def_surface_fingerprint(&parsed_file(src))
    }

    // ── determinism (Step 3) ─────────────────────────────────────────────

    #[test]
    fn same_source_parsed_twice_fingerprints_identically() {
        let src = r#"
codeunit 50100 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        assert_eq!(fp(src), fp(src));
    }

    // ── item 1: the SET of ObjectNodeIds (add/remove/re-id) ──────────────

    #[test]
    fn object_added_changes_fingerprint() {
        let base = r#"
codeunit 50100 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
codeunit 50101 "Other Helper"
{
}
"#;
        assert_ne!(fp(base), fp(variant), "an object added must move item 1");
    }

    #[test]
    fn declared_id_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50101 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "a changed declared_id changes both the ObjKey (item 1) and item 2's declared_id"
        );
    }

    // ── addition beyond §4's literal text: object `name` ─────────────────

    #[test]
    fn object_renamed_with_same_numeric_id_changes_fingerprint() {
        let base = r#"
codeunit 50100 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 "Renamed Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "renaming a numbered object must move the fingerprint: graph.rs's \
             ObjectIndex::build keys graph.resolve_object's by-name lookup on \
             obj.name for EVERY object kind, numeric id or not — see the \
             module doc's 'one addition' section"
        );
    }

    // ── item 2.2: extends_target ──────────────────────────────────────────

    #[test]
    fn extends_target_changed_changes_fingerprint() {
        let base = r#"
tableextension 50100 "Sales Ext" extends Customer
{
}
"#;
        let variant = r#"
tableextension 50100 "Sales Ext" extends Vendor
{
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.3: implements ──────────────────────────────────────────────

    #[test]
    fn implements_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 "Impl" implements IFoo
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 "Impl" implements IBar
{
    procedure Foo()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.6: source_table + source_table_temporary ──────────────────

    #[test]
    fn source_table_changed_changes_fingerprint() {
        let base = r#"
page 50100 "Card"
{
    SourceTable = Customer;
    layout { area(Content) { } }
}
"#;
        let variant = r#"
page 50100 "Card"
{
    SourceTable = Vendor;
    layout { area(Content) { } }
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    #[test]
    fn source_table_temporary_flip_changes_fingerprint() {
        let base = r#"
page 50100 "Card"
{
    SourceTable = Customer;
    layout { area(Content) { } }
}
"#;
        let variant = r#"
page 50100 "Card"
{
    SourceTable = Customer, Temporary;
    layout { area(Content) { } }
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.7: table_no ────────────────────────────────────────────────

    #[test]
    fn table_no_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 "Item Helper"
{
    TableNo = Item;
}
"#;
        let variant = r#"
codeunit 50100 "Item Helper"
{
    TableNo = Customer;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.8: page_controls ───────────────────────────────────────────

    #[test]
    fn page_controls_changed_changes_fingerprint() {
        let base = r#"
page 50100 "Card"
{
    SourceTable = Customer;
    layout { area(Content) { } }
}
"#;
        let variant = r#"
page 50100 "Card"
{
    SourceTable = Customer;
    layout { area(Content) { part(Lines; "Sales Line Subform") { } } }
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.9: fields (added + type changed) ───────────────────────────

    #[test]
    fn table_field_added_changes_fingerprint() {
        let base = r#"
table 50100 "Plain Table"
{
    fields { field(1; "No."; Code[20]) { } }
}
"#;
        let variant = r#"
table 50100 "Plain Table"
{
    fields {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[50]) { }
    }
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    #[test]
    fn table_field_type_changed_changes_fingerprint() {
        let base = r#"
table 50100 "Plain Table"
{
    fields { field(1; "No."; Code[20]) { } }
}
"#;
        let variant = r#"
table 50100 "Plain Table"
{
    fields { field(1; "No."; Code[10]) { } }
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.10: dataitems ──────────────────────────────────────────────

    #[test]
    fn report_dataitem_source_table_changed_changes_fingerprint() {
        let base = r#"
report 50100 T
{
    dataset
    {
        dataitem(Cust; Customer)
        {
        }
    }
}
"#;
        let variant = r#"
report 50100 T
{
    dataset
    {
        dataitem(Cust; Vendor)
        {
        }
    }
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 2.11: parse_incomplete (file-level) ──────────────────────────

    #[test]
    fn recovered_parse_status_changes_fingerprint() {
        let clean = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let broken = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
#if NEVER_CLOSED
        Bar();
    end;
}
"#;
        assert_ne!(fp(clean), fp(broken));
    }

    // ── item 3: the SET of RoutineNodeIds (add/remove/rename/re-arity) ───

    #[test]
    fn routine_added_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;

    procedure Bar()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    #[test]
    fn routine_renamed_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Renamed()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    #[test]
    fn routine_arity_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo(X: Integer)
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    #[test]
    fn routine_param_type_changed_same_arity_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo(X: Integer)
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo(X: Text)
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 20: per-parameter by_ref ("var-ness flipped") ────────────────

    #[test]
    fn param_by_ref_flip_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo(X: Record Customer)
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo(var X: Record Customer)
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 13: access/visibility ─────────────────────────────────────────

    #[test]
    fn access_modifier_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    local procedure Foo()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 18: return_type ───────────────────────────────────────────────

    #[test]
    fn return_type_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo(): Boolean
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo(): Integer
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 14: event_subscribers ─────────────────────────────────────────

    #[test]
    fn event_subscriber_attribute_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterX', '', false, false)]
    local procedure OnAfterX()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnAfterY', '', false, false)]
    local procedure OnAfterX()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 15: subscriber_instance_manual ────────────────────────────────

    #[test]
    fn event_subscriber_instance_manual_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 Sub
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 Sub
{
    EventSubscriberInstance = Manual;

    procedure Foo()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 16: publisher_kind ─────────────────────────────────────────────

    #[test]
    fn publisher_kind_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnAfterX()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 Pub
{
    [BusinessEvent(false)]
    procedure OnAfterX()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── item 17: include_sender ──────────────────────────────────────────────

    #[test]
    fn include_sender_changed_changes_fingerprint() {
        let base = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnAfterX()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(true, false)]
    procedure OnAfterX()
    begin
    end;
}
"#;
        assert_ne!(fp(base), fp(variant));
    }

    // ── exclusion tests: fields §4 says must NOT move the fingerprint ──────

    #[test]
    fn body_only_statement_added_does_not_change_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
        Message('hello');
    end;
}
"#;
        assert_eq!(
            fp(base),
            fp(variant),
            "a body-only statement edit must be rung-1 safe (equal fingerprint)"
        );
    }

    #[test]
    fn local_variable_added_does_not_change_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    var
        MyInt: Integer;
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo()
    var
        MyInt: Integer;
        MyText: Text;
    begin
    end;
}
"#;
        assert_eq!(
            fp(base),
            fp(variant),
            "a routine's LOCAL variables are never resolution-relevant to another file"
        );
    }

    #[test]
    fn comment_and_whitespace_only_change_does_not_change_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    // a new comment, and extra blank lines below


    procedure Foo()
    begin
    end;
}
"#;
        assert_eq!(
            fp(base),
            fp(variant),
            "span/position data is explicitly excluded (audit §3.4/§4) — a \
             pure whitespace/comment shift must never move the fingerprint"
        );
    }

    #[test]
    fn enum_value_added_does_not_change_fingerprint() {
        let base = r#"
enum 50100 "Probe Kind"
{
    Extensible = true;
    value(0; Open) { }
}
"#;
        let variant = r#"
enum 50100 "Probe Kind"
{
    Extensible = true;
    value(0; Open) { }
    value(1; Closed) { }
}
"#;
        assert_eq!(
            fp(base),
            fp(variant),
            "enum VALUES are not a real resolver read (audit §2.3) — only the \
             enum TYPE's own identity (an ordinary ObjectNode) is fingerprinted"
        );
    }

    #[test]
    fn param_name_only_change_does_not_change_fingerprint() {
        let base = r#"
codeunit 50100 T
{
    procedure Foo(X: Integer)
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo(Y: Integer)
    begin
    end;
}
"#;
        assert_eq!(
            fp(base),
            fp(variant),
            "a parameter's NAME is never part of arg-type dispatch — item 20 \
             hashes only (ty, by_ref) per parameter, never the declared name"
        );
    }

    // ── review fix-wave: isolation tests (deletion-probe verified) ────────
    //
    // The Task 7 review deleted item 20's `write_list` call (raw per-
    // parameter `ty`/`by_ref`) and item 21's `write_bool` call (routine-level
    // `parse_incomplete`) and found ALL 30 prior tests stayed green — every
    // existing param-type-change fixture also happened to move `sig_fp`
    // (item 12) and/or `param_sig_key` (item 19), and no prior fixture
    // isolated a routine's OWN `parse_incomplete` from the file-level one
    // (item 11). The three tests below are deliberately constructed so that
    // ONLY the targeted item differs between base/variant — each was run
    // against a temporarily-neutered `def_surface_fingerprint` (the relevant
    // write call commented out) and confirmed to FAIL before being restored;
    // see the fix-wave section of `task-7-report.md` for the probe output.

    #[test]
    fn param_type_case_only_change_moves_fingerprint_via_raw_param_text() {
        // `sig_fp` (item 12, via `source_param_sig_fp`/`normalize_type_text`)
        // and `param_sig_key` (item 19, `trim().to_ascii_lowercase()`) both
        // fold case away, so a CASE-ONLY change to a parameter's declared
        // type text is invisible to both. Only item 20's raw `(ty, by_ref)`
        // read — straight from `RoutineDecl.params`, never normalized — can
        // catch it. Varying only the subtype identifier's case (`Customer`
        // vs `CUSTOMER`), never the `Record` keyword itself, keeps this a
        // pure case difference with zero parsing-ambiguity risk.
        let base = r#"
codeunit 50100 T
{
    procedure Foo(X: Record Customer)
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Foo(X: Record CUSTOMER)
    begin
    end;
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "a case-only param type-text change must move the fingerprint \
             even though sig_fp/param_sig_key both normalize case away — \
             only item 20's raw (ty, by_ref) read catches this"
        );
    }

    #[test]
    fn routine_level_parse_incomplete_isolated_from_file_level() {
        // `Broken` carries a PERMANENT local body error (a dangling `+`
        // operator with no right operand — a genuine grammar error tree-
        // sitter recovers from locally, unlike an unbalanced `#if`, which
        // was probed and found to swallow the enclosing routine's own
        // recognition entirely, not just flag it). `Broken` is IDENTICAL,
        // byte-for-byte, in both base and variant, so the file-level
        // parse_incomplete (item 11, driven by the file root's `has_error`)
        // stays `true` in BOTH versions via `Broken` alone — held constant.
        // `Target` is clean in base and gets the SAME local-error shape in
        // variant: its header (name/params/return type) is unaffected (the
        // error is confined to its own body), so only `Target`'s own
        // routine-level parse_incomplete (item 21) flips.
        let base = r#"
codeunit 50100 T
{
    procedure Target()
    begin
    end;

    procedure Broken()
    begin
        X := 1 +;
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    procedure Target()
    begin
        X := 1 +;
    end;

    procedure Broken()
    begin
        X := 1 +;
    end;
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "Target's OWN parse_incomplete flipping must move the \
             fingerprint even though the file-level parse_incomplete (item \
             11, via Broken's unchanged permanent error) stays true in both \
             — only item 21's routine-level read catches this"
        );
    }

    #[test]
    fn id_less_object_renamed_changes_fingerprint_via_obj_key_name_arm() {
        // An Interface object has no numeric id (`ObjectDecl.id == None`),
        // so its `ObjectNodeId::key` is `ObjKey::Name(name_lc)` — exercising
        // `write_obj_key`'s `Name` branch (the numbered-object tests above
        // only ever exercise the `Id` branch).
        let base = r#"
interface "My Interface"
{
    procedure Foo();
}
"#;
        let variant = r#"
interface "Renamed Interface"
{
    procedure Foo();
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "renaming an id-less object changes its ObjKey::Name identity \
             (item 1) directly"
        );
    }

    // ── t3 whole-branch review: LSP-surface display-relevant additions ────
    // (items 22/23 — see the module doc's "LSP-surface additions" section)

    #[test]
    fn is_trigger_flip_changes_fingerprint() {
        // Same name, same arity — `RoutineNodeId` (name_lc/enclosing_member_lc/
        // params_count/sig_fp) is IDENTICAL between `procedure OnRun()` and
        // `trigger OnRun()`, and no OTHER existing fingerprint field moves
        // either (access/event_subscribers/publisher_kind/include_sender/
        // return_type/param_sig_key/params/parse_incomplete are all
        // unaffected by a bare kind flip) — so before this fix, flipping a
        // routine between Procedure and Trigger was INVISIBLE to the
        // fingerprint, leaving `src/lsp/handlers.rs`'s `symbol_kind_for`
        // (which reads `RoutineNode::is_trigger` for the CallHierarchyItem's
        // icon/kind) serving a STALE classification across rung 1 (which
        // never rebuilds `graph`) until an unrelated rung-2/3 happened to
        // refresh it.
        let base = r#"
codeunit 50100 T
{
    procedure OnRun()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 T
{
    trigger OnRun()
    begin
    end;
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "a procedure<->trigger kind flip (same name/arity) must move the \
             fingerprint — symbol_kind_for's is_trigger read would otherwise \
             go stale across rung 1"
        );
    }

    #[test]
    fn object_renamed_case_only_changes_fingerprint() {
        // Item 2's existing `name.to_ascii_lowercase()` write folds case
        // away entirely, so a CASE-ONLY rename of a NUMBERED object (its
        // `ObjKey::Id` identity — item 1 — never moves either) was
        // previously invisible to the fingerprint, leaving
        // `src/lsp/handlers.rs`'s `object_name_for` (which reads the graph's
        // RAW-cased `ObjectNode::name` for a CallHierarchyItem's `detail`
        // text) serving a stale display name across rung 1.
        let base = r#"
codeunit 50100 "Sales Helper"
{
    procedure Post()
    begin
    end;
}
"#;
        let variant = r#"
codeunit 50100 "SALES HELPER"
{
    procedure Post()
    begin
    end;
}
"#;
        assert_ne!(
            fp(base),
            fp(variant),
            "a case-only object rename (same numeric id, same lowercased \
             name) must move the fingerprint — object_name_for's raw-cased \
             read would otherwise go stale across rung 1"
        );
    }
}
