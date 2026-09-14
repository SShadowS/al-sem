//! `AppUnit`, `AppSetSnapshot`, and `SnapshotBuilder` — the integration
//! linchpin that turns a workspace root into an identity-verified, per-app
//! source set.

use crate::app_package::ParsedAppPackage;
use crate::dependencies::load_all_apps;
use crate::snapshot::compilation::{
    CompilationContext, context_from_app_json, context_from_metadata,
};
use crate::snapshot::identity::{AppId, Provenance, TrustTier};
use crate::snapshot::provider::{
    EmbeddedAppProvider, LocalRepoProvider, SourceProvider, SourceRoot, SymbolOnlyProvider,
    WorkspaceProvider, select_source,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Whether the app set is a closed (all deps known) or open universe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum World {
    /// All reachable dependencies are represented in `AppSetSnapshot::apps`.
    Closed,
    /// The app set may be incomplete (reserved for reverse-dependents — later task).
    Open,
}

/// One app's resolved identity, source, compilation context, and ABI.
#[derive(Clone, Debug)]
pub struct AppUnit {
    pub id: AppId,
    pub provenance: Provenance,
    /// AL source files, if available (None = symbol-only boundary).
    pub source: Option<SourceRoot>,
    /// Per-app preprocessor symbols + version basis for `#if` evaluation.
    pub compilation: CompilationContext,
    /// This app's declared dependencies (each with its real GUID) — drives
    /// dependency-topology-aware resolution. Workspace deps come from app.json;
    /// dependency-app deps from their `.app` NavxManifest.
    pub declared_deps: Vec<crate::dependencies::AppDependency>,
    /// Friend apps THIS app's manifest grants `internal`-member visibility to
    /// (`<InternalsVisibleTo><Module .../></InternalsVisibleTo>`, Task 1.5).
    /// Populated from a dependency `.app`'s NavxManifest; empty for the
    /// workspace unit (its own `internal`s are never called as a dependency
    /// within this closed-world snapshot, so a friend list on app.json's side
    /// is out of scope).
    pub internals_visible_to: Vec<crate::app_package::FriendApp>,
    /// Parsed `.app` symbol table (None for the workspace itself).
    pub abi: Option<ParsedAppPackage>,
    /// Path to the `.app` file (None for the workspace unit).
    pub app_path: Option<PathBuf>,
}

/// The full set of apps visible in a workspace, keyed by identity.
#[derive(Debug, Clone)]
pub struct AppSetSnapshot {
    /// All app units: index 0 is always the workspace app.
    pub apps: Vec<AppUnit>,
    /// Identity of the workspace app (mirrors `apps[0].id`).
    pub workspace_app: AppId,
    pub world: World,
}

/// Builds an `AppSetSnapshot` from a workspace root + optional local checkouts.
#[derive(Debug)]
pub struct SnapshotBuilder {
    /// Root of the AL workspace (must contain `app.json`).
    pub workspace_root: PathBuf,
    /// Local source checkouts to prefer over embedded source, keyed by `AppId`.
    pub local_providers: Vec<(AppId, PathBuf)>,
}

impl SnapshotBuilder {
    /// Build the snapshot, loading the workspace and all `.alpackages` deps.
    ///
    /// Thin wrapper over [`Self::build_with_diagnostics`] for callers that
    /// don't need the dependency-load diagnostics (Tier-1 remediation, H-2).
    pub fn build(&self) -> Result<AppSetSnapshot> {
        self.build_with_diagnostics().map(|(snap, _dropped)| snap)
    }

    /// Same as [`Self::build`], but also returns every
    /// [`crate::dependencies::DroppedDuplicateDependency`] the underlying
    /// `load_all_apps` GUID-level dedup dropped (H-2) — two or more `.app`
    /// files discovered under `.alpackages` shared a real GUID (a stale
    /// ancestor-folder copy, or the identical file present under two scanned
    /// folders) and only the highest-version survivor became an `AppUnit`.
    pub fn build_with_diagnostics(
        &self,
    ) -> Result<(
        AppSetSnapshot,
        Vec<crate::dependencies::DroppedDuplicateDependency>,
    )> {
        let ws = &self.workspace_root;

        // ------------------------------------------------------------------
        // Workspace unit
        // ------------------------------------------------------------------
        let app_json_path = ws.join("app.json");
        let app_json_text = std::fs::read_to_string(&app_json_path)
            .with_context(|| format!("read {}", app_json_path.display()))?;
        let app_json: serde_json::Value = serde_json::from_str(&app_json_text)
            .with_context(|| format!("parse {}", app_json_path.display()))?;

        let get_str = |k: &str| -> String {
            app_json
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let workspace_app = AppId {
            guid: get_str("id"),
            name: get_str("name"),
            publisher: get_str("publisher"),
            version: get_str("version"),
        };

        let ws_compilation = context_from_app_json(&app_json);
        // Workspace's declared dependencies (with their GUIDs) from app.json.
        let mut ws_declared_deps: Vec<crate::dependencies::AppDependency> = app_json
            .get("dependencies")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();
        // Implicit Microsoft Application-/Platform-tier deps (beyond-1B.3b Task
        // 5.5 — THE dominant lever): real BC apps declare Base App / System App
        // via the top-level `application`/`platform` VERSION fields, never via
        // `dependencies[]`. Without this, Base App is systematically absent from
        // every workspace's closure and every cross-Microsoft-layer call
        // resolves `OutOfClosure` -> `Unknown`. See
        // `crate::dependencies::append_implicit_ms_tier_deps` doc.
        crate::dependencies::append_implicit_ms_tier_deps(
            &mut ws_declared_deps,
            &workspace_app.guid,
            ws_compilation.application.as_deref(),
            ws_compilation.platform.as_deref(),
        );

        let ws_source_provider = WorkspaceProvider { root: ws.clone() };
        let ws_source = ws_source_provider
            .try_provide(&workspace_app)
            .context("workspace source provider")?;

        let ws_tier = ws_source
            .as_ref()
            .map(|s| s.tier)
            .unwrap_or(TrustTier::SymbolOnly);
        let ws_hash = ws_source
            .as_ref()
            .map(|s| s.content_hash.clone())
            .unwrap_or_default();

        let ws_unit = AppUnit {
            provenance: Provenance {
                app: workspace_app.clone(),
                tier: ws_tier,
                content_hash: ws_hash,
            },
            id: workspace_app.clone(),
            source: ws_source,
            compilation: ws_compilation,
            declared_deps: ws_declared_deps,
            internals_visible_to: Vec::new(),
            abi: None,
            app_path: None,
        };

        // ------------------------------------------------------------------
        // Dependency units
        // ------------------------------------------------------------------
        let (resolved_deps, dropped_dep_versions) = load_all_apps(ws)?;

        let mut apps: Vec<AppUnit> = Vec::with_capacity(1 + resolved_deps.len());
        apps.push(ws_unit);

        for rd in resolved_deps {
            // Identity from the `.app`'s own NavxManifest (authoritative) — the
            // GUID is the only globally-unique id. Fall back to the app.json dep
            // entry for any field the manifest left blank.
            let m = &rd.package.metadata;
            let dep_id = AppId {
                guid: m.app_id.clone(),
                name: if m.name.is_empty() {
                    rd.dependency.name.clone()
                } else {
                    m.name.clone()
                },
                publisher: if m.publisher.is_empty() {
                    rd.dependency.publisher.clone()
                } else {
                    m.publisher.clone()
                },
                version: if m.version.is_empty() {
                    rd.dependency.version.clone()
                } else {
                    m.version.clone()
                },
            };

            // Self-dependency guard: the workspace's own compiled .app can sit in an ancestor
            // `.alpackages` (monorepo / CI cache). It interns to the SAME AppRef as the workspace
            // source, polluting the graph with duplicate nodes. Exclude it.
            if (!workspace_app.guid.is_empty() && dep_id.guid == workspace_app.guid)
                || (workspace_app.guid.is_empty()
                    && dep_id.name.eq_ignore_ascii_case(&workspace_app.name)
                    && dep_id.version == workspace_app.version)
            {
                continue;
            }

            // Build provider chain: EmbeddedAppProvider → LocalRepoProvider (if matched) → SymbolOnlyProvider.
            let mut providers: Vec<Box<dyn SourceProvider>> = vec![Box::new(EmbeddedAppProvider {
                app_path: rd.app_path.clone(),
            })];
            // Match a configured local provider by GUID when known (the unique
            // identity), else by name (case-insensitive) + version.
            if let Some((id, path)) = self.local_providers.iter().find(|(id, _)| {
                (!dep_id.guid.is_empty() && id.guid == dep_id.guid)
                    || (id.name.eq_ignore_ascii_case(&dep_id.name) && id.version == dep_id.version)
            }) {
                providers.push(Box::new(LocalRepoProvider {
                    app: id.clone(),
                    root: path.clone(),
                }));
            }
            providers.push(Box::new(SymbolOnlyProvider));

            let source = select_source(&dep_id, &providers)?;
            let tier = source
                .as_ref()
                .map(|s| s.tier)
                .unwrap_or(TrustTier::SymbolOnly);
            let content_hash = source
                .as_ref()
                .map(|s| s.content_hash.clone())
                .unwrap_or_default();

            // Real compilation basis + declared deps from the dep's manifest
            // (computed before `rd.package` is moved into `abi`).
            let dep_compilation = context_from_metadata(&rd.package.metadata);
            let mut dep_declared = rd.package.metadata.dependencies.clone();
            // Friend apps THIS dep's own manifest declares (Task 1.5) — e.g.
            // CTS-CDN's manifest lists CDO as a friend, granting CDO
            // visibility into CTS-CDN's `internal` members.
            let dep_friends = rd.package.metadata.internals_visible_to.clone();
            // Implicit Microsoft Application-/Platform-tier deps for THIS dep
            // app too (beyond-1B.3b Task 5.5) — a dependency app (e.g. a
            // Foundation-tier app) can itself implicitly depend on Base
            // App/System App via its own manifest `Application`/`Platform`
            // attributes, same as the workspace.
            crate::dependencies::append_implicit_ms_tier_deps(
                &mut dep_declared,
                &dep_id.guid,
                dep_compilation.application.as_deref(),
                dep_compilation.platform.as_deref(),
            );
            apps.push(AppUnit {
                provenance: Provenance {
                    app: dep_id.clone(),
                    tier,
                    content_hash,
                },
                id: dep_id,
                source,
                compilation: dep_compilation,
                declared_deps: dep_declared,
                internals_visible_to: dep_friends,
                abi: Some(rd.package),
                app_path: Some(rd.app_path.clone()),
            });
        }

        Ok((
            AppSetSnapshot {
                workspace_app,
                apps,
                world: World::Closed,
            },
            dropped_dep_versions,
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // H-2 (Tier-1 remediation, Task T1.2): end-to-end `.app` fixtures for
    // `SnapshotBuilder::build_with_diagnostics`'s GUID-level dedup.
    // -----------------------------------------------------------------------

    /// Write a minimal, REAL `.app` file (40-byte NAVX header + a zip
    /// containing `NavxManifest.xml` + `SymbolReference.json`) — exercises
    /// the actual `open_app_zip`/`extract_app_package`/`load_all_apps`
    /// pipeline, not a hand-built in-memory shortcut.
    fn write_minimal_app(
        dir: &std::path::Path,
        filename: &str,
        guid: &str,
        name: &str,
        publisher: &str,
        version: &str,
    ) -> std::path::PathBuf {
        use std::io::Write;

        let manifest = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{guid}" Name="{name}" Publisher="{publisher}" Version="{version}" Runtime="13.0" /></Package>"#
        );
        // One Codeunit with one Method — enough for `abi_overload_collapsed`
        // to have something to collapse (or not) when this app's ABI is
        // ingested twice vs once.
        let symbol_reference =
            r#"{"Codeunits":[{"Id":50100,"Name":"DupCU","Methods":[{"Name":"DoIt","Id":1}]}]}"#;

        // Build the zip in memory at offset 0 first (guaranteed-correct),
        // then prepend the NAVX header when writing to disk — avoids any
        // dependency on the `zip` crate's handling of a pre-offset writer.
        let mut zip_bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut zip_bytes);
            let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            zip.start_file("NavxManifest.xml", options).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
            zip.start_file("SymbolReference.json", options).unwrap();
            zip.write_all(symbol_reference.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let path = dir.join(filename);
        let mut out = std::fs::File::create(&path).unwrap();
        out.write_all(&[0u8; 40]).unwrap(); // NAVX header (content unused)
        out.write_all(zip_bytes.get_ref()).unwrap();
        path
    }

    fn write_app_json(dir: &std::path::Path) {
        std::fs::write(
            dir.join("app.json"),
            r#"{
    "id": "99999999-0000-0000-0000-000000000099",
    "name": "H2Probe",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
        )
        .unwrap();
    }

    /// "Stale wins" reproduction: two `.app` files sharing one GUID at
    /// different versions (24.0.0.0, 25.0.0.0) sit in `.alpackages`. The
    /// higher version must win, and the drop must be named in the returned
    /// diagnostics — proving the fix end-to-end through the REAL
    /// `load_all_apps` → `SnapshotBuilder` pipeline, not just the pure dedup
    /// function.
    #[test]
    fn build_with_diagnostics_keeps_highest_version_dep_and_reports_the_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_app_json(dir.path());
        let alpackages = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpackages).unwrap();
        let guid = "aaaaaaaa-1111-1111-1111-111111111111";
        write_minimal_app(
            &alpackages,
            "Pub_DupApp_24.0.0.0.app",
            guid,
            "DupApp",
            "Pub",
            "24.0.0.0",
        );
        write_minimal_app(
            &alpackages,
            "Pub_DupApp_25.0.0.0.app",
            guid,
            "DupApp",
            "Pub",
            "25.0.0.0",
        );

        let (snap, dropped) = (SnapshotBuilder {
            workspace_root: dir.path().to_path_buf(),
            local_providers: vec![],
        })
        .build_with_diagnostics()
        .expect("snapshot build");

        let dup_units: Vec<_> = snap.apps.iter().filter(|u| u.id.guid == guid).collect();
        assert_eq!(
            dup_units.len(),
            1,
            "exactly one AppUnit must survive for the duplicated GUID; got {:?}",
            dup_units.iter().map(|u| &u.id.version).collect::<Vec<_>>()
        );
        assert_eq!(dup_units[0].id.version, "25.0.0.0");

        assert_eq!(
            dropped.len(),
            1,
            "the dropped stale version must be reported"
        );
        assert_eq!(dropped[0].guid, guid);
        assert_eq!(dropped[0].kept_version, "25.0.0.0");
        assert_eq!(dropped[0].dropped_version, "24.0.0.0");
    }

    /// Byte-identical duplicate: the SAME version physically present twice —
    /// once in the project's OWN `.alpackages`, once in an ANCESTOR's
    /// `.alpackages` (`find_all_alpackages_folders` walks up the directory
    /// tree on purpose, e.g. for a monorepo's shared package cache; this is
    /// the real-world shape the H-2 brief describes, not two files sitting
    /// side by side in the same scanned folder). Must dedup to exactly one
    /// `AppUnit` BEFORE `build_program_graph` ever ingests it — proving the
    /// fix prevents the `abi_overload_collapsed` poisoning at its source,
    /// not just after the fact.
    #[test]
    fn build_with_diagnostics_dedups_byte_identical_duplicate_no_collapse() {
        let base = tempfile::tempdir().expect("tempdir");
        let project = base.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        write_app_json(&project);
        let own_alpackages = project.join(".alpackages");
        std::fs::create_dir_all(&own_alpackages).unwrap();
        let ancestor_alpackages = base.path().join(".alpackages");
        std::fs::create_dir_all(&ancestor_alpackages).unwrap();

        let guid = "bbbbbbbb-2222-2222-2222-222222222222";
        // Same GUID, same version, two different physical files/locations —
        // `load_all_apps`'s canonical-path dedup only catches the SAME path,
        // not two distinct copies.
        write_minimal_app(
            &own_alpackages,
            "Pub_SameVerApp_1.0.0.0.app",
            guid,
            "SameVerApp",
            "Pub",
            "1.0.0.0",
        );
        write_minimal_app(
            &ancestor_alpackages,
            "Pub_SameVerApp_1.0.0.0_copy.app",
            guid,
            "SameVerApp",
            "Pub",
            "1.0.0.0",
        );

        let (snap, dropped) = (SnapshotBuilder {
            workspace_root: project,
            local_providers: vec![],
        })
        .build_with_diagnostics()
        .expect("snapshot build");

        let dup_units: Vec<_> = snap.apps.iter().filter(|u| u.id.guid == guid).collect();
        assert_eq!(
            dup_units.len(),
            1,
            "a byte-identical duplicate must collapse to exactly one AppUnit"
        );
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].kept_version, dropped[0].dropped_version);

        // Graph-level proof: with only one AppUnit for this GUID,
        // `build_program_graph`'s Step 2b ingests its ABI exactly once, so
        // `DoIt` must NOT be marked `abi_overload_collapsed` (pre-fix, TWO
        // AppUnits sharing this identity would intern to the SAME AppRef,
        // ingest twice, and collapse-poison every routine in the app).
        let cache = crate::program::abi_ingest::AbiCache::new();
        let graph = crate::program::build::build_program_graph(&snap, &cache);
        let do_it = graph
            .routines
            .iter()
            .find(|r| r.id.name_lc == "doit")
            .expect("DoIt must be ingested");
        assert!(
            !do_it.abi_overload_collapsed,
            "DoIt must NOT be abi_overload_collapsed — the duplicate was \
             dedupped before it ever reached ingestion"
        );
    }

    /// CDO pin (Tier-1 remediation, Task T1.2, H-2 re-measure protocol):
    /// names the real duplicate-GUID dependencies the fix found and dropped
    /// on the frozen CDO workspace, so the fix's real-world effect (not just
    /// the synthetic fixtures above) is a durable regression guard.
    ///
    /// Investigative run (2026-07-10): CDO's workspace root
    /// (`DO.Support-SlowDOSetup/DocumentOutput/Cloud`) has its OWN
    /// `.alpackages`, and its ANCESTOR (`DO.Support-SlowDOSetup/
    /// DocumentOutput`) has a SECOND `.alpackages` that `find_all_alpackages_
    /// folders` also scans (by design — a monorepo's shared package cache) —
    /// 10 of CDO's 12 real dependency apps are cached in BOTH,
    /// byte-identical. TWO (`Continia Document Output`, `Continia Connector
    /// App`) additionally have a genuinely STALE extra copy in the ancestor
    /// folder — the literal "stale ancestor-folder copy" scenario the H-2 fix
    /// exists for, confirmed on real data, not just a constructed fixture.
    #[test]
    fn cdo_dedup_names_the_real_dropped_duplicates() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };
        let (_snap, dropped) = (SnapshotBuilder {
            workspace_root: ws,
            local_providers: vec![],
        })
        .build_with_diagnostics()
        .expect("snapshot build");

        // RE-DERIVED 2026-09-14 for the pinned `bc3ccb18` baseline: 12 -> 0.
        //
        // The 12 were never a property of CDO's dependencies. They were a property
        // of the OLD workspace's LAYOUT: that tree had its own `.alpackages` AND an
        // ancestor one (`DocumentOutput/Cloud` plus `DocumentOutput`), and
        // `find_all_alpackages_folders` scans both, so 10 apps appeared twice
        // byte-identically and 2 more carried a genuinely stale ancestor copy.
        //
        // `U:/Git/DO-cdo-baseline/Cloud` has ONE `.alpackages` and an ancestor with
        // none, so that duplicate population cannot form. 0 is not a loosened pin --
        // it is the honest count for a workspace shaped this way, and the
        // byte-identical / stale-name breakdowns that used to follow have no
        // population left to describe (see git history, pre-2026-09-14).
        //
        // Found late, by `ci-steps all` on an issue branch rather than by the
        // baseline arc that caused it: this is a `--lib` test, and NEITHER
        // `scripts/check-goldens` NOR `scripts/cdo-gate` runs `--lib`. That blind
        // spot hid a real failure twice in one session -- the other was
        // `cache_version_grammar_tracks_the_linked_grammar` during the v4.4.0
        // grammar bump. Run `cargo test -p al-sem --lib` after any baseline or
        // grammar move; the golden gate will not tell you.
        //
        // If a future baseline reintroduces an ancestor package cache this becomes
        // nonzero again -- RE-DERIVE it against that workspace, do not restore 12.
        assert!(
            dropped.is_empty(),
            "the pinned bc3ccb18 baseline has ONE .alpackages and no ancestor \
             cache, so no duplicate-GUID drop is possible; got {dropped:#?}. A \
             nonzero count means the workspace gained an ancestor package cache: \
             re-derive this pin against it, and restore the byte-identical and \
             stale-name breakdowns from git history."
        );
    }

    #[test]
    fn builds_snapshot_over_cdo_workspace() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };
        let snap = SnapshotBuilder {
            workspace_root: ws,
            local_providers: vec![],
        }
        .build()
        .expect("snapshot");
        // workspace + >=9 dep apps
        assert!(snap.apps.len() >= 10, "got {}", snap.apps.len());
        // 9/10 deps ship ShowMyCode source → at least 9 units have source
        let with_src = snap.apps.iter().filter(|u| u.source.is_some()).count();
        assert!(with_src >= 9, "expected >=9 source units, got {with_src}");
        // At least one dep has no embedded source (symbol-only)
        let sym_only = snap.apps.iter().filter(|u| u.source.is_none()).count();
        assert!(
            sym_only >= 1,
            "expected >=1 symbol-only units, got {sym_only}"
        );
        // Dependency apps carry their REAL unique GUID (from the .app NavxManifest
        // `App@Id`), not an empty string — the identity foundation 1B builds on.
        let deps_with_guid = snap
            .apps
            .iter()
            .skip(1) // apps[0] = workspace
            .filter(|u| u.id.guid.len() == 36 && u.id.guid.contains('-'))
            .count();
        assert!(
            deps_with_guid >= 9,
            "expected >=9 deps with a real GUID, got {deps_with_guid}"
        );
        // #1: dependency apps carry a REAL compilation basis (runtime/platform
        // from their manifest), not an empty default context.
        let deps_with_runtime = snap
            .apps
            .iter()
            .skip(1)
            .filter(|u| u.compilation.runtime.is_some() || u.compilation.application.is_some())
            .count();
        assert!(
            deps_with_runtime >= 5,
            "expected deps to carry a real compilation basis, got {deps_with_runtime}"
        );
        // #2: dependency topology is captured — at least one dep declares its own
        // dependencies (each with a GUID), enabling topology-aware resolution.
        let some_dep_declares_deps = snap.apps.iter().skip(1).any(|u| {
            !u.declared_deps.is_empty() && u.declared_deps.iter().all(|d| d.app_id.len() == 36)
        });
        assert!(
            some_dep_declares_deps,
            "expected at least one dep to declare its own GUID-bearing dependencies"
        );
        // Dependency apps are in a deterministic, filesystem-independent order
        // (sorted by AppId) so AppRef/NodeId numbering is reproducible (charter C8).
        let dep_ids: Vec<_> = snap.apps[1..]
            .iter()
            .map(|u| (&u.id.guid, &u.id.name, &u.id.publisher, &u.id.version))
            .collect();
        let mut sorted = dep_ids.clone();
        sorted.sort();
        assert_eq!(dep_ids, sorted, "dependency apps must be sorted by AppId");
    }

    #[test]
    fn workspace_is_not_its_own_dependency() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };
        let snap = SnapshotBuilder {
            workspace_root: ws,
            local_providers: vec![],
        }
        .build()
        .expect("snapshot");
        // The workspace app identity must appear exactly once across all units.
        let ws_id = &snap.workspace_app;
        let same = snap
            .apps
            .iter()
            .filter(|u| u.id.guid == ws_id.guid && !ws_id.guid.is_empty())
            .count();
        assert_eq!(
            same, 1,
            "workspace .app cached in .alpackages must not be added as a self-dependency"
        );
    }

    // -----------------------------------------------------------------------
    // beyond-1B.3b Task 5.5 (Step 1a): implicit Base App/System App injection
    // into the workspace `AppUnit.declared_deps`. No CDO_WS needed — a bare
    // temp workspace with only app.json (no .al source, no .alpackages) is
    // enough, since `WorkspaceProvider::try_provide` tolerates zero source
    // files (`Ok(None)`, not an error).
    // -----------------------------------------------------------------------

    fn write_minimal_app_json(dir: &std::path::Path, extra_fields: &str) {
        let app_json = format!(
            r#"{{
    "id": "11111111-0000-0000-0000-000000000001",
    "name": "Task5.5 Probe",
    "publisher": "probe",
    "version": "1.0.0.0"{extra}
}}"#,
            extra = if extra_fields.is_empty() {
                String::new()
            } else {
                format!(",\n{extra_fields}")
            }
        );
        std::fs::write(dir.join("app.json"), app_json).expect("write app.json");
    }

    #[test]
    fn appunit_gets_ms_application_tier_when_application_field_non_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_app_json(dir.path(), r#""application": "24.0.0.0""#);
        let snap = SnapshotBuilder {
            workspace_root: dir.path().to_path_buf(),
            local_providers: vec![],
        }
        .build()
        .expect("snapshot build");
        let ws_unit = &snap.apps[0];
        assert_eq!(
            ws_unit.declared_deps.len(),
            3,
            "MS_APPLICATION_TIER has 3 entries; got {:?}",
            ws_unit.declared_deps
        );
        assert!(
            ws_unit
                .declared_deps
                .iter()
                .any(|d| d.app_id == "437dbf0e-84ff-417a-965d-ed2bb9650972"
                    && d.name == "Base Application"
                    && d.version == "24.0.0.0"),
            "Base App must be injected; got {:?}",
            ws_unit.declared_deps
        );
    }

    #[test]
    fn appunit_gets_ms_platform_tier_when_platform_field_non_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_app_json(dir.path(), r#""platform": "24.0.0.0""#);
        let snap = SnapshotBuilder {
            workspace_root: dir.path().to_path_buf(),
            local_providers: vec![],
        }
        .build()
        .expect("snapshot build");
        let ws_unit = &snap.apps[0];
        assert_eq!(
            ws_unit.declared_deps.len(),
            2,
            "MS_PLATFORM_TIER has 2 entries; got {:?}",
            ws_unit.declared_deps
        );
        assert!(
            ws_unit
                .declared_deps
                .iter()
                .any(|d| d.app_id == "63ca2fa4-4f03-4f2b-a480-172fef340d3f"
                    && d.name == "System Application"
                    && d.version == "24.0.0.0"),
            "System App must be injected; got {:?}",
            ws_unit.declared_deps
        );
    }

    #[test]
    fn appunit_gets_no_implicit_deps_when_application_and_platform_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_app_json(dir.path(), "");
        let snap = SnapshotBuilder {
            workspace_root: dir.path().to_path_buf(),
            local_providers: vec![],
        }
        .build()
        .expect("snapshot build");
        let ws_unit = &snap.apps[0];
        assert!(
            ws_unit.declared_deps.is_empty(),
            "no application/platform field must inject NOTHING (low ripple on \
             minimal fixtures); got {:?}",
            ws_unit.declared_deps
        );
    }
}
