//! 1B.3b Task 1: the DEV-ONLY committed-golden minting tool.
//!
//! 1B.3b Task 3 update: `src/program/resolve` (the gate module —
//! `differential.rs` + `semantic_golden.rs`) now has ZERO `engine::l3`
//! imports. The LAST sanctioned L3 oracle access point in the library is
//! [`al_sem::program::l3_mint`] (OUTSIDE `src/program/resolve`),
//! reached from here either directly ([`project_l3_event_rows`]) or via
//! [`mint_l3_validated_golden`]/[`mint_l3_trigger_golden`] (thin wrappers in
//! `semantic_golden.rs` that delegate to `l3_mint::project_l3` /
//! `l3_mint::project_l3_implicit_trigger_in_scope`). This binary is the ONLY
//! caller of those (plus the in-repo `REGEN_TEMP_GOLDENS` fixture-regen test
//! path) — the runtime audits
//! (`run_cdo_semantic_audit`/`run_cdo_trigger_audit`/`run_cdo_event_audit`)
//! LOAD the committed output instead.
//!
//! Mints + ANONYMIZES (via [`anon::anon`] — see that module's docs for the
//! domain-separation + fixed-salt-governance writeup) the three committed
//! goldens under `tests/goldens/semantic-edges/`:
//!   - `cdo-anon.json`         — Member/Interface ([`mint_l3_validated_golden`])
//!   - `cdo-trigger-anon.json` — ImplicitTrigger ([`mint_l3_trigger_golden`])
//!   - `cdo-event-anon.json`   — EventFlow (`project_l3_event_rows`)
//!
//! All three are written MINIFIED (single-line JSON; the ~13k-site CDO golden
//! is a large committed artifact and pretty-printing roughly doubles it for
//! no review benefit — a diff tool handles structural JSON diffs either way).
//!
//! ALSO writes/merges the GITIGNORED local de-anonymization map
//! (`cdo-deanon-map.json`, `AnonId -> human-readable plaintext`) so a
//! developer with CDO access can reverse a failing anonymized diff back to
//! the exact broken AL code (the committed goldens themselves never carry
//! plaintext).
//!
//! # Reproducibility (1B.3b Task 1 fix)
//!
//! Anonymization uses the FIXED, COMMITTED salt (`anon::ANON_SALT`) by
//! default — running this tool twice against the SAME `CDO_WS` state produces
//! BYTE-IDENTICAL committed goldens, with no secret required. [`ANON_KEY_ENV`]
//! remains as an OPTIONAL override for a non-reproducible, session-local
//! anonymization; a golden minted with it set must NEVER be committed (this
//! tool warns loudly when it's set).
//!
//! # Workspace pinning
//!
//! PIN `CDO_WS` to a clean (or at least a known, tagged) ref at mint time —
//! this tool stamps the workspace's git HEAD SHA + dirty flag into each
//! golden's [`MintMetadata`], and a later audit run warns (not fails) when
//! the workspace it sees has drifted from that stamp. Re-run this tool
//! (re-mint) when intentionally advancing the pin to a new workspace state.
//!
//! Usage: `CDO_WS=<workspace> cargo run --release --bin mint-goldens`
//! (workspace defaults to `$CDO_WS`; an explicit positional arg overrides
//! it). Output discipline: progress/summary goes to stderr; the tool writes
//! files directly, nothing meaningful goes to stdout.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use al_sem::program::l3_mint::project_l3_event_rows;
use al_sem::program::resolve::anon::{self, ANON_KEY_ENV};
use al_sem::program::resolve::semantic_golden::{
    MintMetadata, anonymize_event_rows_with_deanon, anonymize_golden_with_deanon,
    cdo_anon_golden_path, cdo_deanon_map_path, cdo_event_anon_golden_path,
    cdo_trigger_anon_golden_path, dependency_closure_digest, merge_deanon_map,
    mint_l3_trigger_golden, mint_l3_validated_golden, workspace_git_info,
};

fn usage() -> ExitCode {
    eprintln!(
        "usage: CDO_WS=<workspace> cargo run --release --bin mint-goldens \
         [-- <workspace-root>]\n\
         \n\
         Mints + anonymizes the three committed CDO-derived goldens under\n\
         tests/goldens/semantic-edges/ (cdo-anon.json, cdo-trigger-anon.json,\n\
         cdo-event-anon.json) plus the GITIGNORED local de-anon map\n\
         (cdo-deanon-map.json). The LAST sanctioned L3 oracle use (1B.3b Task 1).\n\
         \n\
         <workspace-root> defaults to $CDO_WS when omitted as a positional arg.\n\
         \n\
         Anonymization uses the FIXED, COMMITTED salt by default — re-running\n\
         this tool against the SAME CDO_WS state reproduces byte-identical\n\
         committed goldens, no secret required (see anon.rs's module docs,\n\
         \"Governance\" section). {ANON_KEY_ENV} is an OPTIONAL override for a\n\
         non-reproducible, session-local anonymization; NEVER commit a golden\n\
         minted with it set.\n\
         \n\
         PIN CDO_WS to a clean/tagged ref at mint time — the mint-time git SHA\n\
         + dirty flag are stamped into each golden's metadata, so a later audit\n\
         can warn on workspace drift. Re-mint when intentionally advancing the\n\
         pin."
    );
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positional = args.iter().find(|a| !a.starts_with("--"));

    let workspace_root: PathBuf = match positional {
        Some(p) => PathBuf::from(p),
        None => match std::env::var_os("CDO_WS") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => {
                eprintln!("error: no workspace given and CDO_WS is unset");
                return usage();
            }
        },
    };
    if !workspace_root.exists() {
        eprintln!(
            "error: workspace root does not exist: {}",
            workspace_root.display()
        );
        return ExitCode::FAILURE;
    }

    // 1B.3b Task 1 fix: anonymization defaults to the FIXED, COMMITTED salt
    // (`anon::ANON_SALT`) — no secret required, and the result is
    // REPRODUCIBLE (re-running this tool against the same CDO_WS state
    // byte-matches the committed goldens). `ANON_KEY_ENV` remains an OPTIONAL
    // override for a non-reproducible anonymization; warn loudly when it's
    // set so a developer doesn't accidentally commit a non-reproducible
    // golden.
    let key_overridden = std::env::var(ANON_KEY_ENV)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if key_overridden {
        eprintln!(
            "WARNING: {ANON_KEY_ENV} is set — anonymizing with the OVERRIDE key, \
             NOT the committed fixed salt. This run's output will NOT match the \
             currently-committed goldens and will NOT reproduce on a second run \
             without the same override. Do NOT commit goldens minted this way."
        );
    }

    // 1B.3b Task 1 fix (Fix 4): stamp the mint-time CDO_WS git SHA + dirty
    // flag into every golden's metadata. PIN CDO_WS to a clean/tagged ref
    // before minting; re-mint when intentionally advancing the pin.
    let (workspace_git_sha, workspace_dirty) = workspace_git_info(&workspace_root);
    eprintln!("  workspace git: sha={workspace_git_sha:?} dirty={workspace_dirty:?}");
    // git state covers only TRACKED files; `.alpackages` is gitignored, so the
    // dependency symbols the resolver actually reads are invisible to `dirty`.
    // Stamp them separately or the baseline is only half pinned.
    let dependency_closure_sha256 = dependency_closure_digest(&workspace_root);
    match &dependency_closure_sha256 {
        Some(d) => eprintln!("  .alpackages closure: {d}"),
        None => eprintln!(
            "  .alpackages closure: NONE (no dependency symbols found) — cross-app              resolution will see nothing, and drift in this closure cannot be detected"
        ),
    }
    let mint_metadata = MintMetadata {
        workspace_git_sha,
        workspace_dirty,
        dependency_closure_sha256,
    };

    eprintln!(
        "mint-goldens: workspace={} (1B.3b Task 1 — LAST sanctioned L3 use)",
        workspace_root.display()
    );

    let mut deanon: BTreeMap<String, String> = BTreeMap::new();

    // ── (a) Member/Interface ─────────────────────────────────────────────────
    eprintln!("  minting Member/Interface golden (mint_l3_validated_golden / project_l3)...");
    let member_golden = mint_l3_validated_golden(&workspace_root);
    let mut member_anon =
        anonymize_golden_with_deanon(&member_golden, anon::SITE_DOMAIN_V1, &mut deanon);
    member_anon.metadata = mint_metadata.clone();
    let member_path = cdo_anon_golden_path();
    write_minified(&member_path, &member_anon);
    eprintln!(
        "    {} site(s) -> {}",
        member_anon.entries.len(),
        member_path.display()
    );

    // ── (b) ImplicitTrigger ───────────────────────────────────────────────────
    eprintln!(
        "  minting ImplicitTrigger golden (mint_l3_trigger_golden / project_l3_implicit_trigger_in_scope)..."
    );
    let trigger_golden = mint_l3_trigger_golden(&workspace_root);
    let mut trigger_anon =
        anonymize_golden_with_deanon(&trigger_golden, anon::TRIGGER_OP_DOMAIN_V1, &mut deanon);
    trigger_anon.metadata = mint_metadata.clone();
    let trigger_path = cdo_trigger_anon_golden_path();
    write_minified(&trigger_path, &trigger_anon);
    eprintln!(
        "    {} site(s) -> {}",
        trigger_anon.entries.len(),
        trigger_path.display()
    );

    // ── (c) EventFlow ─────────────────────────────────────────────────────────
    eprintln!("  minting EventFlow golden (project_l3_event_rows)...");
    let event_rows = project_l3_event_rows(&workspace_root);
    let mut event_anon = anonymize_event_rows_with_deanon(&event_rows, &mut deanon);
    event_anon.metadata = mint_metadata.clone();
    let event_path = cdo_event_anon_golden_path();
    write_minified(&event_path, &event_anon);
    eprintln!(
        "    {} pair(s) -> {}",
        event_anon.entries.len(),
        event_path.display()
    );

    // ── de-anon map (gitignored, local-only) ─────────────────────────────────
    let deanon_path = cdo_deanon_map_path();
    let deanon_count = deanon.len();
    merge_deanon_map(&deanon_path, &deanon);
    eprintln!(
        "  merged {deanon_count} de-anon entries -> {} (GITIGNORED, local-only)",
        deanon_path.display()
    );

    eprintln!("mint-goldens: done.");
    ExitCode::SUCCESS
}

fn write_minified<T: serde::Serialize>(path: &Path, value: &T) {
    let json = serde_json::to_string(value).expect("serialize golden to JSON");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create tests/goldens/semantic-edges dir");
    }
    std::fs::write(path, json).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
