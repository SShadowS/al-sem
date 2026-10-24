//! The object that declares a routine (#241).
//!
//! An object id does not name one declaration: an object split across `#if`
//! arms (`#if A codeunit 50100 "Foo" … #else codeunit 50100 "Bar" … #endif`)
//! gives two `ModelObject` rows with one id, because a numbered object's id key
//! is its number. Owner display reads the declaring row through here, never an
//! id -> name map (which keeps one arm's name for both).

use std::collections::HashMap;

use super::workspace::{ModelObject, ModelRoutine};

/// Object rows by id, each id's rows in model order.
pub struct DeclaringObjects<'a> {
    by_id: HashMap<&'a str, Vec<&'a ModelObject>>,
}

impl<'a> DeclaringObjects<'a> {
    pub fn new(objects: &'a [ModelObject]) -> Self {
        let mut by_id: HashMap<&'a str, Vec<&'a ModelObject>> = HashMap::new();
        for o in objects {
            by_id.entry(o.id.as_str()).or_default().push(o);
        }
        DeclaringObjects { by_id }
    }

    /// The object row that declares `r`: `None` when no row has its id; the one
    /// row; of several, the row whose declaration anchor contains the routine's
    /// start in the same file; when none does (rows without an anchor), the LAST
    /// row, the owner an id -> name map gave before #241.
    pub fn of(&self, r: &ModelRoutine) -> Option<&'a ModelObject> {
        let rows = self.by_id.get(r.object_id.as_str())?;
        if let [only] = rows.as_slice() {
            return Some(only);
        }
        let ra = &r.source_anchor;
        let at = (ra.start_line, ra.start_column);
        rows.iter()
            .find(|o| {
                o.source_anchor.as_ref().is_some_and(|a| {
                    a.source_unit_id == ra.source_unit_id
                        && (a.start_line, a.start_column) <= at
                        && at <= (a.end_line, a.end_column)
                })
            })
            .or(rows.last())
            .copied()
    }
}
