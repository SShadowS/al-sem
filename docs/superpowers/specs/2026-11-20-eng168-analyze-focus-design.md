# ENG-168: `alsem analyze --focus`, hiding detectors by concern

Status: draft for review (Fable, sol). Owner request 2026-11-20.

## 1. The problem

An agent used `alsem analyze` on DO to investigate behaviour (what a change does to data,
transactions and events). It got 2,447 findings, 704 of them in the relevant files. Most were
performance findings: database calls in loops, missing `SetLoadFields`. They answer "is this
fast?", not "what does this do?". The agent had to sort the noise from the signal by hand.

The owner asks for a mode that hides performance findings and says what it hid, so nothing
disappears silently.

## 2. What exists today

- **Detector registry** (`src/engine/l5/detectors/mod.rs`): 55 detectors in two lists,
  `default_detectors()` (40) and `opt_in_detectors()` (15). Each `Detector` has `name`, `run`,
  `requires`, `rule_name` and `short_description`. Nothing records what a detector is about.
- **Selection** (`src/engine/gate/presets.rs`): `--preset <name>` (3 fixed member lists) or
  `--detector <ids>` chooses which detectors run; neither given runs the default set.
- **Filtering** (`src/engine/gate/filter.rs` `filter_findings`): `--min-severity` and the
  detector allow-list drop findings after the run; `--scope` drops dependency findings.
- **Formats**: sarif, pr-summary, terminal, json, html (`auto` picks terminal or json).
- **MCP**: there is NO `alsem_analyze` tool. The context server's tools are explain, prove,
  digest, query effects, query touches, tests, impact, status and refresh (`src/mcp/tools.rs`).
  Each tool turns its call into the CLI's argv, run by the CLI's own parser in the engine child.

## 3. Design

### 3.1 A concern per detector, in the registry (one home)

`Detector` gets a field `concern: Concern`, set in each registry entry next to `rule_name`:

```rust
pub enum Concern {
    /// How fast or how costly: SQL round trips, rows loaded, work repeated in loops.
    Performance,
    /// What the code does: data written or lost, transactions and commits, events,
    /// errors, permissions and exposure, wrong values read.
    Behaviour,
    /// How the code is kept: dead or unreachable code, unused parameters, obsolete
    /// calls. Neither a cost nor a change in what runs.
    Maintainability,
}
```

The field is required, so a new detector cannot be added without a concern (the compiler
refuses). Principle 19 (one home per fact): presets, the CLI and any later reader read it from
the registry; nothing keeps a second list.

A third value, `Maintainability`, is my call (principle 6, label honestly): about a sixth of the
detectors are neither about cost nor about behaviour, and filing them under either would
mislabel them. See open question Q1 for what `--focus behaviour` does with them.

### 3.2 The classification

Read from each detector's description and module doc. The reviewers should challenge any row.

| Concern | Detectors |
|---|---|
| Performance (13) | d1 DbOpInLoop, d2 EventFanoutInLoop, d3 MissingSetLoadFields, d4 RepeatedLookupInLoop, d5 SetBasedOpportunity, d18 ConstantFilterInLoop, d36 LateSetLoadFields, d42 CrossCallWrongSetLoadFields, d48 IoInLoop, d55 EventPublishInLoop, d56 CloneBeforeWriteInLoop, d57 SingleInstanceGrowingState, d60 UpgradeLoopShouldBeDataTransfer |
| Behaviour (35) | d7, d8, d9, d10, d11, d13, d17, d21, d22, d29, d33, d34, d35, d37, d39, d40, d41, d43, d44, d45, d46, d47, d49, d50, d51, d52, d53, d54, d58, d59, d61, d62, d63, d64, d65 |
| Maintainability (7) | d12 DeadIntegrationEvent, d14 DeadRoutine, d16 ObsoleteRoutineCall, d19 UnusedParameter, d20 UnreachableAfterExit, d32 ConstantBooleanParameter, d38 SubscriberToObsoleteEvent |

The rows that need a reason:
- **d42 CrossCallWrongSetLoadFields: Performance.** A field missing from `SetLoadFields` is
  loaded just in time by the platform: extra SQL work, never a wrong value (measured on BC 28,
  recorded in the bc-measure skill: 5,000 iterations reading an unloaded field cost four
  statements). To re-check on the BC sidecar before the code lands if a reviewer doubts it.
- **d21 ReadWithoutLoad, d40 TransitiveLoadMissing, d22 FlowFieldWithoutCalcFields:
  Behaviour.** The code reads a blank record or a FlowField's stored 0: a wrong value, not a
  slow one.
- **d57 SingleInstanceGrowingState: Performance.** Memory that grows for the session's life;
  the values stay right.
- **d58 QueryFilterAfterOpen: Behaviour.** The filter does not apply; the query returns rows
  it was meant to exclude.
- **d17 MinVersionDrift: Behaviour.** The call fails at runtime on an older platform.
- **d9 TransactionSpanSummary (info): Behaviour.** It describes transaction spans.
- **d13 CrossAppInternalCall: Behaviour.** It does not compile or run across the apps.
- **d38 SubscriberToObsoleteEvent: Maintainability.** The subscriber still fires today; the
  finding is that it will need moving.

13 + 35 + 7 = 55, every registered detector.

### 3.3 The option

`alsem analyze --focus <concern>[,<concern>...]`, values `performance`, `behaviour`,
`maintainability`. Findings of the other concerns are hidden.

- **Filtering only.** Detectors run as selected today (default set, `--preset` or
  `--detector`); `--focus` drops findings afterwards, in `filter_findings`, next to
  `--min-severity`. Fingerprints, finding ids and suppressions are untouched: the same finding
  carries the same fingerprint with or without `--focus`. Running the hidden detectors costs
  their time, and it is what lets the output count what it hid (§3.4). (Principle 17: keep
  every fact; the trade-off is an opt-in view.)
- **Order.** Selection, then the run, then `--scope`, then `--focus` and `--min-severity`, then
  the baseline comparison, then `--limit`. The hidden counts are taken after `--scope` (they
  count what the user would otherwise have seen); Q3 asks where they sit against
  `--min-severity`.
- **Exit code and `--fail-on`** follow the shown findings, as `--min-severity` already does.
- **`--update-baseline` with `--focus` is refused** (exit 3, config error). A baseline written
  from a filtered run would drop every hidden finding's fingerprint, and the next unfiltered
  run would report them all as new. Reading a baseline (`--baseline` alone) is fine.
- **A focus that names every concern** is accepted and hides nothing (no special case).
- **An unknown value** is a config error naming the known values.

### 3.4 Saying what was hidden (label, never hide: principle 6)

Every format states it, with counts per hidden detector:

- **json**: a top-level `hidden` object, present only with `--focus`:
  `{"focus": ["behaviour"], "total": 1743, "byDetector": [{"detector": "d1-db-op-in-loop",
  "concern": "performance", "count": 612}, ...]}`, detectors in registry order, zero counts
  omitted. A schema version bump for the analyze JSON if it has one (check in the plan).
- **terminal and pr-summary**: one line after the summary:
  `hidden by --focus behaviour: 1,743 findings (performance: d1 612, d3 400, ...; maintainability: d14 90, ...)`.
- **sarif**: `runs[0].invocations[0].properties.hidden` with the same object (SARIF allows
  property bags; the findings list stays the shown ones).
- **html**: the same line in the report header.

### 3.5 MCP

The request asks for "the same option in the MCP tool", but there is no `alsem_analyze` tool
(§2). See Q2.

## 4. Open questions (for the orchestrator or the owner)

- **Q1. Does `--focus behaviour` hide Maintainability findings too?** The request names
  performance only. Recommendation: yes. `--focus` keeps the concerns it names, so
  `--focus behaviour` keeps Behaviour only, and the hidden line counts Maintainability apart
  from Performance, so nothing is silent. Unused parameters and dead routines are noise for a
  behaviour investigation as much as `SetLoadFields` is. Who wants maintenance findings too
  passes `--focus behaviour,maintainability`.
- **Q2. Add an `alsem_analyze` MCP tool?** Recommendation: yes, in this task, because the owner
  asked for the option in MCP and the agent session that prompted this would have used it.
  Shape: the CLI's argv, as every tool does (`workspace`, `focus`, `preset`, `detector`,
  `minSeverity`, `scope`, `limit`, `format` human/json), answers equal to the CLI's stdout
  byte for byte (the existing contract-test pattern). The tool's default is all concerns, as
  on the CLI; its description tells an agent investigating behaviour to pass
  `focus: behaviour`. Cost: a tool entry, its argv mapping, a contract test and a CHANGELOG
  line. If the owner prefers, it becomes its own task.
- **Q3. Are the hidden counts taken before or after `--min-severity`?** Recommendation: after,
  so the line counts exactly the findings that `--focus` alone removed from what would have
  been shown.

## 5. Tests

Each with a red proof (principle 7):
- every registered detector has a concern, and the table above is what the registry holds
  (a test listing the expected concern per detector name: a change of class is a reviewed
  diff, not a silent edit);
- on an r0-corpus workspace with a d1 (performance) and a d34 (behaviour) finding:
  `--focus behaviour` keeps d34 and hides d1, the JSON `hidden` block counts d1 once, and the
  shown d34 finding's fingerprint is byte-equal to the unfiltered run's;
- `--focus performance` the other way round;
- `--update-baseline --focus` exits 3;
- an unknown focus value exits 3 naming the known values;
- each format carries the hidden statement (json, terminal, sarif, pr-summary, html);
- if Q2 is yes: the MCP tool's answer equals the CLI's stdout.

Measure on DO (the owner's case): findings shown and hidden per concern, at `--scope primary`.

## 6. Out of scope

- Changing any detector's severity, default/opt-in class or fingerprint.
- Hiding by file or routine (the agent's "704 in the relevant files"); `--focus` is by concern.
