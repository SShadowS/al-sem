//! App attribution — port of al-sem `src/cli/app-attribution.ts`.
//!
//! Maps a `Finding` to its owning `App` (publisher / name / version) and computes
//! cross-app blame (which OTHER apps appear in the evidence path).
//!
//! Model-lookup strategy (mirrors al-sem):
//!   - An internal `ObjectId` is `${appGuid}/${objectType}/${objectNumber}`, so the
//!     appGuid is the first `/`-delimited segment — `app_for_object_id` extracts it
//!     directly, no Map needed.
//!   - `RoutineId → objectId` uses a `routines_by_id` index built from the resolved
//!     workspace routines.
//!
//! SOURCE-ONLY: a gate run resolves exactly ONE workspace app (one root `app.json`).
//! `blame_for_finding` therefore never sees a cross-app evidence path here — but the
//! resolution + the cross-app blame line are ported faithfully so a future multi-app
//! path needs no behavioral change.

use std::collections::HashMap;

use crate::program::model::workspace::ModelRoutine;

/// The owning `App` identity (port of al-sem `model/entities.ts` `App` — the subset the
/// PR-summary attribution line reads). Sourced from the workspace root `app.json`'s
/// `publisher` / `name` / `version` at L3 assembly (see `run::run_analyze`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub app_guid: String,
    pub publisher: String,
    pub name: String,
    pub version: String,
}

/// Per-model index — mirrors al-sem's `indexFor` (`routinesById`).
pub struct AttributionIndex<'a> {
    routines_by_id: HashMap<&'a str, &'a ModelRoutine>,
    apps_by_guid: HashMap<&'a str, &'a App>,
}

impl<'a> AttributionIndex<'a> {
    pub fn build(routines: &'a [ModelRoutine], apps: &'a [App]) -> Self {
        AttributionIndex {
            routines_by_id: routines.iter().map(|r| (r.id.as_str(), r)).collect(),
            apps_by_guid: apps.iter().map(|a| (a.app_guid.as_str(), a)).collect(),
        }
    }

    /// Resolve an internal `ObjectId` (`${appGuid}/${type}/${num}`) to its owning `App`.
    /// The appGuid is the first `/`-segment — extracted directly (no secondary lookup).
    fn app_for_object_id(&self, object_id: &str) -> Option<&'a App> {
        let app_guid = object_id.split('/').next().filter(|s| !s.is_empty())?;
        self.apps_by_guid.get(app_guid).copied()
    }

    /// The `App` owning the finding's primary location (the routine where the hazard
    /// surfaces). Path: `enclosing_routine_id` → `Routine.object_id` → `app_for_object_id`.
    pub fn app_for_routine_id(&self, routine_id: &str) -> Option<&'a App> {
        let routine = self.routines_by_id.get(routine_id)?;
        self.app_for_object_id(&routine.object_id)
    }
}

/// Cross-app blame for a single finding (port of al-sem `Blame`).
pub struct Blame<'a> {
    /// The app the finding is attributed to (where the hazard surfaces).
    pub owner: Option<&'a App>,
    /// Distinct OTHER apps in the evidence path, sorted by appGuid for determinism.
    pub other_apps: Vec<&'a App>,
    /// True when the evidence path spans more than one app.
    pub cross_app: bool,
}

/// `appForFinding` — resolve the owner app for the finding's primary-location routine.
pub fn app_for_finding<'a>(
    primary_routine_id: &str,
    idx: &AttributionIndex<'a>,
) -> Option<&'a App> {
    idx.app_for_routine_id(primary_routine_id)
}

/// `blameForFinding` — compute cross-app blame.
///
/// - `owner` = the app owning the primary location.
/// - Walk the evidence-path routine ids, collecting each step's app.
/// - `other_apps` = distinct apps in the path that are NOT the owner, sorted by appGuid.
/// - `cross_app` = `other_apps` non-empty.
pub fn blame_for_finding<'a>(
    primary_routine_id: &str,
    evidence_routine_ids: &[String],
    idx: &AttributionIndex<'a>,
) -> Blame<'a> {
    let owner = app_for_finding(primary_routine_id, idx);

    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut other_map: HashMap<&str, &App> = HashMap::new();

    for rid in evidence_routine_ids {
        let Some(app) = idx.app_for_routine_id(rid) else {
            continue;
        };
        // Skip the owner app — we only want the OTHER apps.
        if let Some(o) = owner
            && app.app_guid == o.app_guid
        {
            continue;
        }
        if seen.insert(app.app_guid.as_str()) {
            other_map.insert(app.app_guid.as_str(), app);
        }
    }

    let mut other_apps: Vec<&App> = other_map.into_values().collect();
    other_apps.sort_by(|a, b| a.app_guid.cmp(&b.app_guid));

    Blame {
        owner,
        cross_app: !other_apps.is_empty(),
        other_apps,
    }
}
