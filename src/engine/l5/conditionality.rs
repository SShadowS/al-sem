//! Port of al-sem `src/digest/conditionality.ts`.
//!
//! Conditionality captures how reliably an effect fires relative to the
//! routine's normal (success) exit path.
//!
//! ## Lattice (restrictiveness rank, 0 = least restrictive)
//!   unconditional-on-success → rank 0
//!   conditional              → rank 1
//!   loop-body                → rank 2
//!   guarded-by-IsHandled     → rank 3
//!   error-path               → rank 4
//!   unknown (contaminates — not ranked)
//!
//! ## path conditionality
//!   Most-restrictive along the path; any "unknown" contaminates.
//!
//! ## effect conditionality
//!   Least-restrictive across all paths; if ALL paths are "unknown" OR
//!   truncated && no unconditional path → "unknown".

/// Effect conditionality — the string form used in the JSON golden.
pub type EffectConditionality = &'static str;

pub const UNCONDITIONAL: EffectConditionality = "unconditional-on-success";
pub const CONDITIONAL: EffectConditionality = "conditional";
pub const LOOP_BODY: EffectConditionality = "loop-body";
pub const GUARDED_BY_ISHANDLED: EffectConditionality = "guarded-by-IsHandled";
pub const ERROR_PATH: EffectConditionality = "error-path";
pub const UNKNOWN: EffectConditionality = "unknown";

/// Restrictiveness rank for non-unknown conditionalities.
pub(crate) fn rank(c: EffectConditionality) -> Option<u32> {
    match c {
        "unconditional-on-success" => Some(0),
        "conditional" => Some(1),
        "loop-body" => Some(2),
        "guarded-by-IsHandled" => Some(3),
        "error-path" => Some(4),
        _ => None, // "unknown" → None (contaminates)
    }
}

/// `contextToConditionality` — map a ControlContext string (or None) to
/// EffectConditionality.
///
/// `None` / `"unreachable"` → `"unknown"` (defensive).
pub fn context_to_conditionality(ctx: Option<&str>) -> EffectConditionality {
    match ctx {
        Some("top-level") => UNCONDITIONAL,
        Some("conditional") => CONDITIONAL,
        Some("loop-body") => LOOP_BODY,
        Some("error-path") => ERROR_PATH,
        Some("is-handled-guarded") => GUARDED_BY_ISHANDLED,
        _ => UNKNOWN,
    }
}

/// `pathConditionality` — most-restrictive along the path; unknown contaminates.
pub fn path_conditionality(
    hop_contexts: &[EffectConditionality],
    terminal_ctx: EffectConditionality,
) -> EffectConditionality {
    let all: Vec<EffectConditionality> = hop_contexts
        .iter()
        .copied()
        .chain(std::iter::once(terminal_ctx))
        .collect();
    // Any unknown → contaminate.
    if all.contains(&UNKNOWN) {
        return UNKNOWN;
    }
    // Most restrictive = highest rank.
    let mut max_rank: u32 = 0;
    let mut result: EffectConditionality = UNCONDITIONAL;
    for c in &all {
        if let Some(r) = rank(c)
            && r > max_rank
        {
            max_rank = r;
            result = c;
        }
    }
    result
}

/// `effectConditionality` — least-restrictive across all paths.
///
/// - All unknown → unknown.
/// - Truncated && no unconditional found → unknown.
pub fn effect_conditionality(
    path_conds: &[EffectConditionality],
    truncated: bool,
) -> EffectConditionality {
    if path_conds.is_empty() {
        return UNKNOWN;
    }
    let mut least_rank: Option<u32> = None;
    let mut result: EffectConditionality = UNKNOWN;
    let mut has_unconditional = false;

    for &c in path_conds {
        if c == UNKNOWN {
            continue;
        }
        if let Some(r) = rank(c) {
            if least_rank.map(|lr| r < lr).unwrap_or(true) {
                least_rank = Some(r);
                result = c;
            }
            if c == UNCONDITIONAL {
                has_unconditional = true;
            }
        }
    }

    if least_rank.is_none() {
        return UNKNOWN;
    }
    if truncated && !has_unconditional {
        return UNKNOWN;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_mapping_covers_all_values() {
        // error-path + guarded-by-IsHandled have 0 corpus coverage — pin them here.
        assert_eq!(context_to_conditionality(Some("top-level")), UNCONDITIONAL);
        assert_eq!(context_to_conditionality(Some("conditional")), CONDITIONAL);
        assert_eq!(context_to_conditionality(Some("loop-body")), LOOP_BODY);
        assert_eq!(context_to_conditionality(Some("error-path")), ERROR_PATH);
        assert_eq!(
            context_to_conditionality(Some("is-handled-guarded")),
            GUARDED_BY_ISHANDLED
        );
        // unreachable + None + unknown → defensive UNKNOWN.
        assert_eq!(context_to_conditionality(Some("unreachable")), UNKNOWN);
        assert_eq!(context_to_conditionality(None), UNKNOWN);
        assert_eq!(context_to_conditionality(Some("weird")), UNKNOWN);
    }

    #[test]
    fn path_conditionality_unknown_contaminates() {
        // ANY unknown hop or terminal contaminates the whole path → unknown.
        assert_eq!(path_conditionality(&[UNCONDITIONAL], UNKNOWN), UNKNOWN);
        assert_eq!(path_conditionality(&[UNKNOWN], UNCONDITIONAL), UNKNOWN);
        // Otherwise: most-restrictive (highest rank) wins.
        assert_eq!(
            path_conditionality(&[UNCONDITIONAL, CONDITIONAL], LOOP_BODY),
            LOOP_BODY
        );
        assert_eq!(path_conditionality(&[CONDITIONAL], ERROR_PATH), ERROR_PATH);
        assert_eq!(path_conditionality(&[], UNCONDITIONAL), UNCONDITIONAL);
    }

    #[test]
    fn effect_conditionality_filters_unknown_not_contaminate() {
        // ASYMMETRY (#7): effectConditionality FILTERS OUT unknown paths (least-restrictive
        // across the NON-unknown ones), it does NOT contaminate like pathConditionality.
        // A mix of unknown + unconditional → unconditional (the unknown is dropped).
        assert_eq!(
            effect_conditionality(&[UNKNOWN, UNCONDITIONAL], false),
            UNCONDITIONAL
        );
        // least-restrictive across known paths.
        assert_eq!(
            effect_conditionality(&[ERROR_PATH, CONDITIONAL], false),
            CONDITIONAL
        );
        // all unknown → unknown.
        assert_eq!(effect_conditionality(&[UNKNOWN, UNKNOWN], false), UNKNOWN);
        assert_eq!(effect_conditionality(&[], false), UNKNOWN);
        // truncated && no unconditional found → unknown.
        assert_eq!(effect_conditionality(&[CONDITIONAL], true), UNKNOWN);
        // truncated but an unconditional path exists → keep unconditional.
        assert_eq!(
            effect_conditionality(&[UNCONDITIONAL, CONDITIONAL], true),
            UNCONDITIONAL
        );
    }

    #[test]
    fn multi_path_asymmetry_combination() {
        // The #8-relevant scenario: an effect reached via TWO paths terminating at
        // different contexts — one unconditional (top-level), one conditional. Each
        // path is computed independently (per-path terminal), then the effect folds
        // least-restrictive → unconditional. If the two phases used the SAME logic
        // (e.g. both contaminating), a known+unknown mix would compute wrong.
        let p_uncond = path_conditionality(&[], UNCONDITIONAL); // top-level terminal
        let p_cond = path_conditionality(&[CONDITIONAL], UNCONDITIONAL); // conditional hop
        assert_eq!(p_uncond, UNCONDITIONAL);
        assert_eq!(p_cond, CONDITIONAL);
        assert_eq!(
            effect_conditionality(&[p_uncond, p_cond], false),
            UNCONDITIONAL
        );
    }
}
