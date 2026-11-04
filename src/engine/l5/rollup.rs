//! The write classifier every reader shares (MCP output wave 2, W2-1; design
//! `docs/superpowers/specs/2026-11-04-mcp-output-wave2-design.md` §3.2).
//!
//! One question only: which class is ONE effect contribution in. A table write
//! is `Physical` ("may persist"), `CallerDependent` ("possibly persistent:
//! depends on the caller") or `Temporary`, read off the row's joined
//! `detail.tempState` (the comma-joined L4 states, `digest_cli::TempJoin::
//! render`). It does not answer "does R write T": that is question
//! evaluation, which adds field-guarded matches, caller resolution and the
//! obligation rules (`prove::evaluate_writes_table`, §3.2.1).
//!
//! Rules (§3.2): any `known(false)` → physical, known persistent; else an
//! all-`known(true)` list → temporary; else a list of only `known(true)` and
//! `parameter-dependent(i)` with at least one parameter → caller-dependent;
//! else physical: temp state unknown, or not joined (an empty list: no L4 row,
//! a non-temporary group). Exactly prove's G17 / G17b helpers before W2-1.

use crate::engine::l4::effect_query_cli::RECEIVER_TEMP_STATE;

/// Why a write may persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalReason {
    /// Some originating operation writes a record known not to be temporary.
    KnownPersistent,
    /// No state says persistent, but one is not known.
    TempStateUnknown,
    /// No joined state at all (no L4 row; a group not known temporary).
    NotJoined,
}

/// Whose record a caller-dependent write writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerReason {
    /// A record parameter (the first such state's index).
    Parameter(u32),
    /// Only the routine's own implicit `Rec` ([`RECEIVER_TEMP_STATE`]).
    ImplicitRec,
}

/// One table write's class (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteClass {
    Physical(PhysicalReason),
    CallerDependent(CallerReason),
    Temporary,
}

/// The class of a write whose joined temp states are `states` (the
/// comma-split `detail.tempState`; empty when absent).
#[must_use]
pub fn classify(states: &[&str]) -> WriteClass {
    fn pd(s: &&str) -> bool {
        s.starts_with("parameter-dependent")
    }
    if states.contains(&"known(false)") {
        return WriteClass::Physical(PhysicalReason::KnownPersistent);
    }
    if states.is_empty() {
        return WriteClass::Physical(PhysicalReason::NotJoined);
    }
    if states.iter().all(|s| *s == "known(true)") {
        return WriteClass::Temporary;
    }
    if states.iter().any(pd) && states.iter().all(|s| *s == "known(true)" || pd(s)) {
        let params: Vec<&&str> = states.iter().filter(|s| pd(s)).collect();
        if params.iter().all(|s| **s == RECEIVER_TEMP_STATE) {
            return WriteClass::CallerDependent(CallerReason::ImplicitRec);
        }
        let index = params
            .iter()
            .find(|s| ***s != RECEIVER_TEMP_STATE)
            .and_then(|s| {
                s.trim_start_matches("parameter-dependent(")
                    .trim_end_matches(')')
                    .parse()
                    .ok()
            })
            .unwrap_or(0);
        return WriteClass::CallerDependent(CallerReason::Parameter(index));
    }
    WriteClass::Physical(PhysicalReason::TempStateUnknown)
}

/// The states a row's `detail` lists under `tempState`, split.
#[must_use]
pub fn temp_states(detail: &[(String, String)]) -> Vec<&str> {
    detail
        .iter()
        .find(|(k, _)| k == "tempState")
        .map(|(_, v)| v.split(',').map(str::trim).collect())
        .unwrap_or_default()
}

/// [`classify`] over a row's `detail`.
#[must_use]
pub fn classify_detail(detail: &[(String, String)]) -> WriteClass {
    classify(&temp_states(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §3.2 table, row by row, including the mixed rows (F1) and the
    /// orders the old string tests treated alike.
    #[test]
    fn the_three_classes() {
        use WriteClass::*;
        let r = RECEIVER_TEMP_STATE;
        for (states, want) in [
            (
                vec!["known(false)"],
                Physical(PhysicalReason::KnownPersistent),
            ),
            (
                vec!["known(false)", "unknown"],
                Physical(PhysicalReason::KnownPersistent),
            ),
            (
                vec!["known(false)", "parameter-dependent(0)"],
                Physical(PhysicalReason::KnownPersistent),
            ),
            (vec!["unknown"], Physical(PhysicalReason::TempStateUnknown)),
            (
                vec!["unknown", "parameter-dependent(0)"],
                Physical(PhysicalReason::TempStateUnknown),
            ),
            (vec![], Physical(PhysicalReason::NotJoined)),
            (vec!["known(true)"], Temporary),
            (
                vec!["known(true)", "parameter-dependent(1)"],
                CallerDependent(CallerReason::Parameter(1)),
            ),
            (vec![r], CallerDependent(CallerReason::ImplicitRec)),
            (
                vec![r, "parameter-dependent(2)"],
                CallerDependent(CallerReason::Parameter(2)),
            ),
        ] {
            assert_eq!(classify(&states), want, "{states:?}");
        }
    }
}
