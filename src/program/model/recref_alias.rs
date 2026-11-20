//! RecordRef alias classes of a routine (ENG-145): the one place every reader learns which
//! RecordRef (or Variant) variables may share one record.
//!
//! Measured on BC 28.4 (probe `docs/superpowers/notes/2026-11-20-eng145-recordref-aliasing.md`):
//! `A := B` makes both variables hold ONE record. `GetTable` through either rebinds both (the
//! other then holds the new table, and a temporary record's state with it). `Close` detaches
//! only the variable it is called on, and `Open` needs a closed (so detached) variable, so
//! neither reaches the other. `B.Duplicate()` shares nothing.
//!
//! Readers treat every rebinding call through a class member as reaching every member, `Open`
//! and `Close` included: sound, and coarser than the platform (ENG-157 refines it).

use crate::program::body::features::PVarAssignment;
use al_syntax::IdentifierFoldExt;
use std::collections::HashMap;

/// The alias classes over the variables `tracked` accepts (folded names), from `A := B`
/// assignments between two of them. A variable in no assignment is its own class.
#[derive(Debug, Default)]
pub struct AliasClasses {
    parent: HashMap<String, String>,
    /// Each alias assignment's target and position (line, column).
    pub assignments: Vec<(String, (u32, u32))>,
}

impl AliasClasses {
    #[must_use]
    pub fn of(assignments: &[PVarAssignment], tracked: impl Fn(&str) -> bool) -> Self {
        let mut c = Self::default();
        for a in assignments {
            let l = a.lhs_name.trim().trim_matches('"').fold_identifier();
            let Some(w) = a.rhs_identifier.as_deref() else {
                continue;
            };
            let w = w.trim_matches('"').fold_identifier();
            if !tracked(&l) || !tracked(&w) {
                continue;
            }
            let (rl, rw) = (c.root(&l), c.root(&w));
            c.parent.entry(l.clone()).or_insert_with(|| l.clone());
            c.parent.entry(w.clone()).or_insert_with(|| w.clone());
            if rl != rw {
                c.parent.insert(rl, rw);
            }
            c.assignments.push((
                l,
                (a.source_anchor.start_line, a.source_anchor.start_column),
            ));
        }
        c
    }

    /// The class representative of the folded name `x`.
    #[must_use]
    pub fn root(&self, x: &str) -> String {
        let mut cur = x.to_string();
        while let Some(p) = self.parent.get(&cur) {
            if *p == cur {
                break;
            }
            cur = p.clone();
        }
        cur
    }

    /// Every variable sharing a record with `x` (`x` included), folded.
    #[must_use]
    pub fn members(&self, x: &str) -> Vec<String> {
        let r = self.root(x);
        let mut out: Vec<String> = self
            .parent
            .keys()
            .filter(|n| self.root(n) == r)
            .cloned()
            .collect();
        if out.is_empty() {
            out.push(x.to_string());
        }
        out.sort();
        out
    }

    /// Whether `x` shares its record with another variable.
    #[must_use]
    pub fn has_alias(&self, x: &str) -> bool {
        self.members(x).len() > 1
    }

    /// Every folded name in an alias class of two or more.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.parent.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::body::features::PAnchor;

    fn assign(l: &str, r: Option<&str>, line: u32) -> PVarAssignment {
        PVarAssignment {
            lhs_name: l.to_string(),
            rhs_literal_value: None,
            source_anchor: PAnchor {
                source_unit_id: "ws:test.al".into(),
                start_line: line,
                start_column: 0,
                end_line: line,
                end_column: 0,
                syntax_kind: "assignment_statement".into(),
            },
            rhs_identifier: r.map(str::to_string),
            rhs_member: None,
            lhs_is_member: false,
        }
    }

    #[test]
    fn classes_join_through_chains_and_ignore_untracked_and_expressions() {
        let rows = [
            assign("A", Some("ref"), 1),
            assign("b", Some("a"), 2),
            assign("C", Some("other"), 3),
            assign("D", None, 4),
            assign("e", Some("notrecref"), 5),
        ];
        let tracked = |n: &str| n != "notrecref";
        let c = AliasClasses::of(&rows, tracked);
        assert_eq!(c.members("ref"), ["a", "b", "ref"]);
        assert_eq!(c.members("c"), ["c", "other"]);
        assert!(!c.has_alias("d"), "an expression copy is no alias");
        assert!(!c.has_alias("e"), "an untracked source is no alias");
        assert_eq!(c.assignments.len(), 3);
    }
}
