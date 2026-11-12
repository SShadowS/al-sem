//! TLS-28: the sited graph a one-shot `alsem impact` keeps between its parts
//! (spec `docs/superpowers/specs/2026-11-12-impact-shared-sited.md`).
//!
//! The impact front runs its parts one after another (main, then up, then
//! tests-select), each a fresh drive and so a fresh program build. Inside
//! [`with_sited_cache`] a view stores the sited graph it built, and a later
//! view whose demands read nothing but the sited graph, with the same
//! [`ViewKey`] and the same input bytes, runs on it instead of building.
//!
//! The key is the view key plus the resident engine's inputs stamp
//! ([`Manifest::consumed`]: every input's length and blake3, the walks, the
//! packages, a test universe's injected inputs, the engine and grammar
//! versions), so two reads of different bytes never share a graph. One entry:
//! a store replaces it. Outside the scope nothing is kept.

use std::cell::{Cell, RefCell};

use super::job::{ViewDemand, ViewKey, ViewUniverse};
use crate::engine::resident::manifest::Manifest;
use crate::program::impact_graph::SitedGraph;

struct Kept {
    key: ViewKey,
    stamp: String,
    graph: SitedGraph,
}

thread_local! {
    /// `None`: no scope. `Some(None)`: a scope with nothing kept yet.
    static SLOT: RefCell<Option<Option<Kept>>> = const { RefCell::new(None) };
    static HITS: Cell<usize> = const { Cell::new(0) };
}

/// Whether the scope holds a graph (tests).
#[cfg(test)]
fn kept() -> bool {
    SLOT.with(|s| s.borrow().as_ref().is_some_and(Option::is_some))
}

#[cfg(test)]
thread_local! {
    /// How many views `run_view` built on this thread.
    pub(super) static BUILDS: Cell<usize> = const { Cell::new(0) };
}

/// Run `f` with a sited-graph cache on this thread. The cache (and any graph
/// it holds) is dropped when `f` returns or unwinds; an enclosing scope's
/// cache is restored.
pub fn with_sited_cache<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Option<Kept>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SLOT.with(|s| *s.borrow_mut() = previous);
        }
    }
    let previous = SLOT.with(|s| s.borrow_mut().replace(None));
    let _restore = Restore(previous);
    f()
}

/// Run `f` with no cache on this thread (a part no later part shares a graph
/// with: impact's `--base` side, its own workspace copy); what the enclosing
/// scope kept is dropped first, so no graph lives through `f`'s build, and the
/// scope is restored, empty, after.
pub fn without_sited_cache<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let had_scope = self.0;
            SLOT.with(|s| *s.borrow_mut() = had_scope.then_some(None));
        }
    }
    let had_scope = SLOT.with(|s| s.borrow_mut().take().is_some());
    let _restore = Restore(had_scope);
    f()
}

/// How many views ran on a kept graph on this thread (tests and probes).
#[must_use]
pub fn hits() -> usize {
    HITS.with(Cell::get)
}

fn active() -> bool {
    SLOT.with(|s| s.borrow().is_some())
}

/// Every demand on the view reads the sited graph and no model
/// (`ViewNeeds::SITED`): the views a kept graph can serve.
pub(super) fn sited_only(on_view: &[(usize, &ViewDemand)]) -> bool {
    on_view.iter().all(|(_, d)| d.needs.sited && !d.needs.model)
}

/// The view's inputs stamp, when this view takes part in the cache: inside a
/// scope, a sited-only view (it may hit, and stores what it builds) or a
/// cross-app model view (it stores the graph off its program, for the up
/// part after it).
pub(super) fn stamp_for(on_view: &[(usize, &ViewDemand)]) -> Option<String> {
    if !active() {
        return None;
    }
    let (_, first) = on_view[0];
    let cross_app = first.key.universe == ViewUniverse::CrossApp;
    // A model view with no model instance id keeps its single-app fallback
    // build (`build_view_model`): it lends no cross-app program, keeps nothing.
    let model =
        on_view.iter().any(|(_, d)| d.needs.model) && first.inputs.model_instance_id.is_some();
    (!lossy_twins(&first.inputs) && (sited_only(on_view) || (cross_app && model))).then(|| {
        let _s = crate::engine::perf_trace::span("batch", "batch.sited_cache.stamp");
        Manifest::consumed(&first.inputs, !cross_app).stamp()
    })
}

/// The manifest keys each `.al` file by its lossy path (`al:<rel_posix>`, a
/// source dependency's `src:<spelled>\0<rel>`): two names that are not UTF-8
/// can share one, and the stamp then holds only the last file's hash. Such a
/// read takes no part (review r1, sol and Opus).
fn lossy_twins(inputs: &crate::program::inputs::WorkspaceInputs) -> bool {
    fn twins<'a>(mut names: impl Iterator<Item = &'a str>) -> bool {
        let mut seen = std::collections::HashSet::new();
        names.any(|n| !seen.insert(n))
    }
    twins(inputs.al_files.iter().map(|f| f.rel_posix.as_str()))
        || inputs.source_dependencies.as_ref().is_ok_and(|deps| {
            deps.iter()
                .any(|d| twins(d.files.iter().map(|f| f.virtual_path.as_str())))
        })
}

/// `f` on the kept graph when it was built for `key` from inputs with
/// `stamp`; `None` (and `f` not run) otherwise.
pub(super) fn with_hit<R>(
    key: &ViewKey,
    stamp: &str,
    f: impl FnOnce(&SitedGraph) -> R,
) -> Option<R> {
    SLOT.with(|s| {
        let slot = s.borrow();
        let kept = slot.as_ref()?.as_ref()?;
        (kept.key == *key && kept.stamp == stamp).then(|| {
            HITS.with(|h| h.set(h.get() + 1));
            f(&kept.graph)
        })
    })
}

/// Drop what the scope holds, before any view builds: a kept graph never
/// lives through another build. Outside a scope, nothing.
pub(super) fn clear() {
    SLOT.with(|s| {
        if let Some(slot) = s.borrow_mut().as_mut() {
            *slot = None;
        }
    });
}

/// Keep `graph`, built for `key` from inputs with `stamp`, in place of what
/// the scope held. Outside a scope it is dropped.
pub(super) fn store(key: &ViewKey, stamp: String, graph: SitedGraph) {
    SLOT.with(|s| {
        if let Some(slot) = s.borrow_mut().as_mut() {
            *slot = Some(Kept {
                key: key.clone(),
                stamp,
                graph,
            });
        }
    });
}

#[cfg(test)]
#[path = "sited_cache_tests.rs"]
mod tests;
