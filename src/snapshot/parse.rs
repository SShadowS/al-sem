//! Deep-parse every source-bearing [`AppUnit`] in a snapshot into the owned IR.
//!
//! Symbol-only units (no embedded source) contribute no [`ParsedUnit`]; their
//! ABI is consumed by later resolution phases.

use rayon::prelude::*;
use std::sync::Arc;

use crate::snapshot::identity::{AppId, Provenance};
use crate::snapshot::snapshot::{AppSetSnapshot, AppUnit};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One parsed AL source file within a snapshot unit.
pub struct ParsedFile {
    pub virtual_path: String,
    /// `Arc`-shared (perf safe-wins Task 2): the published `LspSnapshot`'s
    /// `ParsedFileEntry.file` and the updater's working state hold the SAME
    /// parse. Sound because no consumer mutates an `AlFile` after
    /// `al_syntax::parse` — updates always REPLACE whole `ParsedFile`s
    /// (rung-1 `pending` splice / rung-2 `splice_file` / rung-3 wholesale).
    pub file: std::sync::Arc<al_syntax::ir::AlFile>,
    pub provenance: Provenance,
    /// The original AL source text — the SAME `Arc<str>` allocation as the
    /// snapshot's `SourceFile.text` (perf safe-wins Task 1), never a copy.
    pub text: Arc<str>,
}

/// All parsed files for one source-bearing app.
pub struct ParsedUnit {
    pub app: AppId,
    pub files: Vec<ParsedFile>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// File paths (`"<app name>::<virtual_path>"`, sorted) of every parsed source
/// file whose [`al_syntax::ir::ParseStatus`] is `Recovered` — tree-sitter hit
/// error recovery, so that file's IR is PARTIAL (Task 3, preprocessor
/// foundations plan).
///
/// # The invariant this diagnostic exists to serve
///
/// `al_syntax::lower`'s `#if` union-read (see `al_syntax::lower::
/// is_preproc_wrapper`'s doc) is sound for an ABSENCE proof only when the
/// parse itself was `Clean` — a `Recovered` file may have silently DROPPED
/// content tree-sitter could not parse, so "not found in this file's union"
/// is NOT a valid non-existence witness for it. **Any current or future
/// absence/`ProvenAbsent`-shaped claim in this engine MUST consult this
/// diagnostic (or an equivalent per-file `ParseStatus` check) before treating
/// a file's content as complete.** As of Task 3, no such claim exists yet in
/// `src/program` (`ParseStatus::Clean` had ZERO consultation there before this
/// function), so this ships as an ADDITIVE, non-gating diagnostic — a full
/// per-file resolution gate (declining a specific claim once one exists) is
/// deferred until a real consumer needs it.
#[must_use]
pub fn recovered_file_paths(units: &[ParsedUnit]) -> Vec<String> {
    let mut paths: Vec<String> = units
        .iter()
        .flat_map(|unit| {
            let app_name = unit.app.name.clone();
            unit.files
                .iter()
                .filter(|pf| pf.file.parse_status == al_syntax::ir::ParseStatus::Recovered)
                .map(move |pf| format!("{app_name}::{}", pf.virtual_path))
        })
        .collect();
    paths.sort();
    paths
}

/// Parse every source file of every source-bearing app in `snap` in parallel.
///
/// Units whose `source` is `None` (symbol-only boundary apps) are skipped;
/// their ABI is used for resolution in later phases.
///
/// Runs on [`crate::big_stack::big_stack_pool`] — a local rayon pool sized for
/// the `al_syntax` lowerer's recursion (see that module's doc for why the
/// global pool's default stack is insufficient).
#[must_use]
pub fn parse_snapshot(snap: &AppSetSnapshot) -> Vec<ParsedUnit> {
    let pool = crate::big_stack::big_stack_pool();
    pool.install(|| snap.apps.iter().filter_map(parse_unit_in_pool).collect())
}

/// [`parse_snapshot`] for ONE unit: `None` when it has no source.
#[must_use]
pub fn parse_unit(unit: &AppUnit) -> Option<ParsedUnit> {
    crate::big_stack::big_stack_pool().install(|| parse_unit_in_pool(unit))
}

fn parse_unit_in_pool(unit: &AppUnit) -> Option<ParsedUnit> {
    let source = unit.source.as_ref()?;
    #[cfg(test)]
    parse_log::record(unit);
    let files: Vec<ParsedFile> = source
        .files
        .par_iter()
        .map(|f| ParsedFile {
            virtual_path: f.virtual_path.clone(),
            file: Arc::new(al_syntax::parse(&f.text)),
            provenance: unit.provenance.clone(),
            text: Arc::clone(&f.text),
        })
        .collect();
    Some(ParsedUnit {
        app: unit.id.clone(),
        files,
    })
}

/// Test-only record of which `.app` files had their source parsed.
#[cfg(test)]
pub(crate) mod parse_log {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    static LOG: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(super) fn record(unit: &crate::snapshot::snapshot::AppUnit) {
        if let Some(path) = &unit.app_path {
            LOG.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(path.clone());
        }
    }

    /// How many unit parses so far were of an `.app` under `dir` (tests use
    /// their own temp dir, so parallel tests do not see each other).
    pub(crate) fn parses_under(dir: &Path) -> usize {
        LOG.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|p| p.starts_with(dir))
            .count()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_source_units_zero_panics() {
        // Integration test: requires the CDO_WS env var pointing at a real
        // BC workspace with `.alpackages`; skipped (no-op) when unset.
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };
        let snap = crate::snapshot::SnapshotBuilder {
            workspace_root: ws,
            local_providers: vec![],
        }
        .build()
        .unwrap();
        let parsed = parse_snapshot(&snap);
        assert!(!parsed.is_empty());
        let total_files: usize = parsed.iter().map(|u| u.files.len()).sum();
        assert!(
            total_files > 1000,
            "deep parse should cover many files, got {total_files}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 3 (preprocessor foundations plan): `recovered_file_paths`.
    // -----------------------------------------------------------------------

    use crate::snapshot::TrustTier;
    use crate::snapshot::compilation::CompilationContext;
    use crate::snapshot::embedded::SourceFile;
    use crate::snapshot::identity::AppId;
    use crate::snapshot::provider::SourceRoot;
    use crate::snapshot::snapshot::{AppUnit, World};

    fn app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.to_string(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn unit_with_files(id: &AppId, files: Vec<(&str, &str)>) -> AppUnit {
        AppUnit {
            id: id.clone(),
            provenance: Provenance {
                app: id.clone(),
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            },
            source: Some(SourceRoot {
                files: files
                    .into_iter()
                    .map(|(path, text)| SourceFile {
                        virtual_path: path.to_string(),
                        text: text.into(),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            }),
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        }
    }

    const CLEAN_SRC: &str = r#"
codeunit 50000 T
{
    procedure Foo()
    begin
    end;
}
"#;

    /// An unbalanced `#if` (no matching `#endif`) forces tree-sitter error
    /// recovery — `al_syntax::parse`'s `ParseStatus::Recovered`. Mirrors
    /// `al_syntax::lower::tests::unbalanced_if_directive_yields_recovered_
    /// parse_status`, but exercised end to end through `parse_snapshot` +
    /// this diagnostic.
    const RECOVERED_SRC: &str = r#"
codeunit 50001 T
{
    procedure Foo()
    begin
#if NEVER_CLOSED
        Bar();
    end;
}
"#;

    #[test]
    fn recovered_file_paths_fires_only_for_the_broken_file_with_its_path() {
        let ws_id = app_id("Ws");
        let unit = unit_with_files(
            &ws_id,
            vec![("Clean.al", CLEAN_SRC), ("Broken.al", RECOVERED_SRC)],
        );
        let snap = AppSetSnapshot {
            apps: vec![unit],
            workspace_app: ws_id,
            world: World::Closed,
        };
        let parsed = parse_snapshot(&snap);
        let recovered = recovered_file_paths(&parsed);
        assert_eq!(
            recovered,
            vec!["Ws::Broken.al".to_string()],
            "only the file with the unbalanced #if must be reported, with its path"
        );
    }

    #[test]
    fn recovered_file_paths_empty_when_every_file_parses_clean() {
        let ws_id = app_id("Ws");
        let unit = unit_with_files(&ws_id, vec![("Clean.al", CLEAN_SRC)]);
        let snap = AppSetSnapshot {
            apps: vec![unit],
            workspace_app: ws_id,
            world: World::Closed,
        };
        let parsed = parse_snapshot(&snap);
        assert!(
            recovered_file_paths(&parsed).is_empty(),
            "a whole-clean snapshot must report zero recovered files"
        );
    }
}
