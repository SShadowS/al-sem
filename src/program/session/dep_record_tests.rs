//! The answer digest's own contracts (plan item S).

use std::path::Path;

use super::*;
use crate::program::resolve::full::build_context_res;

/// Every query a whole recorded resolution logs has a digest, and asking
/// again against the same state gives the same digest (the rule's "nothing
/// changed" baseline), over every r0-corpus workspace with dependency bodies
/// and the two dependency fixtures.
#[test]
fn digests_are_a_function_of_the_state() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dirs = [
        root.join("tests/fixtures/lsp-diff-deps"),
        root.join("tests/r3a5-fixtures/ws"),
        root.join("tests/r0-corpus/ws-codeunit-rec"),
        root.join("tests/r0-corpus/ws-baseapp-closure"),
    ];
    let mut checked = 0usize;
    for dir in &dirs {
        let ctx = build_context_res(dir).expect("context");
        let (d, rec) = resolve_whole(&ctx);
        let rec = rec.expect("no site-fact collision in the fixtures");
        assert_eq!(rec.files.len(), ctx.dependency_files().len());
        assert_eq!(
            rec.files.last().map_or(0, |f| f.edges.end),
            d.edges.len(),
            "the file ranges cover the edges"
        );
        let index = ResolveIndex::build(ctx.graph());
        let surface = ctx.decl_surface();
        let s = EvalState {
            graph: ctx.graph(),
            index: &index,
            surface: &surface,
        };
        assert_eq!(
            s.tier(),
            rec.tier,
            "a rebuilt index and surface name the same tier"
        );
        for (q, d) in rec.queries.iter().zip(&rec.digests) {
            assert_eq!(eval(q, &s), *d, "{q:?}");
        }
        // The whole recorded resolution equals the plain one.
        let plain = ctx.resolve_dependency_bodies();
        assert_eq!(format!("{:?}", plain.edges), format!("{:?}", d.edges));
        checked += rec.queries.len();
    }
    assert!(checked > 0, "no query recorded");
}

/// `position_in` names exactly the elements of the part.
#[test]
fn position_in_names_only_the_parts_elements() {
    let part = vec![1u64, 2, 3];
    let other = 2u64;
    assert_eq!(position_in(&part, &part[2]), Some(2));
    assert_eq!(position_in(&part, &other), None);
    assert_eq!(position_in::<u64>(&[], &other), None);
}
