//! Root-classification substrate — originally a Rust port of al-sem's §4.3 root-classifier.
//!
//! Ports (originally byte-parity-ported against al-sem's TS source, which lived at
//! `U:\Git\al-sem`; that oracle is now retired and the port is Rust-owned):
//!   - `src/model/root-classification.ts` — [`RootKind`], [`ROOT_KIND_VALUES`],
//!     [`is_externally_reachable_kind`], [`RootClassification`].
//!   - `src/engine/root-classifier.ts` — [`classify_roots`] (AST-only pass).
//!   - `src/config/roots-config.ts` — [`load_roots_config`] (file → validated config).
//!   - `src/engine/root-classifier-overlay.ts` — [`overlay_config_roots`] (config merge).
//!
//! Determinism (R4-F spec Rev 2):
//!   - `kinds` are ordered by ROOT_KIND declaration order, NOT alphabetical —
//!     reproduced via `ROOT_KIND_VALUES.iter().filter(...)`.
//!   - No HashMap/HashSet iteration leaks into output: an accumulator
//!     [`std::collections::BTreeMap`] keyed by internal RoutineId backs the
//!     overlay; the AST pass sorts its `Vec` by RoutineId (ASCII ordinal).
//!   - The internal `RoutineId` is ASCII slash-form, so `<` ordering on the
//!     `String` matches al-sem's `a < b` on its `RoutineId` string.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::engine::l3::l3_workspace::{L3Object, L3Routine, L3Workspace};

// ---------------------------------------------------------------------------
// RootKind — the 13-value union, declaration order is ROOT_KIND_VALUES.
// ---------------------------------------------------------------------------

/// Canonical RootKind values in declaration order (al-sem `ROOT_KIND_VALUES`).
/// The single source of truth for valid kinds + the canonical sort order.
///
/// ELEVEN of the thirteen are DERIVED by the AST pass ([`kinds_for`]). The remaining
/// two — `web-service-exposed` and `job-queue-entrypoint` — are currently
/// supplied only by the config overlay. Source may indicate capability; the
/// overlay supplies user-asserted publication or scheduling information (a
/// config-only root carries `confidence: "user-asserted"`, see
/// [`overlay_config_roots`]).
///
/// Half of that split is asserted executably, and the honest half is worth
/// naming: `ast_pass_emits_exactly_the_eleven_derivable_kinds` proves the eleven ARE
/// emitted, over one hand-built witness set, and
/// `declared_kinds_minus_the_derivable_eleven_are_exactly_the_overlay_only_two`
/// bounds the complement over the CONSTANTS.
///
/// Neither proves the other direction -- that the remaining two are NEVER
/// emitted. A twelfth insertion added in a branch no witness exercises (a
/// `Query` arm, say) would leave both tests green. Saying "asserted executably"
/// flat would be the over-claim CLAUDE.md legislates against.
pub const ROOT_KIND_VALUES: [&str; 13] = [
    "trigger-table",
    "trigger-page",
    // Derived: a Page / PageExtension trigger whose enclosing member is an action
    // wrapper — see [`is_page_action_wrapper`] for which wrappers qualify.
    "page-action",
    "report-trigger",
    "event-subscriber",
    "install-codeunit",
    "upgrade-codeunit",
    "api-page",
    // Overlay-only (see the note above): source may indicate the capability, the
    // overlay supplies the user-asserted publication information.
    "web-service-exposed",
    // Overlay-only (see the note above): source may indicate the capability, the
    // overlay supplies the user-asserted scheduling information.
    "job-queue-entrypoint",
    "public-procedure",
    "test-procedure",
    // Appended (never inserted): `canonical_kinds` filters this array in
    // declaration order, so appending leaves every existing kind's relative
    // order — and therefore every existing golden — unchanged.
    "onrun-codeunit",
];

/// All current kinds are externally reachable (al-sem `isExternallyReachableKind`).
fn is_externally_reachable_kind(kind: &str) -> bool {
    ROOT_KIND_VALUES.contains(&kind)
}

/// Canonicalize a kind set to the documented invariant: deduped + sorted in
/// ROOT_KIND declaration order. Mirrors `ROOT_KIND_ORDER.filter(k => set.has(k))`.
fn canonical_kinds(set: &BTreeSet<String>) -> Vec<String> {
    ROOT_KIND_VALUES
        .iter()
        .filter(|k| set.contains(**k))
        .map(|k| k.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// RootClassification — the per-routine output entry.
// ---------------------------------------------------------------------------

/// Per-routine classification with full provenance (al-sem `RootClassification`).
/// `sourceAnchor` is intentionally NOT carried here — the R4-F stable projection
/// omits it, and no Rust consumer (d50/d51 lookup by routine id) needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootClassification {
    /// Internal RoutineId (`${modelInstanceId}/${hash}`).
    pub routine_id: String,
    pub kinds: Vec<String>,
    pub externally_reachable: bool,
    /// "ast" | "config" | "ast+config".
    pub source: String,
    /// "static" | "user-asserted".
    pub confidence: String,
    pub config_entry_id: Option<String>,
    /// "resolved" | "ambiguous" | "unresolved".
    pub resolution_status: Option<String>,
}

// ---------------------------------------------------------------------------
// AST classifier — classify_roots (root-classifier.ts).
// ---------------------------------------------------------------------------

/// True when a member-trigger's enclosing wrapper is an action DECLARATION — the
/// discriminator behind the derived `page-action` root kind.
///
/// Four grammar rules qualify, and the evidence behind each differs. It is
/// recorded per wrapper so that "matched" is never read as "covered":
///
/// - `action_declaration` — real, and exercised end to end by the corpus
///   (`tests/r0-corpus/ws-sibling-member-triggers`).
/// - `systemaction_declaration` — real: Microsoft's PromptDialog page type
///   documents an `OnAction()` trigger under `systemaction(Generate)`.
/// - `fileuploadaction_declaration` — real: a documented trigger
///   `OnAction(Files: List of [FileUpload])` (note the non-empty signature).
/// - `customaction_declaration` — matched DEFENSIVELY, with **no coverage
///   claimed**. It is not verified as trigger-bearing: Microsoft documents
///   customaction as a client-invoked Power Automate flow (`CustomActionType =
///   Flow`, `FlowId`), a shape with no AL routine to classify. It is matched
///   because a false NEGATIVE on a real form costs more than dead code.
///
/// Two neighbouring rules share the same optional `declaration_body` and are
/// deliberately EXCLUDED:
///
/// - `separator_action` — a visual separator is not invokable. It IS reachable as
///   an enclosing member when written `separator(Name)` (its `name` field is
///   optional), so this exclusion is a live path, not a hypothetical one.
/// - `actionref_declaration` — promotes an action declared elsewhere, and that
///   action's own trigger is already the root. It is additionally UNREACHABLE as
///   an enclosing member: it carries `promoted_name` / `action_name` and no
///   `name` field, so the lowerer's name gate never captures it. Its test pins
///   intent, not a reachable path.
///
/// COVERED, and worth saying so because the gap below invites the opposite
/// assumption: a `PageExtension`'s `addlast(area) { action(X) { trigger
/// OnAction() } }` DOES get `page-action`. `addlast_action_modification`
/// carries `target` and no `name` so it is not captured itself, but its body is
/// `action_body`, which ends in `_body` and therefore INHERITS, so the walk
/// descends to the real inner `action_declaration` and captures that
/// (`grammar.js:2458-2466,2315-2320`). The same holds for the other `add*`
/// forms. The difference from the gap below is structural, not incidental:
/// `add*` introduces a new action node, `modify` alters an existing one, so
/// there is no `action_declaration` to find.
///
/// NOT A GAP either, but easily mistaken for one: a REPORT request-page action.
/// `requestpage { actions { area(x) { action(Y) { trigger OnAction() } } } }`
/// parses all the way down (`lower/mod.rs:619-628` keeps walking through
/// `RequestpageSection`), so this predicate WOULD accept the anchor -- but it is
/// never asked: [`kinds_for`] branches on the OBJECT type first, and a `Report`
/// takes the `report-trigger` arm and never reaches the Page arm. Such a trigger
/// classifies as `report-trigger`. The two untrusted-root consumers are
/// unaffected (`report-trigger` is in both lists exactly as `trigger-page` is),
/// but "no consumer is worse off" would be too strong: EVERY `page-action`-based
/// selection misses these triggers, which includes `alsem fingerprint --roots
/// page-action` AND policy rules scoped by `root.kinds: [page-action]` -- both
/// applicability, which would skip the routine, and an `except:` clause, which
/// would fail to suppress.
///
/// That object-type arm list is a PRE-EXISTING vocabulary boundary and widening
/// it is not this predicate's business: the same list drops `ReportExtension`
/// entirely, and an XmlPort request-page action trigger gets no kind at all and
/// is skipped from the output. Fixing only Report would leave the arm list just
/// as arbitrary as it is now.
///
/// CLOSED (#41), and it used to be the known gap here: an action MODIFICATION
/// (`actions { modify(SomeAction) { trigger OnAfterAction() … } }`) now gains
/// `page-action`. `modify_action_modification` carries `target` and no `name`,
/// so the lowerer needed a wrapper arm plus a target-name fallback before this
/// string could match anything (`crates/al-syntax/src/lower/mod.rs`); with
/// those in place the fifth string below is all this predicate needs.
///
/// The RESIDUAL, and it is the intended answer rather than a leftover: a
/// trigger declared DIRECTLY in an `add*` body — `addlast(Processing) { trigger
/// OnDirect() … }`, a shape the grammar admits — has no declaring member, so it
/// gets no enclosing-member anchor and no `page-action`. Real AL puts such a
/// trigger inside an `action(X)` the `add*` body declares, which IS captured
/// (see the COVERED note above).
fn is_page_action_wrapper(syntax_kind: &str) -> bool {
    matches!(
        syntax_kind,
        "action_declaration"
            | "systemaction_declaration"
            | "fileuploadaction_declaration"
            | "customaction_declaration"
            | "modify_action_modification"
    )
}

/// Compute the set of RootKinds a routine qualifies for, purely from its
/// structural shape + the host object's declared metadata. Mirrors `kindsFor`.
fn kinds_for(routine: &L3Routine, object: &L3Object) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();

    // Trigger kinds — gated on routine.kind === "trigger".
    if routine.kind == "trigger" {
        match object.object_type.as_str() {
            "Table" | "TableExtension" => {
                set.insert("trigger-table".to_string());
            }
            "Page" | "PageExtension" => {
                set.insert("trigger-page".to_string());
                // ADDITIVE: an action's trigger keeps `trigger-page` AND gains
                // `page-action`. The enclosing member wrapper's raw grammar kind
                // reaches here verbatim (`anchor_from_origin` → `PAnchor::syntax_kind`).
                if routine
                    .enclosing_member_range
                    .as_ref()
                    .is_some_and(|a| is_page_action_wrapper(&a.syntax_kind))
                {
                    set.insert("page-action".to_string());
                }
            }
            "Report" => {
                set.insert("report-trigger".to_string());
            }
            _ => {}
        }
    }

    // Codeunit OnRun — the entry point `Codeunit.Run(id)` and the job-queue
    // runner reach. Keyed on routine.kind == "trigger" (NOT the name alone), so a
    // plain procedure that happens to be called OnRun is not a root.
    if routine.kind == "trigger"
        && object.object_type == "Codeunit"
        && routine.name.eq_ignore_ascii_case("OnRun")
    {
        set.insert("onrun-codeunit".to_string());
    }

    // Event-subscriber — direct from routine.kind.
    if routine.kind == "event-subscriber" {
        set.insert("event-subscriber".to_string());
    }

    // Codeunit Subtype-based kinds (case-insensitive).
    if object.object_type == "Codeunit" {
        let subtype = object.object_subtype.as_deref().map(|s| s.to_lowercase());
        if subtype.as_deref() == Some("install") {
            set.insert("install-codeunit".to_string());
        }
        if subtype.as_deref() == Some("upgrade") {
            set.insert("upgrade-codeunit".to_string());
        }
    }

    // Page with PageType=API (case-insensitive) — every routine is HTTP-exposed.
    if (object.object_type == "Page" || object.object_type == "PageExtension")
        && object
            .page_type
            .as_deref()
            .map(|p| p.to_lowercase())
            .as_deref()
            == Some("api")
    {
        set.insert("api-page".to_string());
    }

    // Test procedures — via [Test] attribute on the routine itself.
    if routine
        .attributes_parsed
        .iter()
        .any(|a| a.name.to_lowercase() == "test")
    {
        set.insert("test-procedure".to_string());
    }

    // Public procedures — non-trigger, non-event-subscriber procedures with
    // default access (None accessModifier). Catch-all: only when nothing more
    // specific applied (al-sem checks `kinds.length === 0` BEFORE this push).
    if routine.kind == "procedure" && routine.access_modifier.is_none() && set.is_empty() {
        set.insert("public-procedure".to_string());
    }

    canonical_kinds(&set)
}

/// AST-only root classifier (al-sem `classifyRoots`). Produces a
/// `RootClassification` for every routine that qualifies as >=1 RootKind, sorted
/// by internal RoutineId ascending. Routines whose object is missing are skipped.
pub fn classify_roots(workspace: &L3Workspace) -> Vec<RootClassification> {
    let objects_by_id: BTreeMap<&str, &L3Object> = workspace
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o))
        .collect();

    let mut result: Vec<RootClassification> = Vec::new();
    for routine in &workspace.routines {
        let Some(object) = objects_by_id.get(routine.object_id.as_str()) else {
            continue;
        };
        let kinds = kinds_for(routine, object);
        if kinds.is_empty() {
            continue;
        }
        let externally_reachable = kinds.iter().any(|k| is_externally_reachable_kind(k));
        result.push(RootClassification {
            routine_id: routine.id.clone(),
            kinds,
            externally_reachable,
            source: "ast".to_string(),
            confidence: "static".to_string(),
            config_entry_id: None,
            resolution_status: None,
        });
    }

    // Canonical sort for determinism — RoutineId is an ASCII slash-form string.
    result.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    result
}

// ---------------------------------------------------------------------------
// roots.config.json loader — load_roots_config (roots-config.ts).
// ---------------------------------------------------------------------------

/// A validated roots.config target (al-sem `RootsConfigTarget`).
#[derive(Debug, Clone)]
enum RootsConfigTarget {
    RoutineId(String),
    ObjectRoutine {
        object_id: String,
        routine_name: String,
    },
}

/// A validated roots.config entry (al-sem `RootsConfigEntry`) — only the fields
/// the overlay reads. Diagnostics are NOT part of the projection.
#[derive(Debug, Clone)]
struct RootsConfigEntry {
    id: String,
    target: RootsConfigTarget,
    /// Canonicalized (deduped + ROOT_KIND-ordered) kind list.
    kinds: Vec<String>,
    externally_reachable: Option<bool>,
}

/// A loaded + validated roots.config (al-sem `RootsConfig`). `None` ⇒ missing /
/// malformed at the top level (the overlay then passes AST roots through).
#[derive(Debug, Clone, Default)]
struct RootsConfig {
    roots: Vec<RootsConfigEntry>,
}

/// Parse a target object into one of two accepted shapes. `routineId` takes
/// precedence over `objectId + routineName`. Mirrors `parseTarget`.
fn parse_target(t: &serde_json::Value) -> Option<RootsConfigTarget> {
    let obj = t.as_object()?;
    if let Some(rid) = obj.get("routineId").and_then(|v| v.as_str()) {
        return Some(RootsConfigTarget::RoutineId(rid.to_string()));
    }
    let object_id = obj.get("objectId").and_then(|v| v.as_str());
    let routine_name = obj.get("routineName").and_then(|v| v.as_str());
    if let (Some(o), Some(n)) = (object_id, routine_name) {
        return Some(RootsConfigTarget::ObjectRoutine {
            object_id: o.to_string(),
            routine_name: n.to_string(),
        });
    }
    None
}

/// Validate a parsed JSON value into a `RootsConfig`. Faithfully ports the
/// ACCEPTANCE logic of `validateRootsConfig` (which entries survive + their
/// canonicalized kinds). Diagnostics text is intentionally not reproduced.
fn validate_roots_config(parsed: &serde_json::Value) -> Option<RootsConfig> {
    let obj = parsed.as_object()?;
    // version === 1. al-sem gates on JS `obj.version !== 1` (numeric), which accepts
    // `1`, `1.0`, and `1e0` alike — so compare numerically via `as_f64`, not `as_i64`
    // (the latter would reject the float spelling `1.0` and discard the whole config).
    if obj.get("version").and_then(|v| v.as_f64()) != Some(1.0) {
        return None;
    }
    let roots = obj.get("roots").and_then(|v| v.as_array())?;

    let valid_kinds: BTreeSet<&str> = ROOT_KIND_VALUES.iter().copied().collect();
    let mut entries: Vec<RootsConfigEntry> = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for entry_v in roots {
        let Some(entry) = entry_v.as_object() else {
            continue;
        };
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if seen_ids.contains(id) {
            continue;
        }
        let Some(target) = entry.get("target").and_then(parse_target) else {
            continue;
        };
        let Some(kinds_arr) = entry.get("kinds").and_then(|v| v.as_array()) else {
            continue;
        };
        // Canonicalize: dedup (silent) + sort in ROOT_KIND_VALUES order.
        let mut kind_set: BTreeSet<String> = BTreeSet::new();
        for k in kinds_arr {
            if let Some(ks) = k.as_str()
                && valid_kinds.contains(ks)
            {
                kind_set.insert(ks.to_string());
            }
        }
        let kinds = canonical_kinds(&kind_set);
        if kinds.is_empty() {
            continue;
        }

        // Optional externallyReachable: present-and-boolean → carry; else omit.
        let externally_reachable = match entry.get("externallyReachable") {
            Some(v) => v.as_bool(), // wrong-typed → None (dropped), matching al-sem.
            None => None,
        };

        entries.push(RootsConfigEntry {
            id: id.to_string(),
            target,
            kinds,
            externally_reachable,
        });
        seen_ids.insert(id.to_string());
    }

    Some(RootsConfig { roots: entries })
}

/// True when `<workspaceRoot>/roots.config.json` exists AND loads+validates —
/// i.e. al-sem's `model.identity.rootsConfig !== undefined`. The cli-b snapshot
/// `deriveInputs` includes a `roots-config` input iff this holds. Engine-never-
/// throws (the inner loader is total).
pub fn roots_config_was_loaded(workspace_root: &Path) -> bool {
    load_roots_config(workspace_root).is_some()
}

/// Load + validate `<workspaceRoot>/roots.config.json` (al-sem `loadRootsConfig`).
/// Missing file ⇒ `None` (clean empty, the common case). Parse / validation
/// failure ⇒ `None`. Never throws / panics.
fn load_roots_config(workspace_root: &Path) -> Option<RootsConfig> {
    let path = workspace_root.join("roots.config.json");
    let bytes = std::fs::read(&path).ok()?;
    // Strip a leading UTF-8 BOM (EF BB BF) before JSON parse — mirrors the
    // `charCodeAt(0) === 0xFEFF` slice in al-sem (here on the byte form).
    let slice: &[u8] = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        &bytes[..]
    };
    let text = std::str::from_utf8(slice).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    validate_roots_config(&parsed)
}

// ---------------------------------------------------------------------------
// Overlay — overlay_config_roots (root-classifier-overlay.ts).
// ---------------------------------------------------------------------------

/// Resolve a config target to the matching routines (al-sem `resolveTarget`).
/// `routineId` → exact id match (0 or 1). `objectId + routineName` →
/// case-insensitive name match within the object (may be multiple).
fn resolve_target<'a>(
    target: &RootsConfigTarget,
    workspace: &'a L3Workspace,
) -> Vec<&'a L3Routine> {
    match target {
        RootsConfigTarget::RoutineId(rid) => workspace
            .routines
            .iter()
            .find(|r| &r.id == rid)
            .into_iter()
            .collect(),
        RootsConfigTarget::ObjectRoutine {
            object_id,
            routine_name,
        } => {
            let lc = routine_name.to_lowercase();
            workspace
                .routines
                .iter()
                .filter(|r| &r.object_id == object_id && r.name.to_lowercase() == lc)
                .collect()
        }
    }
}

/// Merge a `RootsConfig` overlay on top of the AST classification result
/// (al-sem `overlayConfigRoots`). `config` None ⇒ AST roots pass through.
///
/// Precedence discipline mirrors al-sem exactly: `ast_by_routine` is a FROZEN
/// snapshot of the AST baseline; `by_routine` is the accumulator (a `BTreeMap`
/// here for deterministic, hash-free iteration). A second config entry on the
/// same routine still unions against the ORIGINAL AST kinds, not entry-1's
/// merged result.
/// A single infrastructure diagnostic emitted during root-classification overlay.
/// Mirrors the `Diagnostic` shape from `src/model/finding.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfraDiagnostic {
    pub severity: String,
    pub stage: String,
    pub message: String,
}

fn overlay_config_roots(
    ast_roots: Vec<RootClassification>,
    config: Option<&RootsConfig>,
    workspace: &L3Workspace,
) -> (Vec<RootClassification>, Vec<InfraDiagnostic>) {
    let Some(config) = config else {
        return (ast_roots, vec![]);
    };

    let mut diagnostics: Vec<InfraDiagnostic> = Vec::new();

    let ast_by_routine: BTreeMap<String, RootClassification> = ast_roots
        .iter()
        .map(|r| (r.routine_id.clone(), r.clone()))
        .collect();
    let mut by_routine: BTreeMap<String, RootClassification> = ast_by_routine.clone();

    for entry in &config.roots {
        let mut matches = resolve_target(&entry.target, workspace);
        // Sort matches by internal id (ASCII ordinal), first wins.
        matches.sort_by(|a, b| a.id.cmp(&b.id));

        if matches.is_empty() {
            continue;
        }
        let ambiguous = matches.len() > 1;
        let winner = matches[0];
        let existing_ast = ast_by_routine.get(&winner.id);
        let cfg_kind_set: BTreeSet<String> = entry.kinds.iter().cloned().collect();

        let resolution_status = if ambiguous {
            "ambiguous".to_string()
        } else {
            "resolved".to_string()
        };

        match existing_ast {
            None => {
                // Config-only root: no AST signal, "user-asserted" confidence.
                let kinds = canonical_kinds(&cfg_kind_set);
                if kinds.is_empty() {
                    continue;
                }
                let externally_reachable = entry
                    .externally_reachable
                    .unwrap_or_else(|| kinds.iter().any(|k| is_externally_reachable_kind(k)));
                by_routine.insert(
                    winner.id.clone(),
                    RootClassification {
                        routine_id: winner.id.clone(),
                        kinds,
                        externally_reachable,
                        source: "config".to_string(),
                        confidence: "user-asserted".to_string(),
                        config_entry_id: Some(entry.id.clone()),
                        resolution_status: Some(resolution_status),
                    },
                );
            }
            Some(existing) => {
                // AST + config corroboration: union kinds, upgrade to "static".
                // Union against the ORIGINAL (frozen) AST kind set.
                let ast_kind_set: BTreeSet<String> = existing.kinds.iter().cloned().collect();
                let cfg_kind_set_existing: BTreeSet<String> = entry.kinds.iter().cloned().collect();

                // kinds-mismatch diagnostic: emit when AST and config disagree.
                // Mirrors al-sem `root-classifier-overlay.ts` lines 145-158.
                let only_ast: Vec<String> = ROOT_KIND_VALUES
                    .iter()
                    .filter(|&&k| ast_kind_set.contains(k) && !cfg_kind_set_existing.contains(k))
                    .map(|&k| k.to_string())
                    .collect();
                let only_cfg: Vec<String> = ROOT_KIND_VALUES
                    .iter()
                    .filter(|&&k| cfg_kind_set_existing.contains(k) && !ast_kind_set.contains(k))
                    .map(|&k| k.to_string())
                    .collect();
                if !only_ast.is_empty() || !only_cfg.is_empty() {
                    // JSON-encode the kind arrays (compact, no spaces) to match al-sem's
                    // `JSON.stringify(onlyAst)` / `JSON.stringify(onlyCfg)`.
                    let only_ast_json =
                        serde_json::to_string(&only_ast).unwrap_or_else(|_| "[]".to_string());
                    let only_cfg_json =
                        serde_json::to_string(&only_cfg).unwrap_or_else(|_| "[]".to_string());
                    diagnostics.push(InfraDiagnostic {
                        severity: "warning".to_string(),
                        stage: "discover".to_string(),
                        message: format!(
                            "[roots-config/kinds-mismatch] roots.config.json entry \"{}\" \
                             disagrees with AST: ast-only={only_ast_json}, \
                             config-only={only_cfg_json}.",
                            entry.id
                        ),
                    });
                }

                let mut unioned: BTreeSet<String> = ast_kind_set;
                unioned.extend(entry.kinds.iter().cloned());
                let kinds = canonical_kinds(&unioned);
                let externally_reachable = entry
                    .externally_reachable
                    .unwrap_or_else(|| kinds.iter().any(|k| is_externally_reachable_kind(k)));
                by_routine.insert(
                    winner.id.clone(),
                    RootClassification {
                        routine_id: winner.id.clone(),
                        kinds,
                        externally_reachable,
                        source: "ast+config".to_string(),
                        confidence: "static".to_string(),
                        config_entry_id: Some(entry.id.clone()),
                        resolution_status: Some(resolution_status),
                    },
                );
            }
        }
    }

    let mut roots: Vec<RootClassification> = by_routine.into_values().collect();
    roots.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    (roots, diagnostics)
}

// ---------------------------------------------------------------------------
// Top-level entry — mirror src/index.ts ~lines 255-271.
// ---------------------------------------------------------------------------

/// Compute `model.rootClassifications`: classify the AST roots, load any
/// `<workspace>/roots.config.json`, then overlay the config on the AST roots.
/// `workspace_root` is `None` for the inline/cross-app paths that have no disk
/// config (⇒ AST-only). Mirrors al-sem's index.ts wiring.
///
/// Returns `(classifications, infra_diagnostics)`. Infrastructure diagnostics
/// (e.g. `kinds-mismatch` warnings) are threaded up to the caller for inclusion
/// in the JSON envelope.
pub fn compute_root_classifications(
    workspace: &L3Workspace,
    workspace_root: Option<&Path>,
) -> (Vec<RootClassification>, Vec<InfraDiagnostic>) {
    let ast_roots = classify_roots(workspace);
    let config = workspace_root.and_then(load_roots_config);
    overlay_config_roots(ast_roots, config.as_ref(), workspace)
}

// ---------------------------------------------------------------------------
// R4-F stable projection — the differential surface (mirrors
// scripts/r4f-root-classification-projection.ts).
//
// Field order is LOAD-BEARING (must byte-match the al-sem golden's serde order):
//   - outer: fixtureName, classificationCount, classifications
//   - inner: routineId, kinds, externallyReachable, source, confidence,
//            [configEntryId], [resolutionStatus]
// `sourceAnchor` is OMITTED. Optionals use `skip_serializing_if`. Internal
// RoutineId is projected to StableRoutineId; entries with no stable mapping are
// skipped (mirrors the projection's empty-id exclusion). Sorted by stable
// routineId ascending.
// ---------------------------------------------------------------------------

/// One stable RootClassification — all ids in stable form.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StableRootClassification {
    #[serde(rename = "routineId")]
    pub routine_id: String,
    pub kinds: Vec<String>,
    #[serde(rename = "externallyReachable")]
    pub externally_reachable: bool,
    pub source: String,
    pub confidence: String,
    #[serde(rename = "configEntryId", skip_serializing_if = "Option::is_none")]
    pub config_entry_id: Option<String>,
    #[serde(rename = "resolutionStatus", skip_serializing_if = "Option::is_none")]
    pub resolution_status: Option<String>,
}

/// The full R4-F root-classification projection for one fixture run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct R4FRootClassProjection {
    #[serde(rename = "fixtureName")]
    pub fixture_name: String,
    #[serde(rename = "classificationCount")]
    pub classification_count: usize,
    pub classifications: Vec<StableRootClassification>,
}

/// Project a resolved workspace's `root_classifications` to the stable R4-F form.
/// Mirrors `projectRootClassifications`: map each internal RoutineId to its
/// StableRoutineId via the routine stable-map; drop entries with no stable id;
/// sort by stable routineId ascending.
pub fn project_r4f_root_classifications(
    resolved: &crate::engine::l3::l3_workspace::L3Resolved,
    fixture_name: &str,
) -> R4FRootClassProjection {
    let map = crate::engine::l4::summary::build_routine_stable_map(&resolved.workspace.routines);

    let mut stable: Vec<StableRootClassification> = Vec::new();
    for rc in &resolved.root_classifications {
        let Some(stable_id) = map.get(&rc.routine_id) else {
            continue;
        };
        if stable_id.is_empty() {
            continue;
        }
        stable.push(StableRootClassification {
            routine_id: stable_id.clone(),
            kinds: rc.kinds.clone(),
            externally_reachable: rc.externally_reachable,
            source: rc.source.clone(),
            confidence: rc.confidence.clone(),
            config_entry_id: rc.config_entry_id.clone(),
            resolution_status: rc.resolution_status.clone(),
        });
    }

    stable.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    R4FRootClassProjection {
        fixture_name: fixture_name.to_string(),
        classification_count: stable.len(),
        classifications: stable,
    }
}

// ---------------------------------------------------------------------------
// Tests — the acceptance matrix for the derived `page-action` root kind.
//
// Every case that exercises CLASSIFICATION runs through `classify_roots` (or
// `compute_root_classifications` for the overlay rows), NEVER through
// `is_page_action_wrapper` or `kinds_for` alone: a helper-only test leaves the
// production call site deletable while the whole suite stays green, which is
// the exact failure CLAUDE.md records five instances of.
//
// The one deliberate exception is
// `declared_kinds_minus_the_derivable_eleven_are_exactly_the_overlay_only_two`,
// which runs through NEITHER -- it is a pure assertion over two constants, and
// so it also passes under the discrimination break. That is correct for what it
// guards (a newly declared kind nobody emits) and is why
// `ast_pass_emits_exactly_the_eleven_derivable_kinds` exists to cover the other
// direction. The two are complementary; neither alone covers both. Every precondition is hand-stated by ASSIGNMENT — no production
// code is asked to produce a shape for the test.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l2::features::PAnchor;
    use crate::engine::l3::al_attributes::AttributeInfo;
    use crate::engine::l3::l3_workspace::RoutineVariables;

    /// The eleven kinds the AST pass derives. A8 asserts the union of `classify_roots`
    /// over the witness workspace EQUALS this; A9 asserts `ROOT_KIND_VALUES` minus
    /// this is exactly the two overlay-only kinds.
    const DERIVABLE_KINDS: [&str; 11] = [
        "trigger-table",
        "trigger-page",
        "page-action",
        "report-trigger",
        "event-subscriber",
        "install-codeunit",
        "upgrade-codeunit",
        "api-page",
        "public-procedure",
        "test-procedure",
        "onrun-codeunit",
    ];

    // -- hand-state constructors -------------------------------------------
    //
    // Every field these default is one the test does NOT depend on. Each test
    // below states its OWN discriminating field explicitly at the call site.

    fn object(id: &str, object_type: &str) -> L3Object {
        L3Object {
            id: id.to_string(),
            app_guid: "app".to_string(),
            object_type: object_type.to_string(),
            object_number: 50100,
            name: "Obj".to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
        }
    }

    fn anchor(syntax_kind: &str) -> PAnchor {
        PAnchor {
            source_unit_id: "ws:test.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: syntax_kind.to_string(),
        }
    }

    fn routine(id: &str, object_id: &str, kind: &str) -> L3Routine {
        L3Routine {
            id: id.to_string(),
            stable_routine_id: format!("stable::{id}"),
            object_id: object_id.to_string(),
            // NOTE: `L3Routine` carries its own `object_type`, which `kinds_for`
            // does NOT read — it branches on the OWNING `L3Object`'s field. See
            // `page_extension_action_trigger_is_page_action` (A3).
            object_type: "Page".to_string(),
            name: "R".to_string(),
            kind: kind.to_string(),
            attributes_parsed: Vec::new(),
            app_guid: "app".to_string(),
            object_number: 50100,
            normalized_signature_hash: "sig".to_string(),
            body_available: true,
            parse_incomplete: false,
            record_variables: Vec::new(),
            record_operations: Vec::new(),
            field_accesses: Vec::new(),
            variables: RoutineVariables::default(),
            parameters: Vec::new(),
            access_modifier: None,
            return_type: None,
            call_sites: Vec::new(),
            operation_sites: Vec::new(),
            statement_tree: None,
            loops: Vec::new(),
            source_anchor: anchor("trigger_declaration"),
            identifier_references: Vec::new(),
            unreachable_statements: Vec::new(),
            has_branching: false,
            var_assignments: Vec::new(),
            condition_references: Vec::new(),
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    fn workspace(objects: Vec<L3Object>, routines: Vec<L3Routine>) -> L3Workspace {
        L3Workspace {
            objects,
            tables: Vec::new(),
            routines,
        }
    }

    /// One `Page`-owned trigger whose enclosing member wrapper kind is stated
    /// literally, run through `classify_roots`. Returns its `kinds`.
    fn page_trigger_kinds(wrapper: Option<&str>) -> Vec<String> {
        let obj = object("app/Page/50100", "Page");
        let mut r = routine("r1", "app/Page/50100", "trigger");
        r.enclosing_member_range = wrapper.map(anchor);
        let ws = workspace(vec![obj], vec![r]);

        let roots = classify_roots(&ws);
        assert_eq!(roots.len(), 1, "the witness routine must be classified");
        roots[0].kinds.clone()
    }

    // -- A1 -----------------------------------------------------------------

    #[test]
    fn action_trigger_is_page_action() {
        assert_eq!(
            page_trigger_kinds(Some("action_declaration")),
            vec!["trigger-page".to_string(), "page-action".to_string()],
            "an action's trigger keeps trigger-page AND gains page-action, in \
             ROOT_KIND_VALUES declaration order"
        );
    }

    // -- A2 -----------------------------------------------------------------

    #[test]
    fn systemaction_trigger_is_page_action() {
        // Real AL: Microsoft's PromptDialog page documents `OnAction()` under
        // `systemaction(Generate)`.
        assert_eq!(
            page_trigger_kinds(Some("systemaction_declaration")),
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
    }

    #[test]
    fn fileuploadaction_trigger_is_page_action() {
        // Real AL: documented trigger `OnAction(Files: List of [FileUpload])`.
        assert_eq!(
            page_trigger_kinds(Some("fileuploadaction_declaration")),
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
    }

    #[test]
    fn customaction_wrapper_is_matched_parser_shape_contract_only() {
        // DEFENSIVE match, and this test is a PARSER-SHAPE CONTRACT — it is NOT a
        // real-AL witness and claims NO coverage. Microsoft documents customaction
        // as a client-invoked Power Automate flow (`CustomActionType = Flow`,
        // `FlowId`), a shape with no AL routine to classify. It is matched only
        // because a false negative on a real form would cost more than dead code.
        assert_eq!(
            page_trigger_kinds(Some("customaction_declaration")),
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
    }

    #[test]
    fn action_modification_trigger_is_page_action() {
        // Issue #41: a pageextension's `actions { modify(X) { trigger
        // OnAfterAction() } }`. The wrapper is `modify_action_modification`,
        // which carries `target` and no `name`; the lowerer captures it as an
        // enclosing member (see `lower/mod.rs`'s
        // `modify_action_modification_target_becomes_enclosing_member`, whose
        // `origin.kind_text` assert is the executable join to this literal).
        assert_eq!(
            page_trigger_kinds(Some("modify_action_modification")),
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
    }

    // -- A3 -----------------------------------------------------------------

    #[test]
    fn page_extension_action_trigger_is_page_action() {
        // The discriminating field is the OWNING OBJECT's `object_type`:
        // `classify_roots` looks the object up and `kinds_for` branches on the
        // OBJECT's type. The routine's own similarly-named `object_type` is set
        // to "Report" ON PURPOSE, and the choice is load-bearing: `"Page" |
        // "PageExtension"` is ONE match arm, so a routine field of "Page" would
        // take the SAME branch and the test would pass whichever field were read.
        // "Report" takes a DIFFERENT arm, so a `kinds_for` that read
        // `routine.object_type` would yield `["report-trigger"]` and fail here.
        //
        // An earlier revision used "Page" and asserted it had stayed "Page",
        // which proved nothing at all (astra, final panel, finding 5).
        let mut obj = object("app/PageExtension/50101", "Page");
        obj.object_type = "PageExtension".to_string();

        let mut r = routine("r1", "app/PageExtension/50101", "trigger");
        r.object_type = "Report".to_string();
        r.enclosing_member_range = Some(anchor("action_declaration"));

        let ws = workspace(vec![obj], vec![r]);
        let roots = classify_roots(&ws);
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].kinds,
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
    }

    // -- A4 -----------------------------------------------------------------

    #[test]
    fn object_level_page_trigger_is_not_page_action() {
        // An object-level trigger (e.g. `OnOpenPage`) has no enclosing member.
        assert_eq!(page_trigger_kinds(None), vec!["trigger-page".to_string()]);
    }

    #[test]
    fn page_field_trigger_is_not_page_action() {
        assert_eq!(
            page_trigger_kinds(Some("page_field")),
            vec!["trigger-page".to_string()]
        );
    }

    #[test]
    fn separator_action_trigger_is_not_page_action() {
        // A REACHABLE exclusion: a visual separator is not invokable, and
        // `separator(Name)` does produce this enclosing-member anchor (the
        // grammar's `name` field on `separator_action` is optional, not absent).
        assert_eq!(
            page_trigger_kinds(Some("separator_action")),
            vec!["trigger-page".to_string()]
        );
    }

    // -- A5 -----------------------------------------------------------------

    #[test]
    fn non_trigger_routine_with_action_anchor_is_not_page_action() {
        // The `routine.kind == "trigger"` gate still holds: a procedure that
        // happens to carry an action-shaped anchor gets neither trigger-page nor
        // page-action. It is left `access_modifier: None` so it IS still
        // classified (as public-procedure) — otherwise the absence assertion
        // below would be vacuous.
        let obj = object("app/Page/50100", "Page");
        let mut r = routine("r1", "app/Page/50100", "procedure");
        r.enclosing_member_range = Some(anchor("action_declaration"));
        let ws = workspace(vec![obj], vec![r]);

        let roots = classify_roots(&ws);
        assert_eq!(roots.len(), 1, "the procedure must still be classified");
        assert_eq!(roots[0].kinds, vec!["public-procedure".to_string()]);
        assert!(
            !roots[0].kinds.iter().any(|k| k == "page-action"),
            "a non-trigger routine must never gain page-action"
        );
    }

    // -- A6 -----------------------------------------------------------------

    #[test]
    fn actionref_trigger_is_not_page_action() {
        // This pins INTENT, not a reachable path. `actionref_declaration` carries
        // `promoted_name` / `action_name` and no `name` field, so the lowerer's
        // name gate never captures it as an enclosing member — the anchor below
        // cannot actually arise from real source. Read it as a contract on the
        // exclusion list, never as end-to-end coverage.
        assert_eq!(
            page_trigger_kinds(Some("actionref_declaration")),
            vec!["trigger-page".to_string()]
        );
    }

    // -- A8 -----------------------------------------------------------------

    /// Witnesses covering all eleven derivable kinds, INCLUDING a real action
    /// trigger. Note what that witness does and does not buy, since an earlier
    /// version of this comment overstated it: because `DERIVABLE_KINDS` now
    /// LISTS `page-action`, omitting the action witness makes A8 fail
    /// immediately, not silently pass after a deletion. The witness is required
    /// for A8 to pass at all; it is not what makes the deletion detectable.
    fn eleven_kind_witness_workspace() -> L3Workspace {
        let mut objects = Vec::new();
        let mut routines = Vec::new();

        // trigger-table
        objects.push(object("app/Table/1", "Table"));
        routines.push(routine("r01", "app/Table/1", "trigger"));

        // trigger-page (object-level) and page-action (action member trigger)
        objects.push(object("app/Page/2", "Page"));
        routines.push(routine("r02", "app/Page/2", "trigger"));
        let mut action_trigger = routine("r03", "app/Page/2", "trigger");
        action_trigger.enclosing_member_range = Some(anchor("action_declaration"));
        routines.push(action_trigger);

        // report-trigger
        objects.push(object("app/Report/3", "Report"));
        routines.push(routine("r04", "app/Report/3", "trigger"));

        // event-subscriber
        objects.push(object("app/Codeunit/4", "Codeunit"));
        routines.push(routine("r05", "app/Codeunit/4", "event-subscriber"));

        // install-codeunit
        let mut install = object("app/Codeunit/5", "Codeunit");
        install.object_subtype = Some("Install".to_string());
        objects.push(install);
        routines.push(routine("r06", "app/Codeunit/5", "procedure"));

        // upgrade-codeunit
        let mut upgrade = object("app/Codeunit/6", "Codeunit");
        upgrade.object_subtype = Some("Upgrade".to_string());
        objects.push(upgrade);
        routines.push(routine("r07", "app/Codeunit/6", "procedure"));

        // api-page
        let mut api = object("app/Page/7", "Page");
        api.page_type = Some("API".to_string());
        objects.push(api);
        routines.push(routine("r08", "app/Page/7", "procedure"));

        // public-procedure
        objects.push(object("app/Codeunit/8", "Codeunit"));
        routines.push(routine("r09", "app/Codeunit/8", "procedure"));

        // test-procedure
        let mut test_proc = routine("r10", "app/Codeunit/8", "procedure");
        test_proc.attributes_parsed = vec![AttributeInfo {
            name: "Test".to_string(),
            args: Vec::new(),
            raw: "[Test]".to_string(),
        }];
        routines.push(test_proc);

        // onrun-codeunit
        objects.push(object("app/Codeunit/9", "Codeunit"));
        let mut onrun = routine("r11", "app/Codeunit/9", "trigger");
        onrun.name = "OnRun".to_string();
        routines.push(onrun);

        workspace(objects, routines)
    }

    #[test]
    fn ast_pass_emits_exactly_the_eleven_derivable_kinds() {
        let ws = eleven_kind_witness_workspace();
        // UNFILTERED union — every kind any witness produces, nothing dropped.
        let emitted: BTreeSet<String> = classify_roots(&ws)
            .iter()
            .flat_map(|rc| rc.kinds.iter().cloned())
            .collect();
        let expected: BTreeSet<String> = DERIVABLE_KINDS.iter().map(|k| (*k).to_string()).collect();
        assert_eq!(
            emitted, expected,
            "the AST pass must emit exactly the eleven derivable kinds"
        );
    }

    // -- A9 -----------------------------------------------------------------

    #[test]
    fn declared_kinds_minus_the_derivable_eleven_are_exactly_the_overlay_only_two() {
        // Asserted, not merely documented: without this a FOURTEENTH declared-but-
        // unemitted kind would leave A8 green.
        let unemitted: Vec<&str> = ROOT_KIND_VALUES
            .iter()
            .copied()
            .filter(|k| !DERIVABLE_KINDS.contains(k))
            .collect();
        assert_eq!(
            unemitted,
            vec!["web-service-exposed", "job-queue-entrypoint"]
        );
    }

    // -- A10: the onrun-codeunit predicate, one row per conjunct ------------
    //
    // A8's witness union proves the kind IS emitted, but it cannot pin WHICH of
    // the three conjuncts earns it: in that workspace `r11` is the only
    // Codeunit-owned trigger AND the only routine named OnRun, so deleting any
    // single conjunct still leaves exactly `r11` qualifying and A8 green. These
    // rows state each precondition literally by ASSIGNMENT and run through
    // `classify_roots`, so each conjunct has its own failing witness. The
    // CHANGELOG and the predicate's own comment ADVERTISE the second row's
    // guard; before A10 it was asserted nowhere.

    /// One object of `object_type`, one routine of `kind` named `name`, run
    /// through `classify_roots`. Returns its `kinds` (empty when unclassified).
    fn codeunit_row(object_type: &str, kind: &str, name: &str) -> Vec<String> {
        let obj = object("app/Codeunit/50100", object_type);
        let mut r = routine("r1", "app/Codeunit/50100", kind);
        r.name = name.to_string();
        let ws = workspace(vec![obj], vec![r]);
        match classify_roots(&ws).first() {
            Some(rc) => rc.kinds.clone(),
            None => Vec::new(),
        }
    }

    #[test]
    fn codeunit_onrun_trigger_is_onrun_codeunit() {
        assert_eq!(
            codeunit_row("Codeunit", "trigger", "OnRun"),
            vec!["onrun-codeunit".to_string()]
        );
        // Case-insensitively, per `eq_ignore_ascii_case`.
        assert_eq!(
            codeunit_row("Codeunit", "trigger", "ONRUN"),
            vec!["onrun-codeunit".to_string()]
        );
    }

    #[test]
    fn codeunit_procedure_named_onrun_is_only_public_procedure() {
        // The guard the CHANGELOG advertises: the predicate keys on
        // `kind == "trigger"`, never the name, so a default-access procedure that
        // happens to be called OnRun must NOT gain the trigger-derived kind.
        assert_eq!(
            codeunit_row("Codeunit", "procedure", "OnRun"),
            vec!["public-procedure".to_string()]
        );
    }

    #[test]
    fn codeunit_trigger_not_named_onrun_is_unclassified() {
        // Pins the name conjunct. A Codeunit has no object-level trigger other
        // than OnRun in real AL, so this is a contract on the predicate, not a
        // reachable shape -- stated literally for exactly that reason.
        assert_eq!(
            codeunit_row("Codeunit", "trigger", "OnSomethingElse"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn non_codeunit_trigger_named_onrun_is_not_onrun_codeunit() {
        // Pins the object_type conjunct. A Table trigger named OnRun keeps its
        // own kind and must not acquire the codeunit one.
        assert_eq!(
            codeunit_row("Table", "trigger", "OnRun"),
            vec!["trigger-table".to_string()]
        );
    }

    #[test]
    fn install_codeunit_onrun_carries_both_kinds() {
        // The subtype rule applies to every routine in the object, triggers
        // included, so an Install codeunit's OnRun legitimately carries two
        // kinds -- in ROOT_KIND_VALUES declaration order, install first.
        let mut obj = object("app/Codeunit/50100", "Codeunit");
        obj.object_subtype = Some("Install".to_string());
        let mut r = routine("r1", "app/Codeunit/50100", "trigger");
        r.name = "OnRun".to_string();
        let ws = workspace(vec![obj], vec![r]);
        let roots = classify_roots(&ws);
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].kinds,
            vec!["install-codeunit".to_string(), "onrun-codeunit".to_string()]
        );
    }

    // -- A11 ----------------------------------------------------------------

    /// One `Page` with a single action trigger (AST kinds: `["trigger-page",
    /// "page-action"]`), plus a `roots.config.json` on disk asserting `kinds` for
    /// that routine. Runs the REAL production entry point
    /// `compute_root_classifications`, so the config loader and the overlay are
    /// both in the loop, not just the overlay. Returns `(kinds, diagnostics)`.
    fn overlay_row(config_kinds: &str) -> (Vec<String>, Vec<InfraDiagnostic>) {
        let obj = object("app/Page/50100", "Page");
        let mut r = routine("r1", "app/Page/50100", "trigger");
        r.enclosing_member_range = Some(anchor("action_declaration"));
        let ws = workspace(vec![obj], vec![r]);

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("roots.config.json"),
            format!(
                r#"{{"version":1,"roots":[{{"id":"E1","target":{{"routineId":"r1"}},"kinds":{config_kinds}}}]}}"#
            ),
        )
        .expect("write roots.config.json");

        let (roots, diags) = compute_root_classifications(&ws, Some(dir.path()));
        assert_eq!(roots.len(), 1, "one routine in, one classification out");
        assert_eq!(
            roots[0].source, "ast+config",
            "the config entry must have matched the AST root"
        );
        (roots[0].kinds.clone(), diags)
    }

    const MISMATCH_PREFIX: &str =
        "[roots-config/kinds-mismatch] roots.config.json entry \"E1\" disagrees with AST: ";

    #[test]
    fn overlay_config_asserting_only_page_action_still_warns_ast_only_trigger_page() {
        // Row 1 of the design's overlay table. BEFORE this change the pair was
        // (["trigger-page"], ["page-action"]); after, config-only is empty but the
        // warning does NOT disappear — the symmetric difference is computed before
        // the union, so dedup can never suppress a mismatch.
        let (kinds, diags) = overlay_row(r#"["page-action"]"#);
        assert_eq!(
            kinds,
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
        assert_eq!(diags.len(), 1, "exactly one kinds-mismatch warning");
        assert_eq!(diags[0].severity, "warning");
        assert_eq!(diags[0].stage, "discover");
        assert_eq!(
            diags[0].message,
            format!("{MISMATCH_PREFIX}ast-only=[\"trigger-page\"], config-only=[].")
        );
    }

    #[test]
    fn overlay_config_asserting_both_kinds_no_longer_warns() {
        // Row 2: before this change the pair was ([], ["page-action"]); now the two
        // sides agree exactly and the warning is gone.
        let (kinds, diags) = overlay_row(r#"["trigger-page","page-action"]"#);
        assert_eq!(
            kinds,
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
        assert!(
            diags.is_empty(),
            "an exactly-agreeing config must emit no diagnostic, got: {diags:?}"
        );
    }

    #[test]
    fn overlay_config_asserting_only_trigger_page_now_warns() {
        // Row 3: a config that was previously SILENT now warns. This is a
        // user-visible new diagnostic and is pinned deliberately, not incidentally.
        let (kinds, diags) = overlay_row(r#"["trigger-page"]"#);
        assert_eq!(
            kinds,
            vec!["trigger-page".to_string(), "page-action".to_string()]
        );
        assert_eq!(diags.len(), 1, "exactly one kinds-mismatch warning");
        assert_eq!(diags[0].severity, "warning");
        assert_eq!(diags[0].stage, "discover");
        assert_eq!(
            diags[0].message,
            format!("{MISMATCH_PREFIX}ast-only=[\"page-action\"], config-only=[].")
        );
    }
}
