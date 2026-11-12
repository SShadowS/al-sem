# One-shot impact builds the program once (TLS-28)

Status: approved by the orchestrator 2026-11-12 (plan and guards). Task TLS-28 (cdo-gate's long
pole), option (A).

## 1. The problem

`cdo-gate`'s last phase, `output_cdo_impact`, took 414 / 466 / 421 s in the three gates of
2026-11-12. It is 2 tests running 5 one-shot `alsem impact` processes on CDO at once:
`cdo_impact_ratchets` (`--diff` JSON, `--diff` human, `--routine`) and
`cdo_impact_base_ratchets` (`--base` JSON `--no-tests`, `--base` human).

One `alsem impact --diff` on CDO (TLS-21's 8-worker trace, release-fast, 145 s wall):

| step | s |
|---|---:|
| main part: program + context build | ~29 |
| main part: digest batches (`digest.stream_batch` ×22) | 70.7 |
| up part: program build again (`program_build_full` + `resolve_dependency_bodies`) | ~10 |
| up part: `graph.sited` + the up walk | 2.4 + 8.4 |
| tests-select part: program build again | ~10 |
| tests-select part: `graph.sited` + select | 2.3 + 1.7 |

The digest batches are already parallel inside (`QueryCx::run` is a `par_iter`;
`prove.reach_effects` is TLS-22's pass): 66 of their 70.7 s. They run one after another on
purpose (one batch's witnesses alive at a time). That is option (B), small, after this.

The program is rebuilt because `impact::run::orchestrate` runs its parts in order (up needs
main's roots, tests-select main's ids), and the CLI runs each part through
`run_part_with_stdin` → `batch::standalone_output`: a fresh read and a fresh drive, so a fresh
`build_view_model`. The three parts' `ViewKey`s are equal (workspace, `CrossApp`,
`no_roots_config`).

## 2. The change

**Step 1: the up part's sited graph serves the tests-select part.** The up and tests-select
parts both demand only the sited graph (`ViewNeeds::sited`, no model). `run_view` builds the
program, builds a `SitedGraph` off the lent program (`sited_from_program`), runs each sited
demand's `Job::sited` on it, and drops both. With a cache scope installed, `run_view` keeps the
`SitedGraph` (not the program) and a later view in the same scope whose demands are all
sited-only and whose key matches skips `build_view_model` and runs `Job::sited` on the kept
graph.

- **Scope.** `batch::with_sited_cache(f)` installs a thread-local slot for `f`'s duration and
  clears it on return (and on unwind). Only `run_impact_cmd` installs it, around
  `orchestrate`. No other command, `alsem batch`, `mcp`, `engine-host` or the LSP sees it, so
  nothing else holds a graph longer than today.
- **Key.** `(ViewKey, stamp)`, where `stamp` is `Manifest::consumed(&inputs, single_app).stamp()`
  (`src/engine/resident/manifest.rs`): every input's bytes (length and blake3), the walk
  listings, the packages, the injected test-universe inputs, the engine and grammar versions.
  Two parts that read different bytes miss. A test universe's part carries its injected inputs,
  so it misses against the production graph.
- **Covered.** A hit needs every demand on the view to be sited-only (`sited && !model`;
  `ViewNeeds::SITED` also sets `graph`, and `run_view` already serves such a demand from the
  sited graph alone). A view with a program-only demand (`graph && !sited`) or a model demand
  builds fresh, as today. Both parts declare `ViewNeeds::SITED` (`impact/jobs.rs`,
  `test_select/jobs.rs`).
- **One entry.** The slot holds the last stored graph only; a new store replaces it.

**Step 2: the main part's program serves the up part.** The main part is a model demand. With
the scope installed and the main view's key cross-app, `run_view` builds the `SitedGraph` off
the main view's lent program too (when no demand on the view asked for it, `ViewNeeds::sited`
unset) and stores it, so the up part hits as well. This holds the sited graph through the main
part's model, context and digest peak. **Kept only if peak memory rises by at most a few
percent against master on CDO `impact --diff` and `--base`; otherwise step 1 alone ships and the
note records the numbers.** Declared-needs rule: this changes a lifetime, never what a build
holds or drops for a view that needs it.

Whether the main view's program yields the same `SitedGraph` as a sited-only build's program
(their profiles may differ) is checked by guard 1 (byte-identical answers), not assumed.

## 3. Guards (each with a discrimination proof)

1. **A hit gives the cold answer.** An impact on a fixture whose parts hit (step 1: up then
   tests-select; step 2: main then up) gives byte-identical output to the same impact with the
   scope not installed. Precondition asserted: the hit happened (a test counter of hits ≥ 1).
   Break: store a graph built from a different program (or skip a member), and the test fails.
2. **A different key or input misses.** Two views in one scope with a different `ViewKey`
   (universe or workspace) or different input bytes (one `.al` file changed between the two
   reads, hand-stated) do not hit (hit counter 0) and answer from their own build. Break:
   compare only the `ViewKey` (drop the stamp), and the changed-bytes case fails.
3. **Uncovered needs build fresh.** A view with a model or `graph` demand after a stored graph
   builds (`build_view_model` ran: a build counter). Break: let any view hit, and the test fails.

Plus: outside `with_sited_cache` nothing is stored (a `run_view` without the scope leaves the
slot empty).

## 4. Measurement

On CDO (release-fast, CDO window from the orchestrator, ≥ 35 GiB free and load < 10 before each
run): `impact --diff` (JSON), `--base` (JSON `--no-tests`) and `--routine`, master against the
branch: wall, CPU, peak; the outputs byte-equal. Then one whole `cdo-gate` for the
`output_cdo_impact` phase time. The `manifest.stamp` cost per view is reported (it hashes every
input; expected well under the ~10 s build it saves).

## 5. Not in scope

- The MCP impact tool already runs its parts on a resident engine child.
- Option (B), the digest batch tail, and the duplicate JSON/human runs in the gate tests
  (option C, dropped by the orchestrator).
- The `--base` side's own build is a different workspace (the base copy): a different key.
