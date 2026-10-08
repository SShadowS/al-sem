//! L4 effect lattice (R3a-2) — faithful port of al-sem's
//! `src/engine/effect-lattice.ts`.
//!
//! Three concerns:
//!   1. `EffectPresence` tri-state: `no < unknown < yes` (monotone lattice).
//!   2. `effect_key_of` — stable, path-insensitive key for a DbEffect
//!      (EXCLUDES `via` — two effects for the same operation are the same fact
//!      regardless of how they propagated). Used for de-duplication.
//!   3. `merge_via` — the 5-rank via-precedence merge:
//!      `direct > implicit-trigger > event-subscriber > dynamic > inherited`.
//!      Equal-rank tie: keep the FIRST argument (al-sem `VIA_RANK[a] >= VIA_RANK[b]
//!      ? a : b` — the `>=` means a wins on tie).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EffectPresence tri-state.
// ---------------------------------------------------------------------------

/// Tri-state effect presence: `no < unknown < yes`. Monotone lattice.
/// Mirrors al-sem `EffectPresence` (`src/model/summary.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectPresence {
    No,
    Unknown,
    Yes,
}

impl EffectPresence {
    fn rank(self) -> u8 {
        match self {
            EffectPresence::No => 0,
            EffectPresence::Unknown => 1,
            EffectPresence::Yes => 2,
        }
    }

    fn from_rank(r: u8) -> Self {
        match r {
            0 => EffectPresence::No,
            1 => EffectPresence::Unknown,
            _ => EffectPresence::Yes,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EffectPresence::No => "no",
            EffectPresence::Unknown => "unknown",
            EffectPresence::Yes => "yes",
        }
    }

    pub fn from_presence_str(s: &str) -> Self {
        match s {
            "yes" => EffectPresence::Yes,
            "no" => EffectPresence::No,
            _ => EffectPresence::Unknown,
        }
    }
}

/// Lattice join: the more-informative presence wins (`yes > unknown > no`). Monotone.
/// Mirrors al-sem `joinPresence`.
pub fn join_presence(a: EffectPresence, b: EffectPresence) -> EffectPresence {
    EffectPresence::from_rank(a.rank().max(b.rank()))
}

// ---------------------------------------------------------------------------
// TempStateKind — the per-record-operation temporariness classification.
// ---------------------------------------------------------------------------

/// The temp-state key kind. Mirrors al-sem `TempState` (`src/model/entities.ts`).
/// Used to compute the `tempStateKey` fragment in `effect_key_of`.
///
/// Derives `Hash` (added for `EffectUniverse`, `src/engine/l4/effect_universe.rs`)
/// so the structured `EffectIdentity` — which embeds a `TempStateKind` — can be
/// used as a `HashMap` key for interning; purely additive, no existing use site
/// relies on `TempStateKind` NOT being `Hash`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TempStateKind {
    /// `{ kind: "known", value: bool }` — statically known true/false.
    Known(bool),
    /// `{ kind: "parameter-dependent", parameterIndex: u32 }`.
    ParameterDependent(u32),
    /// `{ kind: "unknown" }`.
    Unknown,
}

impl TempStateKind {
    /// Short stable key fragment. Mirrors al-sem `tempStateKey`:
    /// `known(true)` → `"t"`, `known(false)` → `"f"`,
    /// `parameter-dependent(i)` → `"p<i>"`, `unknown` → `"u"`.
    pub fn key_fragment(&self) -> String {
        match self {
            TempStateKind::Known(true) => "t".to_string(),
            TempStateKind::Known(false) => "f".to_string(),
            TempStateKind::ParameterDependent(i) => format!("p{i}"),
            TempStateKind::Unknown => "u".to_string(),
        }
    }

    /// Parse from a `PTempState`-shaped JSON value (kind + optional value/parameterIndex).
    /// Used in tests for vector input parsing.
    pub fn from_p_temp_state(ts: &crate::program::body::features::PTempState) -> Self {
        match ts.kind.as_str() {
            "known" => TempStateKind::Known(ts.value.unwrap_or(false)),
            "parameter-dependent" => {
                TempStateKind::ParameterDependent(ts.parameter_index.unwrap_or(0))
            }
            _ => TempStateKind::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// effect_key_of — stable, path-insensitive effect key (EXCLUDES via).
// ---------------------------------------------------------------------------

/// Stable, path-insensitive effect key. Deliberately EXCLUDES `via` — two
/// DbEffects for the same operation are the same fact regardless of how they
/// propagated. Used for de-duplication.
///
/// Key format: `${op}|${tableId}|${operationId}|${tempStateKey}`.
/// Mirrors al-sem `effectKeyOf` (`src/engine/effect-lattice.ts`).
pub fn effect_key_of(
    op: &str,
    table_id: &str,
    operation_id: &str,
    temp_state: &TempStateKind,
) -> String {
    format!(
        "{}|{}|{}|{}",
        op,
        table_id,
        operation_id,
        temp_state.key_fragment()
    )
}

// ---------------------------------------------------------------------------
// merge_via — 5-rank via-precedence merge.
// ---------------------------------------------------------------------------

/// The 5-rank via-precedence: `direct > implicit-trigger > event-subscriber >
/// dynamic > inherited`. Mirrors al-sem `VIA_RANK`:
///   direct=4, implicit-trigger=3, event-subscriber=2, dynamic=1, inherited=0.
///
/// Equal-rank tie-breaker: KEEP the first argument (`a`). (al-sem
/// `VIA_RANK[a] >= VIA_RANK[b] ? a : b` — `>=` means `a` wins on tie.)
pub fn merge_via<'a>(a: &'a str, b: &'a str) -> &'a str {
    if via_rank(a) >= via_rank(b) { a } else { b }
}

/// Owned-string variant of `merge_via` (for code that doesn't have tied lifetimes).
pub fn merge_via_owned(a: &str, b: &str) -> String {
    merge_via(a, b).to_string()
}

fn via_rank(via: &str) -> u8 {
    match via {
        "direct" => 4,
        "implicit-trigger" => 3,
        "event-subscriber" => 2,
        "dynamic" => 1,
        "inherited" => 0,
        _ => 0,
    }
}

/// Map a combined-edge kind to the `via` tag callee effects inherit through it.
/// Mirrors al-sem `viaForEdge` in `summary-runner.ts`.
pub fn via_for_edge_kind(kind: &str) -> &'static str {
    match kind {
        "implicit-trigger" => "implicit-trigger",
        "event-dispatch" => "event-subscriber",
        "dynamic" => "dynamic",
        _ => "inherited",
    }
}
