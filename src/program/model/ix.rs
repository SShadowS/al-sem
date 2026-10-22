//! #160 step 11b (spec §4.3): [`RoutineIx`], a model row's position in
//! `ModelEntities::routines`. A row does not store it (11B-8); it is stored only
//! in `CallEdge::from_ix` and in the per-row producer outputs the projection
//! ([`crate::program::model::projection`]) consumes.

/// A routine row's position in `ModelEntities::routines`. Rows that share one
/// string id (a collision group) have distinct `RoutineIx`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoutineIx(pub u32);

impl RoutineIx {
    /// The row index as a `usize`.
    #[must_use]
    pub fn get(self) -> usize {
        self.0 as usize
    }

    /// The `RoutineIx` of row `ri`.
    ///
    /// # Panics
    /// When `ri` does not fit a `u32` (more than 4 billion rows).
    #[must_use]
    pub fn of(ri: usize) -> Self {
        RoutineIx(u32::try_from(ri).expect("row index fits u32"))
    }
}
