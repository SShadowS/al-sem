//! The change-type classifier of the context-server design (§6.1): one
//! **declaration projection** per workspace file, and the class of a revision
//! (B body-only, S declaration — F file add/remove/rename is part of S —,
//! P package or app identity).
//!
//! `DeclProjection(file)` is blake3 over a canonical encoding, in document order
//! with multiplicity, of every [`ObjectNode`], every [`RoutineNode`] and every
//! [`RoutineMeta`] the file's extraction yields ([`summarize_file`], the one
//! extraction the build itself uses), including every field of every type and
//! nested type, except [`EXCLUDED`]: the positional origins and
//! `RoutineNode.source_unit`.
//!
//! The encoding is the [`Classify`] trait, implemented for every type by
//! exhaustive destructuring with no `..`: a new field is a compile error until
//! its author hashes it here or adds it to [`EXCLUDED`] and bumps
//! [`DECL_PROJECTION_VERSION`]. Serde attributes play no part, and neither does
//! the LSP's `DefSurface` (a different question, and it leaves out fields the
//! resolver reads).

use al_syntax::ir::{AlFile, ObjectKind};

use crate::engine::deps::symbol_reference::SubtypeTag;
use crate::program::dep_summary::summarize_file;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{
    AbiParamRetained, AbiParams, Access, DataitemNode, FieldNode, ObjectNode, ObjectRef,
    PageControlKind, PageControlNode, QueryColumnNode, RoutineNode,
};
use crate::program::resolve::decl_surface::{ParamMeta, RoutineMeta};
use crate::program::resolve::edge::{AbiEventKind, AbiRoutineKind};
use crate::program::resolve::event::{ParsedSubscriberArgs, PublisherKind};
use crate::program::str_pool::SharedStr;
use crate::snapshot::TrustTier;

/// Bumped whenever the encoding or [`EXCLUDED`] changes. Hashed first, so two
/// versions never compare equal.
pub const DECL_PROJECTION_VERSION: u32 = 1;

/// The fields the projection leaves out, and only these: the positions of a
/// declaration (they move with every edit above it; the location domains of
/// §6.5 carry them) and the build-local source-unit stamp.
pub const EXCLUDED: &[&str] = &[
    "RoutineMeta.origin",
    "RoutineMeta.name_origin",
    "RoutineNode.source_unit",
];

/// The canonical byte sink of the projection: every value is written with a
/// type tag or a length, so two different sequences cannot encode alike.
pub struct ProjectionHasher(blake3::Hasher);

impl ProjectionHasher {
    fn new() -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"al-sem-decl-projection");
        h.update(&DECL_PROJECTION_VERSION.to_le_bytes());
        ProjectionHasher(h)
    }

    /// A sink for another digest over the same encoding (the S rule's answer
    /// digests, `session::dep_record`): `domain` keeps the two apart.
    pub(crate) fn with_domain(domain: &[u8]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(domain);
        h.update(&DECL_PROJECTION_VERSION.to_le_bytes());
        ProjectionHasher(h)
    }

    pub(crate) fn finish_bytes(self) -> [u8; 32] {
        *self.0.finalize().as_bytes()
    }

    pub(crate) fn tag(&mut self, t: u8) {
        self.0.update(&[t]);
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.0.update(&v.to_le_bytes());
    }

    fn i64(&mut self, v: i64) {
        self.0.update(&v.to_le_bytes());
    }

    fn len(&mut self, n: usize) {
        self.u64(n as u64);
    }

    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.0.update(s.as_bytes());
    }

    fn finish(self) -> DeclProjection {
        DeclProjection(*self.0.finalize().as_bytes())
    }
}

/// One file's declaration projection.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeclProjection(pub [u8; 32]);

/// A type the projection encodes. Every implementation destructures its value
/// exhaustively (no `..`); see the module doc.
pub trait Classify {
    fn classify(&self, h: &mut ProjectionHasher);
}

impl Classify for bool {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(u8::from(*self));
    }
}

impl Classify for i64 {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.i64(*self);
    }
}

impl Classify for i32 {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.i64(i64::from(*self));
    }
}

impl Classify for usize {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.u64(*self as u64);
    }
}

impl Classify for u64 {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.u64(*self);
    }
}

impl Classify for str {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.str(self);
    }
}

impl Classify for String {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.str(self);
    }
}

impl Classify for SharedStr {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.str(self.as_str());
    }
}

impl<T: Classify> Classify for Option<T> {
    fn classify(&self, h: &mut ProjectionHasher) {
        match self {
            None => h.tag(0),
            Some(v) => {
                h.tag(1);
                v.classify(h);
            }
        }
    }
}

impl<T: Classify> Classify for [T] {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.len(self.len());
        for v in self {
            v.classify(h);
        }
    }
}

impl<T: Classify> Classify for Vec<T> {
    fn classify(&self, h: &mut ProjectionHasher) {
        self.as_slice().classify(h);
    }
}

impl<T: Classify> Classify for Box<[T]> {
    fn classify(&self, h: &mut ProjectionHasher) {
        (**self).classify(h);
    }
}

impl<A: Classify, B: Classify> Classify for (A, B) {
    fn classify(&self, h: &mut ProjectionHasher) {
        let (a, b) = self;
        a.classify(h);
        b.classify(h);
    }
}

impl Classify for AppRef {
    fn classify(&self, h: &mut ProjectionHasher) {
        let AppRef(n) = self;
        h.u64(u64::from(*n));
    }
}

impl Classify for ObjectKind {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
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
        });
    }
}

impl Classify for ObjKey {
    fn classify(&self, h: &mut ProjectionHasher) {
        match self {
            ObjKey::Id(n) => {
                h.tag(0);
                h.i64(*n);
            }
            ObjKey::Name(s) => {
                h.tag(1);
                s.classify(h);
            }
        }
    }
}

impl Classify for ObjectNodeId {
    fn classify(&self, h: &mut ProjectionHasher) {
        let ObjectNodeId { app, kind, key } = self;
        app.classify(h);
        kind.classify(h);
        key.classify(h);
    }
}

impl Classify for RoutineNodeId {
    fn classify(&self, h: &mut ProjectionHasher) {
        let RoutineNodeId {
            object,
            name_lc,
            enclosing_member_lc,
            params_count,
            sig_fp,
        } = self;
        object.classify(h);
        name_lc.classify(h);
        enclosing_member_lc.classify(h);
        params_count.classify(h);
        sig_fp.classify(h);
    }
}

impl Classify for TrustTier {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            TrustTier::Workspace => 0,
            TrustTier::EmbeddedSource => 1,
            TrustTier::LocalSourceVerified => 2,
            TrustTier::LocalSourceApproximate => 3,
            TrustTier::SymbolOnly => 4,
        });
    }
}

impl Classify for Access {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            Access::Public => 0,
            Access::Local => 1,
            Access::Internal => 2,
            Access::Protected => 3,
        });
    }
}

impl Classify for ObjectRef {
    fn classify(&self, h: &mut ProjectionHasher) {
        match self {
            ObjectRef::Name { raw, normalized_lc } => {
                h.tag(0);
                raw.classify(h);
                normalized_lc.classify(h);
            }
            ObjectRef::Id(n) => {
                h.tag(1);
                h.i64(*n);
            }
        }
    }
}

impl Classify for PageControlKind {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            PageControlKind::Part => 0,
            PageControlKind::SystemPart => 1,
            PageControlKind::UserControl => 2,
        });
    }
}

impl Classify for PageControlNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let PageControlNode {
            name_lc,
            kind,
            target,
        } = self;
        name_lc.classify(h);
        kind.classify(h);
        target.classify(h);
    }
}

impl Classify for FieldNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let FieldNode {
            name_lc,
            type_text,
            number,
        } = self;
        name_lc.classify(h);
        type_text.classify(h);
        number.classify(h);
    }
}

impl Classify for DataitemNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let DataitemNode {
            name_lc,
            name,
            source_table,
        } = self;
        name_lc.classify(h);
        name.classify(h);
        source_table.classify(h);
    }
}

impl Classify for QueryColumnNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let QueryColumnNode {
            name_lc,
            source_table,
            field_lc,
        } = self;
        name_lc.classify(h);
        source_table.classify(h);
        field_lc.classify(h);
    }
}

impl Classify for ObjectNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let ObjectNode {
            id,
            name,
            declared_id,
            extends_target,
            implements,
            tier,
            source_table,
            table_no,
            source_table_temporary,
            page_controls,
            fields,
            dataitems,
            query_columns,
            protected_vars,
            parse_incomplete,
            closed_enum,
            internal,
            enum_impls,
            lookup_pages,
            obsolete_gone,
            page_ui,
            dataset,
        } = self;
        h.tag(b'O');
        id.classify(h);
        name.classify(h);
        declared_id.classify(h);
        extends_target.classify(h);
        implements.classify(h);
        tier.classify(h);
        source_table.classify(h);
        table_no.classify(h);
        source_table_temporary.classify(h);
        page_controls.classify(h);
        fields.classify(h);
        dataitems.classify(h);
        query_columns.classify(h);
        protected_vars.classify(h);
        parse_incomplete.classify(h);
        closed_enum.classify(h);
        internal.classify(h);
        enum_impls.classify(h);
        lookup_pages.classify(h);
        obsolete_gone.classify(h);
        page_ui.classify(h);
        dataset.classify(h);
    }
}

impl Classify for crate::program::node_extract::DatasetNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let crate::program::node_extract::DatasetNode {
            known,
            items,
            may_import,
            may_export,
            request_page_hidden,
        } = self;
        known.classify(h);
        may_import.classify(h);
        may_export.classify(h);
        request_page_hidden.classify(h);
        h.u64(items.len() as u64);
        for item in items {
            let crate::program::node_extract::DatasetItemNode {
                kind,
                name,
                source_table,
                use_temporary,
                auto_save,
                auto_update,
                auto_replace,
            } = item;
            use crate::program::node_extract::DatasetItemKind as K;
            h.tag(match kind {
                K::ReportDataitem => 0,
                K::XmlPortTableElement => 1,
                K::QueryDataitem => 2,
            });
            name.classify(h);
            source_table.classify(h);
            use_temporary.classify(h);
            auto_save.classify(h);
            auto_update.classify(h);
            auto_replace.classify(h);
        }
    }
}

impl Classify for crate::program::page_ui::PageUiDecl {
    fn classify(&self, h: &mut ProjectionHasher) {
        let crate::program::page_ui::PageUiDecl {
            editable_false,
            insert_false,
            modify_false,
            delete_false,
            editable_setters,
        } = self;
        editable_false.classify(h);
        insert_false.classify(h);
        modify_false.classify(h);
        delete_false.classify(h);
        h.u64(editable_setters.len() as u64);
        for s in editable_setters {
            use crate::program::page_ui::EditableSetter;
            match s {
                EditableSetter::CurrPage => h.tag(0),
                EditableSetter::Page(r) => {
                    h.tag(1);
                    r.classify(h);
                }
                EditableSetter::Part(name) => {
                    h.tag(2);
                    name.classify(h);
                }
                EditableSetter::Unknown => h.tag(3),
            }
        }
    }
}

impl Classify for ParsedSubscriberArgs {
    fn classify(&self, h: &mut ProjectionHasher) {
        let ParsedSubscriberArgs {
            publisher_object_type,
            publisher_name,
            publisher_id,
            event_name,
            element,
            skip_on_missing_license,
            skip_on_missing_permission,
        } = self;
        publisher_object_type.classify(h);
        publisher_name.classify(h);
        publisher_id.classify(h);
        event_name.classify(h);
        element.classify(h);
        skip_on_missing_license.classify(h);
        skip_on_missing_permission.classify(h);
    }
}

impl Classify for PublisherKind {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            PublisherKind::Integration => 0,
            PublisherKind::Business => 1,
            PublisherKind::Internal => 2,
            PublisherKind::Platform => 3,
        });
    }
}

impl Classify for AbiRoutineKind {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            AbiRoutineKind::Procedure => 0,
            AbiRoutineKind::EventPublisher => 1,
            AbiRoutineKind::EventSubscriber => 2,
        });
    }
}

impl Classify for AbiEventKind {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            AbiEventKind::None => 0,
            AbiEventKind::Integration => 1,
            AbiEventKind::Business => 2,
            AbiEventKind::Internal => 3,
        });
    }
}

impl Classify for SubtypeTag {
    fn classify(&self, h: &mut ProjectionHasher) {
        h.tag(match self {
            SubtypeTag::NoTypeDefinition => 0,
            SubtypeTag::NoName => 1,
            SubtypeTag::AlreadyQuoted => 2,
            SubtypeTag::NoSubtype => 3,
            SubtypeTag::Full => 4,
            SubtypeTag::NameQuoted => 5,
            SubtypeTag::IdOnly => 6,
            SubtypeTag::EmptySubtype => 7,
        });
    }
}

impl Classify for AbiParamRetained {
    fn classify(&self, h: &mut ProjectionHasher) {
        let AbiParamRetained {
            name,
            type_text,
            is_var,
            is_temporary,
            subtype_id,
            subtype_raw_name,
            subtype_tag,
            array_dims,
        } = self;
        name.classify(h);
        type_text.classify(h);
        is_var.classify(h);
        is_temporary.classify(h);
        subtype_id.classify(h);
        subtype_raw_name.classify(h);
        subtype_tag.classify(h);
        array_dims.classify(h);
    }
}

impl Classify for AbiParams {
    fn classify(&self, h: &mut ProjectionHasher) {
        match self {
            AbiParams::Complete(params) => {
                h.tag(0);
                params.classify(h);
            }
            AbiParams::Missing => h.tag(1),
            AbiParams::CollapsedUntrusted => h.tag(2),
        }
    }
}

impl Classify for RoutineNode {
    fn classify(&self, h: &mut ProjectionHasher) {
        let RoutineNode {
            id,
            name,
            is_trigger,
            access,
            tier,
            event_subscribers,
            subscriber_instance_manual,
            publisher_kind,
            include_sender,
            publisher_isolated,
            abi_routine_kind,
            abi_event_kind,
            param_sig_key,
            return_type,
            return_type_id,
            abi_overload_collapsed,
            source_overload_aliased,
            preproc_context,
            // EXCLUDED: a build-local index (see `EXCLUDED`).
            source_unit: _,
            abi_params,
            abi_method_id,
        } = self;
        h.tag(b'R');
        id.classify(h);
        name.classify(h);
        is_trigger.classify(h);
        access.classify(h);
        tier.classify(h);
        event_subscribers.classify(h);
        subscriber_instance_manual.classify(h);
        publisher_kind.classify(h);
        include_sender.classify(h);
        publisher_isolated.classify(h);
        abi_routine_kind.classify(h);
        abi_event_kind.classify(h);
        param_sig_key.classify(h);
        return_type.classify(h);
        return_type_id.classify(h);
        abi_overload_collapsed.classify(h);
        source_overload_aliased.classify(h);
        preproc_context.classify(h);
        abi_params.classify(h);
        abi_method_id.classify(h);
    }
}

impl Classify for ParamMeta {
    fn classify(&self, h: &mut ProjectionHasher) {
        let ParamMeta { name, ty, by_ref } = self;
        name.classify(h);
        ty.classify(h);
        by_ref.classify(h);
    }
}

impl Classify for RoutineMeta {
    fn classify(&self, h: &mut ProjectionHasher) {
        let RoutineMeta {
            name,
            enclosing_member,
            parse_incomplete,
            params,
            // EXCLUDED: positions (see `EXCLUDED`).
            origin: _,
            name_origin: _,
            virtual_path,
            method_id,
            try_function,
        } = self;
        h.tag(b'M');
        name.classify(h);
        enclosing_member.classify(h);
        parse_incomplete.classify(h);
        params.classify(h);
        virtual_path.classify(h);
        method_id.classify(h);
        try_function.classify(h);
    }
}

/// The declaration projection of one parsed workspace file of app `app`: what
/// [`summarize_file`] extracts (the build's own extraction), objects, routines
/// and routine metadata, each in document order with multiplicity.
#[must_use]
pub fn decl_projection(
    app: AppRef,
    tier: TrustTier,
    virtual_path: &str,
    file: &AlFile,
) -> DeclProjection {
    let packed = summarize_file(app, tier, virtual_path, file);
    let mut h = ProjectionHasher::new();
    h.tag(b'F');
    packed.objects.classify(&mut h);
    packed.routines.classify(&mut h);
    h.len(packed.routine_meta.len());
    for (id, meta) in &packed.routine_meta {
        id.classify(&mut h);
        meta.classify(&mut h);
    }
    h.finish()
}

/// The class of one revision (§6.1). `P` is decided by the caller from the
/// package and app-identity inputs before any file is compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeClass {
    /// Every changed file kept its projection; no file added or removed.
    BodyOnly,
    /// Some projection changed, or a file was added, removed or renamed.
    Declaration,
    /// A package, a source dependency or the app's identity changed: rebuild.
    Package,
}

/// Classify the workspace files: `old` and `new` are `(virtual path,
/// projection)` in any order. A path present on one side only is F (part of S).
#[must_use]
pub fn classify_files(
    old: &[(&str, DeclProjection)],
    new: &[(&str, DeclProjection)],
) -> ChangeClass {
    use std::collections::HashMap;
    if old.len() != new.len() {
        return ChangeClass::Declaration;
    }
    let old: HashMap<&str, DeclProjection> = old.iter().copied().collect();
    for (path, p) in new {
        match old.get(path) {
            Some(q) if q == p => {}
            _ => return ChangeClass::Declaration,
        }
    }
    ChangeClass::BodyOnly
}

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;
