//! LSP server implementation for call hierarchy — serves the program-engine
//! backend (T3 Task 15 cutover).
//!
//! Server state is now `Arc<SharedSnapshot>` (the published, immutable
//! `LspSnapshot`) + an `mpsc::Sender<ChangeEvent>` feeding a background
//! [`spawn_updater`] thread, instead of the legacy `Arc<RwLock<Indexer>>`.
//! Every request handler only ever `Arc`-clones the current snapshot
//! (`SharedSnapshot::get`) — no parsing, no graph rebuild, ever happens under
//! a request-facing lock; the updater thread owns all of that work and
//! publishes a fresh snapshot by atomic swap (see `src/lsp/updater.rs`'s
//! module doc).
//!
//! Dispatch is direct: `textDocument/prepareCallHierarchy` /
//! `callHierarchy/{incoming,outgoing}Calls` / `textDocument/codeLens` /
//! the three engine-backed custom requests go straight to the Task 11-13
//! functions (`lsp::handlers`/`lsp::lens`/`lsp::custom`) with the negotiated
//! [`PositionEncoding`]. `al-call-hierarchy/{fieldProperties,actionProperties,
//! telemetryStatus}` are graph-independent (pure source-read / process
//! status) and dispatch to `lsp::custom`'s implementations (relocated
//! verbatim from legacy `src/handlers.rs` at Task 17's legacy deletion —
//! Task 15's cutover already pointed here unchanged before the move).
//!
//! Diagnostics follow "recompute-diff-publish-clear": every snapshot swap
//! (including the very first, batch-built one) runs `lsp::diagnostics::
//! compute_all` and diffs it through one shared [`DiagnosticsState`],
//! publishing only what changed and clearing a uri whose findings dropped to
//! zero — the legacy publish-once-at-startup path never cleared a fixed
//! finding until an unrelated one happened to overwrite it; this closes that
//! gap permanently.
//!
//! # No valid workspace: fail-closed-to-empty, not a crash
//!
//! The program engine's snapshot model fundamentally requires a single
//! AL-app workspace root with a readable `app.json` (see `SnapshotBuilder`'s
//! own doc) — unlike the legacy `Indexer`, which happily ran with zero
//! indexed files. When no workspace folder is given at `initialize`, or a
//! given root fails to build a snapshot (missing/invalid `app.json`), the
//! server still completes the LSP handshake and answers every request under
//! THAT root with an empty result (`None`/`[]`) rather than refusing to
//! start — see [`RootState`]'s `Option` wrapping (one PER configured root
//! since the multi-root refactor below; a broken root degrades only its OWN
//! files, never any other configured root).
//!
//! # Multi-root: per-root `ServerState`, never a merged snapshot
//!
//! [`Workspace`] holds one [`RootState`] per `workspace_folders` entry the
//! client offered at `initialize` ([`configured_roots`]) — each with its OWN
//! [`LspSnapshot`], updater thread, file watcher, and
//! [`DiagnosticsState`](crate::lsp::diagnostics::DiagnosticsState). An
//! inbound request routes to whichever root its `textDocument` URI falls
//! under (longest-prefix match, [`route_uri`]) — mirroring
//! [`lsp::handlers::resolve_virtual_path`](crate::lsp::handlers::resolve_virtual_path)'s
//! single-root version of the same lookup. `incomingCalls`/`outgoingCalls`
//! route via `item.uri` too, EXCEPT that a dependency-embedded-source or
//! ABI-boundary item's `uri` is a synthetic non-`file://` scheme with no
//! path to prefix-match at all — those instead route via a root marker this
//! server stamps into every `CallHierarchyItem.data` it mints
//! ([`route_item`]/[`tag_item_root`]), which is REQUIRED for correctness,
//! not just a fallback: `RoutineNodeId`'s `AppRef` is a raw index into that
//! snapshot's OWN independent `AppRegistry` (`src/program/node.rs`), so the
//! exact same `RoutineNodeId` VALUE can legitimately name two DIFFERENT
//! routines in two different roots' graphs. A single configured root
//! behaves byte-identically to the pre-multi-root server (no marker is ever
//! stamped, no extra warnings are ever logged) — see each routing
//! function's own doc for the exact single-vs-multi-root gate.
//!
//! `workspace/didChangeWorkspaceFolders` (dynamic add/remove of a root after
//! `initialize`) is NOT implemented — see [`handle_notification`]'s arm for
//! that method for why (a real blocker, not laziness: safe REMOVAL needs a
//! cancellation signal `AlFileWatcher`'s loop doesn't have today) and
//! `docs/OUTSTANDING.md` for the tracked follow-up. `al-call-hierarchy/
//! dependencyDocumentSymbol`'s synthetic `al-preview://` uri also has no
//! per-root discriminator yet (see [`dispatch_dependency_document_symbol`]'s
//! doc for the best-effort fallback this uses instead) — a smaller, lower-
//! stakes gap than routing proper, tracked here rather than in
//! `docs/OUTSTANDING.md` since it has no correctness impact (browsing a
//! dependency's symbols, not call-hierarchy identity).

use anyhow::{Context, Result};
use log::{debug, info, warn};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::{
    CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, CodeLensOptions, CodeLensParams, Diagnostic,
    DidSaveTextDocumentParams, InitializeParams, InitializeResult, PositionEncodingKind,
    PublishDiagnosticsParams, ServerCapabilities, Uri,
};
use serde_json::Value;
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config::DiagnosticConfig;
use crate::lsp::custom::{
    DependencyDocumentSymbol, DependencyDocumentSymbolParams, EventPublishersInFileParams,
    EventReferenceAtPositionParams, SymbolPropertiesParams, action_properties,
    dependency_document_symbol, event_publishers_in_file, event_reference_at_position,
    field_properties,
};
use crate::lsp::diagnostics::{DiagnosticsState, compute_all, compute_for_files, rung1_cover};
use crate::lsp::encoding::{PositionEncoding, negotiate};
use crate::lsp::handlers::{ItemData, incoming, outgoing, prepare};
use crate::lsp::lens::code_lenses;
use crate::lsp::snapshot::LspSnapshot;
use crate::lsp::updater::{ChangeEvent, Rung1Delta, SharedSnapshot, SwapScope, spawn_updater};
use crate::protocol::uri_to_path;
use crate::watcher::{AlFileWatcher, FileChange};

/// Everything the server needs once a valid workspace snapshot exists for
/// ONE root. Wrapped in `Option` by [`RootState`] — `None` there means "no
/// usable workspace at this root" — see the module doc.
struct ServerState {
    shared: Arc<SharedSnapshot>,
    tx: mpsc::Sender<ChangeEvent>,
    /// Deliberately unread in production (see `run_server`'s shutdown-sequence
    /// doc — joining it would hang whenever the file watcher is also running).
    /// Kept so the in-module smoke test (`tests::
    /// dispatch_prepare_and_outgoing_then_didsave_bumps_generation_and_republishes_diagnostics`)
    /// can assert the updater thread actually stops once `tx` is its only
    /// live sender — the watcher-free case that test exercises.
    #[allow(dead_code)]
    updater_handle: JoinHandle<()>,
    encoding: PositionEncoding,
    config: DiagnosticConfig,
}

/// A sink for outbound `Message`s, owned by each [`RootState`] so a root
/// can publish diagnostics whenever it is eventually built. Deliberately a
/// closure rather than a `crossbeam_channel::Sender<Message>` — that type
/// is an indirect dependency this crate never names (same rationale as
/// [`publish_changed`]'s generic `send`). `Arc` because the per-root
/// background updater thread needs its own clone.
type MessageSink = Arc<dyn Fn(Message) + Send + Sync>;

/// Wrap a `Connection`'s sender as a [`MessageSink`], logging (never
/// panicking) if the peer has already gone away.
fn message_sink(connection: &Connection) -> MessageSink {
    let sender = connection.sender.clone();
    Arc::new(move |m| {
        if let Err(e) = sender.send(m) {
            warn!("Failed to publish message: {}", e);
        }
    })
}

/// Everything [`RootState::state`] needs to build its root on demand.
struct RootBuild {
    encoding: PositionEncoding,
    sink: MessageSink,
    /// Start this root's file watcher when (and only when) the root is
    /// actually built — a root that is never touched never spawns a
    /// watcher thread either.
    start_watcher: bool,
    /// `--no-diagnostics`: force every detector off for this root, whatever
    /// its `.al-sem.json` says.
    no_diagnostics: bool,
}

/// One configured workspace root plus — once anything actually asks for it
/// — whatever `ServerState` its OWN snapshot build produced.
///
/// The state is built LAZILY, on the first request that routes to this
/// root, and cached thereafter. It is `Some(None)` exactly when THIS root's
/// build was attempted and failed (see the module doc's "no valid
/// workspace" section, applied per root) — [`RootState::state`] logs why
/// and moves on; a broken root never stops any other root from building.
///
/// Laziness is load-bearing, not an optimization: a snapshot retains its
/// root's whole dependency closure INCLUDING every dependency app's source
/// text (`AppSetSnapshot`), and nothing is shared between roots. Building
/// all of them up front cost ~17 GB peak on a 33-root customer workspace
/// whose session never opened a single file. Roots are cheap until touched.
struct RootState {
    /// Always normalized ([`crate::protocol::normalize_path`] — case-folded
    /// on Windows), so it compares directly against a `uri_to_path`'d
    /// inbound URI with no further munging — see [`build_workspace`], the
    /// ONLY place a `RootState` is constructed.
    root: PathBuf,
    /// `OnceCell` (not `Mutex`/`OnceLock`): the whole `Workspace` is only
    /// ever borrowed from the single `main_loop` thread, so no locking is
    /// needed and none is paid for.
    state: OnceCell<Option<ServerState>>,
    build: RootBuild,
}

impl RootState {
    /// A root whose state is already built, for tests that assemble a
    /// `Workspace` by hand (production always goes through the lazy path).
    #[cfg(test)]
    fn prebuilt(root: PathBuf, state: Option<ServerState>, build: RootBuild) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(state);
        RootState {
            root,
            state: cell,
            build,
        }
    }

    /// This root's state, building it on first call. Returns `None` when
    /// the build failed (or previously failed — the failure is cached, so a
    /// broken root is diagnosed once, not on every request).
    fn state(&self) -> Option<&ServerState> {
        self.state
            .get_or_init(|| {
                let mut config = DiagnosticConfig::load(&self.root);
                if self.build.no_diagnostics {
                    config.disable_all();
                }
                let built =
                    build_server_state(&self.root, self.build.encoding, config, &self.build.sink);
                match &built {
                    Some(st) => {
                        info!("Built program snapshot for workspace root {}", self.root.display());
                        if self.build.start_watcher {
                            start_file_watcher(st.tx.clone(), self.root.clone());
                        }
                    }
                    None => warn!(
                        "Failed to build the program snapshot for workspace root {}                          (missing/invalid app.json?) — every request under this root                          will return an empty result until a valid single-app AL                          workspace is opened there; other configured roots are                          unaffected.",
                        self.root.display()
                    ),
                }
                built
            })
            .as_ref()
    }

    /// This root's state ONLY if it has already been built — never triggers
    /// a build. For shutdown and for tests asserting laziness.
    fn built(&self) -> Option<&ServerState> {
        self.state.get().and_then(Option::as_ref)
    }

    /// Consume this root, yielding its state only if one was ever built.
    fn into_built(self) -> Option<ServerState> {
        self.state.into_inner().flatten()
    }
}

/// The whole multi-root session: one [`RootState`] per root
/// [`configured_roots`] extracted from `initialize`. See the module doc's
/// "Multi-root" section for the routing/isolation design this implements.
///
/// Never mutated after [`run_server`] builds it — `workspace/
/// didChangeWorkspaceFolders` is not implemented (see [`handle_notification`]).
struct Workspace {
    roots: Vec<RootState>,
}

impl Workspace {
    /// `true` whenever more than one root is configured — the single gate
    /// every root-marker-stamping/consuming function below checks, so a
    /// single-folder session's wire format and logging stay byte-identical
    /// to the pre-multi-root server (scope discipline: the common case must
    /// never pay for or notice this refactor).
    fn is_multi_root(&self) -> bool {
        self.roots.len() > 1
    }
}

/// Run the LSP server
pub fn run_server(no_watcher: bool, no_telemetry: bool, no_diagnostics: bool) -> Result<()> {
    info!("Starting AL Call Hierarchy LSP server (program-engine backend)");

    let (connection, io_threads) = Connection::stdio();

    // Initialize
    let (id, params) = connection.initialize_start()?;
    let init_params: InitializeParams = serde_json::from_value(params)?;

    let roots = configured_roots(&init_params);

    let init_option_telemetry = init_params
        .initialization_options
        .as_ref()
        .and_then(|v| v.get("telemetry"))
        .and_then(|t| t.get("enabled"))
        .and_then(|b| b.as_bool());
    let telemetry_handle = crate::telemetry::init(crate::telemetry::TelemetryInputs {
        cli_no_telemetry: no_telemetry,
        init_option: init_option_telemetry,
        // Telemetry is a single session-level counter, not a per-app one —
        // deliberately kept on the FIRST configured root only, exactly like
        // the pre-multi-root server (see `primary_workspace_root`'s doc).
        workspace_root: primary_workspace_root(&init_params),
        connection_string: option_env!("AL_CH_TELEMETRY_CONNECTION_STRING").map(String::from),
    });

    // H-12: negotiate the position encoding from `general.positionEncodings`
    // (LSP 3.17). Every handler dispatched below is driven with this SAME
    // negotiated value (Task 15 cutover — legacy handlers served byte
    // columns regardless of negotiation; the new backend never does).
    let client_position_encodings: Option<Vec<String>> = init_params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref())
        .map(|encs| encs.iter().map(|e| e.as_str().to_string()).collect());
    let position_encoding = negotiate(client_position_encodings.as_deref());
    info!("Negotiated position encoding: {:?}", position_encoding);

    let capabilities = ServerCapabilities {
        call_hierarchy_provider: Some(lsp_types::CallHierarchyServerCapability::Simple(true)),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        text_document_sync: Some(lsp_types::TextDocumentSyncCapability::Options(
            lsp_types::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(lsp_types::TextDocumentSyncKind::NONE),
                will_save: None,
                will_save_wait_until: None,
                save: Some(lsp_types::TextDocumentSyncSaveOptions::SaveOptions(
                    lsp_types::SaveOptions {
                        include_text: Some(false),
                    },
                )),
            },
        )),
        position_encoding: Some(match position_encoding {
            PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
            PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        ..Default::default()
    };

    let result = InitializeResult {
        capabilities,
        server_info: Some(lsp_types::ServerInfo {
            name: "al-call-hierarchy".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };

    connection.initialize_finish(id, serde_json::to_value(result)?)?;
    info!("Server initialized");

    // Build one `RootState` per configured root — see `build_workspace`'s
    // doc for the per-root fail-loud-but-isolated build semantics.
    // Watchers are started per root by `RootState::state` when (and only
    // when) that root is actually built — never for a root nothing touches.
    let workspace = build_workspace(
        &roots,
        position_encoding,
        &connection,
        !no_watcher,
        no_diagnostics,
    );

    // The old eager "start a watcher for every built root" loop is gone:
    // `RootState::state` starts a root's watcher at the moment that root is
    // built, so watcher threads track actual use rather than configuration.
    // `--no-watcher` is threaded through `build_workspace` above.
    if no_watcher {
        info!("File watcher disabled (--no-watcher). Using LSP notifications for changes.");
    }

    // Main loop. Takes OWNERSHIP of `connection` (not `&connection`) —
    // verified end to end (a real stdio round trip via a Python LSP client,
    // T3 Task 15's own manual smoke test) that a BORROWED connection hangs
    // the process forever after a clean shutdown/exit: `IoThreads::join`
    // waits for `lsp_server`'s writer thread, which only exits once every
    // `Sender<Message>` clone is dropped — including `connection.sender`
    // ITSELF, which never happens while `connection` is still a live local
    // in `run_server`'s own scope below `io_threads.join()?`. Moving
    // `connection` into `main_loop` means it (and its `sender`) drops at
    // `main_loop`'s own closing brace, BEFORE `io_threads.join()?` runs —
    // matching `lsp_server`'s own `examples/minimal_lsp.rs` pattern exactly
    // (`main_loop(connection, init_params)?; io_thread.join()?;`, connection
    // passed by value). This bug predates this cutover (the legacy
    // `main_loop(connection: &Connection, ...)` had the identical shape) but
    // was never exercised end to end until this task's own verification.
    main_loop(connection, &workspace)?;

    // Dropping each root's `tx` is a best-effort shutdown signal to ITS
    // updater thread — but we deliberately do NOT `st.updater_handle.join()`
    // here: when that root's file watcher is running, it holds its OWN
    // clone of `tx` (`start_file_watcher`) for as long as ITS OWN infinite
    // loop runs — which has no stop signal of its own (matching legacy's
    // watcher thread, which also never exits early; out of Task 15's
    // "event forwarding only" scope for `watcher.rs` to add one). That
    // updater's `rx` therefore never sees every sender dropped, so it never
    // returns from `gather_batch`, so joining it would block this function
    // forever. Dropping `tx` here is still worthwhile — in the
    // `--no-watcher` case (`watcher_started == false`) it's the ONLY signal
    // each updater thread gets, and lets it (and the `sender_bg`-held
    // `Message` sender clone it carries — see below) exit promptly.
    // Only roots that were actually BUILT own a `tx` (or a watcher); an
    // untouched root has nothing to signal. `watcher_started` is therefore
    // derived here rather than at startup — with lazy roots, whether any
    // watcher exists is only knowable once the session is over.
    let watcher_started = !no_watcher && workspace.roots.iter().any(|r| r.built().is_some());
    for root_state in workspace.roots {
        if let Some(st) = root_state.into_built() {
            drop(st.tx);
        }
    }

    // `io_threads.join()` blocks on `lsp_server`'s writer thread, which
    // itself blocks until EVERY `Sender<Message>` clone is dropped —
    // including one captured by the updater thread's diagnostics on_swap
    // closure (`sender_bg` in `build_server_state`), which (per the note
    // above) legitimately never drops while the watcher thread is also
    // alive. Verified end to end (a real stdio round trip, T3 Task 15's own
    // manual smoke test): an unconditional `io_threads.join()?` here hangs
    // the process forever whenever the watcher started. Bounded wait +
    // detach — the SAME idiom `telemetry::shutdown` already uses just below,
    // for the identical reason (a background thread that legitimately never
    // stops on its own must never be joined unconditionally at shutdown):
    // everything meaningful was already flushed by the time `main_loop`
    // returned (the `shutdown` response itself, most importantly), so
    // giving up after a short grace period loses nothing.
    let (io_done_tx, io_done_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = io_done_tx.send(io_threads.join());
    });
    match io_done_rx.recv_timeout(Duration::from_millis(500)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) if watcher_started => debug!(
            "io_threads did not finish within the shutdown budget — expected \
             while the file watcher/updater threads are still alive; exiting anyway"
        ),
        Err(_) => warn!(
            "io_threads did not finish within the shutdown budget (unexpected — no \
             watcher was started this session); exiting anyway"
        ),
    }

    info!("Server shut down");
    crate::telemetry::shutdown(telemetry_handle);
    Ok(())
}

/// Every workspace root this session serves — one entry per
/// `workspace_folders` item (LSP 3.6+, the common case), falling back to the
/// deprecated single `root_uri` for older clients when `workspace_folders`
/// is absent or empty. Each returned path is ALREADY normalized
/// (`uri_to_path` → `protocol::normalize_path`, case-folded on Windows) —
/// every downstream root-matching comparison (`route_uri`/`route_item`/
/// [`build_workspace`]) relies on that. Duplicate folders (a client
/// offering the same path twice, or two URIs that normalize to the same
/// path) are deduplicated — spinning up two independent watchers/updaters
/// over the SAME files would just race them against each other.
///
/// This is the REAL multi-root entry point; [`primary_workspace_root`]
/// (below) is a thin convenience over it for the one caller that
/// deliberately still wants just the first root.
#[allow(deprecated)] // root_uri is deprecated but kept for older LSP clients
fn configured_roots(params: &InitializeParams) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Some(path) = uri_to_path(&folder.uri)
                && !roots.contains(&path)
            {
                roots.push(path);
            }
        }
    }
    if roots.is_empty()
        && let Some(path) = params.root_uri.as_ref().and_then(uri_to_path)
    {
        roots.push(path);
    }
    roots
}

/// The FIRST configured workspace root — for the one remaining
/// session-scoped (not per-root) caller: `TelemetryInputs::workspace_root`
/// (see `run_server`), deliberately kept on the first root rather than
/// aggregated across roots (telemetry is a single session-level counter,
/// not a per-app one). Implemented as `configured_roots(params).into_iter().
/// next()` — see that function for the real multi-root list.
///
/// # History: the single-root decision this function ORIGINALLY encoded
///
/// The paragraphs below are the T3 Task 15 decision record, kept VERBATIM
/// as the permanent history — do not delete it. Its "tracked follow-up" is
/// now DONE: see [`Workspace`]/[`RootState`]/[`route_uri`]/[`route_item`]
/// for the per-root `ServerState` map and URI→root routing it called for.
///
/// ## Decision (T3 Task 15, confirmed in review): single-root is the
/// supported model, not a stopgap
///
/// The program engine's `SnapshotBuilder` models a workspace as exactly one
/// AL app (see its own doc). Legacy's `Indexer` could accept several
/// `workspace_folders` and silently accumulate ALL of them into ONE graph
/// (`index_workspaces`'s per-folder loop, each folder's files merged into
/// the SAME `CallGraph`) — but that was never a real multi-root FEATURE, it
/// was a collision hazard: `graph::QualifiedName { object: Symbol, procedure:
/// Symbol }` has no app/folder discriminator at all (the exact same
/// unscoped-identity shape already named and fixed for the single-workspace
/// case as `LegacyIdentityCollapse` — see `tests/lsp_differential.rs`). Two
/// DIFFERENT apps opened as sibling folders, each declaring their own
/// `Codeunit "Helper"` with its own `DoWork` procedure, would silently
/// collide into ONE last-write-wins slot — `incomingCalls`/`outgoingCalls`
/// could point a caller at the WRONG app's routine entirely, with no
/// diagnostic ever raised. Given that, narrowing to one root is a
/// CORRECTNESS improvement, not a regression, even though it does mean a
/// genuinely multi-folder AL workspace only gets ONE app served per session
/// today. A client offering more than one folder gets the FIRST, with a
/// clear warning (fail-loud, never silently drop work without saying so)
/// rather than an attempt to merge multiple app.json-rooted trees into one
/// snapshot the way legacy did.
///
/// **Tracked follow-up (not that arc's scope) — DONE by this one
/// (`feat/multi-root-lsp`):** real multi-root support needs a per-root
/// `ServerState` map (each root gets its OWN `LspSnapshot`/updater/watcher/
/// `DiagnosticsState` — never one shared snapshot merging distinct apps)
/// plus URI→root routing in `dispatch_request`/`handle_notification` (map
/// an inbound `textDocument` URI to whichever root's `workspace_root` it
/// falls under, mirroring how `resolve_virtual_path` already does the
/// single-root version of this lookup).
#[allow(deprecated)] // root_uri is deprecated but kept for older LSP clients
fn primary_workspace_root(params: &InitializeParams) -> Option<PathBuf> {
    configured_roots(params).into_iter().next()
}

/// Build the initial snapshot, publish its diagnostics, and spawn the
/// background updater. Returns `None` when [`LspSnapshot::build_full_with_parsed`]
/// fails (see the module doc's "no valid workspace" section) — the caller
/// logs the reason.
fn build_server_state(
    workspace_root: &Path,
    encoding: PositionEncoding,
    config: DiagnosticConfig,
    sink: &MessageSink,
) -> Option<ServerState> {
    let (initial, workspace) = LspSnapshot::build_full_with_parsed(workspace_root)?;
    let initial = Arc::new(initial);
    let shared = Arc::new(SharedSnapshot::new(Arc::clone(&initial)));

    // Session-start telemetry is recorded by the FIRST root that builds,
    // not at `initialize`: with lazy roots there is no snapshot to measure
    // until something is actually opened, and forcing one just to count it
    // would reintroduce exactly the eager build this design removes.
    record_session_start_once(&initial);

    let diag_state = Arc::new(Mutex::new(DiagnosticsState::new()));
    // Diagnostics gate: with every detector disabled `compute_all` can only
    // return an empty set, so skip the whole-workspace analysis rather than
    // computing findings nothing will consume.
    if config.any_enabled() {
        let sink_initial = Arc::clone(sink);
        publish_full_diagnostics_diff(
            move |m| sink_initial(m),
            &diag_state,
            &initial,
            encoding,
            &config,
        );
    } else {
        debug!(
            "Diagnostics disabled for {} — skipping initial analysis",
            workspace_root.display()
        );
    }

    let (tx, rx) = mpsc::channel::<ChangeEvent>();
    let diag_state_bg = Arc::clone(&diag_state);
    let sink_bg = Arc::clone(sink);
    let config_bg = config.clone();
    let updater_handle = spawn_updater(
        Arc::clone(&shared),
        rx,
        workspace_root.to_path_buf(),
        workspace,
        move |new, scope| {
            if !config_bg.any_enabled() {
                return;
            }
            let sink = Arc::clone(&sink_bg);
            let send = move |m| sink(m);
            match scope {
                SwapScope::Full => {
                    publish_full_diagnostics_diff(send, &diag_state_bg, new, encoding, &config_bg);
                }
                SwapScope::Rung1(delta) => {
                    publish_rung1_diagnostics_diff(
                        send,
                        &diag_state_bg,
                        new,
                        encoding,
                        &config_bg,
                        delta,
                    );
                }
            }
        },
    );

    Some(ServerState {
        shared,
        tx,
        updater_handle,
        encoding,
        config,
    })
}

/// Build one [`RootState`] per configured root — the multi-root
/// generalization of a single `build_server_state` call. Logs the
/// zero-roots case ONCE, session-level (mirrors the pre-multi-root server's
/// "no workspace folder given" warning exactly); a root whose OWN snapshot
/// build fails (bad/missing `app.json`) degrades to `RootState { state:
/// None, .. }` — this root's own files answer empty (the single-root "no
/// valid workspace" fail path, see module doc), logged individually, but
/// NEVER stops any other root from building — the fail-loud, per-root
/// isolated failure mode the design calls for.
///
/// Each `root` is re-normalized here (`protocol::normalize_path`) even
/// though [`configured_roots`] already normalizes its own output — cheap
/// and idempotent for that caller, but load-bearing for any OTHER caller
/// (e.g. a test) that hands this function a raw, un-normalized path: EVERY
/// `RootState.root` this function produces must be normalized, unconditionally,
/// because `route_uri`/`route_item` compare against it directly.
fn build_workspace(
    roots: &[PathBuf],
    encoding: PositionEncoding,
    connection: &Connection,
    start_watchers: bool,
    no_diagnostics: bool,
) -> Workspace {
    if roots.is_empty() {
        warn!(
            "No workspace folder given at initialize — every request will              return an empty result until a workspace is opened."
        );
    }
    let sink = message_sink(connection);
    let roots: Vec<RootState> = roots
        .iter()
        .map(|raw_root| RootState {
            root: crate::protocol::normalize_path(raw_root),
            state: OnceCell::new(),
            build: RootBuild {
                encoding,
                sink: Arc::clone(&sink),
                start_watcher: start_watchers,
                no_diagnostics,
            },
        })
        .collect();
    info!(
        "Configured {} workspace root(s); each builds its snapshot on first use",
        roots.len()
    );
    Workspace { roots }
}

/// Record session-start telemetry exactly once per process, driven by the
/// FIRST root that successfully builds.
///
/// Pre-lazy this was measured at `initialize` off root 0's snapshot. With
/// lazy roots there is no snapshot to measure until something is actually
/// opened, and forcing one just to count it would reintroduce the eager
/// build this design exists to remove — so the measurement moves to the
/// first real build. A session that never opens a file now records no
/// session start at all, which is the truthful signal: nothing was analyzed.
fn record_session_start_once(snap: &LspSnapshot) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let definitions: usize = snap.decls_by_file.values().map(|v| v.len()).sum();
        let call_sites: usize = snap.edges_by_file.values().map(|v| v.len()).sum();
        let dep_app_count = snap.snap.apps.len().saturating_sub(1);
        crate::telemetry::record_session_start(
            (definitions + call_sites) as u32,
            dep_app_count.min(u8::MAX as usize) as u8,
            dep_app_count > 0,
        );
    });
}

/// Recompute-diff-publish: run [`compute_all`] over `snap`, diff it through
/// `diag_state`, and hand every changed `(uri, diagnostics)` pair to `send`
/// as a `textDocument/publishDiagnostics` notification. Used for the
/// initial (pre-updater) publish and every `SwapScope::Full` (rung-2/3)
/// swap — see [`publish_rung1_diagnostics_diff`] for the rung-1-scoped
/// counterpart, and [`publish_changed`] for the shared "send what changed"
/// tail both funnel through so the two recompute scopes can never drift in
/// how they're published.
fn publish_full_diagnostics_diff(
    send: impl Fn(Message),
    diag_state: &Mutex<DiagnosticsState>,
    snap: &LspSnapshot,
    enc: PositionEncoding,
    cfg: &DiagnosticConfig,
) {
    let all = compute_all(snap, enc, cfg);
    let changed = diag_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .diff(all);
    publish_changed(send, changed);
}

/// The rung-1-scoped counterpart of [`publish_full_diagnostics_diff`]
/// (Tier-2 latency wave, Task 2 / item D): recomputes ONLY `delta`'s
/// recompute cover (`rung1_cover`) via `compute_for_files`, and diffs it
/// through `DiagnosticsState::diff_partial` — never a full workspace
/// recompute. See `rung1_cover`'s own doc for why this cover is a complete
/// substitute for `compute_all` on a rung-1 swap.
fn publish_rung1_diagnostics_diff(
    send: impl Fn(Message),
    diag_state: &Mutex<DiagnosticsState>,
    snap: &LspSnapshot,
    enc: PositionEncoding,
    cfg: &DiagnosticConfig,
    delta: &Rung1Delta,
) {
    let cover = rung1_cover(snap, delta);
    let touched = compute_for_files(snap, enc, cfg, &cover);
    let changed = diag_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .diff_partial(touched);
    publish_changed(send, changed);
}

/// Send every changed `(uri, diagnostics)` pair as a
/// `textDocument/publishDiagnostics` notification. `send` is generic rather
/// than a concrete `crossbeam_channel::Sender<Message>` purely to avoid
/// naming that type here (an indirect dependency, never a direct one of
/// this crate) — every call site just passes a small closure wrapping
/// `Sender::send`.
fn publish_changed(send: impl Fn(Message), changed: Vec<(String, Vec<Diagnostic>)>) {
    for (uri, diagnostics) in changed {
        let Ok(uri) = uri.parse::<Uri>() else {
            warn!("Skipping diagnostics publish for an unparsable uri: {uri}");
            continue;
        };
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: None,
        };
        let notification = Notification::new(
            "textDocument/publishDiagnostics".to_string(),
            serde_json::to_value(params).expect("PublishDiagnosticsParams always serializes"),
        );
        send(Message::Notification(notification));
    }
}

/// Start the file watcher thread for incremental updates: every raw
/// [`FileChange`] is mapped onto the updater's [`ChangeEvent`] vocabulary
/// (see [`to_change_event`]) and pushed onto the SAME channel `didSave`
/// notifications use — one coalesced queue, so a save followed immediately
/// by the watcher's own notification of that same write no longer causes two
/// separate rebuilds (legacy's didSave + watcher paths each reindexed
/// independently).
fn start_file_watcher(tx: mpsc::Sender<ChangeEvent>, workspace_root: PathBuf) {
    thread::spawn(move || {
        let watcher = match AlFileWatcher::new(&workspace_root) {
            Ok(w) => w,
            Err(e) => {
                warn!(
                    "Failed to create file watcher for {}: {}",
                    workspace_root.display(),
                    e
                );
                return;
            }
        };
        info!(
            "File watcher thread started for {}",
            workspace_root.display()
        );

        loop {
            let Some(change) = watcher.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            if tx.send(to_change_event(change)).is_err() {
                info!("Updater channel closed; stopping file watcher thread");
                return;
            }
        }
    });
}

/// Map a raw filesystem [`FileChange`] to the updater's [`ChangeEvent`]
/// vocabulary: a change under `.alpackages` (a dependency add/update/remove
/// — legacy never watched these at all, freezing dependency resolution at
/// startup) escalates to `DepsChanged` regardless of which side of the
/// Modified/Deleted split it came from; a backend-reported overflow escalates
/// to `Overflow` (also a forced full rebuild — see `ChangeEvent`'s doc);
/// everything else is a workspace `.al` file (the only other kind
/// [`AlFileWatcher`] forwards — see its own widened filter) and maps 1:1 onto
/// `FileSaved`/`FileRemoved`.
fn to_change_event(change: FileChange) -> ChangeEvent {
    match change {
        FileChange::Overflow => ChangeEvent::Overflow,
        FileChange::Modified(path) => {
            if path_is_dependency_source(&path) {
                ChangeEvent::DepsChanged
            } else {
                ChangeEvent::FileSaved(path)
            }
        }
        FileChange::Deleted(path) => {
            if path_is_dependency_source(&path) {
                ChangeEvent::DepsChanged
            } else {
                ChangeEvent::FileRemoved(path)
            }
        }
    }
}

fn path_is_dependency_source(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(".alpackages"))
    })
}

/// Main message processing loop. Takes `connection` BY VALUE — see
/// `run_server`'s call site for why that (not `&Connection`) is required for
/// a clean process exit.
fn main_loop(connection: Connection, workspace: &Workspace) -> Result<()> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    break;
                }

                let result = dispatch_request(&req, workspace);
                let response = match result {
                    Ok(value) => Response::new_ok(req.id, value),
                    Err(e) => Response::new_err(
                        req.id,
                        lsp_server::ErrorCode::InternalError as i32,
                        e.to_string(),
                    ),
                };

                connection
                    .sender
                    .send(Message::Response(response))
                    .context("Failed to send response")?;
            }
            Message::Response(_) => {}
            Message::Notification(notif) => {
                handle_notification(workspace, &notif);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-root routing
// ---------------------------------------------------------------------------

/// Map an inbound `textDocument` URI to whichever configured root its path
/// falls under — the multi-root generalization of
/// [`lsp::handlers::resolve_virtual_path`](crate::lsp::handlers::resolve_virtual_path)'s
/// single-root `strip_prefix`. Longest-prefix match: a client CAN offer
/// nested roots (a workspace folder plus a sub-folder that's its own AL
/// app), and the more specific one must win. `None` for an unparsable/
/// non-`file://` uri, or one that falls under NO configured root at all.
fn route_uri<'a>(workspace: &'a Workspace, uri: &str) -> Option<&'a RootState> {
    let parsed: Uri = uri.parse().ok()?;
    let path = uri_to_path(&parsed)?;
    workspace
        .roots
        .iter()
        .filter(|r| path.starts_with(&r.root))
        .max_by_key(|r| r.root.as_os_str().len())
}

/// [`route_uri`] plus the "outside every configured root" warning the
/// fail-loud doctrine calls for — every uri-based request-routing call site
/// below funnels through this (never `route_uri` directly) so the warning
/// can't drift between handlers. Silent (no log) when `workspace.roots` is
/// EMPTY — that case already gets its own ONE-TIME startup warning
/// ([`build_workspace`]), and warning again on every request would just be
/// noise for a session that never had a workspace at all (matches the
/// pre-multi-root server exactly: no per-request log for the `state: None`
/// case).
fn route_uri_or_warn<'a>(
    workspace: &'a Workspace,
    method: &str,
    uri: &str,
) -> Option<&'a RootState> {
    let found = route_uri(workspace, uri);
    if found.is_none() && !workspace.roots.is_empty() {
        warn!(
            "{method}: {uri} is outside every configured workspace root ({} \
             configured) — returning an empty result",
            workspace.roots.len()
        );
    }
    found
}

/// JSON key this server stamps into every `CallHierarchyItem.data` it
/// mints, identifying which configured root produced it — see the module
/// doc's "Multi-root" section for why this is load-bearing, not cosmetic.
/// Only ever stamped when `workspace.is_multi_root()` — a single-folder
/// client's wire format stays byte-identical to the pre-multi-root server.
const ROOT_MARKER_KEY: &str = "__root";

/// Stamp `item.data` with [`ROOT_MARKER_KEY`] = `root` — a no-op if `data`
/// isn't a JSON object (every item this server mints sets `data` to an
/// object; see `lsp::handlers::build_item`/`abi_symbol_item`, so this is
/// purely defensive). Call sites gate this on `workspace.is_multi_root()`
/// via [`tag_item_root_gated`] — never call this directly.
fn tag_item_root(item: &mut CallHierarchyItem, root: &Path) {
    let Some(Value::Object(map)) = item.data.as_mut() else {
        return;
    };
    map.insert(
        ROOT_MARKER_KEY.to_string(),
        Value::String(root.to_string_lossy().into_owned()),
    );
}

/// [`tag_item_root`], gated on `workspace.is_multi_root()` — the single
/// call shape every response-minting arm in `dispatch_request` uses.
fn tag_item_root_gated(workspace: &Workspace, root: &Path, item: &mut CallHierarchyItem) {
    if workspace.is_multi_root() {
        tag_item_root(item, root);
    }
}

/// Route an `incomingCalls`/`outgoingCalls` request's `item` to the root
/// that minted it. Single-root: always that one root (matches pre-multi-root
/// behavior exactly — no marker is ever stamped, so this never even LOOKS
/// at `item.data`). Multi-root: prefers the [`ROOT_MARKER_KEY`] this server
/// stamps into every `CallHierarchyItem.data` it mints — REQUIRED for
/// correctness, not just a convenience: `RoutineNodeId`'s `AppRef`
/// (`src/program/node.rs`) is a raw index into that ROOT's OWN independent
/// `AppRegistry`, so the SAME `RoutineNodeId` VALUE can legitimately name a
/// DIFFERENT routine in a different root's snapshot (two roots each
/// interning their own primary app at index 0) — comparing `data.node`
/// against every root's `decl_by_id` to see which one "has it" would be
/// UNSOUND, capable of silently answering from the wrong app. URI-based
/// routing alone isn't even available for a dependency-embedded-source or
/// ABI-boundary item either: `lsp::handlers::decl_uri`/`abi_symbol_uri` mint
/// a synthetic `al-dep-source://`/`al-preview://` scheme, not `file://`, so
/// [`route_uri`] structurally can't resolve those. Falls back to
/// `route_uri(item.uri)` only when the marker is absent/unparsable (a
/// pre-multi-root client's replayed item, or one that doesn't round-trip
/// `data` verbatim — LSP requires it to, but fail soft here rather than
/// hard-erroring on a technically non-conformant client).
fn route_item<'a>(workspace: &'a Workspace, item: &CallHierarchyItem) -> Option<&'a RootState> {
    if !workspace.is_multi_root() {
        return workspace.roots.first();
    }
    if let Some(Value::Object(map)) = item.data.as_ref()
        && let Some(Value::String(root_str)) = map.get(ROOT_MARKER_KEY)
    {
        let root_path = PathBuf::from(root_str);
        if let Some(rs) = workspace.roots.iter().find(|r| r.root == root_path) {
            return Some(rs);
        }
    }
    route_uri(workspace, item.uri.as_str())
}

/// [`route_item`] plus the same "outside every configured root" warning
/// [`route_uri_or_warn`] gives uri-routed requests.
fn route_item_or_warn<'a>(
    workspace: &'a Workspace,
    method: &str,
    item: &CallHierarchyItem,
) -> Option<&'a RootState> {
    let found = route_item(workspace, item);
    if found.is_none() && !workspace.roots.is_empty() {
        warn!(
            "{method}: item '{}' (uri {}) doesn't match any configured workspace \
             root ({} configured) — returning an empty result",
            item.name,
            item.uri.as_str(),
            workspace.roots.len()
        );
    }
    found
}

/// `dependencyDocumentSymbol` is the one custom request whose `uri` (when
/// given at all — it also accepts bare `app`/`objectType`/`objectName`
/// fields, see `DependencyDocumentSymbolParams`) is a SYNTHETIC
/// `al-preview://` scheme identifying a DEPENDENCY object, never a
/// `file://` workspace document — [`route_uri`]'s path-prefix match
/// structurally cannot apply (`uri_to_path` returns `None` for any
/// non-`file://` scheme), and there is no per-root discriminator anywhere
/// in that scheme today. Single-root: unconditionally that one root,
/// unchanged. Multi-root: try `route_uri` first (handles the rare case of a
/// real `file://` uri); otherwise try every root's snapshot IN ORDER and
/// answer from the first NON-EMPTY result — correct whenever at most one
/// open root actually has a same-named dependency (the overwhelmingly
/// common case: distinct AL app roots rarely share a dependency 1:1), and a
/// graceful best-effort rather than an empty refusal in the rare case they
/// do. A future task closing this gap for real (embedding a root
/// discriminator in the `al-preview://` scheme itself) is tracked in the
/// module doc.
fn dispatch_dependency_document_symbol(
    workspace: &Workspace,
    params: DependencyDocumentSymbolParams,
) -> Vec<DependencyDocumentSymbol> {
    if !workspace.is_multi_root() {
        let Some(state) = workspace.roots.first().and_then(RootState::state) else {
            return Vec::new();
        };
        return dependency_document_symbol(&state.shared.get(), params);
    }
    if let Some(uri) = params.uri.as_deref()
        && let Some(rs) = route_uri(workspace, uri)
        && let Some(state) = rs.state()
    {
        return dependency_document_symbol(&state.shared.get(), params);
    }
    for root_state in &workspace.roots {
        let Some(state) = root_state.state() else {
            continue;
        };
        let result = dependency_document_symbol(&state.shared.get(), params.clone());
        if !result.is_empty() {
            return result;
        }
    }
    debug!(
        "dependencyDocumentSymbol: no configured root's snapshot has a matching \
         dependency object (uri {:?}, app {:?}) — returning an empty result",
        params.uri, params.app
    );
    Vec::new()
}

/// Extract a `CallHierarchyItem`'s `data` payload as an [`ItemData`] —
/// shared by `incomingCalls`/`outgoingCalls`, both of which need it to look
/// up `data.node` in the current snapshot.
fn item_data(item: &CallHierarchyItem) -> Result<ItemData> {
    let data = item
        .data
        .clone()
        .ok_or_else(|| anyhow::anyhow!("call hierarchy item is missing its data payload"))?;
    Ok(serde_json::from_value(data)?)
}

/// Dispatch one LSP request to its handler. Every snapshot-backed method
/// first routes `req`'s uri (or, for `incomingCalls`/`outgoingCalls`, its
/// `item`) to a [`RootState`] via [`route_uri_or_warn`]/[`route_item_or_warn`]
/// — a miss (no configured root matches) OR that root's own `state: None`
/// (no valid workspace snapshot there — see the module doc) both degrade to
/// an empty result, exactly the single-root "no valid workspace" fail path
/// generalized per root. `fieldProperties`/`actionProperties`/
/// `telemetryStatus` are graph-independent and answer unconditionally, with
/// no routing at all.
fn dispatch_request(req: &Request, workspace: &Workspace) -> Result<Value> {
    debug!("Request: {} - {:?}", req.method, req.params);

    match req.method.as_str() {
        "textDocument/prepareCallHierarchy" => {
            let params: CallHierarchyPrepareParams = serde_json::from_value(req.params.clone())?;
            let uri = params
                .text_document_position_params
                .text_document
                .uri
                .as_str();
            let Some(root_state) = route_uri_or_warn(workspace, &req.method, uri) else {
                return Ok(Value::Null);
            };
            let Some(state) = root_state.state() else {
                return Ok(Value::Null);
            };
            let snap = state.shared.get();
            let pos = params.text_document_position_params.position;
            let mut result = prepare(&snap, state.encoding, uri, pos.line, pos.character);
            if let Some(items) = result.as_mut() {
                for item in items {
                    tag_item_root_gated(workspace, &root_state.root, item);
                }
            }
            Ok(serde_json::to_value(result)?)
        }
        "callHierarchy/incomingCalls" => {
            let params: CallHierarchyIncomingCallsParams =
                serde_json::from_value(req.params.clone())?;
            let Some(root_state) = route_item_or_warn(workspace, &req.method, &params.item) else {
                return Ok(Value::Array(Vec::new()));
            };
            let Some(state) = root_state.state() else {
                return Ok(Value::Array(Vec::new()));
            };
            let snap = state.shared.get();
            let data = item_data(&params.item)?;
            let mut result = incoming(&snap, state.encoding, &data);
            for call in &mut result {
                tag_item_root_gated(workspace, &root_state.root, &mut call.from);
            }
            Ok(serde_json::to_value(result)?)
        }
        "callHierarchy/outgoingCalls" => {
            let params: CallHierarchyOutgoingCallsParams =
                serde_json::from_value(req.params.clone())?;
            let Some(root_state) = route_item_or_warn(workspace, &req.method, &params.item) else {
                return Ok(Value::Array(Vec::new()));
            };
            let Some(state) = root_state.state() else {
                return Ok(Value::Array(Vec::new()));
            };
            let snap = state.shared.get();
            let data = item_data(&params.item)?;
            let mut result = outgoing(&snap, state.encoding, &data);
            for call in &mut result {
                tag_item_root_gated(workspace, &root_state.root, &mut call.to);
            }
            Ok(serde_json::to_value(result)?)
        }
        "textDocument/codeLens" => {
            let params: CodeLensParams = serde_json::from_value(req.params.clone())?;
            let uri = params.text_document.uri.as_str();
            let Some(root_state) = route_uri_or_warn(workspace, &req.method, uri) else {
                return Ok(Value::Array(Vec::new()));
            };
            let Some(state) = root_state.state() else {
                return Ok(Value::Array(Vec::new()));
            };
            let snap = state.shared.get();
            let result = code_lenses(&snap, state.encoding, uri, &state.config);
            Ok(serde_json::to_value(result)?)
        }
        "al-call-hierarchy/fieldProperties" => {
            let params: SymbolPropertiesParams = serde_json::from_value(req.params.clone())?;
            Ok(serde_json::to_value(field_properties(params)?)?)
        }
        "al-call-hierarchy/actionProperties" => {
            let params: SymbolPropertiesParams = serde_json::from_value(req.params.clone())?;
            Ok(serde_json::to_value(action_properties(params)?)?)
        }
        "al-call-hierarchy/telemetryStatus" => {
            Ok(serde_json::to_value(crate::telemetry::status())?)
        }
        "al-call-hierarchy/dependencyDocumentSymbol" => {
            let params: DependencyDocumentSymbolParams =
                serde_json::from_value(req.params.clone())?;
            let result = dispatch_dependency_document_symbol(workspace, params);
            Ok(serde_json::to_value(result)?)
        }
        "al-call-hierarchy/eventPublishersInFile" => {
            let params: EventPublishersInFileParams = serde_json::from_value(req.params.clone())?;
            let Some(root_state) = route_uri_or_warn(workspace, &req.method, &params.uri) else {
                return Ok(Value::Array(Vec::new()));
            };
            let Some(state) = root_state.state() else {
                return Ok(Value::Array(Vec::new()));
            };
            let snap = state.shared.get();
            let result = event_publishers_in_file(&snap, state.encoding, &params.uri);
            Ok(serde_json::to_value(result)?)
        }
        "al-call-hierarchy/eventReferenceAtPosition" => {
            let params: EventReferenceAtPositionParams =
                serde_json::from_value(req.params.clone())?;
            let Some(root_state) = route_uri_or_warn(workspace, &req.method, &params.uri) else {
                return Ok(Value::Null);
            };
            let Some(state) = root_state.state() else {
                return Ok(Value::Null);
            };
            let snap = state.shared.get();
            let result =
                event_reference_at_position(&snap, state.encoding, &params.uri, params.position);
            Ok(serde_json::to_value(result)?)
        }
        _ => {
            debug!("Unhandled method: {}", req.method);
            Ok(Value::Null)
        }
    }
}

/// Handle an LSP notification. Only `didSave` does anything: it routes the
/// saved document's uri to its owning root ([`route_uri_or_warn`]) and
/// queues a [`ChangeEvent::FileSaved`] onto the SAME channel that root's
/// file watcher feeds (see [`start_file_watcher`]'s doc) —
/// `didOpen`/`didClose`/`didChange` are no-ops, mirroring legacy
/// (`text_document_sync.change` is negotiated as `NONE`, so the server never
/// relies on editor-buffer content anyway). `workspace/
/// didChangeWorkspaceFolders` is NOT implemented (see the module doc's
/// multi-root section for the real blocker) — logged loudly rather than
/// silently swallowed by the catch-all arm, so a dynamic add/remove is never
/// mistaken for having worked.
fn handle_notification(workspace: &Workspace, notif: &lsp_server::Notification) {
    debug!("Notification: {}", notif.method);

    match notif.method.as_str() {
        "textDocument/didSave" => {
            let Ok(params) =
                serde_json::from_value::<DidSaveTextDocumentParams>(notif.params.clone())
            else {
                return;
            };
            let uri = params.text_document.uri.as_str();
            let Some(root_state) = route_uri_or_warn(workspace, &notif.method, uri) else {
                return;
            };
            let Some(state) = root_state.state() else {
                return;
            };
            let Some(path) = uri_to_path(&params.text_document.uri) else {
                return;
            };
            if path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("al"))
                .unwrap_or(false)
            {
                debug!("Queueing save event for {}", path.display());
                if state.tx.send(ChangeEvent::FileSaved(path)).is_err() {
                    warn!("Updater channel closed; dropping didSave event");
                }
            }
        }
        "textDocument/didClose" => {}
        "textDocument/didOpen" => {}
        "textDocument/didChange" => {}
        "workspace/didChangeWorkspaceFolders" => {
            warn!(
                "workspace/didChangeWorkspaceFolders received but NOT implemented — \
                 dynamic add/remove of a workspace root requires a stop signal for \
                 AlFileWatcher's loop that doesn't exist yet (see the module doc's \
                 multi-root section and docs/OUTSTANDING.md); configured roots are \
                 fixed for the life of this session. Restart the server to pick up \
                 folder changes."
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// T3 Task 15 Step 3 smoke test. `tests/` has no existing LSP stdio harness
/// (`lsp_differential.rs`'s own doc notes `src/server.rs` is binary-only and
/// structurally unreachable from an integration-test crate) — `ServerState`/
/// `dispatch_request`/`handle_notification`/`build_server_state` are all
/// private to this module, so this lives here as an in-binary unit test
/// (`cargo test` runs these too) rather than under `tests/`.
///
/// Drives the REAL wiring this module assembles — `Connection::memory()`
/// stands in for stdio, `dispatch_request`/`handle_notification` are called
/// exactly as `main_loop` calls them — proving the cutover's plumbing, not
/// just the underlying `lsp::handlers`/`lsp::updater` functions in isolation
/// (already covered by Tasks 9-13's own tests).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::path_to_uri;
    use lsp_server::RequestId;
    use lsp_types::{CallHierarchyItem, CallHierarchyOutgoingCall};
    use std::time::Instant;

    fn write_fixture_workspace(dir: &Path) {
        std::fs::write(
            dir.join("app.json"),
            r#"{
    "id": "55555555-0000-0000-0000-000000000015",
    "name": "Task15 Server Cutover Fixture",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
        )
        .expect("write app.json");

        // `Extra` starts with zero callers (unused-procedure HINT expected
        // on the initial publish); the didSave edit below adds a call to it
        // from `DoWork`, which must clear that HINT on republish.
        std::fs::write(
            dir.join("Alpha.al"),
            r#"codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
    end;

    procedure Extra()
    begin
    end;
}
"#,
        )
        .expect("write Alpha.al");

        std::fs::write(
            dir.join("Beta.al"),
            r#"codeunit 50101 "Beta"
{
    procedure Process()
    begin
    end;
}
"#,
        )
        .expect("write Beta.al");

        // Gives `DoWork` a real caller so Alpha.al's ONLY initial finding is
        // `Extra()`'s unused hint — the didSave edit below clears exactly
        // that one, leaving Alpha.al with zero diagnostics (a clean
        // "republish clears to empty" assertion, not a partial one).
        std::fs::write(
            dir.join("Gamma.al"),
            r#"codeunit 50102 "Gamma"
{
    procedure Standalone()
    var
        Alpha: Codeunit "Alpha";
    begin
        Alpha.DoWork();
    end;
}
"#,
        )
        .expect("write Gamma.al");
    }

    #[test]
    fn dispatch_prepare_and_outgoing_then_didsave_bumps_generation_and_republishes_diagnostics() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture_workspace(dir.path());

        let (server_conn, client_conn) = Connection::memory();
        let sink = message_sink(&server_conn);
        let state = build_server_state(
            dir.path(),
            PositionEncoding::Utf8,
            DiagnosticConfig::default(),
            &sink,
        )
        .expect("build_server_state must succeed for a valid fixture workspace");
        let root = state.shared.get().workspace_root.as_path().to_path_buf();
        // Wrap the single built `ServerState` in a one-root `Workspace` — the
        // shape `dispatch_request`/`handle_notification` now require after
        // the multi-root refactor (see the module doc's multi-root section).
        // Rebinding `state` to a REFERENCE into `workspace.roots[0]`
        // immediately below means every `state.foo` access below this line
        // keeps working completely unchanged — this test's fixture,
        // requests, and assertions are otherwise byte-identical to the
        // pre-multi-root version; only this construction and the final
        // shutdown teardown actually differ.
        let workspace = Workspace {
            roots: vec![RootState::prebuilt(
                root,
                Some(state),
                RootBuild {
                    encoding: PositionEncoding::Utf8,
                    sink: Arc::clone(&sink),
                    start_watcher: false,
                    no_diagnostics: false,
                },
            )],
        };
        let state = workspace.roots[0].state().expect("just inserted above");
        assert_eq!(
            state.shared.get().generation,
            0,
            "a fresh build_full_with_parsed is generation 0"
        );

        let alpha_path = dir.path().join("Alpha.al");
        // Built from `LspSnapshot::workspace_root` (case-normalized on
        // Windows — see its own doc), the SAME construction
        // `diagnostics::workspace_uri` uses — NOT `path_to_uri(&alpha_path)`
        // directly, which would keep `dir.path()`'s on-disk casing and could
        // silently mismatch the snapshot's normalized root on a
        // case-insensitive filesystem.
        let alpha_uri = path_to_uri(&state.shared.get().workspace_root.join("Alpha.al"));

        // ── The initial publish must already flag Extra() as unused ───────
        let mut initial_alpha: Option<Vec<Diagnostic>> = None;
        {
            let deadline = Instant::now() + Duration::from_millis(1000);
            while initial_alpha.is_none() {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let Ok(msg) = client_conn.receiver.recv_timeout(remaining) else {
                    break;
                };
                if let Message::Notification(n) = msg
                    && n.method == "textDocument/publishDiagnostics"
                {
                    let params: PublishDiagnosticsParams =
                        serde_json::from_value(n.params).expect("valid publishDiagnostics params");
                    if params.uri.as_str() == alpha_uri.as_str() {
                        initial_alpha = Some(params.diagnostics);
                    }
                }
            }
        }
        let initial_alpha =
            initial_alpha.expect("the initial batch build must publish diagnostics for Alpha.al");
        assert!(
            initial_alpha
                .iter()
                .any(|d| d.message.contains("Extra") && d.message.contains("never called")),
            "Extra() has zero callers and must be flagged unused on the initial publish; got {initial_alpha:#?}"
        );

        // ── dispatch prepareCallHierarchy at Alpha.DoWork's name token ─────
        let do_work = {
            let snap = state.shared.get();
            snap.decls_by_file["Alpha.al"]
                .iter()
                .find(|d| d.name == "DoWork")
                .expect("DoWork decl")
                .clone()
        };
        let prepare_req = Request::new(
            RequestId::from(1),
            "textDocument/prepareCallHierarchy".to_string(),
            serde_json::json!({
                "textDocument": {"uri": alpha_uri.as_str()},
                "position": {
                    "line": do_work.name_origin.start.row,
                    "character": do_work.name_origin.start.column,
                },
            }),
        );
        let prepare_result =
            dispatch_request(&prepare_req, &workspace).expect("prepare dispatch must not error");
        let items: Vec<CallHierarchyItem> =
            serde_json::from_value(prepare_result).expect("prepare result deserializes");
        assert_eq!(items.len(), 1, "must resolve exactly Alpha.DoWork");
        assert_eq!(items[0].name, "DoWork");

        // ── dispatch outgoingCalls: Alpha.DoWork calls Beta.Process ────────
        let outgoing_req = Request::new(
            RequestId::from(2),
            "callHierarchy/outgoingCalls".to_string(),
            serde_json::json!({ "item": items[0] }),
        );
        let outgoing_result =
            dispatch_request(&outgoing_req, &workspace).expect("outgoing dispatch must not error");
        let outgoing: Vec<CallHierarchyOutgoingCall> =
            serde_json::from_value(outgoing_result).expect("outgoing result deserializes");
        assert!(
            outgoing.iter().any(|c| c.to.name == "Process"),
            "Alpha.DoWork's outgoing calls must include Beta.Process; got {outgoing:#?}"
        );

        // ── didSave: add a call to Extra() (still fingerprint-equal — rung
        // 1) — must swap in a new generation AND clear Extra()'s unused hint
        std::fs::write(
            &alpha_path,
            r#"codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Extra();
    end;

    procedure Extra()
    begin
    end;
}
"#,
        )
        .expect("rewrite Alpha.al");

        let did_save = Notification::new(
            "textDocument/didSave".to_string(),
            serde_json::json!({ "textDocument": {"uri": alpha_uri.as_str()} }),
        );
        handle_notification(&workspace, &did_save);

        let mut alpha_after: Option<Vec<Diagnostic>> = None;
        let deadline = Instant::now() + Duration::from_millis(1000);
        while alpha_after.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Ok(msg) = client_conn.receiver.recv_timeout(remaining) else {
                break;
            };
            if let Message::Notification(n) = msg
                && n.method == "textDocument/publishDiagnostics"
            {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(n.params).expect("valid publishDiagnostics params");
                if params.uri.as_str() == alpha_uri.as_str() {
                    alpha_after = Some(params.diagnostics);
                }
            }
        }
        let alpha_after = alpha_after.expect(
            "a didSave must eventually republish diagnostics for Alpha.al (Extra() clears)",
        );
        assert!(
            alpha_after.is_empty(),
            "Extra() is now called from DoWork — the unused-procedure hint must clear; got {alpha_after:#?}"
        );

        assert!(
            state.shared.get().generation > 0,
            "a didSave must bump the published generation"
        );

        let RootState {
            state: root_state, ..
        } = workspace
            .roots
            .into_iter()
            .next()
            .expect("exactly one configured root");
        let ServerState {
            tx, updater_handle, ..
        } = root_state
            .into_inner()
            .flatten()
            .expect("this root's snapshot build succeeded above");
        drop(tx);
        updater_handle
            .join()
            .expect("updater thread must exit cleanly");
    }

    // ── Multi-root tests (feat/multi-root-lsp) ─────────────────────────────
    //
    // Mirror the mechanism above: `Connection::memory()` stands in for
    // stdio, `dispatch_request`/`handle_notification` are called exactly as
    // `main_loop` calls them — but via `build_workspace` (the multi-root
    // entry point) rather than a single `build_server_state` call.

    fn join_all_roots(workspace: Workspace) {
        for root_state in workspace.roots {
            if let Some(st) = root_state.into_built() {
                drop(st.tx);
                st.updater_handle
                    .join()
                    .expect("updater thread must exit cleanly");
            }
        }
    }

    /// (a) Two fixture roots, each answering from its OWN app — and,
    /// crucially, never cross-talking even though they share an object
    /// number, object name, AND routine name (see the fixture-choice
    /// comment below for exactly why that's the load-bearing part of this
    /// test, not an incidental detail).
    #[test]
    fn multi_root_prepare_and_outgoing_route_to_the_correct_root_and_never_cross_talk() {
        // Root A: the existing inline fixture — Codeunit 50100 "Alpha" /
        // DoWork calling ONLY Beta.Process.
        let dir_a = tempfile::tempdir().expect("tempdir a");
        write_fixture_workspace(dir_a.path());

        // Root B: the COMMITTED `tests/fixtures/lsp-incr/` fixture, read
        // directly — copy-free, never mutated (`tests/lsp_incremental_parity.rs`
        // copies it elsewhere before editing; this test only ever reads it).
        // It ALSO declares `codeunit 50100 "Alpha"` with a `DoWork`
        // procedure — the EXACT same object number + object name + routine
        // name as root A, but calling Beta.Process/Calc(Integer)/
        // Calc(Text)/Løbenr instead. This is DELIBERATE: `RoutineNodeId.
        // object.app` is an `AppRef`, a raw index into EACH root's own
        // independent `AppRegistry` (`src/program/node.rs`) — two roots
        // whose primary app is each interned at index 0 can produce
        // BYTE-IDENTICAL `RoutineNodeId` values despite naming completely
        // different routines (see `route_item`'s doc). If routing ever
        // regressed to "search every root's snapshot for this id" instead
        // of using the stamped root marker, this fixture pair turns that
        // into a silent WRONG-ROOT answer instead of a loud failure.
        let dir_b = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp-incr");

        let (server_conn, _client_conn) = Connection::memory();
        let workspace = build_workspace(
            &[dir_a.path().to_path_buf(), dir_b],
            PositionEncoding::Utf8,
            &server_conn,
            false,
            false,
        );
        assert_eq!(workspace.roots.len(), 2);
        assert!(workspace.roots[0].state().is_some(), "root A must build");
        assert!(
            workspace.roots[1].state().is_some(),
            "root B (tests/fixtures/lsp-incr) must build"
        );
        assert!(workspace.is_multi_root());

        let state_a = workspace.roots[0].state().unwrap();
        let state_b = workspace.roots[1].state().unwrap();
        let alpha_a_uri = path_to_uri(&state_a.shared.get().workspace_root.join("Alpha.al"));
        let alpha_b_uri = path_to_uri(&state_b.shared.get().workspace_root.join("Alpha.al"));

        let do_work_a = state_a.shared.get().decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("root A DoWork decl")
            .clone();
        let do_work_b = state_b.shared.get().decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("root B DoWork decl")
            .clone();
        // Pin the collision PRECONDITION: both roots must mint byte-identical
        // RoutineNodeIds for their Alpha.DoWork — that identity collision is
        // exactly what makes the `__root` marker necessary. If fixture drift
        // ever breaks this, the test would silently stop exercising the
        // marker's reason for existing.
        assert_eq!(
            do_work_a.id, do_work_b.id,
            "fixture drift: the two roots no longer collide on RoutineNodeId — \
             re-align the fixtures so this test keeps proving marker-based routing"
        );

        let prepare_req_a = Request::new(
            RequestId::from(1),
            "textDocument/prepareCallHierarchy".to_string(),
            serde_json::json!({
                "textDocument": {"uri": alpha_a_uri.as_str()},
                "position": {
                    "line": do_work_a.name_origin.start.row,
                    "character": do_work_a.name_origin.start.column,
                },
            }),
        );
        let items_a: Vec<CallHierarchyItem> = serde_json::from_value(
            dispatch_request(&prepare_req_a, &workspace).expect("root A prepare must not error"),
        )
        .expect("prepare result deserializes");
        assert_eq!(items_a.len(), 1, "must resolve exactly root A's DoWork");
        assert_eq!(items_a[0].uri.as_str(), alpha_a_uri.as_str());

        let prepare_req_b = Request::new(
            RequestId::from(2),
            "textDocument/prepareCallHierarchy".to_string(),
            serde_json::json!({
                "textDocument": {"uri": alpha_b_uri.as_str()},
                "position": {
                    "line": do_work_b.name_origin.start.row,
                    "character": do_work_b.name_origin.start.column,
                },
            }),
        );
        let items_b: Vec<CallHierarchyItem> = serde_json::from_value(
            dispatch_request(&prepare_req_b, &workspace).expect("root B prepare must not error"),
        )
        .expect("prepare result deserializes");
        assert_eq!(items_b.len(), 1, "must resolve exactly root B's DoWork");
        assert_eq!(items_b[0].uri.as_str(), alpha_b_uri.as_str());

        // The stamped root marker is what makes routing sound here (see
        // `route_item`'s doc) — assert it's present and DISTINCT across
        // roots, not just that the end-to-end behavior happens to come out
        // right.
        let marker = |item: &CallHierarchyItem| -> String {
            item.data
                .as_ref()
                .and_then(|d| d.get(ROOT_MARKER_KEY))
                .and_then(|v| v.as_str())
                .expect("a multi-root item must carry the root marker")
                .to_string()
        };
        assert_ne!(
            marker(&items_a[0]),
            marker(&items_b[0]),
            "two roots' same-shaped DoWork items must carry DISTINCT root markers"
        );

        // ── outgoingCalls must reflect ONLY each root's own edges ─────────
        let outgoing_a: Vec<CallHierarchyOutgoingCall> = serde_json::from_value(
            dispatch_request(
                &Request::new(
                    RequestId::from(3),
                    "callHierarchy/outgoingCalls".to_string(),
                    serde_json::json!({ "item": items_a[0] }),
                ),
                &workspace,
            )
            .expect("root A outgoing must not error"),
        )
        .expect("outgoing result deserializes");
        let mut names_a: Vec<&str> = outgoing_a.iter().map(|c| c.to.name.as_str()).collect();
        names_a.sort_unstable();
        assert_eq!(
            names_a,
            vec!["Process"],
            "root A's DoWork must resolve ONLY root A's own Beta.Process — got {outgoing_a:#?}"
        );

        let outgoing_b: Vec<CallHierarchyOutgoingCall> = serde_json::from_value(
            dispatch_request(
                &Request::new(
                    RequestId::from(4),
                    "callHierarchy/outgoingCalls".to_string(),
                    serde_json::json!({ "item": items_b[0] }),
                ),
                &workspace,
            )
            .expect("root B outgoing must not error"),
        )
        .expect("outgoing result deserializes");
        let mut names_b: Vec<&str> = outgoing_b.iter().map(|c| c.to.name.as_str()).collect();
        names_b.sort_unstable();
        assert_eq!(
            names_b,
            vec!["Calc", "Calc", "Løbenr", "Process"],
            "root B's DoWork must resolve its OWN full call set — got {outgoing_b:#?}"
        );
        assert!(
            !names_b.contains(&"Extra"),
            "root B must never see root A's Extra() — cross-talk"
        );

        // ── incomingCalls on root A's Beta.Process must show root A's
        // DoWork as its ONLY caller — exercises the `.from`-tagging path
        // (outgoingCalls above only exercised `.to`).
        let process_item_a = &outgoing_a
            .iter()
            .find(|c| c.to.name == "Process")
            .expect("root A outgoing includes Process")
            .to;
        let incoming_calls: Vec<lsp_types::CallHierarchyIncomingCall> = serde_json::from_value(
            dispatch_request(
                &Request::new(
                    RequestId::from(5),
                    "callHierarchy/incomingCalls".to_string(),
                    serde_json::json!({ "item": process_item_a }),
                ),
                &workspace,
            )
            .expect("root A incoming must not error"),
        )
        .expect("incoming result deserializes");
        assert_eq!(
            incoming_calls.len(),
            1,
            "root A's Process has exactly ONE caller (DoWork)"
        );
        assert_eq!(incoming_calls[0].from.name, "DoWork");
        assert_eq!(incoming_calls[0].from.uri.as_str(), alpha_a_uri.as_str());

        join_all_roots(workspace);
    }

    /// (b) A `textDocument` uri that falls under NO configured root must
    /// degrade to the same graceful-empty result the server gives for "no
    /// workspace at all" — never an error, never a panic.
    #[test]
    fn multi_root_uri_outside_every_configured_root_degrades_to_graceful_empty() {
        let dir_a = tempfile::tempdir().expect("tempdir a");
        write_fixture_workspace(dir_a.path());
        let dir_b = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp-incr");

        let (server_conn, _client_conn) = Connection::memory();
        let workspace = build_workspace(
            &[dir_a.path().to_path_buf(), dir_b],
            PositionEncoding::Utf8,
            &server_conn,
            false,
            false,
        );
        assert_eq!(workspace.roots.len(), 2);

        // A directory that is genuinely NOT one of the two configured roots
        // — standing in for "a file the editor has open that isn't part of
        // any workspace folder."
        let dir_outside = tempfile::tempdir().expect("tempdir outside");
        let outside_uri = path_to_uri(&dir_outside.path().join("Somewhere.al"));

        let prepare_req = Request::new(
            RequestId::from(1),
            "textDocument/prepareCallHierarchy".to_string(),
            serde_json::json!({
                "textDocument": {"uri": outside_uri.as_str()},
                "position": {"line": 0, "character": 0},
            }),
        );
        let prepare_result =
            dispatch_request(&prepare_req, &workspace).expect("must degrade, never error");
        assert_eq!(
            prepare_result,
            Value::Null,
            "a uri outside every configured root must degrade to the same graceful-empty \
             result as no workspace at all"
        );

        let lens_req = Request::new(
            RequestId::from(2),
            "textDocument/codeLens".to_string(),
            serde_json::json!({ "textDocument": {"uri": outside_uri.as_str()} }),
        );
        let lens_result =
            dispatch_request(&lens_req, &workspace).expect("must degrade, never error");
        assert_eq!(lens_result, Value::Array(Vec::new()));

        // A didSave for the same out-of-root uri must be a silent no-op.
        let did_save = Notification::new(
            "textDocument/didSave".to_string(),
            serde_json::json!({ "textDocument": {"uri": outside_uri.as_str()} }),
        );
        handle_notification(&workspace, &did_save);

        join_all_roots(workspace);
    }

    /// (c) A second root that fails to build (no `app.json` at all — see
    /// the fixture comment below for why this, not a merely-incomplete
    /// `app.json`, is the real failure trigger) must NOT take down the
    /// first, good root.
    #[test]
    fn multi_root_broken_second_root_does_not_take_down_the_first_root() {
        let dir_a = tempfile::tempdir().expect("tempdir a");
        write_fixture_workspace(dir_a.path());

        let dir_broken = tempfile::tempdir().expect("tempdir broken");
        // Deliberately NO app.json at all: `SnapshotBuilder::build`'s
        // `std::fs::read_to_string(app.json)` fails outright — a real,
        // unambiguous build failure. NOT a merely-incomplete app.json (e.g.
        // one literally missing the "id" field): `SnapshotBuilder::build`'s
        // `get_str` helper falls back to an empty string for any missing
        // field, so THAT is not actually a build failure at all (verified
        // against the source before choosing this fixture shape).
        std::fs::write(dir_broken.path().join("Stray.al"), "// not an al app\n")
            .expect("write stray file");

        let (server_conn, _client_conn) = Connection::memory();
        let workspace = build_workspace(
            &[dir_a.path().to_path_buf(), dir_broken.path().to_path_buf()],
            PositionEncoding::Utf8,
            &server_conn,
            false,
            false,
        );
        assert_eq!(workspace.roots.len(), 2);
        assert!(
            workspace.roots[0].state().is_some(),
            "the good root must build"
        );
        assert!(
            workspace.roots[1].state().is_none(),
            "the broken root (no app.json) must fail to build, not panic"
        );

        // Root A must still fully serve prepareCallHierarchy exactly as
        // single-root.
        let alpha_uri = path_to_uri(
            &workspace.roots[0]
                .state()
                .unwrap()
                .shared
                .get()
                .workspace_root
                .join("Alpha.al"),
        );
        let do_work = workspace.roots[0]
            .state()
            .unwrap()
            .shared
            .get()
            .decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("DoWork decl")
            .clone();
        let prepare_req = Request::new(
            RequestId::from(1),
            "textDocument/prepareCallHierarchy".to_string(),
            serde_json::json!({
                "textDocument": {"uri": alpha_uri.as_str()},
                "position": {
                    "line": do_work.name_origin.start.row,
                    "character": do_work.name_origin.start.column,
                },
            }),
        );
        let items: Vec<CallHierarchyItem> = serde_json::from_value(
            dispatch_request(&prepare_req, &workspace).expect("root A prepare must not error"),
        )
        .expect("prepare result deserializes");
        assert_eq!(
            items.len(),
            1,
            "the good root must still serve prepareCallHierarchy normally"
        );
        assert_eq!(items[0].name, "DoWork");

        // A request for a file under the BROKEN root degrades gracefully
        // (empty, no panic) — it's a KNOWN root (routing finds it), just one
        // with no snapshot.
        let broken_uri = path_to_uri(&dir_broken.path().join("Stray.al"));
        let broken_req = Request::new(
            RequestId::from(2),
            "textDocument/prepareCallHierarchy".to_string(),
            serde_json::json!({
                "textDocument": {"uri": broken_uri.as_str()},
                "position": {"line": 0, "character": 0},
            }),
        );
        let broken_result = dispatch_request(&broken_req, &workspace)
            .expect("the broken root must degrade, not error");
        assert_eq!(broken_result, Value::Null);

        join_all_roots(workspace);
    }

    /// `configured_roots` is the behavior change from the pre-multi-root
    /// server: every folder is returned, in order, not just the first (the
    /// old `primary_workspace_root` used to warn-and-truncate here — see
    /// its doc's "History" section).
    #[test]
    fn configured_roots_returns_every_folder_not_just_the_first() {
        let dir_a = tempfile::tempdir().expect("tempdir a");
        let dir_b = tempfile::tempdir().expect("tempdir b");
        let params = InitializeParams {
            workspace_folders: Some(vec![
                lsp_types::WorkspaceFolder {
                    uri: path_to_uri(dir_a.path()),
                    name: "a".to_string(),
                },
                lsp_types::WorkspaceFolder {
                    uri: path_to_uri(dir_b.path()),
                    name: "b".to_string(),
                },
            ]),
            ..Default::default()
        };
        let roots = configured_roots(&params);
        assert_eq!(
            roots,
            vec![
                crate::protocol::normalize_path(dir_a.path()),
                crate::protocol::normalize_path(dir_b.path()),
            ],
            "every configured folder must be returned, in order — no longer just the first"
        );
        assert_eq!(primary_workspace_root(&params), Some(roots[0].clone()));
    }

    /// `configured_roots` dedupes a folder offered twice, and still falls
    /// back to the deprecated single `root_uri` when `workspace_folders` is
    /// absent (a pre-3.6 client) — both unchanged corners of the original
    /// single-root logic, now exercised directly as a pure function.
    #[test]
    #[allow(deprecated)] // exercising the deprecated `root_uri` fallback path
    fn configured_roots_dedupes_and_falls_back_to_root_uri() {
        let dir_a = tempfile::tempdir().expect("tempdir a");

        let dup_params = InitializeParams {
            workspace_folders: Some(vec![
                lsp_types::WorkspaceFolder {
                    uri: path_to_uri(dir_a.path()),
                    name: "a".to_string(),
                },
                lsp_types::WorkspaceFolder {
                    uri: path_to_uri(dir_a.path()),
                    name: "a-again".to_string(),
                },
            ]),
            ..Default::default()
        };
        assert_eq!(
            configured_roots(&dup_params).len(),
            1,
            "the same folder offered twice must not spin up two roots"
        );

        let legacy_params = InitializeParams {
            workspace_folders: None,
            root_uri: Some(path_to_uri(dir_a.path())),
            ..Default::default()
        };
        assert_eq!(
            configured_roots(&legacy_params),
            vec![crate::protocol::normalize_path(dir_a.path())],
            "a pre-3.6 client's root_uri must still work when workspace_folders is absent"
        );
    }

    /// Lazy per-root build (memory: a 33-root workspace built 33 full
    /// program snapshots at `initialize`, each retaining its own copy of
    /// the shared dependency source text — ~17 GB peak on a real customer
    /// workspace, for roots the session never touched). `build_workspace`
    /// must now build NOTHING; a root's snapshot is built on the first
    /// request that routes to it, and only for THAT root.
    #[test]
    fn build_workspace_builds_no_root_until_a_request_routes_to_one() {
        let dir_a = tempfile::tempdir().expect("tempdir a");
        write_fixture_workspace(dir_a.path());
        let dir_b = tempfile::tempdir().expect("tempdir b");
        write_fixture_workspace(dir_b.path());

        let (server_conn, _client_conn) = Connection::memory();
        let workspace = build_workspace(
            &[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
            PositionEncoding::Utf8,
            &server_conn,
            false,
            false,
        );

        assert_eq!(workspace.roots.len(), 2);
        assert!(
            workspace.roots.iter().all(|r| r.built().is_none()),
            "build_workspace must not build any root eagerly"
        );

        // Touching root A builds A — and ONLY A.
        assert!(
            workspace.roots[0].state().is_some(),
            "root A must build on first access"
        );
        assert!(
            workspace.roots[0].built().is_some(),
            "root A must now be cached as built"
        );
        assert!(
            workspace.roots[1].built().is_none(),
            "an untouched root must stay unbuilt"
        );

        join_all_roots(workspace);
    }

    /// Server-side diagnostics gate. The VS Code client disables code-quality
    /// diagnostics by DEFAULT and filters them away on arrival, so computing
    /// them is pure waste in the default configuration. When every detector
    /// is disabled the server must not compute or publish at all.
    #[test]
    fn all_detectors_disabled_publishes_no_diagnostics() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture_workspace(dir.path());
        std::fs::write(
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
        .expect("write .al-sem.json");

        let cfg = DiagnosticConfig::load(&crate::protocol::normalize_path(dir.path()));
        assert!(
            !cfg.any_enabled(),
            "fixture config must disable every detector"
        );

        let (server_conn, client_conn) = Connection::memory();
        let sink = message_sink(&server_conn);
        let state = build_server_state(dir.path(), PositionEncoding::Utf8, cfg, &sink)
            .expect("build must still succeed with diagnostics disabled");

        // The fixture's `Extra()` would otherwise raise an unused-procedure
        // hint on the initial publish (see `write_fixture_workspace`).
        assert!(
            client_conn.receiver.try_iter().next().is_none(),
            "no message may be published when every detector is disabled"
        );

        drop(state.tx);
        state
            .updater_handle
            .join()
            .expect("updater thread must exit cleanly");
    }
}
