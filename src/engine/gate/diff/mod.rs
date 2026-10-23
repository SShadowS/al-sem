//! cli-b/b4 — the `diff` engine. Byte-parity port of al-sem `src/diff/*`.
//!
//! `run_diff_engine` compares two deserialized capability snapshots (each an
//! insertion-ordered [`CborValue`] tree from `snapshot_deserialize`) across the 5
//! diff dimensions (ABI / schema / events / capabilities / permissions), applies
//! the coverage policy, and returns a sorted [`DiffEngineResult`]. The formatters
//! (`format`) render it human / json (envelope-wrapped) / sarif.
//!
//! The engine operates over the `CborValue` tree directly: every snapshot fact is
//! a `CborValue::Map`, read by key — so the diff is byte-faithful to al-sem's TS
//! passes without a separate typed model.

use crate::engine::gate::cbor::CborValue;

pub mod cli;
pub mod fingerprint;
pub mod format;
pub mod indexes;
pub mod passes;
pub mod policy;
pub mod preflight;
pub mod renames;

pub use fingerprint::{DiffCategory, DiffKind};
pub use renames::RenameOverlay;

/// A subject of a diff finding (`{normalizedStableId, oldOriginalStableId?,
/// newStableId?, displayName}`). The `oldOriginalStableId`/`newStableId` slots
/// drive the renderer-side "renamed from X" note.
#[derive(Debug, Clone)]
pub struct DiffSubject {
    pub normalized_stable_id: String,
    pub old_original_stable_id: Option<String>,
    pub new_stable_id: Option<String>,
    pub display_name: String,
}

/// A single diff finding. `details` is the per-pass detail map (already in its
/// final key set); `coverage_state` is attached dynamically by the policy pass.
#[derive(Debug, Clone)]
pub struct DiffFinding {
    pub id: String,
    pub category: DiffCategory,
    pub kind: DiffKind,
    pub severity: Severity,
    pub subject: DiffSubject,
    pub comparison_cone: Vec<String>,
    /// The `details` object as ordered (key, value) pairs (JSON+sarif sort later).
    pub details: Vec<(String, CborValue)>,
    /// Attached by the coverage policy: `{old, new}` coverage statuses.
    pub coverage_state: Option<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Info => "info",
        }
    }
    /// Severity sort rank (al-sem SEVERITY_RANK: critical=0 … info=4).
    pub fn rank(self) -> u8 {
        match self {
            Severity::Critical => 0,
            Severity::High => 1,
            Severity::Medium => 2,
            Severity::Low => 3,
            Severity::Info => 4,
        }
    }
}

/// A diff diagnostic — preflight / rename / policy. Carried as ordered key/value
/// pairs so the human + json renderers serialize them faithfully.
#[derive(Debug, Clone)]
pub struct DiffDiagnostic {
    pub kind: String,
    pub fields: Vec<(String, CborValue)>,
}

/// The diff summary block.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub findings_by_category: [(DiffCategory, u32); 5],
    pub findings_by_severity: [u32; 5], // critical, high, medium, low, info
    pub coverage_incomplete_cones: u32,
    pub renames_applied: u32,
}

#[derive(Debug, Clone)]
pub struct DiffEngineResult {
    pub findings: Vec<DiffFinding>,
    pub diagnostics: Vec<DiffDiagnostic>,
    pub summary: DiffSummary,
    /// A fatal preflight stopped the diff (#220 D5.5: the CLI exits 1).
    pub fatal: bool,
}

pub struct DiffEngineOptions {
    pub coverage_policy: CoveragePolicy,
    pub deterministic: bool,
    pub rename_overlay: Option<RenameOverlay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoveragePolicy {
    Loose,
    Strict,
}

fn empty_summary() -> DiffSummary {
    DiffSummary {
        findings_by_category: [
            (DiffCategory::Abi, 0),
            (DiffCategory::Schema, 0),
            (DiffCategory::Events, 0),
            (DiffCategory::Capabilities, 0),
            (DiffCategory::Permissions, 0),
        ],
        findings_by_severity: [0; 5],
        coverage_incomplete_cones: 0,
        renames_applied: 0,
    }
}

/// Run the full diff engine: preflight → renames → indexes → 5 passes → policy →
/// sort → summary. Mirrors al-sem `runDiffEngine`.
pub fn run_diff_engine(
    old_snap: &CborValue,
    new_snap: &CborValue,
    opts: &DiffEngineOptions,
) -> DiffEngineResult {
    let mut diagnostics: Vec<DiffDiagnostic> = Vec::new();

    let preflight = preflight::run_preflight(old_snap, new_snap, opts.coverage_policy);
    diagnostics.extend(preflight.diagnostics);
    if preflight.fatal {
        return DiffEngineResult {
            findings: Vec::new(),
            diagnostics,
            summary: empty_summary(),
            fatal: true,
        };
    }

    let overlay = opts.rename_overlay.clone().unwrap_or_default();
    let (mut rename_table, rename_diags) = renames::build_rename_table(&overlay);
    diagnostics.extend(rename_diags);
    let renames_applied = rename_table.len() as u32;
    for (old_id, new_id) in renames::pair_signature_changes(old_snap, new_snap, &rename_table) {
        rename_table.insert(old_id, renames::RenameEntry { new_id, auto: true });
    }

    let indexes = indexes::build_diff_indexes(old_snap, new_snap, &rename_table);
    diagnostics.extend(indexes.rename_diagnostics.clone());

    let mut findings: Vec<DiffFinding> = Vec::new();
    findings.extend(passes::diff_abi(&indexes));
    findings.extend(passes::diff_schema(&indexes));
    findings.extend(passes::diff_events(&indexes));
    findings.extend(passes::diff_capabilities(&indexes));
    findings.extend(passes::diff_permissions(&indexes));

    let (policy_findings, policy_diags) =
        policy::apply_coverage_policy(findings, &indexes, opts.coverage_policy);
    diagnostics.extend(policy_diags.clone());

    let mut sorted = policy_findings;
    sort_findings(&mut sorted);

    let summary = compute_summary(&sorted, &policy_diags, renames_applied);

    DiffEngineResult {
        findings: sorted,
        diagnostics,
        summary,
        fatal: false,
    }
}

/// The deterministic finding sort: severity rank, then category (string <),
/// then kind (string <), then id (string <). STABLE. Mirrors `runDiffEngine`'s
/// final `.slice().sort(...)`.
///
/// String comparisons use Rust's ordinal `str::cmp` (byte order). al-sem's `<`
/// on JS strings is UTF-16 code-unit order; for the category/kind/id values here
/// — all ASCII (lowercase hyphenated kinds, hex/guid ids) — ordinal byte order ==
/// UTF-16 order, so this is byte-faithful. (The engine-wide ordinal-vs-localeCompare
/// comparator parity is the known tracked item; not exercised by any non-ASCII id.)
fn sort_findings(findings: &mut [DiffFinding]) {
    findings.sort_by(|a, b| {
        let sa = a.severity.rank();
        let sb = b.severity.rank();
        if sa != sb {
            return sa.cmp(&sb);
        }
        let ac = a.category.as_str();
        let bc = b.category.as_str();
        if ac != bc {
            return ac.cmp(bc);
        }
        let ak = a.kind.as_str();
        let bk = b.kind.as_str();
        if ak != bk {
            return ak.cmp(bk);
        }
        a.id.cmp(&b.id)
    });
}

fn compute_summary(
    findings: &[DiffFinding],
    policy_diags: &[DiffDiagnostic],
    renames_applied: u32,
) -> DiffSummary {
    let mut s = empty_summary();
    for f in findings {
        for entry in s.findings_by_category.iter_mut() {
            if entry.0 == f.category {
                entry.1 += 1;
            }
        }
        s.findings_by_severity[f.severity.rank() as usize] += 1;
    }
    s.coverage_incomplete_cones = policy_diags
        .iter()
        .filter(|d| d.kind == "coverage-incomplete")
        .count() as u32;
    s.renames_applied = renames_applied;
    s
}

// ── shared CborValue field-reading helpers ──────────────────────────────────

/// Read a string field off a `CborValue::Map`.
pub(crate) fn get_str<'a>(v: &'a CborValue, key: &str) -> Option<&'a str> {
    match v {
        CborValue::Map(m) => match m.get(key) {
            Some(CborValue::Text(s)) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Read an array field off a `CborValue::Map`.
pub(crate) fn get_array<'a>(v: &'a CborValue, key: &str) -> Option<&'a [CborValue]> {
    match v {
        CborValue::Map(m) => match m.get(key) {
            Some(CborValue::Array(a)) => Some(a.as_slice()),
            _ => None,
        },
        _ => None,
    }
}

/// Read a top-level array (snapshot fact list), defaulting to empty.
pub(crate) fn snapshot_array<'a>(snap: &'a CborValue, key: &str) -> &'a [CborValue] {
    get_array(snap, key).unwrap_or(&[])
}
