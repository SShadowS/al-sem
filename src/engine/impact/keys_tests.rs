//! `keys.rs`: coverage wording.

use super::*;

/// A complete coverage is not incomplete; each gap makes it so, with words.
#[test]
fn coverage_incomplete_has_a_reason() {
    let c = Coverage {
        status: "complete".into(),
        ..Coverage::default()
    };
    assert!(!c.incomplete());
    assert!(c.why().is_empty());
    let c = Coverage {
        status: "complete".into(),
        unresolved: 2,
        ..Coverage::default()
    };
    assert!(c.incomplete());
    assert_eq!(c.why(), vec!["2 unresolved calls in its cone".to_string()]);
}
