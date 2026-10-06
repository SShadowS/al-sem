//! Site links: which program-engine edges sit at each body-fact site
//! (engine-switch S2b.6; spec G13).
//!
//! The detector model's call and operation sites (`/csN`, `/opN`, DFS-numbered by
//! the body pipeline) and the program engine's call-site edges anchor the same
//! syntax node, so they join on its span. Positions are zero-based rows and UTF-8
//! BYTE columns on both sides (`Utf16Cols::col` passes byte columns through), so no
//! column conversion happens here; LSP encoding is a separate boundary.
//!
//! Two rules the old inline join did not keep:
//! - **Every edge.** Several program edges can share one span (nested calls on one
//!   line do not, but a call and the implicit trigger it fires can). The links keep
//!   all of them, in report order; picking one is the reader's decision.
//! - **App-qualified keys.** A key carries the caller's app as well as the virtual
//!   path, because two apps can both contain `src/Main.al` (S7 links dependency
//!   bodies too).

use std::collections::HashMap;

use crate::program::body::features::PAnchor;
use crate::program::node::AppRef;
use crate::program::resolve::edge::CanonicalSpan;
use crate::program::resolve::full::{ClassifiedEdge, ObligationId, ProgramReport};

/// `(virtual path, start row, start col, end row, end col)`.
pub type SpanKey = (String, u32, u32, u32, u32);

/// The span key of a model anchor (`ws:` unit prefix dropped).
#[must_use]
pub fn model_key(a: &PAnchor) -> SpanKey {
    let unit = a
        .source_unit_id
        .strip_prefix("ws:")
        .unwrap_or(&a.source_unit_id);
    (
        unit.to_string(),
        a.start_line,
        a.start_column,
        a.end_line,
        a.end_column,
    )
}

/// The span key of a program site.
#[must_use]
pub fn program_key(s: &CanonicalSpan) -> SpanKey {
    (
        s.unit.clone(),
        s.start.line,
        s.start.col,
        s.end.line,
        s.end.col,
    )
}

/// Every call-site edge of a program report, by `(caller app, span)`.
pub struct SiteLinks<'r> {
    by_site: HashMap<(AppRef, SpanKey), Vec<&'r ClassifiedEdge>>,
}

impl<'r> SiteLinks<'r> {
    /// Index every call-site obligation's edge, in report order.
    #[must_use]
    pub fn build(report: &'r ProgramReport) -> Self {
        let mut by_site: HashMap<(AppRef, SpanKey), Vec<&'r ClassifiedEdge>> = HashMap::new();
        for ce in &report.edges {
            if !matches!(ce.obligation_id, ObligationId::CallSite { .. }) {
                continue;
            }
            by_site
                .entry((ce.edge.from.object.app, program_key(&ce.edge.site.span)))
                .or_default()
                .push(ce);
        }
        SiteLinks { by_site }
    }

    /// Every edge at `key` in `app`, report order; empty when none.
    #[must_use]
    pub fn at(&self, app: AppRef, key: &SpanKey) -> &[&'r ClassifiedEdge] {
        self.by_site
            .get(&(app, key.clone()))
            .map_or(&[], Vec::as_slice)
    }

    /// Every `(span, edges)` of `app`, unordered.
    pub fn sites_of(
        &self,
        app: AppRef,
    ) -> impl Iterator<Item = (&SpanKey, &[&'r ClassifiedEdge])> + '_ {
        self.by_site
            .iter()
            .filter(move |((a, _), _)| *a == app)
            .map(|((_, k), v)| (k, v.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-stated on a real report: a second edge at an existing span is KEPT
    /// (the old join kept only the first), and an edge at the same path and span
    /// in ANOTHER app is a different site.
    #[test]
    fn links_keep_every_edge_and_qualify_by_app() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let (_ctx, mut report, _) =
            crate::program::resolve::full::build_program_with_coverage(&ws).unwrap();
        let primary = report.primary_app_ref;
        let i = report
            .edges
            .iter()
            .position(|ce| {
                matches!(ce.obligation_id, ObligationId::CallSite { .. })
                    && ce.edge.from.object.app == primary
            })
            .unwrap();
        let key = program_key(&report.edges[i].edge.site.span);
        let before = SiteLinks::build(&report).at(primary, &key).len();

        let copy = |ce: &ClassifiedEdge| ClassifiedEdge {
            obligation_id: ce.obligation_id.clone(),
            edge: ce.edge.clone(),
        };
        let same = copy(&report.edges[i]);
        report.edges.push(same);
        let mut other_app = copy(&report.edges[i]);
        other_app.edge.from.object.app = AppRef(primary.0 + 1000);
        report.edges.push(other_app);

        let links = SiteLinks::build(&report);
        assert_eq!(links.at(primary, &key).len(), before + 1);
        assert_eq!(links.at(AppRef(primary.0 + 1000), &key).len(), 1);
        assert_eq!(
            links.sites_of(primary).map(|(_, e)| e.len()).sum::<usize>(),
            report
                .edges
                .iter()
                .filter(
                    |ce| matches!(ce.obligation_id, ObligationId::CallSite { .. })
                        && ce.edge.from.object.app == primary
                )
                .count()
        );
    }
}
