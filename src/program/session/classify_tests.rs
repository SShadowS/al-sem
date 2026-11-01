//! T-class and T-proj-guard (context-server design §6.1, §11): what the
//! declaration projection sees and what it leaves out. Every pair below states
//! its two sources by assignment; the projections of the two are compared.

use super::*;

fn proj(src: &str) -> DeclProjection {
    proj_at("C.al", src)
}

fn proj_at(path: &str, src: &str) -> DeclProjection {
    let file = al_syntax::parse(src);
    assert_eq!(
        file.parse_status,
        al_syntax::ir::ParseStatus::Clean,
        "the precondition: the fixture parses clean:\n{src}"
    );
    decl_projection(AppRef(0), TrustTier::Workspace, path, &file)
}

const BASE: &str = "codeunit 50100 C\n{\n    procedure P(X: Integer): Integer\n    begin\n        exit(X);\n    end;\n\n    procedure Q()\n    begin\n    end;\n}\n";

/// A body-only edit keeps the projection (B).
#[test]
fn body_edit_keeps_the_projection() {
    let edited = BASE.replace("exit(X);", "X += 1;\n        exit(X * 2);");
    assert_ne!(edited, BASE);
    assert_eq!(proj(BASE), proj(&edited));
}

/// Text inserted above a declaration moves every origin, which the projection
/// leaves out (B).
#[test]
fn newline_before_a_declaration_keeps_the_projection() {
    let edited = BASE.replace("    procedure Q()", "\n\n    procedure Q()");
    assert_ne!(edited, BASE);
    assert_eq!(proj(BASE), proj(&edited));
}

/// Reordering two declarations changes the projection: it is in document
/// order (S).
#[test]
fn declaration_reorder_changes_the_projection() {
    let reordered = "codeunit 50100 C\n{\n    procedure Q()\n    begin\n    end;\n\n    procedure P(X: Integer): Integer\n    begin\n        exit(X);\n    end;\n}\n";
    assert_ne!(proj(BASE), proj(reordered));
}

/// A parameter type change and a return type change each change it (S).
#[test]
fn signature_and_return_type_changes_change_the_projection() {
    let param = BASE.replace("P(X: Integer)", "P(X: Decimal)");
    let ret = BASE.replace("): Integer", "): Decimal");
    let by_ref = BASE.replace("P(X: Integer)", "P(var X: Integer)");
    let renamed_param = BASE.replace("P(X: Integer)", "P(Y: Integer)");
    for edited in [&param, &ret, &by_ref, &renamed_param] {
        assert_ne!(edited, BASE);
        assert_ne!(proj(BASE), proj(edited), "{edited}");
    }
}

/// A routine repeated (multiplicity) changes it.
#[test]
fn a_duplicate_declaration_changes_the_projection() {
    let dup = BASE.replace(
        "    procedure Q()\n    begin\n    end;\n",
        "    procedure Q()\n    begin\n    end;\n\n    procedure Q()\n    begin\n    end;\n",
    );
    assert_ne!(proj(BASE), proj(&dup));
}

/// `[MethodId(n)]` and `[TryFunction]` live only in `RoutineMeta` (the method
/// id of a routine the runtime calls by number, and the Boolean return the
/// compiler hashes into a method id); each change is S.
#[test]
fn method_id_and_try_function_change_the_projection() {
    let with_mid = BASE.replace("    procedure Q()", "    [MethodId(42)]\n    procedure Q()");
    let other_mid = BASE.replace("    procedure Q()", "    [MethodId(43)]\n    procedure Q()");
    let try_fn = BASE.replace("    procedure Q()", "    [TryFunction]\n    procedure Q()");
    assert_ne!(proj(BASE), proj(&with_mid));
    assert_ne!(proj(&with_mid), proj(&other_mid));
    assert_ne!(proj(BASE), proj(&try_fn));
}

/// An enum's extensibility and its implementation map decide `closed_enum` and
/// `enum_impls` (#264); each change is S.
#[test]
fn enum_extensibility_and_impls_change_the_projection() {
    let closed = "enum 50100 E implements I\n{\n    Extensible = false;\n    value(0; A) { Implementation = I = \"Impl A\"; }\n    value(1; B) { Implementation = I = \"Impl B\"; }\n}\n";
    let open = closed.replace("Extensible = false;", "Extensible = true;");
    let remapped = closed.replace("I = \"Impl B\"", "I = \"Impl A\"");
    assert_ne!(proj_at("E.al", closed), proj_at("E.al", &open));
    assert_ne!(proj_at("E.al", closed), proj_at("E.al", &remapped));
}

/// An event subscriber's attribute is part of `RoutineNode` (S when it moves to
/// another event).
#[test]
fn subscriber_target_changes_the_projection() {
    let sub = "codeunit 50101 S\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::C, 'OnA', '', false, false)]\n    local procedure H()\n    begin\n    end;\n}\n";
    let moved = sub.replace("'OnA'", "'OnB'");
    assert_ne!(proj_at("S.al", sub), proj_at("S.al", &moved));
}

/// The same declarations under another path differ: `RoutineMeta` carries the
/// virtual path (a rename is F anyway).
#[test]
fn the_virtual_path_is_part_of_the_projection() {
    assert_ne!(proj_at("A.al", BASE), proj_at("B.al", BASE));
}

/// `classify_files`: equal projections on the same paths are B; a changed
/// projection, an added, a removed and a renamed file are each S.
#[test]
fn classify_files_cases() {
    let a = proj(BASE);
    let b = proj(&BASE.replace("): Integer", "): Decimal"));
    let same = [("A.al", a), ("B.al", a)];
    assert_eq!(classify_files(&same, &same), ChangeClass::BodyOnly);
    assert_eq!(
        classify_files(&same, &[("A.al", a), ("B.al", b)]),
        ChangeClass::Declaration
    );
    assert_eq!(
        classify_files(&same, &[("A.al", a), ("B.al", a), ("C.al", a)]),
        ChangeClass::Declaration
    );
    assert_eq!(
        classify_files(&same, &[("A.al", a)]),
        ChangeClass::Declaration
    );
    assert_eq!(
        classify_files(&same, &[("A.al", a), ("Renamed.al", a)]),
        ChangeClass::Declaration
    );
}

/// T-proj-guard: the exclusion list is exactly the positional origins and the
/// source-unit stamp, and an origin-only difference between two otherwise
/// equal `RoutineMeta`s does not reach the hash while every other field does.
/// Hand-stated by assignment on one extracted meta.
#[test]
fn exclusion_list_is_exactly_the_positions() {
    assert_eq!(
        EXCLUDED,
        &[
            "RoutineMeta.origin",
            "RoutineMeta.name_origin",
            "RoutineNode.source_unit"
        ]
    );
    let file = al_syntax::parse(BASE);
    let meta = RoutineMeta::from_decl(&file.objects[0].routines[0], "C.al");
    let hash = |m: &RoutineMeta| {
        let mut h = ProjectionHasher::new();
        m.classify(&mut h);
        h.finish()
    };
    let mut moved = meta.clone();
    moved.origin.byte = 1000..2000;
    moved.origin.start.row += 7;
    moved.name_origin.byte = 1010..1011;
    assert_eq!(hash(&meta), hash(&moved));
    let mut changed = meta.clone();
    changed.try_function = !changed.try_function;
    assert_ne!(hash(&meta), hash(&changed));
    let mut changed = meta.clone();
    changed.method_id = Some(7);
    assert_ne!(hash(&meta), hash(&changed));
    let mut changed = meta;
    changed.parse_incomplete = !changed.parse_incomplete;
    assert_ne!(hash(&file_meta_base()), hash(&changed));

    // The source-unit stamp is excluded; every other routine field reaches it.
    let mut objects = Vec::new();
    let mut routines = Vec::new();
    crate::program::node_extract::extract_nodes(
        AppRef(0),
        &file,
        TrustTier::Workspace,
        &mut objects,
        &mut routines,
    );
    let rhash = |r: &RoutineNode| {
        let mut h = ProjectionHasher::new();
        r.classify(&mut h);
        h.finish()
    };
    let r = routines[0].clone();
    let mut stamped = r.clone();
    stamped.source_unit = std::num::NonZeroU32::new(9);
    assert_eq!(rhash(&r), rhash(&stamped));
    let mut changed = r.clone();
    changed.abi_method_id = Some(1);
    assert_ne!(rhash(&r), rhash(&changed));
    let mut changed = r;
    changed.access = Access::Internal;
    assert_ne!(rhash(&routines[0]), rhash(&changed));
}

fn file_meta_base() -> RoutineMeta {
    let file = al_syntax::parse(BASE);
    RoutineMeta::from_decl(&file.objects[0].routines[0], "C.al")
}
