//! AL global-builtin allowlist — two-tier recognition for bare (no-receiver) calls.
//!
//! Tier 1: Curated DISPOSITION overlay (~25 entries). These hand-listed names carry
//! a specific semantic disposition that downstream consumers need:
//!   - "control-terminating": `Error` stops execution (used by flow/ordering analysis)
//!   - "pure-terminal": side-effect-free builtins (used by temp-state analysis)
//!
//! Tier 2: Generated CATALOG (`global_builtins`). The complete set of 785 method names
//! extracted from the AL compiler DLL's ClassDocumentationResources (AL 18.0.2293710,
//! 97 types).  Any bare call not resolved to the caller's own object and not in Tier 1
//! that matches the catalog gets "pure-terminal".  See `global_builtins.rs` for the
//! soundness rationale.

use crate::program::resolve::global_builtins;

/// Disposition of a recognized no-receiver global builtin, else `None`.
/// The string variants match al-sem ("pure-terminal" | "control-terminating").
///
/// Lookup order:
///   1. Curated hand-list (disposition overlay — Error MUST be "control-terminating").
///   2. Generated catalog (`global_builtins`) — everything else from the compiler DLL.
pub fn global_builtin_disposition(name: &str) -> Option<&'static str> {
    let name_lc = name.to_lowercase();
    match name_lc.as_str() {
        // Tier 1: curated disposition overlay.
        "error" => Some("control-terminating"),
        "copystr" | "maxstrlen" | "strlen" | "strsubstno" | "format" | "lowercase"
        | "uppercase" | "convertstr" | "delchr" | "padstr" | "incstr" | "abs" | "round"
        | "power" | "userid" | "companyname" | "currentdatetime" | "today" | "time"
        | "workdate" | "createguid" | "isnullguid" => Some("pure-terminal"),
        // Tier 2: generated catalog — all remaining compiler-intrinsic globals.
        _ if global_builtins::is_global_builtin(&name_lc) => Some("pure-terminal"),
        _ => None,
    }
}
