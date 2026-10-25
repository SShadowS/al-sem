//! The object that declares a routine (#241).
//!
//! An object id does not name one declaration: an object split across `#if`
//! arms (`#if A codeunit 50100 "Foo" … #else codeunit 50100 "Bar" … #endif`)
//! gives two `ModelObject` rows with one id, because a numbered object's id key
//! is its number. Owner display reads the declaring row through here, never an
//! id -> name map (which keeps one arm's name for both).

use std::collections::HashMap;

use super::workspace::{ModelObject, ModelRoutine};
use crate::program::body::features::PAnchor;

/// One id's rows. A `Vec`, not a `SmallVec<[_; 1]>`: SmallVec is invariant in
/// `'a`, which breaks every borrow of `DetectorContext<'v>` (#279, measured).
type Rows<'a> = Vec<&'a ModelObject>;

/// Object rows by id, each id's rows in model order.
pub struct DeclaringObjects<'a> {
    by_id: HashMap<&'a str, Rows<'a>>,
}

impl<'a> DeclaringObjects<'a> {
    pub fn new(objects: &'a [ModelObject]) -> Self {
        let mut by_id: HashMap<&'a str, Rows<'a>> = HashMap::new();
        for o in objects {
            by_id.entry(o.id.as_str()).or_default().push(o);
        }
        DeclaringObjects { by_id }
    }

    /// The retained census's shallow walk (#160 §4.1.5): the id map and each
    /// id's row list.
    pub(crate) fn census_tally(&self) -> crate::engine::census::Tally {
        let mut t = crate::engine::census::Tally::default();
        t.buf::<(&str, Rows<'a>)>(self.by_id.len())
            .bufs(self.by_id.values());
        t
    }

    /// The object row that declares `r`: `None` when no row has its id; the one
    /// row; of several, the row whose declaration anchor contains the routine's
    /// start in the same file (the innermost, when error-recovered anchors
    /// overlap: the latest start, then the earliest end, then model order); when
    /// none does (rows without an anchor), the LAST row,
    /// the owner an id -> name map gave before #241.
    pub fn of(&self, r: &ModelRoutine) -> Option<&'a ModelObject> {
        let rows = self.by_id.get(r.object_id.as_str())?;
        if let [only] = rows.as_slice() {
            return Some(only);
        }
        let ra = &r.source_anchor;
        rows.iter()
            .filter_map(|o| Some((*o, o.source_anchor.as_ref()?)))
            .filter(|(_, a)| {
                a.source_unit_id == ra.source_unit_id && a.contains(ra.start_line, ra.start_column)
            })
            .min_by(|(_, a), (_, b)| {
                let start = |x: &PAnchor| (x.start_line, x.start_column);
                let end = |x: &PAnchor| (x.end_line, x.end_column);
                start(b).cmp(&start(a)).then(end(a).cmp(&end(b)))
            })
            .map(|(o, _)| o)
            .or(rows.last().copied())
    }
}
