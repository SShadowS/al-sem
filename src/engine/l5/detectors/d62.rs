//! D62 — `FeatureTelemetry.LogUsage` before the success path (OPT-IN).
//! BCQuality `feature-usage-only-after-success`: usage logged before a fallible
//! step (record write or explicit Error call later in the routine) counts
//! failed runs as feature usage.
//!
//! Join: member call `<v>.LogUsage(..)` where `<v>`'s DECLARED type contains
//! `codeunit "feature telemetry"` (text match — the System Application codeunit
//! is not in workspace source), with any record write op or error-call
//! operation site strictly AFTER it in the same routine (straight-line source
//! order). Severity: low. Confidence: possible.

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, before_anchor};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::{PAnchor, PCFNNode, PCallee};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d62-telemetry-before-success";

const WRITE_OPS: &[&str] = &[
    "Insert",
    "Modify",
    "Delete",
    "DeleteAll",
    "ModifyAll",
    "Rename",
];

/// Which arm of an `if`/`case` ancestor a descent passed through, on the way
/// down to a located site. `Case(idx)` is the index of the chosen
/// `case-branch` child (the else/default branch, if any, is simply the last
/// entry in `PCFNNode.children` — see `ir_walk.rs`'s `Case` lowering).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arm {
    Then,
    Else,
    Case(usize),
}

/// One step of a root-to-target trail: the ancestor `if`/`case` node's OWN
/// `source_range` (its identity — two different physical if/case statements
/// never share a range, even when textually adjacent/sequential) paired with
/// the arm the descent took at that ancestor.
type TrailEntry = (Option<(u32, u32, u32, u32)>, Arm);

/// Identifies one call-site/operation-site leaf in the statement tree. `id`
/// matches `PCFNNode.callsite_id`/`operation_id` directly when the site's own
/// id lives in that namespace (true for a `LogUsage` call site and a
/// `WRITE_OPS` record operation); `range` is the fallback for a site whose id
/// does NOT appear in the tree under that name — an `error-call`
/// `POperationSite` carries its own synthetic op-counter id, but the CFN leaf
/// for `Error(...)` is built from the paired CALL SITE id instead (see
/// `ir_walk.rs`'s op/cs-index bookkeeping), so id-matching a POperationSite of
/// kind `error-call` never succeeds and range is what actually locates it.
struct Locator<'a> {
    id: Option<&'a str>,
    range: (u32, u32, u32, u32),
}

impl Locator<'_> {
    fn matches(&self, node: &PCFNNode) -> bool {
        if let Some(id) = self.id
            && (node.callsite_id.as_deref() == Some(id) || node.operation_id.as_deref() == Some(id))
        {
            return true;
        }
        node.source_range == Some(self.range)
    }
}

fn anchor_tuple(a: &PAnchor) -> (u32, u32, u32, u32) {
    (a.start_line, a.start_column, a.end_line, a.end_column)
}

/// Finds `want` inside `node`'s subtree, returning its root-to-target trail
/// (outermost ancestor first) if found. A site inside a node's
/// `condition_leaves` (the if/case's own CONDITION or scrutinee, e.g. the
/// `TryX()` in `if TryX() then`) is evaluated BEFORE either arm runs — it is
/// deliberately NOT treated as being "in" either arm, so no trail entry is
/// added for the current ancestor when a match is found there (the caller
/// simply keeps unwinding to whatever contains the if/case as a whole).
fn locate(node: &PCFNNode, want: &Locator) -> Option<Vec<TrailEntry>> {
    if want.matches(node) {
        return Some(Vec::new());
    }
    if let Some(leaves) = &node.condition_leaves {
        for leaf in leaves {
            if let Some(trail) = locate(leaf, want) {
                return Some(trail);
            }
        }
    }
    match node.kind.as_str() {
        "if" => {
            if let Some(children) = &node.children {
                for c in children {
                    if let Some(mut trail) = locate(c, want) {
                        trail.insert(0, (node.source_range, Arm::Then));
                        return Some(trail);
                    }
                }
            }
            if let Some(else_children) = &node.else_children {
                for c in else_children {
                    if let Some(mut trail) = locate(c, want) {
                        trail.insert(0, (node.source_range, Arm::Else));
                        return Some(trail);
                    }
                }
            }
            None
        }
        "case" => {
            if let Some(children) = &node.children {
                for (idx, c) in children.iter().enumerate() {
                    if let Some(mut trail) = locate(c, want) {
                        trail.insert(0, (node.source_range, Arm::Case(idx)));
                        return Some(trail);
                    }
                }
            }
            None
        }
        // Any other container (block/repeat/while/for/foreach/case-branch/...)
        // has no exclusivity semantics of its own — its children run
        // sequentially (or, for a loop, repeatedly), never as alternatives —
        // so descending adds no trail entry.
        _ => {
            if let Some(children) = &node.children {
                for c in children {
                    if let Some(trail) = locate(c, want) {
                        return Some(trail);
                    }
                }
            }
            if let Some(else_children) = &node.else_children {
                for c in else_children {
                    if let Some(trail) = locate(c, want) {
                        return Some(trail);
                    }
                }
            }
            None
        }
    }
}

/// True iff `a` and `b` sit in DIFFERENT, mutually exclusive arms of the SAME
/// `if`/`case` ancestor in `tree` — i.e. at most one of them can execute in a
/// given run. Compares the two root-to-target trails entry-by-entry: as long
/// as both keep naming the SAME physical ancestor (by its `source_range`
/// identity) they're walking down the same real subtree, so the trails are
/// still "in agreement"; the moment source_ranges disagree at some depth,
/// that's actually two unrelated (e.g. sequential, non-nested) conditionals —
/// no exclusivity can be proven from that. A genuine fork is found only when
/// the ancestor identity MATCHES but the chosen arm differs. Either site
/// failing to locate (a body-less routine, a tree gap) also proves nothing —
/// the safe default is "not exclusive" (keep firing), never the reverse.
fn mutually_exclusive(tree: &PCFNNode, a: &Locator, b: &Locator) -> bool {
    let Some(trail_a) = locate(tree, a) else {
        return false;
    };
    let Some(trail_b) = locate(tree, b) else {
        return false;
    };
    for (ea, eb) in trail_a.iter().zip(trail_b.iter()) {
        if ea.0.is_some() && ea.0 == eb.0 {
            if ea.1 != eb.1 {
                return true;
            }
            continue;
        }
        // Different physical ancestor at this depth (or an unidentifiable
        // node) — the trails have left the shared subtree; nothing further
        // to compare.
        return false;
    }
    false
}

pub fn detect_d62(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_terminal_log = 0u64;
    let mut skipped_exclusive_branch = 0u64;

    for routine in &ws.routines {
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        let ft_vars: Vec<String> = routine
            .variables
            .iter()
            .filter(|v| {
                let t = v.declared_type.to_lowercase();
                t.starts_with("codeunit") && t.contains("feature telemetry")
            })
            .map(|v| v.name.to_lowercase())
            .collect();
        if ft_vars.is_empty() {
            continue;
        }

        for cs in &routine.call_sites {
            let PCallee::Member { receiver, method } = &cs.callee else {
                continue;
            };
            if !method.eq_fold_identifier("LogUsage") {
                continue;
            }
            if !ft_vars.contains(&receiver.to_lowercase()) {
                continue;
            }
            candidates_considered += 1;

            // A later fallible site only counts as a real risk if it can
            // ACTUALLY run in the same execution as the log — a write/Error
            // sitting textually later but in a MUTUALLY EXCLUSIVE arm of the
            // same if/case (the `if Success then LogUsage() else Error(...)`
            // idiom) can never fire alongside it. `before_anchor` alone only
            // tests source-text order, so every raw candidate additionally
            // goes through `mutually_exclusive` against the statement tree.
            let log_locator = Locator {
                id: Some(cs.id.as_str()),
                range: anchor_tuple(&cs.source_anchor),
            };
            let not_exclusive_with_log = |site: &Locator| {
                !routine
                    .statement_tree
                    .as_ref()
                    .is_some_and(|t| mutually_exclusive(t, &log_locator, site))
            };

            let raw_write_after = routine.record_operations.iter().any(|op| {
                WRITE_OPS.contains(&op.op.as_str())
                    && before_anchor(&cs.source_anchor, &op.source_anchor)
            });
            let write_after = routine.record_operations.iter().any(|op| {
                WRITE_OPS.contains(&op.op.as_str())
                    && before_anchor(&cs.source_anchor, &op.source_anchor)
                    && not_exclusive_with_log(&Locator {
                        id: Some(op.id.as_str()),
                        range: anchor_tuple(&op.source_anchor),
                    })
            });

            let raw_error_after = routine.operation_sites.iter().any(|s| {
                s.kind == "error-call" && before_anchor(&cs.source_anchor, &s.source_anchor)
            });
            let error_after = routine.operation_sites.iter().any(|s| {
                s.kind == "error-call"
                    && before_anchor(&cs.source_anchor, &s.source_anchor)
                    && not_exclusive_with_log(&Locator {
                        id: Some(s.id.as_str()),
                        range: anchor_tuple(&s.source_anchor),
                    })
            });

            let fallible_after = write_after || error_after;
            if !fallible_after {
                if raw_write_after || raw_error_after {
                    skipped_exclusive_branch += 1;
                } else {
                    skipped_terminal_log += 1;
                }
                continue;
            }

            let confidence: FindingConfidence = to_confidence(&[], "possible");
            let id = format!("d62/{}/{}", routine.id, cs.id);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: "Feature usage logged before success".into(),
                root_cause: format!(
                    "{} calls FeatureTelemetry.LogUsage before fallible work later in the \
                     routine — runs that fail after the log still count as feature usage.",
                    routine.name
                ),
                severity: "low".to_string(),
                confidence,
                primary_location: anchor_of(&cs.source_anchor, routine),
                evidence_path: vec![EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: Some(cs.id.clone()),
                    loop_id: None,
                    source_anchor: anchor_of(&cs.source_anchor, routine),
                    note: "LogUsage before fallible operations".to_string(),
                }],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: "Move LogUsage after the operation's success point (end of \
                                  the routine / after the final write)."
                        .into(),
                    safety: "high".into(),
                }],
                provenance: vec![Evidence {
                    source: "tree-sitter",
                    note: None,
                }],
                actionable_anchor: None,
                fingerprint: None,
                event_kind: None,
                cross_extension_subscribers: None,
                cohort_contexts: None,
            };
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("terminalLog", skipped_terminal_log);
    stats.add_skip("exclusiveBranch", skipped_exclusive_branch);
    Ok(DetectorOutput::no_diag(findings, stats))
}
