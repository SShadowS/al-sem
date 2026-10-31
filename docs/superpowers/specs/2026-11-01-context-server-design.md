# A persistent context server for al-sem (requirements and design input)

Status: **measure + spec, revision 0** — design input for the next round (Fable designs,
an independent reviewer reviews). No engine code changed. Branch `spec/context-server`,
from master `47939dad`.

What this document is: the facts, the numbers, the constraints and the options, with a
recommendation that the design round is free to overturn. Every number below was measured
for this spec (§3 says how); nothing is quoted from an older document unless it says so.

## 0. Summary

AI agents (Claude Code and others) want to ask al-sem small questions about one routine
("what does `IssueReminder` do, who reaches it, can it commit?") many times in a session.
Today every question is a fresh process that rebuilds the whole model. On the CDO baseline
that is **5–9 s and 0.75–0.9 GB per question single-app, and 14–29 s and 3.4 GB per
question cross-app** (the default). Most of that time is the model build, which is the
same for every question about the same workspace revision.

A resident server that keeps the built model answers the next question in:

| Question (CDO) | Today, cold, cross-app | Warm, cross-app (measured as the batch increment) | Warm, single-app |
|---|---:|---:|---:|
| `query effects` / `query touches` | 14.4–14.6 s | **≈ 0 s** (4 queries cost the same as 1) | ≈ 0 s |
| `digest --routine` | 28.1 s | **≈ 0.4 s** | ≈ 0 s |
| `prove <r> may-commit` | 29.1 s | **≈ 1.4 s** | ≈ 0 s |
| `explain --routine` | 27.5 s | **≈ 4.2 s** (1.7–7.4 s by routine) | ≈ 0.4 s |

"Warm" here is what one more job costs inside one `alsem batch` run, which shares one
build — a real, measured lower bound on what a server could do with today's engine, minus
any server overhead. Two engine facts limit it, and the design must face both (§4):

1. **Explain's per-request work is not small cross-app** (~2–2.6 s whole-program snapshot
   derive + 0–4 s per-root witness reconstruction + 0.8 s card), because each explain
   builds its own whole-program "tree" (`TreeDemand { tests: true }` never shares,
   `src/engine/session.rs`).
2. **The engine has no incremental front end.** After any edit, the model is rebuilt
   from scratch: 5 s single-app, 14 s cross-app. The LSP's own incremental updater
   (rung 1: ~112 ms end to end) works on a different, lighter structure (`LspSnapshot`,
   `BuildProfile::LIGHT`) that does not contain the `Model` these commands read.

**Recommendation (option C, §6):** one shared, engine-side "workspace session" core that
owns a built view per workspace revision, plus two thin front ends: an MCP server over
stdio (new `alsem mcp`, the priority) and, later, LSP custom methods in the existing
`al-call-hierarchy` server. Answers carry an explicit revision marker; the default is
"wait for the current revision", with an opt-in "answer from the last built revision now".
Output is the CLI's JSON byte for byte, pinned by a contract test. Start single-workspace,
one build at a time, cross-app by default with a measured ~3.4 GB peak.

## 1. The problem

### 1.1 Who asks

| Client | How it talks | What it asks | What it cares about |
|---|---|---|---|
| Claude Code and other agents | MCP (Model Context Protocol) over stdio; the agent's host starts the server as a child process | explain / prove / digest / query about routines it is reading or editing; maybe `graph` for test selection | latency per question; answers that match the file it just edited; output size it can read |
| Editors (VS Code AL extension, others) | LSP (the existing `al-call-hierarchy` server) | call hierarchy, code lens, diagnostics today; possibly evidence cards on hover later | never block the editor; no memory blow-up |
| Scripts and CI | the CLI, `alsem batch` | everything | byte-stable output; already served by `batch` |

bc-dev-mcp (an external MCP server owned by the same team) already consumes `alsem graph`
output for test selection (`docs/superpowers/specs/2026-10-13-callgraph-export-design.md`).

### 1.2 What it costs today

CDO baseline (`/home/sshadows/Projects/DO-cdo-baseline/Cloud`, detached at `bc3ccb18`,
579 workspace `.al` files, 11 dependency `.app`s), `release-fast` build of `47939dad`,
one process per question, `/usr/bin/time -v`. The machine was shared (load average ~6 on
22 cores), so treat ±10 % as noise.

| Command | Single-app wall | Single-app peak RSS | Cross-app wall | Cross-app peak RSS | JSON out |
|---|---:|---:|---:|---:|---:|
| `explain --routine IssueReminder` | 5.71 s | 752 MB | 27.47 s | 3,399 MB | 157 KB / 308 KB |
| `explain --diff` (2 routines, real body edit) | 5.94 s | 888 MB | 26.85 s | 3,382 MB | 314 KB / 616 KB |
| `explain --table "CDO Queue Entry" --field "Language Code"` | 5.68 s | 768 MB | 23.35 s | 3,387 MB | 532 KB |
| `prove IssueReminder may-commit` | 8.95 s | 767 MB | 29.08 s | 3,350 MB | 23 KB / 2 KB |
| `digest --routine IssueReminder` | 6.24 s | 758 MB | 28.07 s | 3,368 MB | 2.8 MB / 5.6 MB |
| `digest --diff` (same diff) | 6.17 s | 878 MB | 27.97 s | 3,371 MB | 5.6 MB / 11.2 MB |
| `query effects --routine IssueReminder` | 5.37 s | 765 MB | 14.62 s | 3,364 MB | 626 KB / 670 KB |
| `query touches --table "CDO Queue Entry"` | 5.15 s | 761 MB | 14.36 s | 3,350 MB | 440 KB |

The brief's starting numbers (explain cross-app 27 s / 3.5 GB, single-app 5.8 s /
0.78 GB) are confirmed on a fresh build.

Note the output sizes: a single `digest` is 2.8–11 MB of JSON. An agent cannot read that
in its context. The MCP front end needs a size answer (§5.9).

## 2. Words

- **Workspace revision**: one frozen read of a workspace's files (`WorkspaceInputs`,
  `src/program/inputs.rs`) and everything computed from it. Two requests on the same
  revision see the same model.
- **Model / target**: the program-backed `Model` (`src/program/model/workspace.rs`)
  wrapped as `AnalysisTarget::{SingleApp, CrossApp}` (`src/engine/gate/run.rs:190`).
  Cross-app it sits inside `R3a5CrossAppBase` with the execution relation.
- **View**: `src/engine/session.rs`'s `View`: a target plus cells (`rel`, `caps`, `raw`,
  `tree`, `l4`, `det`, `rules`, `ordering`, …), each filled at most once by
  `View::prepare` from a `Plan`, and *released* after use. "A released cell is never
  refilled."
- **Universe**: single-app (`--single-app`, the workspace alone) or cross-app (the
  default: the workspace plus the dependency code it needs, `BuildProfile::FULL`).
- **Warm cost**: the time to answer one more question once the model for the current
  revision is built.
- **MCP**: Model Context Protocol — JSON-RPC 2.0 between an agent host (client) and a
  tool server; a server exposes *tools* with JSON-Schema inputs.

## 3. How it was measured

All measurement scripts lived in `/tmp/ctxsrv/` and are not committed; this section is
enough to rerun them.

- Binaries: `cargo build --profile release-fast --bin alsem --bin al-call-hierarchy`
  into a private `CARGO_TARGET_DIR`, at `47939dad`.
- Per command: `/usr/bin/time -v` for wall and peak RSS; `ALSEM_TRACE=1` with
  `ALSEM_TRACE_FILE` for the stage split (the trace's own RSS fields are Windows-only and
  read 0 on Linux).
- "Warm" proxy: `alsem batch` manifests with 1, 2, 4 or 8 jobs on the same workspace and
  universe. They share one view (`src/engine/batch/mod.rs`, "Jobs on one workspace and
  universe share one view"). Increment per extra job = (wall(N) − wall(1)) / (N − 1).
- Batch output equals standalone output byte for byte (`cmp` on explain single/cross,
  prove cross, query single): the batch driver's existing contract, and the basis of the
  contract test in §8.
- LSP: a small Python LSP client drove `al-call-hierarchy --no-watcher --no-telemetry
  --verbose` on a throwaway copy of the CDO workspace (`cp -a`), sending `initialize`,
  one `prepareCallHierarchy` (the first request triggers the root's build), then edits +
  `didSave`, polling `callHierarchy/incomingCalls` until the new caller appeared. RSS from
  `/proc/<pid>/status`.
- The small diff: two one-token body edits in `CDO Issue Document` (`IssueReminder` and
  `IssueFinanceChargeMemo`), as a unified diff relative to the workspace root.

## 4. Where the time goes

### 4.1 Model build vs. command work (one cold process)

Single-app (trace spans, `explain --routine`, typical of all eight):

| Stage | ms |
|---|---:|
| snapshot build (read files, identify apps) | 331 |
| parse | 1,240 |
| dependency layer | 1,068 |
| assemble graph | 333 |
| resolve (full program) | 1,572 |
| model assembly + call adapter + events | ~240 |
| **model build total** | **≈ 4,900** |

Cross-app (`crossapp.base_total`, 14.1–14.4 s in every command):

| Stage | ms |
|---|---:|
| program build (snapshot, parse, dep layer, graph, resolve) | 4,992 |
| resolve dependency bodies | 4,349 |
| demand | 148 |
| model assembly | 2,208 |
| attach program calls (adapter, events, drops) | 1,993 |
| relation build | 279 |
| base facts | 299 |
| **model build total** | **≈ 14,400** |

Command work after the build (cold, one job):

| Command | Single-app after build | Cross-app after build | What it is |
|---|---:|---:|---|
| query effects / touches | 0.15–0.2 s | 0.36 s | L4 summaries (`context.compute_summaries`), then the query |
| explain | ~0.7 s | 9–12 s | cone composition (0.21 / 5.5–5.9 s) + snapshot derive (0.26 / 1.9–2.7 s) + per-root witness (0.22 / 0–3 s) + card (0.85 s cross) |
| digest | ~1.0 s | ~13.7 s | detector context (cone, tree, roots, tx spans, summaries) + default detector run (~0.16 s) |
| prove | ~3.7 s | ~14.4 s | as digest, plus ~2.8 s (single) / ~4 s (cross) untraced inside the context build. By elimination it is prove's own tree read (`gaps_in_cone`), the only plan difference from digest; not confirmed |

### 4.2 What one more question costs on a built view (batch increments)

| Batch | Single-app wall | Cross-app wall | Increment per extra job |
|---|---:|---:|---|
| 1 explain | 5.78 s | 27.16 s | — |
| 2 explain | 6.38 s | 34.55 s | single +0.60 s; cross +7.4 s (DocHandle: witness 4.2 s) |
| 8 explain | 8.56 s | 56.70 s | single **+0.40 s**; cross **+4.2 s** |
| 1 prove | 9.17 s | 29.18 s | — |
| 4 prove | 9.04 s | 33.43 s | single ≈ 0; cross **+1.4 s** |
| 4 digest | 6.24 s (1 digest: 6.24 s standalone) | 29.28 s (1: 28.07 s) | single ≈ 0; cross **+0.4 s** |
| 4 query effects | 5.40 s (1: 5.37 s) | 14.60 s (1: 14.62 s) | **≈ 0** both |
| mixed 6 (explain, prove, digest, query effects, explain field, query touches) | 10.33 s (sum of standalone: 37.1 s) | 45.32 s (sum: 137 s) | — |

Why explain does not get cheap cross-app: each explain job is its own tree
(`explain_plan` sets `tests: true`, and the tree-sharing rule G16 in `View::prepare`
shares only `Effects` readers *without* `--tests`). "Each tree is a whole-program
snapshot" (the comment at the G16 site), so every explain pays a snapshot derive
(1.7–2.6 s cross-app) and its own per-root witness reconstruction (`digest.roots`,
0–4.2 s, depends on how much code the routine reaches). Prove and digest share one tree
for all their roots, so they get cheap.

Peak RSS did not grow with the number of jobs (cross-app 3,352–3,402 MB for 1–8 jobs;
single-app 743–905 MB).

### 4.3 What the resident LSP server holds today

`al-call-hierarchy` on the CDO copy, no `--l4-host`:

| Measure | Value |
|---|---:|
| First request (lazy root build) | 4.40 s |
| RSS after the build / after 15 s idle | 666 / 673 MB (peak 699 MB) |
| `incomingCalls` median (warm) | 0.26 ms |
| Body edit (new local + call) + `didSave` → visible in `incomingCalls` | 112 ms (includes the 100 ms debounce) |
| New procedure (signature-level edit) + `didSave` → visible | 1,319 ms |
| RSS after both edits | 709 MB |

With `--l4-host`: see §4.4.

What it holds (`src/lsp/snapshot.rs`, `LspSnapshot`): the program graph
(`Arc<ProgramGraph>`), the dependency layer, the workspace parse (`ParsedFileEntry`
per file), per-file call edges, event edges, and a declaration surface. Built with
`BuildProfile::LIGHT`: **dependency bodies are summarized and dropped** during the parse.

What it does **not** hold, and what the CLI commands need:

| Needed by explain / prove / digest / query | In the LSP today? |
|---|---|
| the `Model` (`attach_program_calls`) | no — the LSP never builds one |
| `ViewNeeds::rules`, `ViewNeeds::card_text` (explain; built at model assembly from the IR) | no |
| dependency bodies (cross-app needs `FULL`) | no — `LIGHT` drops them |
| execution relation, cone composition, L4 summaries, detector context | no (only the hidden `--l4-host` builds `rel` + `l4`, in its own throwaway build, §4.4) |

So "add explain to the LSP" is not "add a handler": it is a second engine build inside the
server process, at the CLI's cost and memory. The cheap structure the LSP keeps current
in 112 ms is not what these commands read.

### 4.4 Incremental update cost for what would back the server

- **LSP rungs** (above): 112 ms body edit, 1.3 s signature edit, on `LspSnapshot` only.
  They do not produce a `Model`.
- **The L4 host** (`--l4-host`, #222, `src/engine/incremental.rs`): per revision it
  "rebuilds the front end fresh (inputs, program, model, relation)" and reuses only the
  L4 solve per SCC. It keeps "no model, program or relation" between revisions. Its
  spec's non-goals: "No incremental front end", "No incremental det, cone, caps, witness
  pass or runs". Measured on CDO in this run: the table at the end of this section.
- **What forces a full rebuild even in the L4 host**: adding, removing or renaming an
  `.al` file (every routine id changes with the file list), any `app.json` change, any
  `.app` change in `.alpackages`.

Consequence: with today's engine, the freshness cost of a server answer after an edit is
**one full model build: ~5 s single-app, ~14 s cross-app**, plus the command's warm cost.
An incremental `Model` is a separate, large project (the epic's step-13 note and #222
both deferred it); this spec does not assume it.

L4-host measurements (CDO copy, `--l4-host`):

| Measure | Value |
|---|---:|
| LSP root build (first request) | 4.28 s |
| First host refresh published (Full, no baseline), after the root build | 19.2 s |
| Body edit + `didSave` → visible in `incomingCalls` (LSP rung) | 110 ms |
| Body edit → host publishes (`Incremental, seeded 33202 of 33285` SCCs) | 18.7 s |
| Signature edit → visible in `incomingCalls` | 1,584 ms |
| Signature edit → host publishes (generation 4, seeded 33202 of 33286; it queued behind the revert's generation 3) | 34.6 s |
| RSS: after the LSP build / after the first host refresh / after both edits | 719 MB / 3,144–3,204 MB / 3,464 MB |
| Peak RSS (VmHWM) over the run | 4,252 MB |

Read: even when only 83 of 33,285 SCCs are re-solved, a refresh takes ~19 s, because the
front end (cross-app model, 14 s) is rebuilt every time. And the long-lived process kept
**3.1–3.5 GB resident** although the host keeps only the L4 baseline between revisions —
compare 1.34 GB resident right after the same build in an `alsem` process (§5.6). One
known difference: `alsem` uses mimalloc with `purge_delay = 0` (`src/bin/alsem.rs`),
while `al-call-hierarchy` keeps the platform allocator (glibc here), which returns freed
memory to the OS less readily. Whether the gap is the allocator, retained data, or both
was not separated. A server design must measure resident memory in the long-lived server
process itself, not infer it from CLI runs.

## 5. Requirements

Each requirement has an id so the design and the review can refer to it.

### 5.1 Questions served

- **R1.** Serve `explain` (all three modes: `--routine`, `--diff`, `--table/--field`),
  `prove` (every question kind), `digest` (`--routine`, `--file`, `--diff`, `--tests`,
  `--include-reads`) and `query` (`effects`, `touches`), with the same arguments and
  defaults as the CLI.
- **R2.** `graph` is optional. It reads the cross-app *program*, not the model
  (`ViewNeeds::GRAPH`), and its output is a whole-program JSONL file. If served, it
  should write to a path (as `--out`) rather than return the stream.
- **R3.** Out of scope for the first version: `analyze`, `fingerprint`, `diff`,
  `events`, `policy`. They are whole-workspace reports, not per-routine questions, and
  `batch` already serves them. The core must not make adding them later hard.
- **R4.** A cheap "status" question: which workspaces are loaded, the revision of each,
  whether a rebuild is running, its phase, and the memory in use.

### 5.2 Output identity

- **R5.** For the same question on the same workspace revision, the server's answer is
  **byte-identical** to the CLI's stdout (with `--deterministic` where the command has
  it, and `--format json`). Pinned by a contract test (§8). Exit-code semantics map to a
  field (`exitCode`), not to a protocol error: "no routine matched" (exit 2) is an
  answer.
- **R6.** Diagnostics a CLI run prints on stderr (job lines) are returned too, as a
  separate field, never mixed into the JSON.

### 5.3 Freshness after edits

- **R7.** Every answer names the revision it was computed from: a monotonically
  increasing `generation` per workspace, plus a content stamp of the inputs (the same
  idea as the goldens' `workspace_git_sha` / `dependency_closure_sha256`, but over the
  frozen `WorkspaceInputs`).
- **R8.** Default mode **"fresh"**: before answering, the server checks the workspace
  (file stamps) and, if anything changed since the built revision, rebuilds and answers
  from the new revision. The caller waits (5 s / 14 s on CDO).
- **R9.** Opt-in mode **"latest-built"**: answer immediately from the last built revision
  and say so (`stale: true`, the changed paths it knows about, the generation being
  built). Agents doing exploratory reading can take it; agents checking their own edit
  must not.
- **R10.** Change detection must not depend on the client telling the server: agents
  edit files with their own tools and do not send LSP `didSave`. A stat-based check per
  request (579 + 11 files on CDO — cheap) or a file watcher (the LSP already has one,
  `src/watcher.rs`) is required. Unsaved editor buffers are out of scope (the LSP also
  ignores `didChange`).
- **R11.** Optional, later: a background rebuild on change (like the L4 host), so a
  "fresh" request often finds the build already done.

### 5.4 Concurrency and cancellation

- **R12.** Many requests may arrive while one is running. Questions on the same revision
  run concurrently or are queued; they never trigger a second build of the same
  revision.
- **R13.** At most one model build per workspace at a time (memory, §5.6). A new
  revision supersedes a running build of an older one (cancel between phases, as the L4
  host already does with `L4Control`/`cancel`).
- **R14.** Requests can be cancelled (MCP `notifications/cancelled`, LSP
  `$/cancelRequest`). A cancelled request stops its per-request work; it does not cancel
  a shared build that other requests wait for.
- **R15.** The LSP front end must never block call-hierarchy or diagnostics behind a
  multi-second analysis request. Today `main_loop` (`src/server.rs`) dispatches requests
  one at a time on one thread, so this needs a worker.

### 5.5 Workspaces

- **R16.** One server may hold several workspaces (one per root). First version: one
  workspace per MCP server process is acceptable, because the agent host starts one
  server per project. The core's API should still key everything by workspace path.
- **R17.** Single-app and cross-app are separate views of one workspace; a server may
  hold both (≈ 0.75 + 3.4 GB peak on CDO) or only the one asked for. Default: build
  what the first question asks for (cross-app by CLI default).

### 5.6 Memory

- **R18.** Peak and resident memory per workspace must be stated and bounded. Measured
  on CDO: cross-app peak 3.35–3.40 GB for every served command, single-app 0.74–0.91 GB;
  peak did not grow with 8 jobs on one view. The peak is reached during
  `attach_program_calls`, before a ~1.5 GB drop (epic 2026-10-18 §3). Sampled VmRSS
  every 50 ms in a CLI process (this run):

  | | Single-app | Cross-app |
  |---|---:|---:|
  | build peak | 762 MB | 3,362–3,365 MB |
  | RSS right after the build (what a view retains) | 278 MB | 1,344–1,374 MB |
  | during an explain on it (cone, derive, witness) | up to 612 MB | up to 2,296 MB |
  | during a query on it | — | up to 1,395 MB |

  But in the long-lived LSP process with `--l4-host` the same build left **3.1–3.5 GB
  resident** (§4.4). The design must state which number it budgets for and measure it in
  the server itself.
- **R19.** A rebuild must not double memory: build the new revision, then drop the old
  one; or drop first when "fresh" is the only mode in use. Building next to a live
  revision would peak at ~6.8 GB cross-app on CDO.
- **R20.** A configurable memory budget; when over it, drop idle views (LRU) and rebuild
  on demand. An idle timeout that frees everything after a quiet spell (agent sessions
  are bursty).
- **R21.** Must not use the full-model cone composition (`WitnessScope::All`) to make
  explain cheap: that is the 46.4 GB path the cone-memory spec
  (`2026-10-31-cone-memory-design.md`) is still shrinking.

### 5.7 Startup and crash behaviour

- **R22.** The server starts and answers its protocol handshake at once (MCP
  `initialize`, `tools/list`) without building anything; the first question pays the
  build. Optional: pre-build when started with a workspace argument.
- **R23.** A panic inside one request must not kill the server or corrupt the shared
  view: the per-request work runs on its own thread (or under `catch_unwind`) over
  read-only shared state. A panic inside a *build* drops that build and reports an error
  to every waiting request; the next request retries. (Contrast: the L4 host aborts the
  whole LSP process on a panic, by design for #222.)
- **R24.** If the server dies, the agent host restarts it (MCP stdio servers are child
  processes). Nothing persists, so a restart is a cold start (≈ 5 s / 14 s on CDO).
- **R25.** stdout is the protocol channel: nothing else may write to it (the engine's
  job lines go to stderr today; the LSP's spawned-binary test already guards stdout).

### 5.8 Security

- **R26.** No network listener by default. MCP over **stdio** only in the first version.
- **R27.** If a socket transport is added later (to share one server between several
  agents): a Unix domain socket / Windows named pipe under the user's state directory
  (`~/.al-sem/`, see `src/state_paths.rs`), owner-only permissions; never TCP on
  `0.0.0.0`. MCP's Streamable HTTP transport only on `127.0.0.1` and only behind a flag.
- **R28.** The server reads only inside the workspace roots it was given and their
  `.alpackages` closure, and writes nothing except files the caller names (`out` paths,
  which must be inside the workspace or a temp dir). Paths in requests are canonicalized
  and checked against the roots.
- **R29.** Telemetry follows the CLI's rules (`--no-telemetry`, `docs/telemetry.md`); no
  source text in telemetry.

### 5.9 Output size for agents

- **R30.** The full CLI JSON is always available (byte-identical, R5), but an MCP tool
  must not push 5–11 MB into an agent's context by default. Options for the design: an
  `out` path (the CLI already has `--out`) with the tool returning a short summary and the
  path; a server-side size cap that returns the summary plus a resource link; or narrower
  projections (one card, top N effects). The contract test pins the full form; any
  summary is derived from it.

## 6. Options

### 6.A Custom requests on the existing LSP server

Add `al-call-hierarchy/explain`, `…/prove`, `…/digest`, `…/query` to `dispatch_request`
in `src/server.rs`. Each would build (or reuse) a `View` next to the `LspSnapshot`.

- For: one resident process per workspace for editor users; reuses the watcher, the
  multi-root routing and the updater's change events.
- Against: the LSP holds none of what these commands need (§4.3), so the server would
  carry two engines: the `LIGHT` snapshot (0.7 GB) and a `FULL` model view (3.4 GB peak
  cross-app) — about 4 GB per editor window, paid by every editor user who never asks.
  The main loop is single-threaded, so a 4–14 s request blocks call hierarchy unless a
  worker pool is added. Agents do not speak LSP; Claude Code reaches LSP servers only
  through its own LSP plugin surface, not as general tools. The method names must start
  with the frozen `al-call-hierarchy/` prefix, tying a new product surface to the legacy
  name.
- Warm latency: as §4.2. Memory: ~0.7 GB + 0.75 GB (single) or + 3.4 GB peak (cross).

### 6.B A separate `alsem serve` process (MCP only)

A new `alsem` subcommand that holds views and speaks MCP over stdio. No LSP changes.

- For: smallest change to existing behaviour; memory only for users who start it; MCP is
  the protocol agents use; the CLI binary already contains every command.
- Against: the editor does not benefit; if later the LSP also wants evidence cards, a
  second implementation of "hold a view and answer" would appear unless the core is
  shared — which is option C.

### 6.C One shared session core, two thin front ends (recommended)

A library module (working name `engine::server` or `engine::resident`) that owns, per
workspace and universe: the frozen inputs of the last built revision, the built
`AnalysisTarget`, the request-independent cells (`rel`, `l4`, `rules`, ordering), a build
state machine (idle / building generation g / failed), and a request runner. Front ends:

1. `alsem mcp` (stdio MCP server): first.
2. LSP custom methods on `al-call-hierarchy`, using the same core on a worker thread:
   later, and only if editors ask. Method names under `al-call-hierarchy/` (frozen
   prefix), e.g. `al-call-hierarchy/explain`.

- For: one engine path for every consumer (the project's "one engine, two consumers"
  rule); the batch driver already proves that many jobs can run over one view with
  byte-identical output; the MCP front end ships without touching the editor.
- Against: the session core needs the `View` lifecycle changed (below), which is real
  engine work in a module with many invariants.

**The engine change C needs (and B too).** `View` today is a one-shot: `View::new` takes
the target by value, `prepare` fills cells once from the union of a *known* job list and
releases them ("a released cell is never refilled"), `ViewCx` hands `compute` a
`&mut View`. A server does not know the next request. The design must choose:

- **C1. Re-run the batch path per request over a retained target.** Keep the
  `AnalysisTarget` (behind `Arc`) between requests; per request make a fresh `View`
  over it and run the job's `resolve` → `prepare` → `compute` (the steps
  `run_on_target`, `src/engine/batch/mod.rs`, already shows for tests). Warm cost =
  exactly the batch increments of §4.2 *plus* re-filling `rel`/`l4`/cone composition per
  request unless those are cached too (cross-app `rel` is already an `Arc` in the base;
  the cone composition is 5.5–7 s cross-app and is the next thing to keep). Smallest
  change; concurrency = one view per request over a shared read-only target (needs
  `View` to borrow or `Arc` the target).
- **C2. A long-lived view with cells that are never released** and per-request cells
  (witness raw, tree, effects) kept per request. Faster (no recomposition), but it breaks
  the release discipline that keeps today's CLI peaks where they are, and every cell's
  "filled once" assert becomes a cache-invalidation rule.
- **C3. Micro-batching**: queue requests that arrive within a short window and run them
  as one batch (`Plan::merge`), which is exactly today's `drive`. Cheapest to build,
  natural for "an agent fires 5 explains in parallel", but the first request in a window
  waits for the window and the rest.

Recommendation: **C1 with the cone composition (`caps` derived part) and `l4` cached per
revision**, measured before committing; C3 as an optimization on top for bursts. This
keeps the CLI path (and its goldens) the single source of truth.

### 6.D Other options considered

- **D1. Persistent on-disk cache instead of a server** (the dependency pack cache spec,
  2026-08-07): cuts the cold build for *dependencies* (cross-app's 4.3 s dependency bodies
  + part of the 5 s program build) for every CLI run, including CI. It does not remove the
  per-request rebuild of the workspace model. Complementary, not a replacement; the
  epic's non-goal "no persistent cache" would have to be lifted.
- **D2. Agents call `alsem batch`** with all their questions at once. Works today, no
  server. Agents rarely know all questions up front; each batch still pays the build.
- **D3. Fork-server / pre-built process image** (build once, `fork` per request on
  Linux). Fast and isolated, but not portable (Windows is the owner's main platform:
  `U:/Git/...` paths), and fork + rayon thread pools is unsafe. Rejected.

### 6.E Estimated warm latency and memory per option (CDO)

From §4.2 (batch increments) and §4.3; "server overhead" (JSON-RPC, file-stamp check of
~590 files) is assumed ≤ 50 ms and is not measured.

| Option | Warm query | Warm digest | Warm prove | Warm explain | After an edit ("fresh") | Resident memory |
|---|---:|---:|---:|---:|---|---|
| A (LSP + model) | ≈ 0 / ≈ 0 | ≈ 0 / 0.4 s | ≈ 0 / 1.4 s | 0.4 / 4.2 s | + 5 s / 14 s rebuild | 0.7 GB + model (≤ 0.9 / ≤ 3.4 GB peak) per editor window |
| B or C1 (no cell cache) | ≈ 0.2 / 0.4 s | ~1 / ~14 s | ~4 / ~15 s | ~0.7 / ~12 s | + 5 s / 14 s | idle 0.28 / 1.35 GB (CLI-measured; 3.1+ GB seen in a long-lived process), 0.6 / 2.3 GB during a request, 0.76 / 3.4 GB during a build — only when started |
| C1 + cached `l4` and cone composition | ≈ 0 / ≈ 0 | ≈ 0 / 0.4 s | ≈ 0 / 1.4 s | 0.4 / 4.2 s | + 5 s / 14 s (+ cone 5.5–7 s cross) | as above, plus the cached composition (not measured; the cone-memory spec's numbers apply) |
| C3 micro-batch | as C1 + window | | | | | |

(single-app / cross-app in each cell.)

The honest headline: **a server makes query/digest/prove near-instant and single-app
explain sub-second; cross-app explain stays at ~2–7 s per routine** until explain's tree
is shared or made subject-scoped (an engine follow-up, §10 Q4).

## 7. MCP front end

### 7.1 Protocol and transport

- MCP revisions seen in the Rust SDK available locally (`rmcp` 3.7.0 in the cargo
  registry, not a dependency of al-sem): `2024-11-05`, `2025-03-26`, `2025-06-18`,
  `2025-11-25` (the newest with an `initialize` handshake), and `2026-07-28` (replaces the
  handshake with per-request `_meta`, SEP-2567; standard HTTP headers, SEP-2243).
- Transport: **stdio** (newline-delimited JSON-RPC 2.0 on stdin/stdout, logs on stderr).
- Version policy: answer `initialize` with `2025-11-25` (or the client's version if
  older and supported); accept `2026-07-28`-style requests without a handshake if the
  client sends them. Which revision Claude Code sends today is an open question (§10).

### 7.2 SDK or hand-rolled

| | `rmcp` (official Rust SDK) | Hand-rolled JSON-RPC over stdio |
|---|---|---|
| In `Cargo.lock` today | no | uses `serde_json` (present) |
| Runtime | async, `tokio` (al-sem has `tokio` only as an optional telemetry dependency with `rt`, `macros`, `time`, `sync`) | plain threads, like `lsp-server` |
| Extra crates | default features pull `schemars`, `futures`, `async-trait`, `tokio-util`, macros; HTTP/OAuth crates only behind features | none |
| MSRV / edition | Rust 1.88, edition 2024 (al-sem: edition 2024, toolchain 1.96 — fine) | — |
| Protocol churn | tracks spec revisions (it already knows `2026-07-28`) | we track it ourselves |
| Fit with the engine | engine work is CPU-bound rayon; must `spawn_blocking` every call | natural |

The server needs four MCP methods: `initialize`, `tools/list`, `tools/call`,
`notifications/cancelled` (plus `ping`). The existing `lsp-server` crate (0.10, in the
lockfile) already implements stdio JSON-RPC framing for LSP, but MCP uses
newline-delimited JSON, not `Content-Length` headers, so it cannot be reused as-is.

Recommendation: **hand-rolled, thin, synchronous** (a few hundred lines: a reader thread,
a dispatcher, a writer), with the protocol types in one module and a conformance test
against recorded client transcripts. Revisit `rmcp` if the server later needs HTTP
transport, resources, or sampling. This is a judgment call for the design round, not a
measured result.

### 7.3 Tools

Names use an `alsem_` prefix (agents see tools from many servers side by side). Every
tool takes `workspace` (absolute path; may default to the server's start argument) and
`freshness` (`"fresh"` default, or `"latest-built"`), and `singleApp` (bool, default
false). Every result is:

```json
{
  "exitCode": 0,
  "generation": 7,
  "inputsStamp": "blake3:…",
  "stale": false,
  "universe": "cross-app",
  "timing": { "waitedForBuildMs": 0, "computeMs": 412 },
  "stderr": [],
  "output": { "…": "the CLI's JSON document, unchanged" }
}
```

returned as MCP `structuredContent`, with the same JSON as a text content block (the
`output` member is the byte-identical CLI JSON; the contract test compares that string,
not a re-serialization). When `out` is given, `output` is replaced by
`{ "path": …, "bytes": … }` and a short summary (R30).

| Tool | Maps to | Input schema (beyond the common fields) | Notes |
|---|---|---|---|
| `alsem_explain` | `alsem explain --format json` | exactly one of: `routine` (string); `diff` (string, unified diff text) + optional `diffRoot`; `table` + `field` (strings) | `--emit-policy` out of scope |
| `alsem_prove` | `alsem prove --deterministic` | `routine` (string), `question` (enum + `writes-table:<name>` / `publishes-event:<name>` pattern), `closedWorld` (bool) | |
| `alsem_digest` | `alsem digest --deterministic` | one or more of `routines` (string[]), `files` (string[]), `diff` (string) + `diffRoot`; `tests`, `includeReads` (bool); `out` (path) | output is MBs: `out` or summary by default |
| `alsem_query_effects` | `alsem query effects --format json --deterministic` | `routine` | |
| `alsem_query_touches` | `alsem query touches --format json --deterministic` | `table`, optional `from`, `direction` (`down`/`up`/`both`) | |
| `alsem_graph` (optional) | `alsem graph --out` | `out` (path, required) | whole-program JSONL; never inline |
| `alsem_status` | — | none | loaded workspaces, generation, build phase, RSS, last build time |

Inputs are converted to the CLI's own argv and parsed by the binary's clap `Cli`, as
`alsem batch` already does for manifest jobs (`src/engine/batch/manifest.rs`: "each job's
`argv` is parsed by the binary's own clap `Cli`, so every flag, default and message is
the standalone one"). That makes R5 hold by construction for argument handling.

## 8. Test plan

- **T1. Contract: server == CLI.** For each tool and each mode (explain ×3, prove, digest
  ×3, query ×2), on an r0-corpus fixture in the normal suite and on CDO under
  `scripts/cdo-gate`: run the CLI, run the same question through the server's request
  runner, assert the `output` bytes are equal. Both universes. Discrimination proof:
  change one byte of the server path's serialization (e.g. drop the trailing newline, or
  serialize with `to_string` instead of the CLI's normalizer) and watch T1 fail; revert.
- **T2. Contract over the protocol.** Same as T1 but through a spawned `alsem mcp`
  process over stdio, so framing, `structuredContent` and the text block are covered.
  Break: emit a log line on stdout; T2 must fail.
- **T3. Freshness, "fresh" mode.** Build; edit a fixture file (body change that changes
  an explain card); ask; assert the answer equals a CLI run on the edited tree and that
  `generation` went up. Break: skip the stamp check; T3 must fail.
- **T4. Freshness, "latest-built" mode.** Same edit; assert the old answer comes back
  with `stale: true` and the changed path listed; then a "fresh" ask returns the new one.
- **T5. File add / remove / rename and `app.json` / `.app` changes** each produce a new
  revision (hand-state each change; do not rely on the watcher firing).
- **T6. Concurrency.** N parallel requests on one revision trigger exactly one build
  (count builds through a test hook); answers equal their serial answers.
- **T7. Cancellation.** Cancel a request mid-compute: its response is the MCP cancelled
  outcome; a concurrent request on the same build still completes; the next request
  does not rebuild.
- **T8. Superseded build.** Edit during a build: the stale build is cancelled or its
  result is not published as current (mirror the L4 host's T3/T4 tests).
- **T9. Panic isolation.** A test-only request that panics leaves the server answering
  the next request from the same view (R23).
- **T10. Memory.** Under `cdo-gate` (release build), resident RSS after a build and after
  K requests stays within a stated bound; a rebuild never exceeds the stated peak (R19).
- **T11. Path safety.** A request naming a path outside the workspace or an `out` path
  outside allowed roots is refused (R28).

Every test hand-states its precondition (CLAUDE.md "pin the USE") and records its break.

## 9. Rollout

1. **Engine step**: make a built `AnalysisTarget` shareable across views (Arc or
   borrow), add a "run one job over a retained target" entry next to `drive`, with T1 on
   the library entry. No new binary yet. CLI and goldens unchanged.
2. **`alsem mcp`** (hidden or marked experimental): stdio, one workspace, "fresh" mode
   only, explain/prove/digest/query/status. T1, T2, T3, T5, T11.
3. Concurrency, cancellation, "latest-built", memory budget and idle timeout. T4, T6–T10.
4. Cache `l4` and the cone composition per revision (measure first; C1 cached row of
   §6.E).
5. Optional: background rebuild on change (R11), reusing the L4 host's
   generation/cancel pattern; `alsem_graph`.
6. Optional: LSP front end (`al-call-hierarchy/explain` etc.) on a worker thread, behind
   a flag, if an editor consumer exists.
7. Engine follow-ups that shrink warm cross-app explain (§10 Q4) and the rebuild (an
   incremental model; the dependency pack cache).

CHANGELOG entries per step; `docs/` page for configuring the MCP server in Claude Code
(`claude mcp add alsem -- alsem mcp <workspace>`).

## 10. Open questions for the owner

1. **Default universe for agents.** Cross-app is the CLI default (3.4 GB, 14 s rebuild);
   single-app is 0.75 GB, 5 s rebuild and sub-second explain. Should the MCP server
   default to cross-app like the CLI, or to single-app with cross-app on request?
2. **Default freshness.** "fresh" (wait 5–14 s after every edit) vs. "latest-built"
   (instant, possibly stale, marked). The spec proposes "fresh" as the default.
3. **Where does the server live?** A new `alsem mcp` subcommand (recommended) vs. a mode
   of `al-call-hierarchy` (which would tie the MCP surface to the frozen legacy name).
4. **Explain's per-request tree.** Accept ~2–7 s warm cross-app explain for now, or
   schedule an engine change first (share one tree across explain jobs, or a
   subject-scoped derive/witness that does not rebuild the whole-program snapshot per
   card)? This is the biggest lever on agent latency after the server itself.
5. **Lift the epic's "no daemon / no persistent cache" non-goals?** (2026-10-18 epic §2;
   #222 §2.) This spec needs the first; the pack cache (D1) needs the second.
6. **Several agents, one server?** stdio means one server per agent session (memory ×N
   for N sessions on one workspace). A shared server over a local socket (R27) fixes that
   but needs locking and lifetime rules. Needed in v1?
7. **Which MCP revision does Claude Code use now**, and must we support the
   handshake-less `2026-07-28` from day one?
8. **SDK choice**: hand-rolled (recommended here) vs. `rmcp` (adds an async runtime and
   ~10 crates to the main binary). Any policy on new dependencies?
9. **Output size policy** for MCP: summary + file by default for digest, or always the
   full JSON?
10. **Resident memory in a long-lived process.** The CLI drops to 1.35 GB after a
    cross-app build, but the LSP with `--l4-host` stayed at 3.1–3.5 GB. Is that
    acceptable for a server, or should the design include allocator choice (the LSP
    binary does not use `alsem`'s mimalloc + immediate purge) or run builds in a short-lived child process that hands back only
    what is kept? (The second also isolates build crashes, R23.)
11. **Windows**: the owner's main machine is Windows (`U:/Git/…`); the measurements here
    are Linux. Is a Windows re-measure of the cold build required before the design?

## 11. Facts the design should not have to rediscover

- The batch driver (`src/engine/batch/mod.rs` `drive`) already builds one view per
  (workspace, universe, no-roots-config) key, merges plans, and produces standalone-
  identical bytes. The server is "drive, but the job list is open-ended".
- `ViewNeeds` (rules, card_text) are out of `ViewKey`: a view whose union has them builds
  them for everyone. A server that will serve explain should build with
  `ViewNeeds::EXPLAIN` from the start (cross-app model assembly with rules + card text
  measured at ~0.3 s more than without: 14.4 vs. 14.1 s base, within noise).
- `query` releases the cross-app base parts (`Cell::Base`) after its prepare; a server
  must not release what later requests need (`src/engine/session.rs` `Cell::Base` doc).
- G16: digest and prove trees over one revision can share one tree over the union of
  their roots; explain cannot (`tests: true`).
- The LSP's `DepCache` (`src/program/dep_cache.rs`) shares the dependency tier between
  roots in one process; a server holding several workspaces with the same dependencies
  can use it.
- The L4 host shows the generation / cancel / publish-only-if-current pattern
  (`src/lsp/l4_host.rs`), already reviewed and tested.
- Wire strings: anything added to the LSP must use the frozen `al-call-hierarchy/`
  prefix (CLAUDE.md); the MCP server has no legacy names and should use `alsem`.
