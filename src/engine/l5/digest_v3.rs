//! The digest JSON document 3.0.0 (MCP output wave 2, W2-2; design
//! `docs/superpowers/specs/2026-11-04-mcp-output-wave2-design.md` §4.4).
//!
//! Every effect row is kept (multiplicity included: `factId` is not a row
//! key). What changes from the 2.x shape (`--schema 2`, still rendered by
//! `digest_cli::project_digest_document`):
//!
//! - paths are `paths: [{hops, terminal}]`, with structured hops `{edge, to,
//!   line}` whose `to` is a key of `payload.routines`, the line the call
//!   site's in the CALLING routine's file; one path on a decision effect by
//!   default (may-persist and caller-dependent writes, COMMIT, HTTP, FILE,
//!   BACKGROUND_TASK, ISOLATED_STORAGE, TELEMETRY, UI_*, ERROR_*), none on
//!   events, reads and temporary rows; `pathsKept`, `pathsOmitted` and
//!   `pathsProducerTruncated` state the two truncations apart;
//! - a group row names its crossing (`crossing`, a key of
//!   `entries[].crossings`, which holds the site, targets and prefix once),
//!   lists every table as `{id, display}` and every event grouped by
//!   publisher (`eventsByPublisher`); a may-persist group row carries one
//!   representative terminal witness (`paths[0]`, `representative: true`);
//! - `class` on table writes, `status` (`live` / `literal-dead`) on every row;
//! - only non-trivial `scopedGuarantees` (`scopedGuaranteesFolded` counts the
//!   rest), entry diagnostics as counts, dependency unresolved call sites as
//!   counts and top callees;
//! - `entries[].summary` (the rollup's `RollupTotals`), coverage
//!   `assumptions` / `limits`.
//!
//! `--detail` (unfolded 3.x) keeps every path (up to 3), every scoped
//! guarantee, every diagnostic, every unresolved item and every
//! per-resource group witness.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::engine::l5::digest::ProjectedHop;
use crate::engine::l5::digest_cli::{
    DigestEffectFull, DigestEntryFull, anchor_to_value, routine_to_value,
    scoped_guarantee_to_value, unresolved_item_to_value,
};
use crate::engine::l5::rollup::{WriteClass, classify_detail};
use crate::engine::l5::routine_rollup::{
    RenderOpts, RollupCx, RoutineRollup, class_json, crossing_key, is_group, limit_values,
};
use crate::engine::line_base::shown;

/// How the digest document is rendered (design §5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DigestRender {
    /// `--schema 2`: the 2.x document (old shape over the current analysis).
    pub schema2: bool,
    /// `--schema 3`: the 3.x document, kept for one release after 4.0.0
    /// (output validation V5, §7.3). The default is 4.0.0 (`digest_v4`).
    pub schema3: bool,
    /// `--schema 4`: the 4.0.0 document, kept for one release after 5.0.0
    /// (round-2 output validation §4.6). The default is 5.0.0 (`digest_v5`).
    pub schema4: bool,
    /// `--detail`: unfolded 3.x.
    pub detail: bool,
    /// `--compact`: no indentation.
    pub compact: bool,
    /// `--max-paths N`.
    pub max_paths: Option<usize>,
    /// `--surface` (output validation §6.1).
    pub surface: crate::engine::follow_up::Surface,
}

impl DigestRender {
    /// The 2.x document (`--schema 2`).
    #[must_use]
    pub fn schema2() -> DigestRender {
        DigestRender {
            schema2: true,
            ..DigestRender::default()
        }
    }

    /// The path cap and fold of §5.
    #[must_use]
    pub fn opts(&self) -> RenderOpts {
        RenderOpts::new(self.detail, self.max_paths).with_surface(self.surface)
    }
}

/// `prefix` + the first 12 hex of sha256 of each id; every key of a
/// colliding group extends by 4 hex, repeated until all are unique (§4.4.2).
#[must_use]
pub fn hashed_keys(prefix: &str, ids: &BTreeSet<String>) -> BTreeMap<String, String> {
    let hex: BTreeMap<&str, String> = ids
        .iter()
        .map(|id| {
            let d = Sha256::digest(id.as_bytes());
            (
                id.as_str(),
                d.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            )
        })
        .collect();
    let mut len: BTreeMap<&str, usize> = ids.iter().map(|i| (i.as_str(), 12)).collect();
    loop {
        let mut by_key: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for (id, n) in &len {
            by_key
                .entry(hex[id][..(*n).min(64)].to_string())
                .or_default()
                .push(id);
        }
        let colliding: Vec<&str> = by_key
            .values()
            .filter(|v| v.len() > 1)
            .flatten()
            .copied()
            .collect();
        if colliding.is_empty() || colliding.iter().all(|id| len[id] >= 64) {
            return len
                .iter()
                .map(|(id, n)| {
                    (
                        (*id).to_string(),
                        format!("{prefix}{}", &hex[id][..(*n).min(64)]),
                    )
                })
                .collect();
        }
        for id in colliding {
            *len.get_mut(id).expect("a known id") += 4;
        }
    }
}

/// Is a row a decision effect, whose witness is shown by default (§4.4.2)?
fn decision(e: &DigestEffectFull) -> bool {
    match e.effect_type.as_str() {
        "DB_INSERT" | "DB_MODIFY" | "DB_DELETE" => {
            classify_detail(&e.detail) != WriteClass::Temporary
        }
        "COMMIT" | "HTTP" | "FILE" | "BACKGROUND_TASK" | "ISOLATED_STORAGE" | "TELEMETRY" => true,
        t => t.starts_with("UI_") || t.starts_with("ERROR_"),
    }
}

/// A scoped guarantee that says nothing beyond its label (Fable N8).
fn trivial(sg: &crate::engine::l5::ordering_engine::ScopedGuarantee) -> bool {
    sg.scope == "owning-routine"
        && sg.intervening_boundary == "none"
        && !sg.valid_for_refutation
        && sg.commit_effectiveness.is_none()
}

/// The rows of a site-enumerated type the default document keeps whole.
const WHOLE_ROWS: usize = 1;

/// Each row's render options (MCP output step 2): a site-enumerated type
/// (`digest::SITE_ENUMERATED`, one row per site) keeps its first
/// [`WHOLE_ROWS`] row whole (one witness per type, as `prove` prints); the
/// others are `None`, one place each in `moreSites` ([`more_site`]).
/// `--detail`: every row whole.
fn row_opts(entry: &DigestEntryFull, opts: RenderOpts) -> Vec<Option<RenderOpts>> {
    let site = |e: &DigestEffectFull| {
        !opts.detail && crate::engine::l5::digest::SITE_ENUMERATED.contains(&e.effect_type.as_str())
    };
    // The shortest witness first: a row's shortest listed path, or its
    // group's terminal witness.
    let len = |e: &DigestEffectFull| {
        e.via_paths
            .iter()
            .filter(|p| !p.is_empty())
            .map(Vec::len)
            .min()
            .or_else(|| {
                e.group_terminal_path
                    .as_ref()?
                    .as_ref()
                    .ok()
                    .map(|w| w.steps.len())
            })
            .unwrap_or(usize::MAX)
    };
    let mut by_type: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, e) in entry.effects.iter().enumerate() {
        if site(e) {
            by_type.entry(e.effect_type.as_str()).or_default().push(i);
        }
    }
    let mut whole: BTreeSet<usize> = BTreeSet::new();
    for ix in by_type.values_mut() {
        ix.sort_by_key(|&i| len(&entry.effects[i]));
        whole.extend(ix.iter().take(WHOLE_ROWS));
    }
    entry
        .effects
        .iter()
        .enumerate()
        .map(|(i, e)| (!site(e) || whole.contains(&i)).then_some(opts))
        .collect()
}

/// The crossings whose `prefix` (the path to the entry point) the document
/// prints: those a whole row names. A crossing only folded rows lie beyond
/// prints its sites and targets; `--detail`, every prefix.
fn prefixed_crossings(entry: &DigestEntryFull, opts: RenderOpts) -> BTreeSet<String> {
    entry
        .effects
        .iter()
        .zip(row_opts(entry, opts))
        .filter(|(e, o)| o.is_some() && is_group(e))
        .map(|(e, _)| crossing_key(e))
        .collect()
}

/// A row [`row_opts`] folds, as one item of `entries[].moreSites[<type>]
/// [<where>]`, a `; `-joined string: `<where>` is its file (the item
/// `<line>`), `beyond` for a row beyond a crossing (the item `<crossing
/// key>`), or `no position kept`; then its conditionality unless it is
/// `unknown`, and ` literal-dead` when the root cannot reach it.
fn more_site(e: &DigestEffectFull, crossings: &BTreeMap<String, String>) -> (String, String) {
    let (place, at) = if is_group(e) {
        let key = crossing_key(e);
        (
            "beyond".to_string(),
            crossings.get(&key).cloned().unwrap_or(key),
        )
    } else {
        match (&e.evidence.file, e.evidence.line) {
            (Some(f), Some(l)) => (f.clone(), shown(l).to_string()),
            (Some(f), None) => (f.clone(), String::new()),
            _ => ("no position kept".to_string(), String::new()),
        }
    };
    let mut s = at;
    if e.conditionality != "unknown" {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(e.conditionality);
    }
    if e.literal_unreachable {
        s.push_str(" literal-dead");
    }
    (place, s.trim_start().to_string())
}

/// How many of an own row's listed paths the document shows (§4.4.2).
fn shown_paths(e: &DigestEffectFull, opts: RenderOpts) -> usize {
    if opts.detail || decision(e) {
        opts.path_cap.min(e.via_paths.len())
    } else {
        0
    }
}

/// Whether an own row with no listed path shows its searched witness
/// ([`DigestEffectFull::own_path`]): as a listed path would show.
fn shows_own_path(e: &DigestEffectFull, opts: RenderOpts) -> bool {
    (opts.detail || decision(e)) && opts.path_cap > 0 && e.via_paths.iter().all(Vec::is_empty)
}

/// One structured hop of a witness: `{edge, to, line, file}`, `edgeId`
/// with `--detail`.
fn step_value(
    rt: &RoutineTable,
    s: &crate::engine::l5::group_path::WitnessStep,
    detail: bool,
) -> Value {
    let mut v = json!({"edge": s.edge, "to": rt.key(&s.to)});
    if let Some(l) = s.line {
        v["line"] = json!(shown(l));
    }
    if let Some(f) = &s.file {
        v["file"] = json!(f);
    }
    if detail && let Some(id) = &s.edge_id {
        v["edgeId"] = json!(id);
    }
    v
}

/// The group witnesses a row shows: every one with `--detail`, else the
/// representative of a decision row.
fn shown_witnesses(
    e: &DigestEffectFull,
    opts: RenderOpts,
) -> Vec<(Option<&String>, &crate::engine::l5::group_path::GroupPath)> {
    // W2-6 (2f): a resource-less group's (COMMIT, HTTP, …) own terminal
    // witness, beside the per-resource ones of a table group.
    let all = e
        .group_resource_paths
        .iter()
        .filter_map(|(r, w)| w.as_ref().ok().map(|w| (Some(r), w)))
        .chain(
            e.group_terminal_path
                .iter()
                .filter_map(|w| w.as_ref().ok().map(|w| (None, w))),
        );
    if opts.detail {
        all.collect()
    } else if e.group_resources.is_empty() && e.effect_type != "COMMIT" {
        // The default carries a resource-less group's witness for a COMMIT
        // only (§3.4.2: a may-persist row's representative, and a COMMIT's
        // own); HTTP, UI, ERROR, … witnesses come with --detail (the size
        // budget, §9 q1).
        Vec::new()
    } else if decision(e) && opts.path_cap > 0 {
        all.take(1).collect()
    } else {
        Vec::new()
    }
}

/// Every stable routine id the document names, and no other (T-W2-8):
/// roots, the hop targets of every shown path and prefix, crossing targets
/// and the steps of every shown group witness.
#[must_use]
pub fn referenced_routines(
    entries: &[DigestEntryFull],
    rollups: &[RoutineRollup],
    opts: RenderOpts,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let hops = |ids: &mut BTreeSet<String>, p: &[ProjectedHop]| {
        ids.extend(p.iter().filter_map(|h| h.inner.to_routine_id.clone()));
    };
    for (entry, r) in entries.iter().zip(rollups) {
        ids.insert(entry.routine_id.clone());
        for (e, opts) in entry
            .effects
            .iter()
            .zip(row_opts(entry, opts))
            .filter_map(|(e, o)| o.map(|o| (e, o)))
        {
            if is_group(e) {
                for (_, w) in shown_witnesses(e, opts) {
                    ids.extend(w.steps.iter().map(|s| s.to.clone()));
                }
            } else {
                for p in &e.via_paths[..shown_paths(e, opts)] {
                    hops(&mut ids, p);
                }
                if shows_own_path(e, opts)
                    && let Some(Ok(w)) = &e.own_path
                {
                    ids.extend(w.steps.iter().map(|s| s.to.clone()));
                }
            }
        }
        let prefixed = prefixed_crossings(entry, opts);
        for c in &r.crossings {
            ids.extend(c.target_ids.iter().cloned());
            if !prefixed.contains(&c.id) {
                continue;
            }
            if let Some(p) = c
                .prefix_row
                .and_then(|i| entry.effects[i].via_paths.iter().min_by_key(|p| p.len()))
            {
                hops(&mut ids, p);
            }
        }
    }
    ids
}

/// The document's routine table and its keys.
pub struct RoutineTable {
    pub keys: BTreeMap<String, String>,
    pub table: Value,
    /// Stable id → the routine's default file.
    files: BTreeMap<String, String>,
}

impl RoutineTable {
    #[must_use]
    pub fn build(
        entries: &[DigestEntryFull],
        rollups: &[RoutineRollup],
        cx: &RollupCx<'_>,
        opts: RenderOpts,
    ) -> RoutineTable {
        let ids = referenced_routines(entries, rollups, opts);
        let keys = hashed_keys("r:", &ids);
        let mut table = Map::new();
        let mut files = BTreeMap::new();
        for id in &ids {
            // V3 §2.1: the one display and its parts, the selector and the
            // JSON-only handle.
            let mut v = json!({"stableId": id, "display": cx.qualified(id)});
            for (k, x) in cx.names.identity_json(id) {
                v[k] = x;
            }
            let handle = cx.handle(id);
            if !handle.is_empty() {
                v["handle"] = json!(handle);
            }
            if let Some(f) = cx.default_file(id) {
                v["file"] = json!(f);
                files.insert(id.clone(), f);
            }
            table.insert(keys[id].clone(), v);
        }
        RoutineTable {
            keys,
            table: Value::Object(table),
            files,
        }
    }

    fn key(&self, id: &str) -> Value {
        self.keys.get(id).map_or_else(|| json!(id), |k| json!(k))
    }

    /// A path's hops: `{edge, to, line}`, `file` when the call site's unit
    /// is not the calling routine's default file, `edgeId` with `--detail`.
    fn hops(&self, root: &str, hops: &[ProjectedHop], detail: bool) -> Vec<Value> {
        let mut caller = root.to_string();
        hops.iter()
            .map(|h| {
                let h = &h.inner;
                let mut v = json!({"edge": h.edge_kind.as_deref().unwrap_or(h.kind)});
                match &h.to_routine_id {
                    Some(t) => v["to"] = self.key(t),
                    None => {
                        v["to"] = json!(
                            h.to_display
                                .clone()
                                .or_else(|| h.callee_display.clone())
                                .unwrap_or_default()
                        )
                    }
                }
                if let Some(a) = &h.anchor {
                    if let Some(l) = a.line {
                        v["line"] = json!(shown(l));
                    }
                    if self.files.get(&caller) != Some(&a.file) {
                        v["file"] = json!(a.file);
                    }
                }
                if detail
                    && let Some(id) = h
                        .callsite_id
                        .as_ref()
                        .or(h.event_id.as_ref())
                        .or(h.operation_id.as_ref())
                {
                    v["edgeId"] = json!(id);
                }
                if let Some(t) = &h.to_routine_id {
                    caller = t.clone();
                }
                v
            })
            .collect()
    }
}

/// One effect row of 3.0.0 (module doc).
fn effect_v3(
    e: &DigestEffectFull,
    root: &str,
    rt: &RoutineTable,
    crossings: &BTreeMap<String, String>,
    boundary_names: &BTreeMap<String, String>,
    opts: RenderOpts,
) -> Value {
    let mut m = Map::new();
    let ty = e.effect_type.as_str();
    m.insert("type".into(), ty.into());
    m.insert("factId".into(), e.fact_id.clone().into());
    m.insert("provenance".into(), e.provenance.into());
    m.insert("confidence".into(), "static".into());
    m.insert("conditionality".into(), e.conditionality.into());
    m.insert("transactionContext".into(), e.transaction_context.into());
    m.insert("guarantees".into(), json!(e.guarantees));
    // F73: the refutable labels, only when one is; a label of `guarantees`
    // not listed there is descriptive only
    // (`digest_cli::refutable_guarantees`).
    if let Some(r) = crate::engine::l5::digest_cli::refutable_guarantees(e) {
        m.insert("refutableGuarantees".into(), r);
    }
    let mut detail = Map::new();
    for (k, v) in &e.detail {
        // 1d's first-ten comma string: `resources` holds the list (§4.4.2).
        if k != "resources" {
            detail.insert(k.clone(), v.clone().into());
        }
    }
    // Round-2 G41: a group's boundary in the one display (the crossing's
    // own name, `Codeunit "CTS-SYS Azure Blob" (6225924)::Put`), never the
    // snapshot's unquoted identity display.
    if is_group(e)
        && detail.contains_key("boundary")
        && let Some(name) = boundary_names.get(&crossing_key(e))
    {
        detail.insert("boundary".into(), name.clone().into());
    }
    m.insert("detail".into(), Value::Object(detail));
    m.insert(
        "status".into(),
        if e.literal_unreachable {
            "literal-dead"
        } else {
            "live"
        }
        .into(),
    );
    // r3 E2 (additive): a literal-dead row's loaded-subscriber assumption.
    if e.literal_unreachable {
        let assumes: Vec<String> = e
            .flow_notes
            .iter()
            .flat_map(|n| &n.assumed)
            .map(|k| k.short())
            .collect();
        if !assumes.is_empty() {
            m.insert("assumes".into(), assumes.into());
        }
    }
    let write = matches!(ty, "DB_INSERT" | "DB_MODIFY" | "DB_DELETE");
    if write {
        let (class, reason) = class_json(classify_detail(&e.detail));
        m.insert("class".into(), class.into());
        m.insert("classReason".into(), reason.into());
    }
    if is_group(e) {
        let key = crossing_key(e);
        m.insert(
            "crossing".into(),
            crossings.get(&key).cloned().unwrap_or(key).into(),
        );
        if ty == "EVENT_PUBLISH" {
            // O2: every event, grouped losslessly by publisher.
            let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (id, _) in &e.group_resources {
                let (p, n) = id.split_once("/event/").unwrap_or((id.as_str(), ""));
                by.entry(p).or_default().push(n);
            }
            m.insert(
                "eventsByPublisher".into(),
                Value::Array(
                    by.into_iter()
                        .map(|(p, ns)| json!({"publisher": p, "events": ns}))
                        .collect(),
                ),
            );
        } else if !e.group_resources.is_empty() {
            m.insert(
                "resources".into(),
                Value::Array(
                    e.group_resources
                        .iter()
                        .map(|(id, d)| json!({"id": id, "display": d}))
                        .collect(),
                ),
            );
        }
        // Fable N12: one representative terminal witness per may-persist
        // group row; every per-resource witness with --detail.
        // The same selection the routine table reads (`shown_witnesses`).
        let witnesses = shown_witnesses(e, opts);
        let witness = |r: Option<&String>, w: &crate::engine::l5::group_path::GroupPath| {
            let mut v = json!({
                "hops": w.steps.iter().map(|s| step_value(rt, s, opts.detail)).collect::<Vec<_>>(),
                "terminal": {
                    "operationId": w.terminal.operation_id,
                    "file": w.terminal.file,
                    "line": w.terminal.line.map(shown),
                },
                "throughEntryPoint": w.through_entry_point,
            });
            if let Some(r) = r {
                v["resource"] = json!(r);
            }
            v
        };
        // W2-6: no witness is invented; a resource-less group says why it
        // has none.
        if let Some(Err(n)) = &e.group_terminal_path {
            m.insert("pathUnavailable".into(), n.reason.into());
        }
        if opts.detail {
            m.insert(
                "witnesses".into(),
                Value::Array(witnesses.iter().map(|&(r, w)| witness(r, w)).collect()),
            );
        } else if let Some(&(r, w)) = witnesses.first() {
            let mut v = witness(r, w);
            v["representative"] = json!(true);
            m.insert("paths".into(), Value::Array(vec![v]));
        }
    } else {
        m.insert("evidence".into(), anchor_to_value(&e.evidence));
        let shown_paths = shown_paths(e, opts);
        let terminal = json!({
            "operationId": e.origin_operation_ids.first(),
            "file": e.evidence.file,
            "line": e.evidence.line.map(shown),
        });
        let mut paths: Vec<Value> = e.via_paths[..shown_paths]
            .iter()
            .map(|p| json!({"hops": rt.hops(root, p, opts.detail), "terminal": terminal}))
            .collect();
        // Final CDO review, item 2: a write row with no listed path carries
        // its searched witness (`searched: true`; one through a call: a
        // zero-hop one adds nothing to `evidence`), or why it has none.
        if paths.is_empty() && shows_own_path(e, opts) {
            match &e.own_path {
                Some(Ok(w)) if !w.steps.is_empty() => paths.push(json!({
                    "hops": w.steps.iter().map(|s| step_value(rt, s, opts.detail)).collect::<Vec<_>>(),
                    "terminal": {
                        "operationId": w.terminal.operation_id,
                        "file": w.terminal.file,
                        "line": w.terminal.line.map(shown),
                    },
                    "searched": true,
                })),
                Some(Err(n)) => {
                    m.insert("pathUnavailable".into(), n.reason.into());
                }
                _ => {}
            }
        }
        m.insert("paths".into(), Value::Array(paths));
        m.insert("pathsKept".into(), e.via_paths.len().into());
        m.insert(
            "pathsOmitted".into(),
            (e.via_paths.len() - shown_paths).into(),
        );
        m.insert(
            "pathsProducerTruncated".into(),
            e.via_paths_truncated.into(),
        );
    }
    let sg: Vec<Value> = e
        .scoped_guarantees
        .iter()
        .filter(|sg| opts.detail || !trivial(sg))
        .map(scoped_guarantee_to_value)
        .collect();
    if !sg.is_empty() {
        m.insert("scopedGuarantees".into(), Value::Array(sg));
    }
    Value::Object(m)
}

/// One entry of 3.0.0 (module doc).
#[must_use]
pub fn entry_v3(
    entry: &DigestEntryFull,
    r: &RoutineRollup,
    rt: &RoutineTable,
    cx: &RollupCx<'_>,
    opts: RenderOpts,
) -> Value {
    let root = entry.routine_id.as_str();
    // Crossings, keyed `c:` + hash of the boundary identity.
    let ids: BTreeSet<String> = r.crossings.iter().map(|c| c.id.clone()).collect();
    let ckeys = hashed_keys("c:", &ids);
    let boundary_names: BTreeMap<String, String> = r
        .crossings
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    let mut crossings = Map::new();
    let prefixed = prefixed_crossings(entry, opts);
    for c in &r.crossings {
        let mut v = json!({
            "id": c.id,
            "boundary": c.name,
            "targets": c.target_ids.iter().map(|t| rt.key(t)).collect::<Vec<_>>(),
            "sites": c.sites,
        });
        if !prefixed.contains(&c.id) {
            // Only folded rows lie beyond it (`moreSites`): no prefix
            // path, as they print none (`--detail`: printed).
            v["prefixOmitted"] = json!(true);
        } else if let Some(p) = c.prefix_row {
            let e = &entry.effects[p];
            if let Some(path) = e.via_paths.iter().min_by_key(|p| p.len()) {
                v["prefix"] = json!({"hops": rt.hops(root, path, opts.detail)});
            }
        }
        crossings.insert(ckeys[&c.id].clone(), v);
    }
    // MCP output step 2: past the first rows of a site-enumerated type, a
    // row is its place in `moreSites` (`--detail`: every row in `effects`).
    let mut effects: Vec<Value> = Vec::new();
    let mut more: BTreeMap<&str, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for (e, o) in entry.effects.iter().zip(row_opts(entry, opts)) {
        match o {
            Some(o) => effects.push(effect_v3(e, root, rt, &ckeys, &boundary_names, o)),
            None => {
                let (place, item) = more_site(e, &ckeys);
                more.entry(e.effect_type.as_str())
                    .or_default()
                    .entry(place)
                    .or_default()
                    .push(item);
            }
        }
    }
    let folded: usize = if opts.detail {
        0
    } else {
        entry
            .effects
            .iter()
            .flat_map(|e| &e.scoped_guarantees)
            .filter(|sg| trivial(sg))
            .count()
    };
    let mut m = Map::new();
    // V3 §2.1: the one display as `qualifiedDisplay` (`display` keeps the
    // routine's name, `objectDisplay` stays), `signature`, `member`,
    // `selector` and `handle` beside `stableId`.
    let mut routine = routine_to_value(entry);
    for (k, x) in cx.names.identity_json(root) {
        let k = if k == "display" {
            "qualifiedDisplay".to_string()
        } else {
            k
        };
        routine[k] = x;
    }
    let handle = cx.handle(root);
    if !handle.is_empty() {
        routine["handle"] = json!(handle);
    }
    m.insert("routine".into(), routine);
    m.insert(
        "coverage".into(),
        json!({
            "status": entry.coverage_status,
            "reasons": entry.coverage_reasons,
            "assumptions": ["loaded-program"],
            "limits": limit_values(r, entry),
        }),
    );
    m.insert("summary".into(), r.totals.to_json());
    m.insert("crossings".into(), Value::Object(crossings));
    m.insert("effects".into(), Value::Array(effects));
    // r3 §7 R1 (additive, 5.x only: `digest_v5::strip_assumption_records`):
    // the loaded-subscriber assumptions the literal-dead rows rest on, one
    // record each with the sites it rules out. The rows' `assumes` strings
    // stay.
    let records = crate::engine::l5::digest_cli::assumption_records(
        &entry.effects,
        (0..entry.effects.len()).filter(|&i| entry.effects[i].literal_unreachable),
    );
    if !records.is_empty() {
        m.insert(
            "assumptionRecords".into(),
            records.iter().map(|x| x.to_json()).collect(),
        );
    }
    if !more.is_empty() {
        // Per type and place, the items as one `; `-joined string.
        let joined: BTreeMap<&str, BTreeMap<String, String>> = more
            .into_iter()
            .map(|(t, places)| {
                (
                    t,
                    places
                        .into_iter()
                        .map(|(p, items)| (p, items.join("; ")))
                        .collect(),
                )
            })
            .collect();
        m.insert("moreSites".into(), json!(joined));
    }
    m.insert("scopedGuaranteesFolded".into(), folded.into());
    // Unresolved: workspace and not-placed items (candidates as a count);
    // dependency items as counts and top callees. Every item with --detail.
    let g = &r.gaps;
    let items: Vec<Value> = if opts.detail {
        entry
            .unresolved
            .iter()
            .map(unresolved_item_to_value)
            .collect()
    } else {
        g.workspace
            .iter()
            .chain(&g.not_placed)
            .map(|&i| {
                let u = &entry.unresolved[i];
                let mut v = unresolved_item_to_value(u);
                if let Some(o) = v.as_object_mut()
                    && o.remove("candidates").is_some()
                {
                    o.insert(
                        "candidateCount".into(),
                        u.candidates.as_ref().map_or(0, Vec::len).into(),
                    );
                }
                v
            })
            .collect()
    };
    m.insert("unresolved".into(), Value::Array(items));
    m.insert(
        "unresolvedInDependencies".into(),
        json!({
            "total": g.dependency,
            "polymorphic": g.dependency_polymorphic,
            "dynamicTarget": g.dependency_dynamic,
            "topCallees": g.top_callees.iter().take(10)
                .map(|(c, n)| json!({"callee": c, "count": n}))
                .collect::<Vec<_>>(),
        }),
    );
    if opts.detail {
        m.insert(
            "entryDiagnostics".into(),
            crate::engine::l5::digest_cli::entry_diagnostics_to_value(entry),
        );
    } else {
        let mut counts: BTreeMap<&str, Value> = BTreeMap::new();
        for d in &entry.entry_diagnostics {
            match d.reason {
                Some(reason) => {
                    let slot = counts.entry(d.kind).or_insert_with(|| json!({}));
                    let n = slot[reason].as_u64().unwrap_or(0) + 1;
                    slot[reason] = json!(n);
                }
                None => {
                    let slot = counts.entry(d.kind).or_insert_with(|| json!(0));
                    *slot = json!(slot.as_u64().unwrap_or(0) + 1);
                }
            }
        }
        m.insert("entryDiagnosticCounts".into(), json!(counts));
    }
    m.insert(
        "unresolvedTraversal".into(),
        json!({
            "maxDepth": entry.unresolved_traversal.max_depth,
            "truncated": entry.unresolved_traversal.truncated,
            "visitedRoutines": entry.unresolved_traversal.visited_routines,
        }),
    );
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §4.4.2 (astra 5): a collision extends EVERY colliding key by 4 hex,
    /// repeated until all are unique; the keys are stable. Hand-stated: the
    /// keys of 3,000 ids are unique at the 12-hex default (no collision to
    /// extend), and the function is a function of the id alone.
    #[test]
    fn hashed_keys_are_unique_and_stable() {
        let ids: BTreeSet<String> = (0..3000).map(|i| format!("id-{i}")).collect();
        let a = hashed_keys("r:", &ids);
        let b = hashed_keys("r:", &ids);
        assert_eq!(a, b);
        let distinct: BTreeSet<&String> = a.values().collect();
        assert_eq!(distinct.len(), ids.len());
        assert!(a.values().all(|k| k.len() == 2 + 12));
        let one: BTreeSet<String> = ["id-7".to_string()].into();
        assert_eq!(hashed_keys("r:", &one)["id-7"], a["id-7"]);
    }
}
