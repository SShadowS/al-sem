//! L3 record-type resolution — Rust port of al-sem's
//! `src/resolve/record-types.ts` (`resolveRecordTypes` + `recordTableNameOf`).
//!
//! Three passes over each routine's record vars + ops, plus implicit `Rec`/`xRec`
//! resolution via the object's "effective own table":
//!   1. declared record vars → resolve `tableName` against the symbol table.
//!   2. record ops → derive `tableId` from the matching record var; then the
//!      lexical-scope fallback over `features.variables` (FIRST-wins / innermost
//!      declaration wins on a name collision; params → locals → globals order
//!      means locals always shadow globals) for still-unset ops.
//!   3. implicit `Rec`/`xRec` ops (still unset) → the effective-own-table for
//!      Table / Page / TableExtension / PageExtension, NEVER overriding an
//!      explicit local `Rec` that pass (1)/(2) already resolved.
//!
//! Quoted-identifier parity (MUST match TS EXACTLY, do NOT "fix"):
//!   - `record_table_name_of` strips surrounding quotes from `declaredType`.
//!   - `extendsTargetName` / `sourceTableName` are stored already-stripped at
//!     index time (al-sem `extractExtendsTargetName` / SourceTable unquote), then
//!     passed RAW to `table_by_name` / `object_by_type_name` (which lower-case).

use super::l3_workspace::{L3Object, L3Routine};
use super::symbol_table::SymbolTable;
use al_syntax::IdentifierFoldExt;

use crate::engine::l2::node_util::strip_quotes;

/// Extract the TABLE NAME from a record variable's `declaredType` string.
///
///   `Record "Sales Line"`   → `Sales Line`
///   `Record Customer`       → `Customer`
///   `Record "X" temporary`  → `X`
///
/// Returns `None` for non-record types (e.g. `Integer`, `Codeunit "Foo"`).
/// Strips a trailing ` temporary` modifier, then unquotes the remaining name.
pub fn record_table_name_of(declared_type: &str) -> Option<String> {
    let trimmed = declared_type.trim();
    // Match a leading `Record` keyword (case-insensitive), word-bounded.
    let lower = trimmed.to_lowercase();
    if !lower.starts_with("record") {
        return None;
    }
    let rest_bytes = &trimmed[6..]; // "record".len() == 6 (ASCII)
    // Word-boundary check: the char after `record` must NOT be alphanumeric/_.
    if let Some(c) = rest_bytes.chars().next()
        && (c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let mut rest = rest_bytes.trim().to_string();
    // Strip a trailing ` temporary` modifier (case-insensitive, with leading ws).
    rest = strip_trailing_temporary(&rest);
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let name = strip_quotes(rest).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Strip a trailing `\s+temporary\s*$` (case-insensitive) — mirrors TS
/// `rest.replace(/\s+temporary\s*$/i, "")`. `program::resolve::receiver` has
/// a VERBATIM DUPLICATE of this function for its identical Record-type-name
/// parsing — NOT consolidated here via an import because
/// `src/program/resolve` is enforced L3-independent (see
/// `tests/program_resolve_harness.rs`'s
/// `resolve_module_has_no_stray_engine_l3_l2_imports` guard). Both copies
/// shared the same char-boundary panic/mis-parse bug (T2.4); if this logic
/// changes again, update `receiver.rs`'s copy too.
///
/// Char-boundary safe by construction: never computes a byte offset from a
/// RE-CASED copy of the string and slices the ORIGINAL with it. That old
/// approach panics whenever a character's `to_lowercase()` byte length
/// differs from its own (e.g. `ẞ`, U+1E9E, 3 bytes → `ß`, 2 bytes) and, even
/// where it doesn't panic, silently misjudges the whitespace boundary for
/// characters that GROW under lowering (e.g. Turkish `İ`, U+0130, 2 bytes →
/// `i̇`, 3 bytes), leaving a real `İ Temporary` table name un-stripped.
/// Instead this walks `char_indices()` on the original string directly and
/// ASCII-folds each char against the literal ASCII word "temporary" (a
/// faithful port of the TS `/i` flag, itself a simple ASCII fold for a
/// pure-ASCII pattern) — every byte offset used to slice comes straight from
/// `trimmed_end`, so it is always a valid char boundary.
fn strip_trailing_temporary(s: &str) -> String {
    const WORD: &str = "temporary";
    let trimmed_end = s.trim_end();
    let indices: Vec<(usize, char)> = trimmed_end.char_indices().collect();
    let word_len = WORD.chars().count();
    if indices.len() <= word_len {
        // No room for a preceding whitespace char — `\s+temporary` needs one.
        return trimmed_end.to_string();
    }
    let tail_start = indices.len() - word_len;
    let tail_matches = indices[tail_start..]
        .iter()
        .zip(WORD.chars())
        .all(|(&(_, c), w)| c.eq_ignore_ascii_case(&w));
    if !tail_matches || !indices[tail_start - 1].1.is_whitespace() {
        return trimmed_end.to_string();
    }
    let word_byte_start = indices[tail_start].0;
    trimmed_end[..word_byte_start].to_string()
}

/// Resolve a source-table / extends-target reference — which may be a table NAME
/// (native AL source: `SourceTable = "Service Invoice Header"`) OR a table NUMBER
/// (dependency `.app` symbols emit `SourceTable` / extends targets as the table's
/// object number, e.g. `"5992"`) — to its internal `L3Table.id`. A numeric ref is
/// routed through the `Table` object-number index to recover the table name, then
/// to the `L3Table`. Returns `None` when nothing resolves (out-of-source table).
fn resolve_table_ref_to_id(symbols: &SymbolTable, table_ref: &str) -> Option<String> {
    if let Ok(number) = table_ref.trim().parse::<i64>() {
        // Numeric (dep-symbol) form: object-number → table object → name → L3Table.
        let obj = symbols.object_by_type_number("Table", number)?;
        return symbols.table_by_name(&obj.name).map(|t| t.id.clone());
    }
    symbols.table_by_name(table_ref).map(|t| t.id.clone())
}

/// Run the 3-pass record-type resolution over one routine, mutating each record
/// var / op's resolved `table_id` (internal TableId; the L3 dump projects it as a
/// StableTableId). Mirrors `resolveRecordTypes`'s per-routine body EXACTLY.
pub fn resolve_routine_record_types(
    routine: &mut L3Routine,
    object: Option<&L3Object>,
    symbols: &SymbolTable,
) {
    // --- pass 1: resolve declared record variables ---------------------------
    // name (lowercased) -> index into record_variables, for matching ops below.
    use std::collections::HashMap;
    let mut var_index_by_name: HashMap<String, usize> = HashMap::new();
    for (i, variable) in routine.record_variables.iter_mut().enumerate() {
        // intentionally last-wins — record_variables has no name duplicates at this layer (Task 3 global-promotion must preserve this invariant)
        var_index_by_name.insert(variable.name.to_lowercase(), i);
        if let Some(table_name) = &variable.table_name {
            // `resolve_table_ref_to_id` accepts a NAME or a NUMBER — a synthetic
            // implicit `Rec` seeded from a codeunit `TableNo = <number>` carries the
            // number string, which `table_by_name` alone could not resolve.
            if let Some(tid) = resolve_table_ref_to_id(symbols, table_name) {
                variable.table_id = Some(tid);
            }
        }
    }

    // --- pass 2a: resolve record ops against their declaring record var ------
    // Task 3 (temp-state): besides tableId, (re)derive `op.temp_state` from the
    // matched record var. After global promotion, `record_variables` carries the
    // object-global record vars (their Known(true/false) temp signal the L2 body
    // walk never saw — it only knew params/locals), so a member-var op like
    // `Files.DeleteAll()` now resolves to the global's Known(true) instead of the
    // L2-forwarded Unknown (the CDO false-critical root cause). `record_variables`
    // is NAME-UNIQUE here (promotion skips shadowed globals), so the last-wins
    // `var_index_by_name` map above resolves each name to its single (innermost,
    // because shadowed globals were never added) record var — honoring AL
    // shadowing without needing first-wins.
    for op in routine.record_operations.iter_mut() {
        if let Some(&vi) = var_index_by_name.get(&op.record_variable_name.to_lowercase()) {
            let variable = &routine.record_variables[vi];
            op.record_variable_id = Some(variable.id.clone());
            if let Some(tid) = &variable.table_id {
                op.table_id = Some(tid.clone());
            }
            op.temp_state = Some(variable.temp_state.clone());
        }
    }

    // --- pass 2b: lexical-scope fallback over features.variables --------------
    // FIRST-wins on a name collision (innermost declaration wins). `routine.variables`
    // is ordered params → locals → globals, so the first entry for a name is the
    // innermost (param/local) scope. Using `.entry().or_insert()` means a later
    // global with the same name is silently ignored rather than clobbering the
    // local — the correct AL lexical-scope rule, and a prerequisite for the
    // tempState backfill (Task 3) that promotes globals into the list.
    // The unset guard below is essential — never override a tableId pass (1)/(2a)
    // already set.
    let mut variable_decl_by_name: HashMap<String, String> = HashMap::new();
    for v in routine.variables.iter() {
        variable_decl_by_name
            .entry(v.name.to_lowercase())
            .or_insert(v.declared_type.clone());
    }
    for op in routine.record_operations.iter_mut() {
        if op.table_id.is_some() {
            continue;
        }
        let Some(decl) = variable_decl_by_name.get(&op.record_variable_name.to_lowercase()) else {
            continue;
        };
        let Some(table_name) = record_table_name_of(decl) else {
            continue;
        };
        if let Some(table) = symbols.table_by_name(&table_name) {
            op.table_id = Some(table.id.clone());
        }
    }

    // --- pass 3: implicit Rec/xRec via the object's effective own table -------
    // NEVER overrides an explicit local `Rec` (the unset guard). The effective
    // own table:
    //   Table          → the table itself (object.name).
    //   Page           → the declared SourceTable (object.source_table_name).
    //   TableExtension → the EXTENDED table (object.extends_target_name).
    //   PageExtension  → the base page's SourceTable (resolve the base page via the
    //                    extends target, then read ITS source_table_name).
    let own_table_id: Option<String> = match object {
        Some(obj) => match obj.object_type.as_str() {
            "Table" => resolve_table_ref_to_id(symbols, &obj.name),
            "Page" => obj
                .source_table_name
                .as_deref()
                .and_then(|st| resolve_table_ref_to_id(symbols, st)),
            "TableExtension" => obj
                .extends_target_name
                .as_deref()
                .and_then(|et| resolve_table_ref_to_id(symbols, et)),
            "PageExtension" => obj.extends_target_name.as_ref().and_then(|et| {
                let base_page = symbols.object_by_type_name("Page", et)?;
                let st = base_page.source_table_name.as_deref()?;
                resolve_table_ref_to_id(symbols, st)
            }),
            _ => None,
        },
        None => None,
    };

    if let Some(own_id) = own_table_id {
        for op in routine.record_operations.iter_mut() {
            if op.table_id.is_some() {
                continue;
            }
            let name = op.record_variable_name.to_lowercase();
            if name != "rec" && name != "xrec" {
                continue;
            }
            op.table_id = Some(own_id.clone());
        }
        // d22 FN fix: the IMPLICIT `Rec` record VARIABLE (registered at L2 with no
        // table_name so it never hijacks the extends resolution above) gets its
        // table_id from the SAME effective-own-table, so field accesses on the
        // implicit Rec resolve their table (d22 FlowField check, etc.). Only fills
        // an unresolved `rec`/`xrec` var — never overrides a declared one.
        for rv in routine.record_variables.iter_mut() {
            if rv.table_id.is_some() {
                continue;
            }
            let name = rv.name.to_lowercase();
            if name != "rec" && name != "xrec" {
                continue;
            }
            rv.table_id = Some(own_id.clone());
        }
    }

    // --- pass: RecordRef GetTable / OpenTemporary local-only tempState (Task 12 / G6) ----
    //
    // A RecordRef variable (declaredType == "RecordRef") has NO associated table, so passes
    // 1–3 above never touch its record ops (they leave temp_state = Unknown). Two forms CAN
    // set it deterministically, but ONLY when the call is unconditional (no branching in the
    // routine at all AND the call site is not inside a loop):
    //
    //   RecRef.Open(no, true)  → Known(true)   (OpenTemporary form)
    //   RecRef.Open(no)        → Known(false)  (no second arg ⇒ non-temporary)
    //   RecRef.Open(no, false) → Known(false)  (explicit false)
    //   RecRef.GetTable(SomeRec) → SomeRec's tempState (from record_variables by name)
    //
    // SOUNDNESS: only set Known(true) from exactly `Open(_, true)` or GetTable of a
    // Known(true) source.  Anything uncertain (conditional, in-loop, unknown second arg,
    // unknown source var) → leave Unknown (conservative; never wrongly Known(true)).
    //
    // The derivation only fires when has_branching == false so we don't need to inspect
    // individual call-site control contexts (which are not yet populated at this stage).
    if !routine.has_branching {
        // Build set of RecordRef var names (lc) from the routine's variable list.
        let recordref_var_names: std::collections::HashSet<String> = routine
            .variables
            .iter()
            .filter(|v| v.declared_type.to_lowercase() == "recordref")
            .map(|v| v.name.to_lowercase())
            .collect();

        if !recordref_var_names.is_empty() {
            // Build the record-var temp map (lc name → known temp value) for GetTable.
            let record_var_temp: HashMap<String, Option<bool>> = routine
                .record_variables
                .iter()
                .map(|rv| (rv.name.to_lowercase(), rv.temp_state_known_value()))
                .collect();

            // Scan call sites for GetTable / Open on RecordRef vars (unconditional only).
            let mut recref_temp: HashMap<String, crate::engine::l2::features::PTempState> =
                HashMap::new();
            for cs in &routine.call_sites {
                // Must not be inside a loop.
                if !cs.loop_stack.is_empty() {
                    continue;
                }
                let crate::engine::l2::features::PCallee::Member { receiver, method } = &cs.callee
                else {
                    continue;
                };
                let receiver_lc = receiver.to_lowercase();
                if !recordref_var_names.contains(&receiver_lc) {
                    continue;
                }
                let method_lc = method.to_lowercase();
                match method_lc.as_str() {
                    "open" => {
                        // RecRef.Open(no)        → Known(false)
                        // RecRef.Open(no, false) → Known(false)
                        // RecRef.Open(no, true)  → Known(true)
                        // RecRef.Open(no, <other>) → skip (Unknown, conservative)
                        let second_arg = cs.argument_texts.get(1).map(|t| t.trim().to_lowercase());
                        let ts = match second_arg.as_deref() {
                            None => Some(crate::engine::l2::scope::ts_known(false)),
                            Some("false") => Some(crate::engine::l2::scope::ts_known(false)),
                            Some("true") => Some(crate::engine::l2::scope::ts_known(true)),
                            Some(_) => None, // non-literal second arg → conservative Unknown
                        };
                        if let Some(ts) = ts {
                            recref_temp.insert(receiver_lc, ts);
                        }
                    }
                    "gettable" => {
                        // RecRef.GetTable(SomeRec) → inherit SomeRec's tempState.
                        // First arg must be a bare identifier naming a known record var.
                        let Some(source_name) = cs.argument_texts.first() else {
                            continue;
                        };
                        let source_lc = source_name.trim().to_lowercase();
                        // Only bare identifiers (no dots, no parens, no spaces after trim).
                        if source_lc.contains('.')
                            || source_lc.contains('(')
                            || source_lc.contains(' ')
                        {
                            continue;
                        }
                        let Some(known_val) = record_var_temp.get(&source_lc).copied().flatten()
                        else {
                            continue; // source var absent or not Known → Unknown
                        };
                        recref_temp
                            .insert(receiver_lc, crate::engine::l2::scope::ts_known(known_val));
                    }
                    _ => {}
                }
            }

            // Apply derived temp states to matching record ops.
            for op in routine.record_operations.iter_mut() {
                let var_lc = op.record_variable_name.to_lowercase();
                if let Some(ts) = recref_temp.get(&var_lc) {
                    op.temp_state = Some(ts.clone());
                }
            }
        }
    }

    // --- FINAL override pass (Task 4 / G3, RV-8): table-level temp precedence --
    //
    // "One precedence rule everywhere: table-level temp (`TableType = Temporary`)
    // ⇒ Known(true) REGARDLESS of var modifier or a stamped PD(i)."
    //
    // Runs AFTER ALL table_id resolution above (declared vars, ops, lexical
    // fallback, AND implicit Rec/xRec pass-3), so `table_id` is FINAL — including
    // implicit-Rec ops in a Table object's own triggers. For each record op whose
    // resolved table is `is_temporary`, force `temp_state = Known(true)`; apply the
    // same to the matching record VARIABLE (so a by-var PARAM of a temp table
    // reports Known(true), not the PD(i) stamped at L2 — RV-8).
    //
    // PRECEDENCE: the table-level override WINS over everything (keyword,
    // no-keyword, by-value, by-var, PD). It is purely ADDITIVE toward Known(true)
    // — it only UPGRADES; it NEVER downgrades a Known(true) to false and NEVER
    // forces Known(false). The only signal is the exact structural `TableType`
    // property, so the upgrade is sound.
    for op in routine.record_operations.iter_mut() {
        let Some(tid) = &op.table_id else { continue };
        let is_temp = symbols.table_by_id(tid).map(|t| t.is_temporary) == Some(true);
        if is_temp {
            op.temp_state = Some(crate::engine::l2::scope::ts_known(true));
        }
    }
    for variable in routine.record_variables.iter_mut() {
        let Some(tid) = &variable.table_id else {
            continue;
        };
        let is_temp = symbols.table_by_id(tid).map(|t| t.is_temporary) == Some(true);
        if is_temp {
            variable.temp_state = crate::engine::l2::scope::ts_known(true);
        }
    }

    // --- routine ENTRY-guard override (G-2 Part 2) -----------------------------
    //
    // `if not <X>.IsTemporary[()] then Error(...)` as the routine's FIRST
    // executable statement (detected structurally at L3 assembly —
    // `entry_temp_guard_receiver_of` in l3_workspace.rs) PROVES `<X>` is
    // temporary for the entire body: the routine errors at runtime otherwise,
    // so every op it performs on `<X>` only ever runs on a temp record. Force
    // `Known(true)` on `<X>`'s ops + matching record var (the runtime-guard
    // analog of the structural `TableType = Temporary` override above).
    //
    // Purely ADDITIVE toward Known(true) — never downgrades, never forces
    // Known(false). Only the EXACT entry-guard shape sets the receiver; any
    // deviation left it `None` (conservative — detectors keep firing).
    if let Some(guard_receiver) = routine.entry_temp_guard_receiver.clone() {
        let guard_receiver = guard_receiver.as_str();
        for op in routine.record_operations.iter_mut() {
            if op.record_variable_name.eq_fold_identifier(guard_receiver) {
                op.temp_state = Some(crate::engine::l2::scope::ts_known(true));
            }
        }
        for variable in routine.record_variables.iter_mut() {
            if variable.name.eq_fold_identifier(guard_receiver) {
                variable.temp_state = crate::engine::l2::scope::ts_known(true);
            }
        }
    }

    // --- page SourceTableTemporary override (Task 5 / G4, RV-8) ---------------
    //
    // A page declared `SourceTableTemporary = true` always loads its SourceTable
    // as a temporary copy — the implicit `Rec` and `xRec` are ALWAYS temporary.
    // When `object.source_table_temporary == Some(true)`, force `Known(true)` on
    // every record op whose variable name (lowercased) is `rec` or `xrec`.
    //
    // Purely ADDITIVE toward Known(true) — never downgrades. Runs after the
    // table-level override so both can apply independently; they compose without
    // interference (both only upgrade → Known(true), never downgrade).
    if object.map(|o| o.source_table_temporary == Some(true)) == Some(true) {
        for op in routine.record_operations.iter_mut() {
            let name = op.record_variable_name.to_lowercase();
            if name == "rec" || name == "xrec" {
                op.temp_state = Some(crate::engine::l2::scope::ts_known(true));
            }
        }
        // The implicit Rec/xRec record VARIABLE (d22 FN registration) must agree —
        // on a SourceTableTemporary page it is genuinely temporary. Without this the
        // L2-default Known(false) would wrongly mark a temp-page Rec as physical.
        for variable in routine.record_variables.iter_mut() {
            let name = variable.name.to_lowercase();
            if name == "rec" || name == "xrec" {
                variable.temp_state = crate::engine::l2::scope::ts_known(true);
            }
        }
        // ...and so must a call argument that passes the implicit Rec/xRec: a
        // callee's `var` record param bound to it writes the temporary copy.
        // L2 leaves such a binding without a temp state when no Rec variable
        // is declared, which the L4 PD substitution and the capability cone
        // read as Unknown (physical).
        for cs in routine.call_sites.iter_mut() {
            for b in cs.argument_bindings.iter_mut() {
                if b.source_kind == "implicit-rec" {
                    b.source_temp_state = Some(crate::engine::l2::scope::ts_known(true));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_table_name_of_basics() {
        assert_eq!(
            record_table_name_of("Record Customer").as_deref(),
            Some("Customer")
        );
        assert_eq!(
            record_table_name_of("Record \"Sales Line\"").as_deref(),
            Some("Sales Line")
        );
        assert_eq!(
            record_table_name_of("Record \"X\" temporary").as_deref(),
            Some("X")
        );
        assert_eq!(
            record_table_name_of("Record Customer temporary").as_deref(),
            Some("Customer")
        );
        assert_eq!(record_table_name_of("Integer"), None);
        assert_eq!(record_table_name_of("Codeunit \"Foo\""), None);
        assert_eq!(record_table_name_of("RecordRef"), None);
        assert_eq!(record_table_name_of("Record"), None);
        // Parity landmine: `Record temporary` has NO whitespace BEFORE
        // "temporary" once `Record` is consumed, so TS's `/\s+temporary\s*$/i`
        // does NOT match — rest stays "temporary" and is returned verbatim (the
        // table name "temporary"). Reproduce, don't "fix".
        assert_eq!(
            record_table_name_of("Record temporary").as_deref(),
            Some("temporary")
        );
        // With a real name + trailing modifier the strip DOES fire.
        assert_eq!(
            record_table_name_of("Record  Item   temporary  ").as_deref(),
            Some("Item")
        );
    }

    // -- T2.4 (3): strip_trailing_temporary char-boundary safety --

    #[test]
    fn strip_trailing_temporary_unicode_prefix_no_space_does_not_panic() {
        // "ẞ" (U+1E9E, 3 UTF-8 bytes) lowercases to "ß" (2 bytes). The old
        // implementation sliced the ORIGINAL string with a byte offset computed
        // from the LOWERCASED copy — panics mid-char when "ẞ" sits directly
        // against "temporary" (no separating whitespace, so `\s+temporary`
        // shouldn't strip anyway; the point is reaching that decision must not
        // panic).
        assert_eq!(strip_trailing_temporary("ẞtemporary"), "ẞtemporary");
    }

    #[test]
    fn strip_trailing_temporary_turkish_i_strips_correctly() {
        // "İ" (Turkish dotted capital I, U+0130, 2 bytes) lowercases to "i̇"
        // (i + combining dot above, 3 bytes) — GROWS. The old byte-math bug
        // computed a boundary that landed on the wrong byte, so the
        // "preceded by whitespace" check failed and " temporary" was silently
        // left unstripped. Must correctly strip "temporary" (trailing
        // whitespace before the word is kept, same as the other cases below —
        // callers `.trim()` the result).
        assert_eq!(strip_trailing_temporary("İ temporary"), "İ ");
    }

    #[test]
    fn strip_trailing_temporary_normal_cases_regression() {
        assert_eq!(
            strip_trailing_temporary("Sales Header Temporary"),
            "Sales Header "
        );
        assert_eq!(strip_trailing_temporary("Sales Header"), "Sales Header");
        assert_eq!(strip_trailing_temporary("temporary"), "temporary");
        assert_eq!(strip_trailing_temporary("X  TEMPORARY"), "X  ");
    }
}
