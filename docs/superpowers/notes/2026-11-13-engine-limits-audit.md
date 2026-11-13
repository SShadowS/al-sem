# Engine limits and open engine tasks against master (TLS-44)

`docs/engine-limits.md` was written on 2026-11-09 (master `58d0c94e`). Since then waves 0–2,
ENG-01..12, 18, 22, 25 and 27 and the TLS lane landed. This audit marks every item fixed, still
open or changed against master `14fad659`, and checks every open engine task (ENG-13 to ENG-29)
for whether its claim still holds.

**Method.** Two read-only agents (sonnet) read the master tree, the CHANGELOG, `git log` and the
coord task files, one for the limits doc and one for the open tasks. Nothing was built or run, so
`still-open` / `still-true` means the gap is still in the code as read, not that it was
reproduced. A `fixed` verdict needs the code that fixes it, not only a CHANGELOG line. I checked
the claims that change a doc row or a task's scope in the source:
- `pin_needs` called from `src/bin/alsem.rs:4616` (B1);
- `is_checked_codeunit_run` reads `PCallSite::checked_run` (`transaction_spans.rs:555-562`, L23);
- `resolve_implicit_trigger` maps only `On<Op>` names (`resolver.rs:2976-2990`, ENG-15);
- `RoutineNodeId` has no kind field (`node.rs:227`, ENG-20);
- there is no `may_return`, only `control_flow::may_exit` (ENG-14);
- `dataset.rs:16`: "`RunRequestPage` runs no dataset" (ENG-19).

## Summary

**Open engine tasks: none is fixed or fully covered by another. Two need a re-scope, two pairs
should merge, and four dependencies are ordering only.**

| task | verdict | recommendation |
|---|---|---|
| ENG-13 blank-field guard (d44) | still-true | keep; drop its dep on ENG-17 (ordering only) |
| ENG-14 #310 remainder | partly | keep, **re-scope**: the `may_return` item names a function that does not exist; the real gap is that `unconditional_exit_kind` / `may_exit` miss `CurrReport.Skip`/`Break` |
| ENG-15 table-extension `OnBefore/OnAfter<Op>` | still-true | keep; reuse `OpenFamilies::of`'s stem rule (`program_calls.rs:2049-2056`) instead of a second name table; **merge with ENG-16** (same function) |
| ENG-16 symbol-only table has no trigger nodes | still-true | keep, after or with ENG-15 |
| ENG-17 field guard and RecordRef table events | still-true | keep as measure-first; drop its dep on ENG-21 (ordering only) |
| ENG-19 `Report.RunRequestPage` not a run | still-true | keep, **merge with ENG-21** (one request-page spec) |
| ENG-20 same-name procedure hides a trigger | partly | keep, **re-scope**: the model's stable id already hashes the kind (`ids.rs:326`); the collision is the program graph's `RoutineNodeId` (no kind; first entry wins, `graph.rs:24/139`). Likely no stable-id or golden move; probe first |
| ENG-21 cancelled request page, `OnPreReport` | still-true | keep, merged with ENG-19 |
| ENG-23 no test for `CbGraph::of_tree`'s exclusion | still-true | keep (small) |
| ENG-28 fingerprint hashes `opN`/`loopN` | still-true | keep |
| ENG-29 non-UTF-8 injected folder | still-true | keep; add `manifest.rs:798`, a second `injected_spelling` reader |

ENG-24 and ENG-26 are in progress and own L28's d50 walk and L36's `protected var` remainder.

**The limits doc: about 130 rows.** Roughly 22 are fixed, 6 changed and 95 still open; about 25 of
the open ones could not be decided by reading, and are marked `undecided` with the probe that
would decide them. **Nothing the doc calls fixed is still broken.** Eight passages were wrong or
stale; the first six are corrected in `docs/engine-limits.md` in this change (marked "TLS-44
audit"):
1. B1's row described the crash as live; fixed in wave 0 (`pin_needs` at `alsem.rs:4616`).
2. B3's row read in the present tense; fixed in wave 0.
3. The B8 list read as open; all but the query `scope:` line (`effect_query_cli.rs:5611/5618`)
   and `.gitignore` are fixed, and B8.4's remainder is L15.
4. L23 item 4 said `is_checked_codeunit_run` ignores a codeunit variable's `.Run`; false since
   ENG-22. **Not corrected here:** the same stale sentence in the doc comment at
   `src/engine/l5/traps.rs:83-86` (code, so left for an engine change).
5. Section 3 said L35/L36 are in no wave; both are fixed by ENG-12, waves 0–2 are done and most
   of L28 is fixed by ENG-22.
6. L14's status word was `confirmed` though fixed in wave 1; now `fixed`.
7. Not corrected: the evidence pointer `target/tmp/engine-limits/` is not in git.
8. Not corrected: the status key has no word for "mostly fixed, remainder stated" (L28 stays
   `partial`, L36 is `fixed` with a remainder).

**Unowned:** every L-number planned for waves 3–8 (L1–L6, L9–L13, L15, L17–L19, L21–L22,
L24–L27, L29–L34) has no coord task, nor do the two open B8 lines.

## Detail: the limits doc (agent table, master `14fad659`)

### 1. Wrong answers and a crash

| item | doc says | verdict | evidence | closed by / owner |
|---|---|---|---|---|
| B1 | row text describes the crash as live; header says fixed in wave 0 | fixed (row text stale) | `pin_needs` called from `src/bin/alsem.rs:4607-4616`; `PrepareError::Needs` refusal in `src/engine/resident/revision.rs:1835`; CHANGELOG "engine limits wave 0, B1" | wave 0 |
| B2 | fixed in wave 2, remainder in "B2 in detail" | changed (fixed, with a remainder) | see the B2 rows below | wave 2; remainder: ENG-14 (split shapes only) |
| B3 | row text says wrong `no`; header says fixed in wave 0 | fixed (row text stale) | CHANGELOG "wave 0, B3"; `RoutineNode::publisher_isolated` | wave 0 |
| B4 | fixed in wave 2 | fixed | `commit_scope::declared` (`src/engine/l5/commit_scope.rs`); CHANGELOG "wave 2, B4/L16" | wave 2 |
| B5 | fixed in wave 2, left in L19/L35/L36 | fixed (L35, L36 now fixed too) | CHANGELOG "wave 2, B5" | wave 2, ENG-12 |
| B6 | fixed in wave 2, left in L20 | fixed (L20 remainder open) | `src/program/resolve/dataset.rs`; CHANGELOG "wave 2, B6" | wave 2; remainder ENG-19/20/21 |
| B7 | fixed in wave 2, left in L23/L28 | fixed (L28 mostly fixed by ENG-22) | `src/engine/l5/traps.rs`; CHANGELOG "wave 2, B7/L23 + L30" | wave 2, ENG-22 |
| B8 | header: `scope:` line and `.gitignore` still open | changed (6 of 8 fixed, 2 open) | rows below | wave 0; open: unowned |
| B8.1 fingerprint prints `W` for insert | listed as wrong | fixed | CHANGELOG "wave 0, B8 first half"; `src/engine/l5/table_rights.rs` | wave 0 |
| B8.2 single-app throws-error follow-up | listed as wrong | fixed | `FollowUp::cross_app_only` (`src/engine/follow_up.rs`) | wave 0 |
| B8.3 `--memory-budget` help says 6G | listed as wrong | fixed | help text from `budget.rs` constants (`src/engine/resident/budget.rs:78`) | wave 0 |
| B8.4 `locks: none observed` blames dependency depth | listed as wrong | changed | blames depth only when the cone enters dependency code (`effect_query_cli.rs:5506-5525`); the UpdLock half is L15 | wave 0; rest L15 (unowned) |
| B8.5 interface call with no implementer | listed as wrong | fixed | CHANGELOG "wave 0, B8 first half" (`unresolved_cone::place_items`) | wave 0 |
| B8.6 stale answer labelled `served: fresh` | listed as wrong | fixed | CHANGELOG "wave 0, B8 second half"; `freshness {requested, served, residual}` | wave 0 |
| B8.7 explain repeats dependency names | listed as wrong | fixed | CHANGELOG "wave 0, B8 second half" | wave 0 |
| B8.8 test-app discovery stops at 64 `app.json` | listed as wrong | fixed (for fixture folders; cap kept) | `src/engine/test_select/scope.rs:71-86` | wave 0 |
| B8 open: query `scope:` line says "one dependency routine deep" with no dependency entered | still open | still-open | `effect_query_cli.rs:5611` and `:5618` print it with no `enters_dependency` check | unowned |
| B8 open: test-app discovery ignores `.gitignore` | still open | still-open | no `gitignore` in `src/engine/test_select/` | unowned |
| B9 | fixed by ENG-06; left: extension `OnBefore/OnAfter<Op>` | fixed; left still-open | `resolve_implicit_trigger` maps only base names (`resolver.rs:2976-2990`) | ENG-06; left: ENG-15 |
| B10 | fixed by ENG-06; left (1) fixed by ENG-12; left (2) symbol-only table | fixed; left (2) still-open | `SiteFacts::record_table_unresolved` (`full.rs:258`); `call_result_record` (`demand.rs:47`) | ENG-06, ENG-12; left (2): ENG-16 |

### 2. B2 "Left" list

| item | verdict | evidence | owner |
|---|---|---|---|
| no configuration chosen | still-open | `preprocessorSymbols` read nowhere in product code | L22 (unowned) |
| temp / SourceTable-temporary / ObsoleteState / Access / SingleInstance in one arm | still-open (undecided, not probed) | no CHANGELOG entry | unowned |
| other readers (route text, graph label, ordering `#if` TryFunction, lowerer mark) | still-open (undecided) | no CHANGELOG entry | unowned |
| unreadable `[EventSubscriber]` is a gap | fixed | `unreadable-event-subscriber` in `combined_graph.rs`, `confidence.rs` | wave 2 |
| reading arms incl. split shapes | still-open | split shapes are ENG-14's list | ENG-14 (split shapes); rest unowned |
| imprecise but safe (always-raising arm, both-arms attribute) | still-open | no change | none |

### 3. L-numbers

| item | doc says | verdict | evidence | owner |
|---|---|---|---|---|
| L1 deep reads/locks/validations | found | still-open | scope text `effect_query_cli.rs:1833, 5611` | wave 4 (unowned) |
| L2 rename misses declaration consumers | confirmed | still-open | `effect_query_cli.rs:6903` "not a complete rename inventory" | wave 5 (unowned) |
| L3 no lock state/order | found | still-open | no lock facts in `src` | unowned |
| L4 platform option types | partial | still-open | `domain_of` unchanged (`literal_values.rs:298-307`) | unowned |
| L5 pruning gaps by design | confirmed | still-open | same | unowned |
| L6 ChangeCompany | confirmed | still-open (undecided) | no entry | unowned |
| L7 subscriber error escape; implicit-error naming | partial | changed (B3 part done; naming undecided) | B3 CHANGELOG | unowned |
| L8 guards past an unanalyzed dependency entry | confirmed | still-open (undecided) | no entry | unowned |
| L9 test-selection reach 128 | confirmed | still-open | `REACH_LIMIT: usize = 128` (`test_select/select.rs:112`) | unowned |
| L10 `not resolved` mixes three cases | confirmed | still-open (undecided) | no entry | unowned |
| L11 loaded apps only | confirmed | still-open | `snapshot.rs:51` closed world | unowned |
| L12 digest does not load the test app | confirmed | still-open | `digest_cli.rs:5507-5530` names sibling test apps only | unowned |
| L13 memory climbs to ~8 GB | confirmed | still-open (undecided) | budget code unchanged in kind | unowned |
| L14 speed/memory gate | confirmed | fixed (status word corrected) | `scripts/cdo-speed`; CHANGELOG wave 1; TLS-18/19/20/33/34 since | wave 1 |
| L15 ReadIsolation/UpdLock/`FindSet(true)` not locks | confirmed | still-open | `readisolation`/`updlock` only in `d64.rs` and catalogs | wave 4 (unowned) |
| L16 CommitBehavior | fixed (remainder) | fixed (remainder, below) | `commit_scope::declared` | wave 2, ENG-09 |
| L17 permissions | confirmed | still-open | no change | unowned |
| L18 background sessions | partial | still-open (undecided) | no entry | unowned |
| L19 FlowField reads | partial | still-open as stated | no change after round 4 | unowned |
| L20 dataset ops | fixed (remainder) | fixed (remainder, below) | `dataset.rs`, `resolver.rs:2234 run_triggers` | wave 2, ENG-07, ENG-08 |
| L21 `var` record param caller-dependent | partial | still-open (undecided) | no entry | unowned |
| L22 `#if` configuration | partial | still-open (step 1 done) | see B2 | unowned |
| L23 traps | fixed (remainder) | fixed (remainder; one sentence corrected) | `traps.rs` | wave 2, ENG-10 |
| L24 SOAP/OData published objects | partial | still-open (undecided) | no entry | unowned |
| L25 three "open world" cases | confirmed | still-open | `vocab.rs:124, 163` | unowned |
| L26 freshness misses a same-size/time rewrite | confirmed | still-open | manifest keys size and mtime only (`manifest.rs:34, 74`) | unowned |
| L27 `callers-have-pending-writes` ignores `LockTable` | new | still-open | not in `callers_context.rs` / `pending_writes.rs` | unowned |
| L28 checked run inside a write txn | partial | changed (main fault fixed by ENG-22) | `PCallSite::checked_run`; `transaction_spans.rs:555-562` | ENG-22, ENG-11; d50 walk: ENG-24 |
| L29 Text/Code literals | new | still-open | `domain_of` unchanged | unowned |
| L30 ErrorBehavior::Collect | partial | still-open as stated | no `Collectible` reading in product code | unowned |
| L31 background-target blocker is global | new | still-open | `callers_context.rs:764, 1972` | unowned |
| L32 impact lacks the `roots.config.json` overlay | new | still-open (undecided) | `overlay_config_roots` only in `root_classification.rs` | unowned |
| L33 `query touches --all` lists a temp var-param pass | new | still-open (undecided) | no entry | unowned |
| L34 baseline missing test libraries | new | still-open | owner decision | owner |
| L35 `GetBySystemId` | fixed | fixed | `record_ops.rs:86, 139` | ENG-12 |
| L36 record array elements | fixed | fixed (remainder stated) | `record_ops.rs:30, 108`; `full.rs:252, 1462, 1758` | ENG-12, ENG-25; left: ENG-26 |

### 4. The "in detail" Left lists

- **L16:** item 2 (checked run commit) fixed by ENG-09 and its d50/spans line by ENG-22; the
  `CbGraph` exclusion test is ENG-23; items 1, 3, 6, 7 and item 2's other remainders are still
  open (several undecided), unowned.
- **L20:** `Report.RunRequestPage` (ENG-19), the same-name procedure (ENG-20) and the cancelled
  request page (ENG-21) are still open; the checked import, export failure, LSP trigger edges,
  `DataItemTableView`/`DataItemLink` and the `SaveAsWord` overload are open and unowned.
- **L23:** item 4's last sentence was wrong (corrected); item 6's `DisableWriteInsideTryFunctions`
  is fixed by ENG-10; items 1, 1b, 2, 2b, 5, 6's remainder are still open, unowned.
- **L28:** the main wrong answer and `IsInWriteTransaction()` are fixed by ENG-22; the d50 walk is
  ENG-24; the over-reports and unmeasured run shapes are still open, unowned.
