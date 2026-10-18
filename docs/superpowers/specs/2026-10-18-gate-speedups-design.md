# Gate speed-ups: post-merge identity skip, nextest, sccache on H:

Date: 2026-10-18. Owner: Torben Leth. Status: design, awaiting approval.

## Why

One merged issue spends 15–30 minutes in `post-merge` re-running the four gates on a
cold, disposable worktree (incident log: #103 ≈ 15 min, #80 ≈ 15.5 min, #23 ≈ 28 min),
right after the same gates passed on the same code. Cold builds of three profiles
(dev, release with full LTO, `cdo`) are also what drove free memory to 2 GB on
2026-10-18, when Claude Code killed the gate run and the post-merge twice while a
second session was building.

## Trust model (owner's decision)

The LOCAL gates (`ci-steps all`, `check-goldens --verify-coverage`, `check-goldens`,
`cdo-gate`) are the real check. GitHub CI (Linux, `ubuntu-latest`) is a safety net for
two things: a file left uncommitted, and an OS-specific bug. It stays as it is, and the
merge keeps waiting for it. So the duplicate to remove is the post-merge re-run of the
local gates on code those gates already passed.

## Already in place (no work)

- Faster linker: the repo's `.cargo/config.toml` links with `rust-lld.exe`
  (commit `0adb3ec1`).
- Shared compile cache: the user-level `~/.cargo/config.toml` sets
  `rustc-wrapper = "sccache"` for every repo. It works (4590 cache files written on
  2026-10-18) but sits at its 10 GiB cap (11 GB on disk) on C:, so it evicts.

## Goals

1. **Post-merge identity skip.** When the merged commit is provably the code the gates
   passed, `post-merge` records that and does not rebuild. Otherwise it runs the full
   gates exactly as today.
2. **nextest.** Test runs in the gate scripts use `cargo nextest run`; doc-tests still
   run with `cargo test --doc`.
3. **sccache on H:.** The cache moves to `H:\sccache` (NVMe, 1.2 TB free) with a 50 GiB
   limit.

## Non-goals

- Changing GitHub CI's role or making the merge stop waiting for it.
- Changing what the gates check (`ci-steps all` stays CI's exact bar: release clippy,
  full-LTO release build, `perf_bounds`).
- A shared `CARGO_TARGET_DIR` (concurrent sessions would contend; the executor already
  detects that as `contended-build`).
- Deleting the old C: cache. It stays until the owner removes it.

## Anti-goals (a failure even if it "works")

- A post-merge that reports "verified" for a commit whose code, toolchain or grammar
  differs from what the gates ran on. The identity skip must fail CLOSED: any doubt,
  missing evidence or error means the full gates run.
- A skip that is silent. The payload, the incident record and the run report must
  say plainly which path was taken and why.
- nextest changing which tests run or what counts as a pass (every test that runs
  today still runs; doc-tests included).

## 1. Post-merge identity skip

### When the skip applies (all must hold; otherwise full gates)

The evidence is this run's attestation (`.agent/runs/<run>/attestation.json`, written by
`attest`: `B`, `H`, `final_head`, `gates`, `issue`, …) and this run's merge record.

1. An attestation exists for this run, names the same issue, and every gate in it is `0`
   (`ci-steps-all`, `check-goldens-coverage`, `check-goldens`, plus `cdo-gate` unless it
   attested docs-only).
2. The merge commit's first parent is the attested base `B` (the squash landed on the
   base the gates were run against, so `origin/master` had not moved).
3. The merge commit's tree equals `final_head`'s tree, byte for byte (`git rev-parse
   <sha>^{tree}`). A squash of the branch onto an unmoved base produces exactly that.
4. `H..final_head` changes only evidence files (the same rule `freeze-check` applies;
   it already passed before `attest`, and is re-checked here, not trusted).
5. The environment the gates ran in is the same one post-merge would use: the
   `tree-sitter-al` grammar commit (a dirty grammar clone refuses the skip), the
   `rustc --version`, and, when `cdo-gate` is required, the CDO workspace identity
   (path, git SHA, dirty state, dependency closure stamp — the same three stamps the
   CDO goldens carry) recorded when the gates ran equal the current ones. An attestation
   without them never qualifies for the skip. *(My call: CDO identity included.)*
6. **Each required gate has an executor-written record** that passed on H's tree. Today
   `cmd_run` (cli.py ~646-675) only prints JSON, and the attestation's `gates` map is
   typed by the conductor, so it is a claim, not evidence. New: every `run` appends a
   record to `.agent/runs/<run>/gate-runs.jsonl`: gate name, exit code, HEAD, tree,
   clean/dirty after the run, `rustc --version`, grammar commit, CDO identity (for
   `cdo-gate`), supervised/timed-out. The skip requires, for each required gate, that
   gate's LATEST record to have exit 0, a clean tree, and tree == H's tree. A gate run on
   a dirty tree or on another tree does not count. *(My call: records are the
   executor's, not the conductor's.)* Which gates are required follows post-merge's own
   rule: `cdo-gate` whenever post-merge's own `_docs_only` check would run it, not what
   the attestation claims.

`recover` mints a merge record without an attestation from this run, so a recovered
follow-through never skips; it runs the full gates as today.

### What a skip does

- Still does everything before the gates today: provenance, lane, fetch, fast-forward,
  reachability, opening the `verifying` incident.
- Instead of the gate block: resolves the incident with a note naming the path, e.g.
  `verified by identity: tree <tree> == attested final_head <sha>; gates on H <sha>`,
  releases the lane as a clean verdict does, creates no verification worktree.
- Payload: `ok: true`, `gates` copied from the attestation, plus
  `verified_by: "identity"` (or `"gates"` on the full path) and the reason any skip
  condition failed when it did not apply (`identity_refused: [...]`), so a fallback is
  visible too.
- The path taken is in the payload, the incident record and the run report. No new
  issue comment (no new GitHub write per merge). *(My call.)*

### Escape hatch

`post-merge --full` forces the full gates (operator or conductor choice, e.g. after a
toolchain incident). Default is the identity check.

## 2. nextest in the gate scripts

**Measure first, per script (owner's decision).** nextest runs each test in its own
process. Tests that build a shared context once per process (`program_resolve_harness`'s
`static CDO_SHARED: OnceLock`, the `cli` umbrella's model caches) would rebuild it per
test. So each gate script is timed both ways on the same warm tree (wall time and peak
memory), and a script switches to nextest only where nextest is not worse on either.
Where it is worse (likely `cdo-gate`), that script keeps `cargo test`, with a one-line
comment giving the measured numbers. The measurements go in the CHANGELOG entry. The
switches below apply only to scripts that pass that test.

- `scripts/ci-steps` `test`: `cargo nextest run --workspace` followed by
  `cargo test --workspace --doc`; same env unsets as today. The tree stamp is written
  only when both pass.
- `scripts/check-goldens`: the golden targets via `cargo nextest run` with the same
  target list; the regen path (`REGEN_TEMP_GOLDENS=1`) must keep working.
- `scripts/cdo-gate`: the CDO-gated tests via nextest; `program_resolve_harness` keeps
  running serially (today `--test-threads=1`; in nextest a test group or `-j 1` for that
  invocation, builder's choice).
- `perf_bounds` stays on `cargo test --release` (timing tests; one process is the
  measured setting).
- GitHub CI installs nextest (e.g. `taiki-e/install-action`), so CI runs the same runner.
- If `cargo nextest` is missing locally, the scripts fail with a clear message rather
  than silently falling back (a fallback would make local and CI differ).
- The existing `.config/nextest.toml` holds any settings needed (slow-test timeouts, the
  serial group, a no-fail-fast profile for `check-goldens`, which today runs
  `--no-fail-fast`). Tests that mutate process env (the `cli` umbrella's `ENV_LOCK`) are safe under
  nextest's process-per-test model; the builder confirms no test relies on running in
  one process with another.

## 3. sccache on H:

- sccache's own config file `%APPDATA%\Mozilla\sccache\config\config`:
  `[cache.disk] dir = "H:\\sccache"`, `size = 53687091200` (50 GiB).
- Restart the sccache server (`sccache --stop-server`; the next build starts it).
- Verify with `sccache --show-stats` (cache location `H:\sccache`, max size 50 GiB) and
  a build that records hits.
- Machine-local only; nothing in the repo changes.

## Measurement (before/after)

- Post-merge wall time on a real merge: full path vs identity path (target: identity
  path well under a minute).
- `scripts/ci-steps test` and `scripts/check-goldens` wall time, `cargo test` vs nextest,
  same warm tree.
- sccache hit rate on a fresh worktree's first build, before and after the move.
- Same test counts before and after nextest (no test dropped).

## Constraints

- The executor (`scripts/agentflow`), the gate scripts, `.github/` and `.config/` are
  changed directly at the owner's request, outside the orchestrator (the flow itself may
  not touch them).
- The executor's own test suite (`scripts/agentflow` tests) covers the identity skip:
  each of conditions 1–6 failing on its own forces the full path; all holding skips.
- CHANGELOG entry; CLAUDE.md "Test tiers" timings re-measured after.

## Left to the builder

- Exact shapes of the new attestation fields and payload keys (named above as intent).
- nextest profile details and how the serial group is expressed.
- Exact record format of `gate-runs.jsonl` (fields named above as intent).
