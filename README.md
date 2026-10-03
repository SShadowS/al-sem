# al-sem

Static analysis for Business Central AL that understands your whole solution — your
workspace *and* every extension in your `.alpackages` — as one program. It finds the
performance and transaction bugs your customers report, and it answers "who calls this?"
and "what breaks if I change this?" across app boundaries.

[![Rust](https://img.shields.io/badge/rust-1.85+-orange)](https://rust-lang.org)
[![GitHub release](https://img.shields.io/github/v/release/SShadowS/al-sem)](https://github.com/SShadowS/al-sem/releases)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

## What's in the box

**`alsem analyze` — the code checker. Start here.**
Scans a workspace and reports the problems that actually bite BC projects: a database
call inside a `FindSet` loop, a `Commit` in the wrong place, a missing `SetLoadFields`,
a `TryFunction` whose result nobody checks, an `IntegrationEvent` nobody subscribes to,
and 49 more.
*Why: it catches the performance and transaction bugs customers report — before they do.
Reads well in a terminal, and produces reports your build pipeline can display.*

```bash
alsem analyze path/to/workspace
```

**`al-call-hierarchy` — the editor server.**
Real "who calls this procedure?" and "what does this procedure call?" for AL in your
editor, including calls that come from your dependencies. Plus reference counts above
each procedure, and warnings for unused procedures and overly complex code.
*Why: Find References in the AL extension stops at the edge of your workspace. This
doesn't.*

```bash
al-call-hierarchy      # speaks LSP over stdio — point your editor's LSP client at it
```

**`alsem query`, `digest`, `prove`, `diff`, `events` — impact analysis.**
Answer questions like "which code writes to this table, anywhere up or down the call
chain?", "what does the code I just changed actually touch?", "did this release change
any event or table another extension depends on?"
*Why: refactoring and upgrades in a big BC codebase are scary because you can't see the
blast radius. These show it to you.*

```bash
alsem query touches path/to/workspace --table "Sales Header"
```

**`al-call-hierarchy --project <ws>` — a one-shot health snapshot.**
Index a workspace from the command line and print definition/call-site counts. Add
`--analyze` for a per-procedure quality report (cyclomatic complexity, parameter counts,
line counts, fan-in, unused procedures) as `text`, `json`, or `csv`.
*Why: a fast look at a codebase with no editor and no setup.*

```bash
al-call-hierarchy --project path/to/workspace --analyze --format text
```

The editor server's binary is called `al-call-hierarchy` rather than `al-sem`: it was this
project's original name, and editor integrations spawn it by that name, so it kept it when
the project was renamed.

`aldump` also ships in the repo. It dumps this engine's own internals and exists for
developing this project — you don't need it.

## Install

Prebuilt binaries for each release are on the
[releases page](https://github.com/SShadowS/al-sem/releases).

From source (Rust 1.85 or newer — the crates use edition 2024):

```bash
git clone https://github.com/SShadowS/al-sem
cd al-sem
git clone https://github.com/SShadowS/tree-sitter-al
cargo build --release
```

The second clone is the AL grammar. It is not pinned: the build uses whatever is on its
`main` branch, the same as CI. Run `git -C tree-sitter-al pull` to update it.

Point any command at the directory containing `app.json`. Dependencies are read from
`.alpackages/` — embedded source is used when a dependency ships it, and the
`SymbolReference.json` inside the `.app` otherwise. The highest compatible version wins.

## What `alsem analyze` finds

54 checks in total: 43 run by default, 11 more are opt-in. A sample of what they cover:

| Check | Plain English |
|-------|---------------|
| `d1-db-op-in-loop` | A database call runs once per loop iteration — including when the loop is in a *caller*, several procedures up |
| `d3-missing-setloadfields` | A record is read, only a couple of its fields are used, and no `SetLoadFields` narrowed the read |
| `d8-commit-in-transaction` | A `Commit` runs partway through a posting transaction, so a later failure leaves data half-written |
| `d34-commit-in-loop` / `d35-commit-in-event-subscriber` | `Commit` inside a loop; `Commit` reachable from an event subscriber, where the publisher can no longer roll back |
| `d11-modify-without-get` | `Modify` or `Validate` on a record variable that was never loaded first — nothing in the procedure, or anything it calls, ever read it |
| `d12-dead-integration-event` | A published event nobody subscribes to |
| `d22-flowfield-without-calcfields` | A FlowField is read without `CalcFields`, so it silently reads zero |
| `d53-ignored-tryfunction-result` | A `TryFunction` is called and its boolean result is thrown away — the error is swallowed |
| `d16-obsolete-routine-call` | Code still calls something marked `Obsolete` |
| `d60-upgrade-loop-should-be-datatransfer` | An upgrade codeunit loops row by row where `DataTransfer` would do it in one statement |

Every finding names the routine, the file and line, how confident the engine is, and a
concrete fix. Real output, from a small test workspace:

```
Analysed 4 routines (4 with bodies, 0 parse-incomplete); 1/1 source units parsed; 0 opaque app(s).

HIGH (1):
  [d1-db-op-in-loop] Database operation inside a loop — A loop in CallerA reaches Modify on MC Customer in ModifyHelper, which has no loop of its own — the operation runs once per iteration of that loop. (Also reached from 2 other in-loop ancestors.)
    ws:src/D1MultiCaller.al:19:9 in D1 Multi Caller :: ModifyHelper
    confidence: likely
    fix (medium): Move the database operation outside the loop, or batch it into a set-based operation.
```

Note the "Also reached from 2 other in-loop ancestors" — the loop and the database call
are in **different procedures**. Finding that is the whole point of resolving the call
graph properly.

Useful flags:

| Flag | What it does |
|------|--------------|
| `--format terminal \| json \| sarif \| html \| pr-summary` | Default `auto`: terminal when you run it by hand, JSON when piped |
| `--preset transaction-integrity` \| `--preset bcquality` | Run a themed bundle instead of the defaults |
| `--detector d1-db-op-in-loop,d8-commit-in-transaction` | Run only the checks you name. This and `--preset` are how you reach the 11 opt-in checks; they're mutually exclusive |
| `--min-severity high` | Drop everything below a severity |
| `--group-by object \| routine \| table \| detector \| file` | Reorganize terminal output |
| `--baseline al-sem.baseline.json` | Suppress findings listed in the file — adopt the tool on an old codebase without fixing 500 things first. `--update-baseline` rewrites it to the current state |
| `--fail-on high` | Exit 1 when anything at or above that severity survives filtering — this is the CI gate |
| `--require-dependencies` | Exit 4 if dependencies are missing, instead of quietly analyzing less code |

That last one matters more than it looks. If a dependency can't be read, the engine sees
less of your program and therefore reports fewer problems. By default it warns; with this
flag a CI build fails instead of going green for the wrong reason.

## In your editor

`al-call-hierarchy` is an LSP server, so any editor with an LSP client can drive it. It
handles `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`,
`callHierarchy/outgoingCalls`, `textDocument/codeLens`, and pushes
`textDocument/publishDiagnostics`. See [LSP.md](LSP.md) for wiring it into an extension.

What you get:

- **Call hierarchy** that crosses app boundaries — including callers inside a dependency,
  when that dependency ships source.
- **Code lens** with reference counts, plus complexity/length/parameter-count warnings
  above procedures that cross your configured thresholds.
- **Diagnostics** for unused procedures and code-quality problems, refreshed on save.
- **Multi-root workspaces**: each folder is indexed separately, and one broken folder
  degrades only itself.
- **Correct AL casing**, including outside ASCII — `Løbenr` and `LØBENR` are the same
  identifier, matching the compiler.

Editing a procedure body reparses just that file (about 13 ms on a 1000-file benchmark);
changing a signature reparses what depends on it. You keep working while it happens.

## The rest of the toolbox

Grouped by the question you're trying to answer, not by command name.

**"What touches this table?"**

```bash
alsem query touches <ws> --table "Sales Header"              # every routine, transitively
alsem query touches <ws> --table "Sales Header" --from PostDocument --direction up
alsem query effects <ws> --routine PostDocument               # everything this routine touches, and via what
```

`--direction up` is the interesting one: which of this routine's callers reach the table
through some *other* branch.

**"What breaks if I change this?"**

```bash
alsem digest <ws> --file src/Posting.al       # what the code in this file actually touches
alsem digest <ws> --diff my.patch             # same, resolved straight from a diff (or `-` for stdin)
alsem prove <ws> PostDocument may-commit      # yes / no / unknown, with the reason
```

`prove` answers one question about one routine: `may-commit`,
`commits-on-success-path`, `writes-table:<table>`, `publishes-event:<event>`,
`reaches-ui`, `throws-error`.

**"What changed between two versions?"**

```bash
alsem diff <old-ws-or-snapshot> <new-ws-or-snapshot>   # ABI, table schema, events, capabilities, permissions
alsem events fanout <ws>                               # publishers, subscriber counts, coverage
alsem events chains <ws>                               # follow each publisher through its subscribers
```

**Plumbing.**

```bash
alsem policy check <ws>                                 # bundled default rules, or your own al-sem.policy.yaml
alsem policy check <ws> --policy team-rules.yaml         # an explicit rule file
alsem policy explain no-commit-in-event-subscribers      # what one rule actually says
alsem fingerprint <ws>                                  # capability summary per entry point
alsem cache prune --dry-run                             # tidy ~/.al-sem/cache/
```

`policy` is worth a look even though the name is dull: it enforces team rules that no
generic checker knows about — "no `Commit` in an event subscriber", "no HTTP calls from a
table trigger", "install codeunits must not write business data". Those three ship in the
bundled default rule set; you can add your own in a YAML file.

## Why you can trust the answers

Most analysis tools stop at your workspace boundary, or start guessing once code gets
indirect. This one doesn't do either.

On a real ~18,000-call Business Central workspace, the engine resolves **every single
call** — through interfaces, event publishers, record and page built-ins, and into
dependencies that ship only symbols. Nothing lands in an "I couldn't work this out"
bucket. Calls that genuinely can't be known before runtime are reported as exactly that,
never guessed at and never silently dropped.

That's what makes the rest of it worth using. When `alsem query touches` says nothing
else writes to your table, or the code lens says a procedure has no callers, it's a
measured statement about your whole program — not a best effort.

Full breakdown, the measurement method, and how to reproduce it:
[docs/resolution.md](docs/resolution.md).

## Speed

Numbers below are medians from a 1000-file synthetic benchmark on a dev machine. A
release-mode CI gate (`tests/perf_bounds.rs`) runs on every pull request and fails if any
of them exceeds 3x its target — the targets sit well above the measurements, so the gate
catches an order-of-magnitude regression rather than ordinary noise.

| Operation | Measured |
|-----------|----------|
| Index 1000 files from scratch | ~75 ms |
| "Who calls this?" / "what does this call?" | ~8 µs / ~7 µs |
| "Who calls this?" on a procedure with 999 callers | ~16 ms |
| Reparse after a body edit | ~13 ms, plus ~8 ms to refresh diagnostics |

The 999-caller case is slower on purpose: every caller's position is re-read from that
file's current text rather than served from a stored range, so the answer can't be stale.
16 ms is still invisible in a "who calls this" panel.

## Configuration

| Location | Purpose |
|----------|---------|
| `~/.al-sem/config.json` | Diagnostic thresholds, telemetry opt-out |
| `<workspace>/.al-sem.json` | Per-workspace overrides |
| `--no-watcher`, `--no-telemetry`, `--verbose` | Runtime flags (see `--help`) |

If you used this before it was renamed, your existing `~/.al-call-hierarchy/config.json`
and `<workspace>/.al-call-hierarchy.json` are still read when the new files are absent, so
nothing resets. `AL_CH_TELEMETRY` keeps working alongside `AL_SEM_TELEMETRY` too.

## Telemetry

Anonymous, opt-out failure-diagnostics telemetry helps find resolution gaps hit by real
projects. **No raw identifiers, paths, or source leave your machine** — identifier names
are salted-hashed per installation. Off by default in debug builds, tests, and CI.
Disable via `AL_SEM_TELEMETRY=0` / `DO_NOT_TRACK=1`, `--no-telemetry`, or the config file.
Details: [docs/telemetry.md](docs/telemetry.md); auditable source:
[src/telemetry/](src/telemetry/).

## Working on the engine itself

- [docs/architecture.md](docs/architecture.md) — how the pieces fit together, and where
  to find them
- [docs/resolution.md](docs/resolution.md) — the call-resolution taxonomy and its
  measurement
- [CLAUDE.md](CLAUDE.md) — build commands, testing rules, golden-file workflow
- [CHANGELOG.md](CHANGELOG.md) — full history

---

**Author**: Torben Leth
**License**: GPL-3.0 (see [LICENSE](LICENSE))
