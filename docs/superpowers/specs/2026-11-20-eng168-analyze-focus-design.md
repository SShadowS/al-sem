# ENG-168: `alsem analyze --focus`, hiding detectors by concern

Status: revision 2, after review r1 (Fable, sol) and the orchestrator's answers (2026-11-20).
Owner request 2026-11-20.

## 1. The problem

An agent used `alsem analyze` on DO to investigate behaviour (what a change does to data,
transactions and events). It got 2,447 findings, 704 of them in the relevant files. Most were
performance findings: database calls in loops, missing `SetLoadFields`. They answer "is this
fast?", not "what does this do?". The agent had to sort the noise from the signal by hand.

The owner asks for a mode that hides performance findings and says what it hid, so nothing
disappears silently.

## 2. What exists today

- **Detector registry** (`src/engine/l5/detectors/mod.rs`): 55 detectors, `default_detectors()`
  (40) and `opt_in_detectors()` (15). Each `Detector` has `name`, `run`, `requires`, `rule_name`
  and `short_description` (`src/engine/l5/registry.rs:333`). Nothing records what a detector is
  about. Detectors run independently; their outputs are concatenated and sorted (detector
  first, `registry.rs:770`); dedupe is per detector. Fingerprints are computed per finding
  inside each detector.
- **Selection** (`src/engine/gate/presets.rs`): `--preset <name>` (3 fixed member lists) or
  `--detector <ids>`; neither runs the default set.
- **The pipeline after the run** (`src/engine/gate/run.rs` `analyze_view`, lines 872-962), in
  this order: `--min-severity` and the detector allow-list (`filter_findings`), `--scope`,
  `--limit`, the baseline (`--baseline`, by fingerprint), inline suppression
  (`// al-sem-ignore`, by file, detector and line).
- **Formats**: sarif, pr-summary, terminal, json, html (`auto` picks terminal or json);
  `--group-by` has its own terminal renderer (`run.rs:1015-1023`).
- **MCP**: there is no `alsem_analyze` tool (`src/mcp/tools.rs:189-197`).

## 3. Design

### 3.1 A concern per detector, in the registry (one home)

`Detector` gets a required field `concern: Concern`, set in each registry entry next to
`rule_name`:

```rust
pub enum Concern {
    /// How fast or how costly: SQL round trips, rows loaded, work repeated in loops,
    /// memory held.
    Performance,
    /// What the code does: data written or lost, transactions and commits, events, errors,
    /// exposure and permissions, values read wrong.
    Behaviour,
    /// How the code is kept: dead or unreachable code, unused parameters, obsolete or
    /// internal APIs, version drift. A risk for the next build, not a change in what runs.
    Maintainability,
}
```

The field is required, so no detector can be added without one (the compiler refuses). The 15
`Detector { .. }` literals outside the registry (tests) get one too. Presets, the CLI and later
readers read the field; nothing keeps a second list (principle 19). The third value is the
honest label for detectors that are neither cost nor behaviour (principle 6).

### 3.2 The classification

From each detector's module doc; both reviewers read all 55 (`d39_persist.rs` is a helper, not
a detector).

| Concern | Detectors |
|---|---|
| Performance (13) | d1 DbOpInLoop, d2 EventFanoutInLoop, d3 MissingSetLoadFields, d4 RepeatedLookupInLoop, d5 SetBasedOpportunity, d18 ConstantFilterInLoop, d36 LateSetLoadFields, d42 CrossCallWrongSetLoadFields, d48 IoInLoop, d55 EventPublishInLoop, d56 CloneBeforeWriteInLoop, d57 SingleInstanceGrowingState, d60 UpgradeLoopShouldBeDataTransfer |
| Behaviour (33) | d7, d8, d9, d10, d11, d21, d22, d29, d33, d34, d35, d37, d39, d40, d41, d43, d44, d45, d46, d47, d49, d50, d51, d52, d53, d54, d58, d59, d61, d62, d63, d64, d65 |
| Maintainability (9) | d12 DeadIntegrationEvent, d13 CrossAppInternalCall, d14 DeadRoutine, d16 ObsoleteRoutineCall, d17 MinVersionDrift, d19 UnusedParameter, d20 UnreachableAfterExit, d32 ConstantBooleanParameter, d38 SubscriberToObsoleteEvent |

13 + 33 + 9 = 55. The rows with a reason:
- **d42: Performance.** Its module doc: the callee's read of a field the caller did not load
  costs an extra round trip (`d42.rs:4-12`). It is not the unloaded-record case.
- **d21, d40, d22: Behaviour.** A blank record read, an unloaded or mutated callee input, a
  FlowField's stored value: a wrong value, not a slow one.
- **d57: Performance.** Session memory that grows; the finding is the growth.
- **d48: Performance** by its text (repeated external calls, `d48.rs:438-441`), though it is a
  member of the `transaction-integrity` preset (`presets.rs:20`): see §3.3.
- **d58: Behaviour.** The filter set after `Open` does not apply.
- **d9 (info): Behaviour.** It reports what a transaction spans.
- **d13: Maintainability** (review, Fable). Its own text: a call into another app's internal
  routine "breaks encapsulation and can stop compiling on any minor version bump"
  (`d13.rs:120-124`). It runs today; the risk is the next build.
- **d17: Maintainability** (review, Fable and sol). It compares the manifest's MinVersion with
  the resolved dependency and needs only one call into it (`d17.rs:148-185`); it never shows
  that a called API is missing at the minimum.
- **d16, d38: Maintainability.** Both report a deprecation, `Removed` (high) as well as
  `Pending` (info); the label is the detector's subject, not a promise that the finding is low
  risk.

### 3.3 The option

`alsem analyze --focus <concern>[,<concern>...]`, values `performance`, `behaviour` (alias
`behavior`), `maintainability`. Findings of the concerns not named are hidden.
`--focus behaviour` keeps Behaviour only; who also wants maintenance findings passes
`--focus behaviour,maintainability` (orchestrator, Q1).

- **Filtering only.** Detector selection and the run are unchanged; `--focus` drops findings
  afterwards. Fingerprints, finding ids, the baseline match and inline suppression are
  per-finding, so a shown finding is byte-equal with and without `--focus` (principle 17:
  keep every fact; the view is opt-in). Running the hidden detectors costs their time, and it
  is what lets the output count what it hid.
- **Where.** A partition in `analyze_view` right after `--scope` and before `--limit`, over
  the existing `(summary, raw)` pairs; not inside `filter_findings`. The existing order is
  otherwise unchanged, and a run without `--focus` is byte-identical to today's.
  `--focus` must sit before `--limit`: findings are sorted by detector, so a limit applied
  first would fill the cap with hidden findings.
- **What is counted** (Q3, orchestrator: after `--min-severity`). The hidden counts are the
  findings removed by `--focus` after `--min-severity`, the allow-list and `--scope`, and
  before `--limit`, the baseline and inline suppression. So they can include findings a
  baseline or an `al-sem-ignore` would have dropped anyway. Every format labels them so:
  "hidden by --focus (before limit, baseline and suppressions)".
- **A detector not in the registry** cannot occur (every finding comes from a registered
  detector); if one did, its finding is kept, not hidden (fail open: never hide what cannot be
  classified).
- **Presets and `--detector` compose with it.** `--preset transaction-integrity --focus
  behaviour` hides d48's findings, a preset member, and the hidden line says so.
- **Refused combinations** (exit 3, config error, before any analysis or write; checked in
  `plan_analyze`, so batch and the shared job entry refuse too):
  - `--update-baseline --focus`: the baseline writer replaces the fingerprint set, so a
    focused baseline would drop every hidden finding, and the next full run would report
    them all as new. (The same hazard exists today with `--min-severity`, `--detector`,
    `--preset` and `--limit`; filed as its own task.)
  - `--fail-on --focus`: the exit code follows the shown findings, so a focused run could
    turn a failing gate clean. `--focus` is an investigation view, not a gate (principle 4:
    a new gate starts advisory). A later task can allow it once wanted.
  - Reading a baseline (`--baseline` alone) is allowed.
- **An unknown value** is a config error naming the known values. A focus naming every
  concern hides nothing.
- **The dependency preflight** (exit 4) is unchanged.

### 3.4 Saying what was hidden (label, never hide: principle 6)

One tally, computed once and handed to every renderer: the focus, the total, and per hidden
detector its name, concern and count (registry order, zero counts omitted). Raw findings, not
groups or rollups. Machine-readable in JSON and SARIF (orchestrator):

- **json**: `payload.summary.hidden`, present only with `--focus` (an additive key under an
  opt-in flag, as `--with-evidence` adds `evidence`, so no schema bump; `format_json.rs:453`):
  `{"focus": ["behaviour"], "before": "limit, baseline, suppressions", "total": 1743,
  "byDetector": [{"detector": "d1-db-op-in-loop", "concern": "performance", "count": 612},
  ...]}`. `detectorStats` stays run-level (all detectors that ran).
- **sarif**: `runs[0].properties.hidden`, the same object (the `Run` has no `invocations`
  today, `format_sarif.rs:336-346`). SARIF consumers ignore property bags and treat a missing
  alert as fixed, so a focused SARIF run also writes to stderr
  `al-sem: warning: --focus: a partial report (N findings hidden); do not upload it as the
  full scan`, and `--help` says the same.
- **terminal, and `--group-by`'s renderer**: one line after the findings, also when none are
  shown (never a bare "no findings"):
  `hidden by --focus behaviour (before limit, baseline and suppressions): 1,743 findings
  (performance: d1 612, d3 400, ...; maintainability: d14 90, ...)`.
- **pr-summary**: the same line before the empty-result early return, separate from its
  "more ... see SARIF" overflow; with nothing shown it says "no findings in this view".
- **html**: the same line in the report header, also on the empty branch.
- **No model** (the workspace failed to assemble): the output says "focus requested, analysis
  unavailable", never a hidden count of zero.

### 3.5 MCP

The MCP surface comes with ENG-171 (`alsem_analyze`, after ENG-168; orchestrator, Q2). ENG-168
keeps the option in the CLI, so the tool maps `focus` to `--focus` with nothing else to add.

## 4. Tests

Each correctness guard with a red proof (principle 7). No new r0-corpus fixture (it would move
three golden families); hand-stated rows plus an existing workspace:
- **Registry:** every registered detector has the concern in a test's expected table (name ->
  concern); a reclassification is a reviewed diff.
- **Partition** (unit, hand-stated findings of d1, d34 and d14 with set fingerprints):
  `--focus behaviour` keeps d34 and hides d1 and d14, the tally counts each once, and d34's
  summary and fingerprint are byte-equal to the unfiltered pass; `--focus performance` the
  other way; `--focus behaviour,maintainability` keeps d34 and d14.
- **Order:** with `--limit 1`, a hidden d1 never takes the slot (focus before limit); a
  finding the baseline would drop is still counted (the stated definition).
- **No focus:** the output is byte-identical to today's (the existing goldens hold).
- **Refusals:** `--update-baseline --focus` and `--fail-on --focus` exit 3, writing nothing;
  an unknown value exits 3 naming the known values; `behavior` is accepted.
- **Formats:** the tally in json and sarif; the line in terminal, `--group-by`, pr-summary and
  html, including when focus hides every finding; the no-model case.
- **Preset:** `--preset transaction-integrity --focus behaviour` hides d48 and counts it.
- **Measured on DO** (the owner's case, `--scope primary`): findings shown and hidden per
  concern, against the unfocused run; the shown findings' fingerprints equal.

CHANGELOG entry.

## 5. Out of scope

- Any detector's severity, default or opt-in class, or fingerprint.
- Hiding by file or routine.
- The MCP tool (ENG-171).
- The baseline-update hazard with the existing filters (its own task, §3.3).
