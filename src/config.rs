//! Configuration for diagnostic thresholds.
//!
//! Config resolution order (later wins per field):
//! 1. Built-in defaults
//! 2. Global config at `~/.al-sem/config.json`
//! 3. Workspace config at `{workspace}/.al-sem.json`
//!
//! Both locations fall back to their pre-rename names (`~/.al-call-hierarchy/config.json`
//! and `{workspace}/.al-call-hierarchy.json`) when the current file is absent, so an
//! install that predates the rename keeps its thresholds — see [`crate::state_paths`].

use log::{info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// All configurable diagnostic thresholds (fully resolved, no Options)
#[derive(Debug, Clone)]
pub struct DiagnosticConfig {
    pub complexity_enabled: bool,
    pub complexity_warning: u32,
    pub complexity_critical: u32,
    pub length_enabled: bool,
    pub length_warning: u32,
    pub length_critical: u32,
    pub params_enabled: bool,
    pub params_warning: u32,
    pub params_critical: u32,
    pub fan_in_enabled: bool,
    pub fan_in_warning: usize,
    pub unused_procedures: bool,
}

impl Default for DiagnosticConfig {
    fn default() -> Self {
        Self {
            complexity_enabled: true,
            complexity_warning: 5,
            complexity_critical: 10,
            length_enabled: true,
            length_warning: 20,
            length_critical: 50,
            params_enabled: true,
            params_warning: 4,
            params_critical: 7,
            fan_in_enabled: true,
            fan_in_warning: 20,
            unused_procedures: true,
        }
    }
}

/// JSON schema for config files (both global and workspace)
#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    diagnostics: DiagnosticsSection,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsSection {
    complexity: Option<ThresholdPair>,
    parameters: Option<ThresholdPair>,
    line_count: Option<ThresholdPair>,
    fan_in: Option<ThresholdSingle>,
    unused_procedures: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ThresholdPair {
    enabled: Option<bool>,
    warning: Option<u32>,
    critical: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ThresholdSingle {
    enabled: Option<bool>,
    warning: Option<u32>,
}

/// Returns the global config path to READ: `~/.al-sem/config.json`, or the pre-rename
/// `~/.al-call-hierarchy/config.json` when only that one exists.
fn global_config_path() -> Option<PathBuf> {
    crate::state_paths::state_path_for_read("config.json")
}

/// Parse a config file, returning None if missing or invalid.
fn load_file(path: &Path) -> Option<ConfigFile> {
    if !path.exists() {
        return None;
    }

    info!("Loading config from {}", path.display());

    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read {}: {}", path.display(), e);
            return None;
        }
    };

    match serde_json::from_str(&contents) {
        Ok(f) => Some(f),
        Err(e) => {
            warn!("Failed to parse {}: {}", path.display(), e);
            None
        }
    }
}

/// Merge two DiagnosticsSections. `overlay` values take priority over `base`.
fn merge_sections(base: DiagnosticsSection, overlay: DiagnosticsSection) -> DiagnosticsSection {
    DiagnosticsSection {
        complexity: merge_threshold_pair(base.complexity, overlay.complexity),
        parameters: merge_threshold_pair(base.parameters, overlay.parameters),
        line_count: merge_threshold_pair(base.line_count, overlay.line_count),
        fan_in: merge_threshold_single(base.fan_in, overlay.fan_in),
        unused_procedures: overlay.unused_procedures.or(base.unused_procedures),
    }
}

fn merge_threshold_pair(
    base: Option<ThresholdPair>,
    overlay: Option<ThresholdPair>,
) -> Option<ThresholdPair> {
    match (base, overlay) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (Some(b), Some(o)) => Some(ThresholdPair {
            enabled: o.enabled.or(b.enabled),
            warning: o.warning.or(b.warning),
            critical: o.critical.or(b.critical),
        }),
    }
}

fn merge_threshold_single(
    base: Option<ThresholdSingle>,
    overlay: Option<ThresholdSingle>,
) -> Option<ThresholdSingle> {
    match (base, overlay) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (Some(b), Some(o)) => Some(ThresholdSingle {
            enabled: o.enabled.or(b.enabled),
            warning: o.warning.or(b.warning),
        }),
    }
}

/// Apply defaults to a merged DiagnosticsSection, producing the final config.
fn apply_defaults(section: DiagnosticsSection) -> DiagnosticConfig {
    let defaults = DiagnosticConfig::default();

    DiagnosticConfig {
        complexity_enabled: section
            .complexity
            .as_ref()
            .and_then(|c| c.enabled)
            .unwrap_or(defaults.complexity_enabled),
        complexity_warning: section
            .complexity
            .as_ref()
            .and_then(|c| c.warning)
            .unwrap_or(defaults.complexity_warning),
        complexity_critical: section
            .complexity
            .as_ref()
            .and_then(|c| c.critical)
            .unwrap_or(defaults.complexity_critical),
        length_enabled: section
            .line_count
            .as_ref()
            .and_then(|c| c.enabled)
            .unwrap_or(defaults.length_enabled),
        length_warning: section
            .line_count
            .as_ref()
            .and_then(|c| c.warning)
            .unwrap_or(defaults.length_warning),
        length_critical: section
            .line_count
            .as_ref()
            .and_then(|c| c.critical)
            .unwrap_or(defaults.length_critical),
        params_enabled: section
            .parameters
            .as_ref()
            .and_then(|c| c.enabled)
            .unwrap_or(defaults.params_enabled),
        params_warning: section
            .parameters
            .as_ref()
            .and_then(|c| c.warning)
            .unwrap_or(defaults.params_warning),
        params_critical: section
            .parameters
            .as_ref()
            .and_then(|c| c.critical)
            .unwrap_or(defaults.params_critical),
        fan_in_enabled: section
            .fan_in
            .as_ref()
            .and_then(|c| c.enabled)
            .unwrap_or(defaults.fan_in_enabled),
        fan_in_warning: section
            .fan_in
            .as_ref()
            .and_then(|c| c.warning)
            .map(|v| v as usize)
            .unwrap_or(defaults.fan_in_warning),
        unused_procedures: section
            .unused_procedures
            .unwrap_or(defaults.unused_procedures),
    }
}

impl DiagnosticConfig {
    /// `true` when at least one detector would ever emit a finding.
    ///
    /// When this is `false` computing diagnostics is pure waste: every
    /// detector is off, so `compute_all` can only ever return an empty set.
    /// The server checks this before doing any diagnostic work — the VS Code
    /// client ships with code-quality diagnostics DISABLED by default and
    /// drops them client-side on arrival, so without this gate a default
    /// install pays a full whole-workspace analysis to produce output that
    /// nothing consumes.
    pub fn any_enabled(&self) -> bool {
        self.complexity_enabled
            || self.length_enabled
            || self.params_enabled
            || self.fan_in_enabled
            || self.unused_procedures
    }

    /// Turn every detector off — the in-memory equivalent of a config file
    /// with all `enabled: false`. Backs `--no-diagnostics`, which lets a
    /// client that discards code-quality diagnostics skip producing them.
    pub fn disable_all(&mut self) {
        self.complexity_enabled = false;
        self.length_enabled = false;
        self.params_enabled = false;
        self.fan_in_enabled = false;
        self.unused_procedures = false;
    }

    /// Load config by merging: defaults → global → workspace.
    pub fn load(workspace_root: &Path) -> Self {
        // Phase 1: Load both config files
        let global = global_config_path().and_then(|p| load_file(&p));
        let workspace = load_file(&crate::state_paths::workspace_config_for_read(
            workspace_root,
        ));

        // Phase 2: Merge sections (global as base, workspace as overlay)
        let merged = match (global, workspace) {
            (None, None) => DiagnosticsSection::default(),
            (Some(g), None) => g.diagnostics,
            (None, Some(w)) => w.diagnostics,
            (Some(g), Some(w)) => merge_sections(g.diagnostics, w.diagnostics),
        };

        // Phase 3: Apply defaults
        apply_defaults(merged)
    }
}

/// One root's live [`DiagnosticConfig`], shared by the server, the
/// diagnostics publisher and the watch loop (#221). `reloaded` records that
/// a watcher reload already stored a config read from disk.
pub struct ConfigHandle {
    root: PathBuf,
    no_diagnostics: bool,
    inner: std::sync::RwLock<(DiagnosticConfig, bool)>,
}

impl ConfigHandle {
    pub fn new(root: PathBuf, no_diagnostics: bool) -> Self {
        let mut cfg = DiagnosticConfig::default();
        if no_diagnostics {
            cfg.disable_all();
        }
        ConfigHandle {
            root,
            no_diagnostics,
            inner: std::sync::RwLock::new((cfg, false)),
        }
    }

    /// A clone of the current config; the lock is released on return.
    pub fn get(&self) -> DiagnosticConfig {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
            .clone()
    }

    /// The startup install. Stores only while no reload has run: a reload
    /// during startup read the file later, so it wins in either order (SN1).
    pub fn install_initial(&self, cfg: DiagnosticConfig) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.1 {
            inner.0 = cfg;
        }
    }

    /// Store a config the watcher read from disk.
    pub fn reload(&self, cfg: DiagnosticConfig) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = (cfg, true);
    }

    /// This root's config as it is on disk now, `--no-diagnostics` applied.
    pub fn read_disk(&self) -> DiagnosticConfig {
        let mut cfg = DiagnosticConfig::load(&self.root);
        if self.no_diagnostics {
            cfg.disable_all();
        }
        cfg
    }

    /// [`Self::read_disk`], then [`Self::reload`] it.
    pub fn reload_from_disk(&self) {
        self.reload(self.read_disk());
    }
}

/// Telemetry section of `~/.al-sem/config.json`. All fields optional;
/// the telemetry subsystem applies its own defaults from the spec.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TelemetryFileConfig {
    pub enabled: Option<bool>,
    pub connection_string: Option<String>,
    pub flush_interval_secs: Option<u64>,
    pub batch_size: Option<u32>,
    pub queue_capacity: Option<u32>,
    pub dedup_ttl_secs: Option<u64>,
    pub handler_empty_sample_rate: Option<u32>,
}

impl TelemetryFileConfig {
    /// Load from a config file path. Returns an empty config if missing/invalid.
    pub fn load_at(path: &Path) -> Self {
        // Reuse existing parsing helper. We need a wrapper struct because
        // `ConfigFile` only carries `diagnostics`.
        #[derive(Deserialize, Default)]
        struct Wrapper {
            #[serde(default)]
            telemetry: TelemetryFileConfig,
        }

        if !path.exists() {
            return Self::default();
        }
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        match serde_json::from_str::<Wrapper>(&contents) {
            Ok(w) => w.telemetry,
            Err(_) => Self::default(),
        }
    }

    /// Merge global + workspace files. Workspace overlays global per-field.
    pub fn load_merged(workspace_root: &Path) -> Self {
        let global = global_config_path()
            .map(|p| Self::load_at(&p))
            .unwrap_or_default();
        let workspace = Self::load_at(&crate::state_paths::workspace_config_for_read(
            workspace_root,
        ));
        Self::merge(global, workspace)
    }

    fn merge(base: Self, overlay: Self) -> Self {
        Self {
            enabled: overlay.enabled.or(base.enabled),
            connection_string: overlay.connection_string.or(base.connection_string),
            flush_interval_secs: overlay.flush_interval_secs.or(base.flush_interval_secs),
            batch_size: overlay.batch_size.or(base.batch_size),
            queue_capacity: overlay.queue_capacity.or(base.queue_capacity),
            dedup_ttl_secs: overlay.dedup_ttl_secs.or(base.dedup_ttl_secs),
            handler_empty_sample_rate: overlay
                .handler_empty_sample_rate
                .or(base.handler_empty_sample_rate),
        }
    }
}

/// A byte size in a config file: a number of bytes, or text the engine's
/// `--memory-budget` takes (`"6GiB"`, `"512MiB"`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ByteSize {
    Bytes(u64),
    Text(String),
}

impl ByteSize {
    /// The number of bytes.
    pub fn bytes(&self) -> Result<u64, String> {
        match self {
            ByteSize::Bytes(b) => Ok(*b),
            ByteSize::Text(t) => crate::engine::resident::budget::parse_bytes(t),
        }
    }
}

/// The `engine` section of `~/.al-sem/config.json` and the workspace's
/// `.al-sem.json` (pre-rename spellings read too, through
/// [`crate::state_paths`]): how the LSP server's context-server requests
/// (`al-call-hierarchy/explain`, ... — design 2026-11-01 §9) run their engine
/// child. All fields optional; the workspace file overlays the global one
/// per field.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EngineFileConfig {
    /// Whether the requests are served (default: yes; nothing runs until
    /// the first one).
    pub enabled: Option<bool>,
    /// The `alsem` binary the engine child runs. A relative path is taken
    /// relative to the folder of the file that names it. Default: `alsem`
    /// next to the LSP server's own binary.
    pub executable: Option<String>,
    /// Each child's `--memory-budget` (default: the engine's, by universe).
    pub memory_budget: Option<ByteSize>,
    /// Start each child with `--low-mem`.
    pub low_mem: Option<bool>,
    /// Each child's `--idle-timeout` in seconds (0: never).
    pub idle_timeout_secs: Option<u64>,
    /// The most engine children alive at once (default 2).
    pub max_children: Option<usize>,
}

impl EngineFileConfig {
    /// The `engine` section of the file at `path`, its `executable` made
    /// absolute against the file's folder. Empty when the file is missing
    /// or does not parse (logged).
    pub fn load_at(path: &Path) -> Self {
        #[derive(Deserialize, Default)]
        struct Wrapper {
            #[serde(default)]
            engine: EngineFileConfig,
        }
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut cfg = match serde_json::from_str::<Wrapper>(&contents) {
            Ok(w) => w.engine,
            Err(e) => {
                warn!(
                    "Failed to parse the engine section of {}: {e}",
                    path.display()
                );
                return Self::default();
            }
        };
        if let (Some(exe), Some(dir)) = (&cfg.executable, path.parent()) {
            let p = Path::new(exe);
            if p.is_relative() {
                cfg.executable = Some(dir.join(p).to_string_lossy().into_owned());
            }
        }
        cfg
    }

    /// The global file overlaid by `workspace_root`'s file (when given).
    pub fn load_merged(workspace_root: Option<&Path>) -> Self {
        let global = global_config_path()
            .map(|p| Self::load_at(&p))
            .unwrap_or_default();
        match workspace_root {
            Some(root) => Self::merge(
                global,
                Self::load_at(&crate::state_paths::workspace_config_for_read(root)),
            ),
            None => global,
        }
    }

    fn merge(base: Self, overlay: Self) -> Self {
        Self {
            enabled: overlay.enabled.or(base.enabled),
            executable: overlay.executable.or(base.executable),
            memory_budget: overlay.memory_budget.or(base.memory_budget),
            low_mem: overlay.low_mem.or(base.low_mem),
            idle_timeout_secs: overlay.idle_timeout_secs.or(base.idle_timeout_secs),
            max_children: overlay.max_children.or(base.max_children),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// The engine section: every field read, sizes as bytes or text, a
    /// relative executable taken against the file's folder, the workspace
    /// file overlaying the global one per field, and a missing or broken
    /// file read as empty. DISCRIMINATION (2026-11-02): letting the global
    /// file win for `enabled` fails "the workspace file wins"; reverted, it
    /// passes.
    #[test]
    fn engine_section_is_read_and_merged() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().join(".al-sem.json");
        fs::write(
            &ws,
            r#"{"engine": {"enabled": false, "executable": "bin/alsem", "memoryBudget": "3GiB", "lowMem": true, "idleTimeoutSecs": 5, "maxChildren": 1}}"#,
        )
        .unwrap();
        let cfg = EngineFileConfig::load_at(&ws);
        assert_eq!(cfg.enabled, Some(false));
        assert_eq!(
            cfg.executable.as_deref().map(PathBuf::from),
            Some(dir.path().join("bin/alsem"))
        );
        assert_eq!(cfg.memory_budget.unwrap().bytes(), Ok(3 << 30));
        assert_eq!(
            (cfg.low_mem, cfg.idle_timeout_secs, cfg.max_children),
            (Some(true), Some(5), Some(1))
        );
        let global = EngineFileConfig {
            enabled: Some(true),
            idle_timeout_secs: Some(9),
            memory_budget: Some(ByteSize::Bytes(7)),
            ..EngineFileConfig::default()
        };
        let overlay = EngineFileConfig {
            enabled: Some(false),
            ..EngineFileConfig::default()
        };
        let merged = EngineFileConfig::merge(global, overlay);
        assert_eq!(merged.enabled, Some(false), "the workspace file wins");
        assert_eq!(merged.idle_timeout_secs, Some(9), "the global file fills");
        assert_eq!(merged.memory_budget, Some(ByteSize::Bytes(7)));
        assert_eq!(
            EngineFileConfig::load_at(&dir.path().join("missing.json")),
            EngineFileConfig::default()
        );
        fs::write(&ws, "not json").unwrap();
        assert_eq!(EngineFileConfig::load_at(&ws), EngineFileConfig::default());
    }

    #[test]
    fn test_default_config() {
        let config = DiagnosticConfig::default();
        assert_eq!(config.complexity_warning, 5);
        assert_eq!(config.complexity_critical, 10);
        assert_eq!(config.params_warning, 4);
        assert_eq!(config.params_critical, 7);
        assert_eq!(config.length_critical, 50);
        assert_eq!(config.fan_in_warning, 20);
        assert!(config.unused_procedures);
    }

    #[test]
    fn test_load_missing_file() {
        let dir = TempDir::new().unwrap();
        let config = DiagnosticConfig::load(dir.path());
        assert_eq!(config.complexity_warning, 5); // default
    }

    #[test]
    fn test_load_partial_config() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".al-sem.json"),
            r#"{ "diagnostics": { "complexity": { "warning": 8 } } }"#,
        )
        .unwrap();
        let config = DiagnosticConfig::load(dir.path());
        assert_eq!(config.complexity_warning, 8);
        assert_eq!(config.complexity_critical, 10); // default preserved
        assert_eq!(config.params_warning, 4); // default preserved
    }

    // The next two tests pin the rename fallback at the real USE — `DiagnosticConfig::
    // load`, the function the server calls — not at `state_paths::resolve_for_read`.
    // Testing the helper alone would let the call site lose the fallback while staying
    // green. Both preconditions are written literally as files, so neither depends on
    // production code to produce them.

    #[test]
    fn workspace_config_under_the_pre_rename_name_is_still_honoured() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".al-call-hierarchy.json"),
            r#"{ "diagnostics": { "complexity": { "warning": 8 } } }"#,
        )
        .unwrap();
        assert!(
            !dir.path().join(".al-sem.json").exists(),
            "precondition: only the pre-rename file exists"
        );

        let config = DiagnosticConfig::load(dir.path());
        assert_eq!(
            config.complexity_warning, 8,
            "an install that predates the rename must not silently revert to defaults"
        );
    }

    /// #221 T14 (SN1): a reload that ran during startup read the file later,
    /// so it wins over the startup install in either order.
    #[test]
    fn t14_config_reload_during_startup_beats_the_initial_install() {
        let old = DiagnosticConfig::default();
        let new = DiagnosticConfig {
            unused_procedures: false,
            ..DiagnosticConfig::default()
        };
        assert!(old.unused_procedures, "precondition: old and new differ");

        let h = ConfigHandle::new(PathBuf::from("/ws"), false);
        h.reload(new.clone());
        h.install_initial(old.clone());
        assert!(
            !h.get().unused_procedures,
            "T14: reload then install_initial must keep the reloaded config"
        );

        let h = ConfigHandle::new(PathBuf::from("/ws"), false);
        h.install_initial(old);
        h.reload(new);
        assert!(
            !h.get().unused_procedures,
            "T14: install_initial then reload must end on the reloaded config"
        );
    }

    #[test]
    fn the_current_workspace_config_wins_over_the_pre_rename_one() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".al-sem.json"),
            r#"{ "diagnostics": { "complexity": { "warning": 8 } } }"#,
        )
        .unwrap();
        fs::write(
            dir.path().join(".al-call-hierarchy.json"),
            r#"{ "diagnostics": { "complexity": { "warning": 99 } } }"#,
        )
        .unwrap();

        let config = DiagnosticConfig::load(dir.path());
        assert_eq!(
            config.complexity_warning, 8,
            "the legacy file must not shadow the current one when both are present"
        );
    }

    #[test]
    fn test_load_full_config() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".al-sem.json"),
            r#"{
                "diagnostics": {
                    "complexity": { "warning": 8, "critical": 15 },
                    "parameters": { "warning": 5, "critical": 10 },
                    "lineCount": { "warning": 30, "critical": 80 },
                    "fanIn": { "warning": 30 },
                    "unusedProcedures": false
                }
            }"#,
        )
        .unwrap();
        let config = DiagnosticConfig::load(dir.path());
        assert_eq!(config.complexity_warning, 8);
        assert_eq!(config.complexity_critical, 15);
        assert_eq!(config.params_warning, 5);
        assert_eq!(config.params_critical, 10);
        assert_eq!(config.length_warning, 30);
        assert_eq!(config.length_critical, 80);
        assert_eq!(config.fan_in_warning, 30);
        assert!(!config.unused_procedures);
    }

    #[test]
    fn test_load_disabled_categories() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".al-sem.json"),
            r#"{
                "diagnostics": {
                    "complexity": { "enabled": false },
                    "parameters": { "enabled": false },
                    "lineCount": { "enabled": false },
                    "fanIn": { "enabled": false },
                    "unusedProcedures": false
                }
            }"#,
        )
        .unwrap();
        let config = DiagnosticConfig::load(dir.path());
        assert!(!config.complexity_enabled);
        assert!(!config.params_enabled);
        assert!(!config.length_enabled);
        assert!(!config.fan_in_enabled);
        assert!(!config.unused_procedures);
        // Thresholds still have defaults even when disabled
        assert_eq!(config.complexity_warning, 5);
        assert_eq!(config.complexity_critical, 10);
    }

    #[test]
    fn test_load_invalid_json() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".al-sem.json"), "not json").unwrap();
        let config = DiagnosticConfig::load(dir.path());
        assert_eq!(config.complexity_warning, 5); // falls back to default
    }

    #[test]
    fn test_merge_sections_deep() {
        let global = DiagnosticsSection {
            complexity: Some(ThresholdPair {
                enabled: None,
                warning: Some(8),
                critical: Some(15),
            }),
            parameters: Some(ThresholdPair {
                enabled: Some(false),
                warning: None,
                critical: None,
            }),
            line_count: None,
            fan_in: None,
            unused_procedures: Some(false),
        };
        let workspace = DiagnosticsSection {
            complexity: Some(ThresholdPair {
                enabled: None,
                warning: Some(5),
                critical: None,
            }),
            parameters: None,
            line_count: None,
            fan_in: None,
            unused_procedures: Some(true),
        };

        let merged = merge_sections(global, workspace);
        let config = apply_defaults(merged);

        // complexity.warning: workspace 5 overrides global 8
        assert_eq!(config.complexity_warning, 5);
        // complexity.critical: global 15 (workspace didn't set it)
        assert_eq!(config.complexity_critical, 15);
        // complexity.enabled: default true (neither set it)
        assert!(config.complexity_enabled);
        // parameters.enabled: global false (workspace didn't set it)
        assert!(!config.params_enabled);
        // parameters.warning: default 4 (neither set it)
        assert_eq!(config.params_warning, 4);
        // unusedProcedures: workspace true overrides global false
        assert!(config.unused_procedures);
        // lineCount: all defaults
        assert_eq!(config.length_warning, 20);
        assert_eq!(config.length_critical, 50);
    }

    #[test]
    fn test_global_config_path() {
        let path = global_config_path();
        // Should return Some on any system with a home directory
        assert!(path.is_some());
        let p = path.unwrap();

        // Which of the two directories comes back depends on what exists in the real
        // `$HOME` of whoever is running the tests, so this cannot pin one answer without
        // becoming machine-dependent. What it CAN pin is that the answer is always one of
        // the two sanctioned locations and always the right file — the choice between
        // them is pinned hermetically in `state_paths::tests`.
        assert_eq!(
            p.file_name().and_then(|f| f.to_str()),
            Some("config.json"),
            "the global config is always config.json"
        );
        let parent = p
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|f| f.to_str())
            .expect("the path has a named parent directory");
        assert!(
            parent == crate::state_paths::STATE_DIR
                || parent == crate::state_paths::LEGACY_STATE_DIR,
            "global config resolved outside both sanctioned state directories: {}",
            p.display()
        );
    }

    #[test]
    fn test_load_telemetry_section() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".al-sem.json"),
            r#"{
                "telemetry": {
                    "enabled": false,
                    "connectionString": "InstrumentationKey=foo;IngestionEndpoint=https://x.azure.com/",
                    "flushIntervalSecs": 7,
                    "batchSize": 256,
                    "queueCapacity": 1024,
                    "dedupTtlSecs": 600,
                    "handlerEmptySampleRate": 20
                }
            }"#,
        )
        .unwrap();
        let tcfg = TelemetryFileConfig::load_at(&dir.path().join(".al-sem.json"));
        assert_eq!(tcfg.enabled, Some(false));
        assert_eq!(
            tcfg.connection_string.as_deref(),
            Some("InstrumentationKey=foo;IngestionEndpoint=https://x.azure.com/")
        );
        assert_eq!(tcfg.flush_interval_secs, Some(7));
        assert_eq!(tcfg.batch_size, Some(256));
        assert_eq!(tcfg.queue_capacity, Some(1024));
        assert_eq!(tcfg.dedup_ttl_secs, Some(600));
        assert_eq!(tcfg.handler_empty_sample_rate, Some(20));
    }

    #[test]
    fn test_load_telemetry_missing_section_yields_empty() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".al-sem.json"), r#"{ "diagnostics": {} }"#).unwrap();
        let tcfg = TelemetryFileConfig::load_at(&dir.path().join(".al-sem.json"));
        assert!(tcfg.enabled.is_none());
        assert!(tcfg.connection_string.is_none());
    }
}
