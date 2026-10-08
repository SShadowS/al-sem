//! `RoutineInterner` — interns a routine's internal id (`ModelRoutine.id` /
//! `RoutineSummary.routine_id`) as a compact `RoutineIx(u32)`.
//!
//! Unlike [`crate::engine::l4::effect_universe::EffectUniverse`] (which grows
//! lazily during solving — new effect identities are discovered as PD
//! substitution proceeds), every routine id the db-effect solver will ever
//! see is already known BEFORE solving starts (the complete workspace
//! routine set). `RoutineInterner::build_canonical` exploits that: it
//! populates the WHOLE interner up front, in one deterministic pass, rather
//! than growing ad hoc during the per-SCC solve.
//!
//! ⟨spec rev4⟩ Assignment order is CANONICAL, not insertion-order: every
//! entry is interned in ascending `(stable_routine_id, routine_id)` order, so
//! ascending `RoutineIx` is stable across repeated builds of the SAME
//! workspace — not merely self-consistent within one loaded interner (which
//! is all plain solve-order interning would give, since Tarjan/HashMap
//! iteration order is not itself a cross-build-stable property).

use std::collections::HashMap;

/// A compact, interned handle to a routine's internal id. Cheap to copy and
/// use as a `HashMap`/`HashSet` key — the replacement for the `String`-keyed
/// per-member maps (`SccPresence::by_member`, the via map, `PdState`,
/// `solve_side_facts`'s per-member maps) the closed-form db-effect solver
/// used to key by the routine's raw id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct RoutineIx(pub u32);

/// Interner: routine id (`&str`) -> `RoutineIx`, plus the reverse `Vec` for
/// `RoutineIx -> &str` lookup.
pub struct RoutineInterner {
    by_id: Vec<String>,
    by_routine_id: HashMap<String, RoutineIx>,
}

impl Default for RoutineInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutineInterner {
    pub fn new() -> Self {
        RoutineInterner {
            by_id: Vec::new(),
            by_routine_id: HashMap::new(),
        }
    }

    /// Intern `routine_id`, creating a fresh `RoutineIx` on first sight.
    /// Stable across repeated calls with an equal id (re-interning an
    /// already-seen id returns the SAME ix, never a duplicate) — mirrors
    /// [`crate::engine::l4::effect_universe::EffectUniverse::intern`]'s own
    /// contract. Assignment order is plain intern-call order; CALLERS that
    /// need the canonical cross-build-stable order use [`Self::build_canonical`]
    /// instead of hand-rolled `intern` calls in solve order.
    pub fn intern(&mut self, routine_id: &str) -> RoutineIx {
        if let Some(&ix) = self.by_routine_id.get(routine_id) {
            return ix;
        }
        let ix = RoutineIx(self.by_id.len() as u32);
        self.by_id.push(routine_id.to_string());
        self.by_routine_id.insert(routine_id.to_string(), ix);
        ix
    }

    /// Look up an already-interned routine id without creating one.
    pub fn get(&self, routine_id: &str) -> Option<RoutineIx> {
        self.by_routine_id.get(routine_id).copied()
    }

    /// The routine id for an interned ix. Panics if `ix` was never produced
    /// by this interner — every `RoutineIx` in circulation is expected to
    /// have come from exactly one `RoutineInterner` (mirrors
    /// `EffectUniverse::identity`'s own panic contract).
    pub fn name(&self, ix: RoutineIx) -> &str {
        &self.by_id[ix.0 as usize]
    }

    /// Number of distinct routine ids interned so far.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Build a fully-populated interner from `(routine_id, stable_routine_id)`
    /// pairs, in CANONICAL deterministic order: every entry is interned in
    /// ascending `(stable_routine_id, routine_id)` order (the `routine_id`
    /// tie-break gives a total order defensively — two routines should never
    /// share a `stable_routine_id` in a well-formed workspace, but nothing
    /// here assumes that), so ascending `RoutineIx` is stable across
    /// repeated builds of the SAME workspace (spec rev4), not merely
    /// self-consistent within one loaded interner.
    pub fn build_canonical<'a, I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut sorted: Vec<(&str, &str)> = entries.into_iter().collect();
        sorted.sort_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)));
        let mut interner = Self::new();
        for (routine_id, _stable_routine_id) in sorted {
            interner.intern(routine_id);
        }
        interner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_idempotent_and_name_roundtrips() {
        let mut i = RoutineInterner::new();
        let x1 = i.intern("x");
        let x2 = i.intern("x");
        assert_eq!(
            x1, x2,
            "re-intern of the same id returns the same RoutineIx"
        );
        assert_eq!(i.name(x1), "x");
    }

    #[test]
    fn get_returns_none_before_intern_and_some_after() {
        let mut i = RoutineInterner::new();
        assert_eq!(i.get("nope"), None);
        let ix = i.intern("nope");
        assert_eq!(i.get("nope"), Some(ix));
    }

    #[test]
    fn distinct_ids_get_distinct_ascending_ixs_in_intern_order() {
        let mut i = RoutineInterner::new();
        let a = i.intern("a");
        let b = i.intern("b");
        assert_eq!(a, RoutineIx(0));
        assert_eq!(b, RoutineIx(1));
    }

    /// ⟨spec rev4⟩ `build_canonical` assigns ascending `RoutineIx` by
    /// `stable_routine_id` order — NOT by input/insertion order.
    #[test]
    fn build_canonical_orders_by_stable_routine_id_not_input_order() {
        // Input order is deliberately NOT stable-id sorted (c, a, b).
        let entries = vec![
            ("c_id", "stable::c"),
            ("a_id", "stable::a"),
            ("b_id", "stable::b"),
        ];
        let interner = RoutineInterner::build_canonical(entries);
        assert_eq!(interner.get("a_id"), Some(RoutineIx(0)));
        assert_eq!(interner.get("b_id"), Some(RoutineIx(1)));
        assert_eq!(interner.get("c_id"), Some(RoutineIx(2)));
    }

    /// The whole point of canonical ordering: two builds fed the SAME
    /// `(routine_id, stable_routine_id)` set in DIFFERENT input order must
    /// assign the SAME `RoutineIx` to each id.
    #[test]
    fn build_canonical_is_reproducible_regardless_of_input_order() {
        let entries_a = vec![
            ("c_id", "stable::c"),
            ("a_id", "stable::a"),
            ("b_id", "stable::b"),
        ];
        let entries_b = vec![
            ("b_id", "stable::b"),
            ("c_id", "stable::c"),
            ("a_id", "stable::a"),
        ];
        let ia = RoutineInterner::build_canonical(entries_a);
        let ib = RoutineInterner::build_canonical(entries_b);
        assert_eq!(ia.get("a_id"), ib.get("a_id"));
        assert_eq!(ia.get("b_id"), ib.get("b_id"));
        assert_eq!(ia.get("c_id"), ib.get("c_id"));
    }
}
