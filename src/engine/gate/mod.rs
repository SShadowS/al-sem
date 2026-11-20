//! The production `analyze` GATE path — port of al-sem's `analyze` CLI command
//! (`src/cli/index.ts`) Stage 1 (projection + filters + SARIF; NO baseline / inline
//! suppression — those are Stage 3).
//!
//! Layout mirrors al-sem:
//!   - `presets`      — `src/detectors/presets.ts` + the CLI detector-selection logic.
//!   - `projection`   — `src/projection/finding-summary.ts` (`project_finding`).
//!   - `filter`       — `src/projection/finding-filters.ts` + scope/limit.
//!   - `format_sarif` — `src/cli/format-sarif.ts` (SARIF 2.1.0 + RULES + codeFlows).
//!   - `run`          — the `analyze` pipeline lib entry (`run_analyze`).
//!
//! The `alsem` bin (`src/bin/alsem.rs`) is a thin clap wrapper over `run::run_analyze`.

pub mod app_attribution;
pub mod baseline;
pub mod cache_prune;
pub mod cbor;
pub mod diff;
pub mod events;
pub mod exit_code;
pub mod filter;
pub mod focus;
pub mod format_html;
pub mod format_json;
pub mod format_pr_summary;
pub mod format_sarif;
pub mod format_terminal;
pub mod inline_suppression;
pub mod model_instance_id;
pub mod ordered_json;
pub mod policy;
pub mod preflight;
pub mod presets;
pub mod projection;
pub mod run;
pub mod snapshot_deserialize;
pub mod version;
pub mod workspace_diagnostics;
