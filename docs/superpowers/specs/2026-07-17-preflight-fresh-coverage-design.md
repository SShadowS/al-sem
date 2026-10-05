# Preflight on the fresh resolver — design

**Date:** 2026-07-17
**Status:** approved (user + two adversarial external reviews folded in:
gpt-5.6-sol round 1 + round-2 verify, gemini-3.1-pro; every load-bearing claim
verified against source before adoption)
**Source item:** `docs/OUTSTANDING.md` → "§1 preflight fix"; origin
`docs/2026-07-16-scanner-validation-and-bcquality-candidates.md` §1.

## Problem

`alsem analyze` preflight-warns `analysis coverage degraded — 1045 unresolved
callsite(s)` on DO, a workspace where the AUTHORITATIVE fresh resolver reports
`unknown = 0`. The warning is doubly wrong:

1. It comes from the retired-advisory L3 engine, whose `unknown` definition the
   project explicitly de-authorized (CLAUDE.md "Project Direction & The Moat").
2. Worse: the gate's L3 coverage path (`L3Resolved::project_coverage_disk` →
   `project_coverage`, `src/engine/l3/coverage.rs:239`) resolves with **empty
   declared deps** and labels every app `"source"` — so the count is inflated by
   design (all dependency calls unresolvable) AND `opaqueApps` is **structurally
   empty** on this path. The current message can never even report the one fact
   it was designed for.

Additionally, the fail-closed early returns (`empty_output_result`,
`src/engine/gate/run.rs:466+`) construct a coverage with
`unresolved_callsites: vec![]` — an unreadable/fail-closed workspace preflights
**silently clean** today.

## Decision (user-approved direction)

**Always run the fresh resolver inside `analyze`** and key the preflight on its
output. Measured cost: ~3.8 s on DO (the largest real workspace; analyze goes
~6–7 s → ~10–11 s). Accepted.

## Design

### 1. Program-engine entry: `FreshCoverage`

A narrow public entry on the program-engine side, factored from the REAL
pipeline `aldump --program-call-graph-stats` drives — `resolve_full_program` →
`build_context` → `SnapshotBuilder::build` → `parse_snapshot` →
`assemble_program_graph` (`src/program/resolve/full.rs`) — not a second
hand-rolled pipeline:

```rust
pub struct FreshCoverage {
    /// primaryScoped `unknown` — TRUE resolution failures (`ambiguousResolved`
    /// excluded), the realUnknownRate definition.
    pub unknown: usize,
    /// The resolve run's own coverage contract (every obligation classified).
    pub coverage_holds: bool,
    /// Files whose parse was ParseStatus::Recovered — IR may have dropped
    /// content, so unknown==0 does NOT prove completeness over them.
    pub recovered_files: usize,
    /// Symbol-only dependency apps, from the FRESH snapshot
    /// (`AppUnit::source == None`) — one engine, one dependency universe.
    /// SCOPED to the primary app's reachable dependency closure and excluding
    /// the primary itself: `load_all_apps` deliberately loads EVERY `.app` in
    /// (ancestor) `.alpackages` without app.json filtering
    /// (`src/dependencies.rs:419+`), so an unscoped list would let an
    /// UNRELATED cached package flip `--require-dependencies` to exit 4.
    /// Display identity = `AppId.name`; deduped, sorted (name, then guid) for
    /// deterministic messages.
    pub opaque_apps: Vec<String>,
}

pub fn fresh_coverage(ws: &Path) -> Result<FreshCoverage, String>
```

NOT a bare `usize`: `coverage_holds == false` and `recovered_files > 0` can
each coexist with `unknown == 0` and must not launder into "coverage complete"
(instrument-honesty doctrine). The `Err` arm preserves the underlying snapshot/
build error text — today `resolve_full_program`/`build_context` erase it via
`.build().ok()?` into a bare `Option` (`src/program/resolve/full.rs`), so the
implementation MUST refactor `build_context` (or add a sibling) to return
`Result`; wrapping the existing `Option` cannot recover the message.

Out of scope (recorded as follow-ups, not silently dropped): reporting
dependency ABI-ingestion errors and declared-but-missing dependencies; sharing
one parse between the L3 and fresh passes.

### 2. Preflight semantics

`evaluate_preflight` gains a first-class **could-not-verify** state and labelled
clauses; signature moves from `(usize, &[String], bool)` to consuming
`Result<FreshCoverage, String>` + `required`:

- `Ok` with `unknown == 0 && coverage_holds && recovered_files == 0` →
  clean; opaque apps present → degraded with ONLY the opaque clause (that is a
  genuine cone limitation, kept fail-open).
- `Ok` with `unknown > 0` → degraded: `N unknown resolution edge(s)`
  ("resolution edge", not "call edge" — the histogram spans Call/Run/
  ImplicitTrigger/EventFlow).
- `Ok` with `!coverage_holds` or `recovered_files > 0` → degraded with its own
  explicit clause (`coverage contract violated`, `N recovered file(s)`); never
  the clean message.
- `Err(e)` → degraded: `coverage could not be verified: <e>`. Never
  silent-clean.
- `failed = degraded && required` (unchanged fail-open contract; the warning
  always goes to stderr, exit 4 only under `--require-dependencies`).

**Combined clauses are all retained** (no clause overwrites another), in fixed
order: unknown edges, coverage-contract violation, recovered files, opaque
apps; comma-joined, opaque names pre-sorted. An all-signals unit test pins
this.

Clean message becomes **`resolution coverage verified`** — NOT the old
"dependency coverage complete", which over-claims while ABI-ingestion-error and
declared-but-missing-dependency checks are still deferred follow-ups.

### 3. Wiring, sequencing, and the fail-closed hole

**Sequencing (memory peak):** `run_analyze` computes `fresh_coverage(ws)`
FIRST and drops the whole `ProgramContext` immediately, keeping only the tiny
`FreshCoverage` value, BEFORE L3 assembly — so the two semantic models are
never resident together (the naive "add the fresh resolve next to the L3 model"
would roughly double peak memory on big workspaces). The duplicated `.app`
discovery/unzip I/O between the two passes is real but ALREADY included in the
measured ~3.8 s (full `aldump` wall time); accepted, see follow-ups.

> **Superseded 2026-10-05 (B3 Phase A).** This sequencing no longer holds.
> `alsem analyze` now keeps the `ProgramContext` and its report after the
> preflight, because the detectors read the program engine's call resolution
> (`full::build_program_with_coverage` → `program_calls::attach_program_calls`).
> The two semantic models ARE resident together while the adapter runs; both
> are dropped before the detector context is built. `fresh_coverage` and the
> preflight verdict cache (`docs/superpowers/specs/2026-08-01-preflight-verdict-cache.md`)
> were deleted: a cached verdict cannot give the context the adapter needs.
> The text above is kept as the record of the original design.

`gate/run.rs` then evaluates preflight from the retained `FreshCoverage`
instead of `coverage.unresolved_callsites.len()` / `coverage.opaque_apps`.

**Formatted output follows the same universe:** the coverage views that print
opaque apps — JSON `payload.summary.opaqueApps`
(`src/engine/gate/format_json.rs:305+`), the terminal coverage line
(`format_terminal.rs:314+/449+`), and the HTML coverage line — switch to the
FRESH opaque list too. Leaving them on the L3 list (structurally `[]` on the
gate path) would let one run print "N symbol-only apps" on stderr and
`"opaqueApps": []` in JSON. This deliberately moves the affected goldens.
`routinesAnalyzed`/`sourceUnitsParsed` stay L3-sourced (unchanged).

**Fail-closed early returns** (`empty_output_result`) stop fabricating a clean
result — today the function unconditionally ends `Ok((out, exit::CLEAN, None))`
(`src/engine/gate/run.rs`, tail), i.e. silent clean, exit 0, no warning, even
under `--require-dependencies`. It gains the could-not-verify arm: reason = the
fail-closed provider diagnostic when one exists, else the defined fallback
`workspace contained no readable AL source units` (some fail-closed paths
produce ZERO diagnostics — e.g. valid app.json with no readable `.al` files);
warn always, exit 4 under `--require-dependencies`.

### 4. CI-visible semantic change (CHANGELOG under Changed)

Exit-4 (`--require-dependencies`) re-keys from the inflated L3 no-deps multiset
(which included Ambiguous/MemberNotFound/ExternalTarget) to fresh `unknown` +
verify-failure. Consequences to document:

- workspaces that exited 4 can now exit 0/1 (the DO case — the point);
- fail-closed workspaces newly exit 4 under `--require-dependencies`;
- stderr wording changes (`unresolved callsite(s)` → `unknown resolution
  edge(s)` + new clauses).

### 5. Tests

- `preflight.rs` unit tests: rewrite for the new states/wording (clean /
  unknown / opaque-only / contract-violated / recovered / could-not-verify /
  ALL-SIGNALS-combined, each × required on/off).
- Deliberate rebaselines — the full pin inventory
  (`tests/cli/gate_prsummary_differential.rs`): the warning oracle pinning
  `"unresolved callsite"` (:636), `anti_degenerate_preflight_exit_four` (:586),
  `oracle_exit_precedence_preflight_wins_over_findings` (:653), and the
  exit-codes golden matrix (`tests/gate-goldens/exit-codes.json` — exit-4 cells
  keyed to the old semantics). Plus the formatter goldens that carry
  `opaqueApps` / the coverage line (§3).
- New integration assertions: DO-shaped no-warning case via a fixture whose dep
  has EMBEDDED SOURCE (a symbol-only dep would correctly still warn via the
  opaque clause — the two cases must not be conflated); a separate symbol-only
  fixture asserting the opaque-only warning; verifier-error arm on the
  fail-closed path (both with a provider diagnostic and with the zero-diagnostic
  fallback); helper count == `primary_histogram.unknown` on a fixture with a
  genuine unknown.
- `scripts/check-goldens` before commit (pre-commit hook enforces).

## Alternatives rejected

- **Lazy adjudication** (L3 first, fresh only when L3 flags): cheaper on clean
  workspaces but keeps two engines in the signal path and misses the
  L3=0/fresh>0 corner. Rejected for one-authority simplicity.
- **Message-only reword**: keeps the misleading warning. Fails the goal.
- **Bare `usize` count**: launders contract violations and recovered parses
  into "complete". Rejected on instrument-honesty grounds (review finding).

## Follow-ups (recorded in `docs/OUTSTANDING.md`)

- Shared parse: L3 assembly consuming `ProgramContext::parsed()` — removes the
  duplicate parse + `.app` unzip (~halves the added cost) and the TOCTOU
  between the two passes.
- Dependency ABI-ingestion error + declared-but-missing dependency reporting in
  `FreshCoverage` (then the clean message can strengthen again).
- `empty_output_result`'s doc-comment claims stub formats return `Err` ("no
  silent pass") while the code returns `Ok(CLEAN)` — the exit part is fixed by
  this design; align the remaining doc/behavior while in there.
