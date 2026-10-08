//! The DEV-ONLY committed-golden minting tool.
//!
//! Engine-switch S9.0d (owner decision R1): the semantic-edges golden is minted
//! from the AL COMPILER's call graph, not from the retired L3 engine. Input is the
//! graph `altool graph extract-whole` writes over the workspace and its dependency
//! sources (`scripts/compiler-graph` produces it reproducibly). Output, under
//! `tests/goldens/semantic-edges/`:
//!   - `cdo-compiler-anon.json` — every (caller, class, callee) pair of the
//!     compiler graph whose caller is in the workspace app, anonymized
//!     (`program::resolve::compiler_golden`), minified.
//!   - the GITIGNORED local de-anonymization map (`cdo-deanon-map.json`), merged,
//!     so a developer with CDO access can read a failing audit back to AL.
//!
//! # Reproducibility
//!
//! Anonymization uses the FIXED, COMMITTED salt (`anon::ANON_SALT`); the compiler
//! graph is byte-deterministic for the same inputs (S9.0b). Minting twice from the
//! same workspace, dependency closure and AL extension gives byte-identical output.
//! [`ANON_KEY_ENV`] remains an OPTIONAL override for a non-reproducible
//! anonymization; never commit a golden minted with it set.
//!
//! # Pinning
//!
//! PIN `CDO_WS` to the baseline (`U:/Git/DO-cdo-baseline/Cloud`, see CLAUDE.md).
//! The golden stamps the workspace's git HEAD, dirty flag and dependency-closure
//! digest (audits fail on drift under `ENFORCE_CDO_WS=1`) and the AL extension the
//! graph came from (`--compiler`). Re-mint when the pin or the compiler moves.
//!
//! Usage:
//!   `cargo run --release --bin mint-goldens -- --compiler-graph <graph.jsonl>
//!    --compiler <extension> [<workspace-root>]`
//! (workspace defaults to `$CDO_WS`). `--restamp` rewrites only the
//! dependency-closure stamp (a digest-scheme change on an unmoved baseline).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use al_sem::program::resolve::anon::ANON_KEY_ENV;
use al_sem::program::resolve::compiler_golden::{
    CompilerGolden, CompilerStamp, cdo_compiler_golden_path, fixture_compiler_golden_path,
    fixture_workspace_root, load_compiler_golden, mint_compiler_golden,
};
use al_sem::program::resolve::compiler_oracle::CompilerGraph;
use al_sem::program::resolve::semantic_golden::{
    MintMetadata, cdo_deanon_map_path, dependency_closure_digest, merge_deanon_map,
    workspace_git_info,
};

fn usage() -> ExitCode {
    eprintln!(
        "usage: mint-goldens --compiler-graph <graph.jsonl> --compiler <extension> \
         [<workspace-root>]\n\
         \x20      mint-goldens --restamp [<workspace-root>]\n\
         \n\
         Mints tests/goldens/semantic-edges/cdo-compiler-anon.json from the AL\n\
         compiler's call graph (`scripts/compiler-graph <workspace> <out-dir>`\n\
         writes it and prints the extension name), plus the GITIGNORED local\n\
         de-anon map. <workspace-root> defaults to $CDO_WS. Anonymization uses the\n\
         fixed committed salt; {ANON_KEY_ENV} overrides it (never commit that).\n\
         \n\
         --fixture  mint tests/goldens/semantic-edges/fixture-compiler-anon.json\n\
                    instead (workspace: tests/fixtures/semantic-golden), with no\n\
                    workspace stamps.\n\
         --restamp  rewrite ONLY the golden's dependency-closure stamp (a\n\
                    digest-scheme change on an unmoved baseline); refuses if the\n\
                    golden's git stamp differs from the workspace."
    );
    ExitCode::FAILURE
}

/// The value after `flag`, if present.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// The workspace to mint from. `--fixture` always mints from the fixture
/// workspace and never reads `$CDO_WS` (which once made it mint the fixture graph
/// against CDO's app id: 0 pairs); otherwise the positional argument, else
/// `$CDO_WS`.
fn select_workspace(
    fixture: bool,
    positional: Option<&str>,
    cdo_ws: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    match (fixture, positional) {
        (true, None) => Ok(fixture_workspace_root()),
        (true, Some(p)) => Err(format!(
            "--fixture mints from {} and takes no workspace (got {p})",
            fixture_workspace_root().display()
        )),
        (false, Some(p)) => Ok(PathBuf::from(p)),
        (false, None) => match cdo_ws {
            Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
            _ => Err("no workspace given and CDO_WS is unset".to_string()),
        },
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let graph_path = flag_value(&args, "--compiler-graph");
    let compiler = flag_value(&args, "--compiler");
    let values: Vec<&str> = [graph_path, compiler].into_iter().flatten().collect();
    let positional = args
        .iter()
        .find(|a| !a.starts_with("--") && !values.contains(&a.as_str()));

    // `--fixture`: the in-repo fixture golden. Its workspace is this repository,
    // so git and closure stamps mean nothing: none are written, and the fixture
    // test checks no drift.
    let fixture = args.iter().any(|a| a == "--fixture");
    let workspace_root = match select_workspace(
        fixture,
        positional.map(String::as_str),
        std::env::var_os("CDO_WS"),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return usage();
        }
    };
    if !workspace_root.exists() {
        eprintln!(
            "error: workspace root does not exist: {}",
            workspace_root.display()
        );
        return ExitCode::FAILURE;
    }
    if std::env::var(ANON_KEY_ENV).is_ok_and(|v| !v.is_empty()) {
        eprintln!(
            "WARNING: {ANON_KEY_ENV} is set — anonymizing with the OVERRIDE key, NOT \
             the committed fixed salt. Do NOT commit goldens minted this way."
        );
    }

    // Stamp the workspace: git covers tracked files only, so the gitignored
    // `.alpackages` closure is stamped separately (#29). A probe failure aborts.
    let (workspace_git_sha, workspace_dirty) = workspace_git_info(&workspace_root);
    eprintln!("  workspace git: sha={workspace_git_sha:?} dirty={workspace_dirty:?}");
    let closure = match dependency_closure_digest(&workspace_root) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot digest the dependency closure: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("  dependency closure: {closure}");

    if args.iter().any(|a| a == "--restamp") {
        return restamp(workspace_git_sha.as_deref(), workspace_dirty, &closure);
    }
    let (Some(graph_path), Some(compiler)) = (graph_path, compiler) else {
        eprintln!("error: --compiler-graph and --compiler are both required");
        return usage();
    };

    let Some(workspace_guid) = workspace_guid(&workspace_root) else {
        eprintln!("error: cannot read the workspace app id from app.json");
        return ExitCode::FAILURE;
    };
    let graph = match CompilerGraph::read(Path::new(graph_path)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "  compiler graph {graph_path}: edges {:?}, unmapped {}",
        graph.edge_kinds, graph.unmapped_edges
    );
    let metadata = if fixture {
        MintMetadata::default()
    } else {
        MintMetadata {
            workspace_git_sha,
            workspace_dirty,
            dependency_closure_sha256: Some(closure),
        }
    };
    let (golden, deanon) = mint_compiler_golden(
        &graph,
        &workspace_guid,
        metadata,
        CompilerStamp {
            extension: compiler.to_string(),
        },
    );
    let path = if fixture {
        fixture_compiler_golden_path()
    } else {
        cdo_compiler_golden_path()
    };
    write_minified(&path, &golden);
    eprintln!("  {} pair(s) -> {}", golden.pairs.len(), path.display());
    if !fixture {
        merge_deanon_map(&cdo_deanon_map_path(), &deanon);
        eprintln!(
            "  merged {} de-anon entries (GITIGNORED, local-only)",
            deanon.len()
        );
    }
    ExitCode::SUCCESS
}

/// The workspace app's id from `app.json`, lower-case.
fn workspace_guid(workspace_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace_root.join("app.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(v.get("id")?.as_str()?.to_ascii_lowercase())
}

/// `--restamp` (#29): rewrite ONLY the dependency-closure stamp, for a change of
/// digest SCHEME on an unmoved baseline. Refuses unless the golden's git stamp
/// equals the workspace's current one; a moved workspace needs a re-mint.
fn restamp(sha: Option<&str>, dirty: Option<bool>, closure: &str) -> ExitCode {
    let path = cdo_compiler_golden_path();
    let Some(mut golden): Option<CompilerGolden> = load_compiler_golden(&path) else {
        eprintln!("error: {} failed to load", path.display());
        return ExitCode::FAILURE;
    };
    let m = &golden.metadata;
    if m.workspace_git_sha.as_deref() != sha || m.workspace_dirty != dirty {
        eprintln!(
            "error: stamped git {:?}/dirty={:?} != current {sha:?}/dirty={dirty:?}; the \
             workspace moved, so re-mint instead of re-stamping",
            m.workspace_git_sha, m.workspace_dirty
        );
        return ExitCode::FAILURE;
    }
    golden.metadata.dependency_closure_sha256 = Some(closure.to_string());
    write_minified(&path, &golden);
    eprintln!("re-stamped {} with {closure}", path.display());
    ExitCode::SUCCESS
}

fn write_minified<T: serde::Serialize>(path: &Path, value: &T) {
    let json = serde_json::to_string(value).expect("serialize golden to JSON");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create tests/goldens/semantic-edges dir");
    }
    std::fs::write(path, json).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--fixture` ignores a set `$CDO_WS` (stated directly: some other workspace)
    /// and refuses an explicit workspace; without `--fixture` the positional wins,
    /// then `$CDO_WS`.
    #[test]
    fn fixture_mints_from_the_fixture_workspace_whatever_cdo_ws_says() {
        let cdo = || Some(std::ffi::OsString::from("U:/somewhere/else"));
        assert_eq!(
            select_workspace(true, None, cdo()),
            Ok(fixture_workspace_root())
        );
        assert!(select_workspace(true, Some("ws"), cdo()).is_err());
        assert_eq!(
            select_workspace(false, Some("ws"), cdo()),
            Ok(PathBuf::from("ws"))
        );
        assert_eq!(
            select_workspace(false, None, cdo()),
            Ok(PathBuf::from("U:/somewhere/else"))
        );
        assert!(select_workspace(false, None, None).is_err());
    }
}
