# Gate Speed-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `post-merge` rebuilding code the gates already passed, run tests with nextest where it measures better, and give sccache 50 GiB on H:.

**Architecture:** `agentflow run` writes one record per gate run (what ran, on which tree, in which environment) to the run directory. `post-merge` checks the six fail-closed conditions against the attestation, the merge commit and those records. If all six hold it resolves the incident without building; otherwise it runs the gates exactly as today. nextest replaces `cargo test` per gate script only where a measurement shows it is no worse.

**Tech Stack:** Python 3 (`scripts/agentflow`, pytest), bash gate scripts, cargo-nextest 0.9.145, sccache, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-18-gate-speedups-design.md`

## Global Constraints

- Work on branch `gate-speedups` in worktree `U:/Git/al-sem-gate-speedups`. The main checkout stays on `master` (the orchestrator's preflight needs it). Merging to `master` needs the owner's explicit request.
- Edit files with Edit/Write only, never sed/python rewrites. `rustfmt <file>` only, never `cargo fmt`. Stage named paths only, never `git add -A`.
- Fail closed: any missing evidence, unreadable file or git error in the identity check means the full gates run. It never raises out of `post-merge`.
- No silent skip: the payload always carries `verified_by` (`"identity"` or `"gates"`) and `identity_refused` (a list, empty on a skip).
- `ci-steps all` stays CI's exact bar. `perf_bounds` stays on `cargo test --release`.
- No `cargo nextest` installed means the script fails with a clear message, never a fallback to `cargo test`.
- Never set `CDO_WS` globally. Pass `CDO_WS=U:/Git/DO-cdo-baseline/Cloud` per command.
- Don't pipe long gates through `| tail`. Redirect to a log file and grep it.
- CHANGELOG entry under `## [Unreleased]`.

## Review Focus

1. `final_head` or `H` is not available in the root repo (the issue branch was deleted after the squash). The identity check must refuse with a reason, not crash post-merge. Pinned in Task 3 (`test_refuses_when_final_head_is_unknown`).
2. A gate re-ran after a fix: an older red record, then a green one (must count), or an older green, then a red one (must refuse). Pinned in Task 2 (`test_latest_record_wins`).
3. `gate-runs.jsonl` has a torn last line (the run was killed mid-write). A bad line is skipped and the check still decides. Pinned in Task 2 (`test_torn_line_is_ignored`).
4. `CDO_WS` is unset when post-merge runs but `cdo-gate` is required. The check must refuse (`cdo-identity-changed`), never treat "none" == "none". Pinned in Task 3.
5. The grammar clone has local edits. The check must refuse even when the commit matches. Pinned in Task 2 (`test_dirty_grammar_is_recorded`) and Task 3.

---

### Task 1: Move sccache to H: (machine-local, no commit)

**Files:** `%APPDATA%\Mozilla\sccache\config\config` (create; outside the repo).

- [ ] **Step 1:** Record the "before" state: `sccache --show-stats > <scratchpad>/sccache-before.txt`.
- [ ] **Step 2:** Write the config file:

```toml
[cache.disk]
dir = "H:\\sccache"
size = 53687091200
```

- [ ] **Step 3:** `sccache --stop-server`, then `sccache --show-stats`. Expected: `Cache location  Local disk: "H:\sccache"` and `Max cache size  50 GiB`.
- [ ] **Step 4:** In the worktree, `cargo build` (dev). Then run `sccache --show-stats` again and confirm `Cache writes` > 0 and that `H:\sccache` exists. The old C: cache is left alone.

### Task 2: Gate-run records and the environment fingerprint

**Files:**
- Create: `scripts/agentflow/identity.py`
- Modify: `scripts/agentflow/cli.py` (`cmd_run`, ~line 646)
- Test: `scripts/agentflow/tests/test_identity.py`

**Interfaces:**
- Produces:
  - `GATE_RUNS = "gate-runs.jsonl"` (in `ctx.run_dir`)
  - `environment(ctx: Ctx, cwd: Path, *, cdo: bool) -> dict`, which returns `{"rustc": str|None, "grammar": {"commit": str|None, "dirty": bool}, "cdo": dict|None}`. `cdo` is filled only when `cdo=True`: `{"path": CDO_WS or None, "git_sha", "dirty", "closure"}`.
  - `cdo_identity(ws: str | None) -> dict` (`path` None when unset; `closure` is the sha256 over the sorted `(relpath, size, sha256(content))` of every `*.app` under the workspace's own `.alpackages` and each ancestor `.alpackages` up to the git top level, the same discovery `dependencies::discover_app_files` does).
  - `record_gate_run(ctx: Ctx, name: str, result: supervise.Result, cwd: Path, *, supervised: bool, clean_before: bool) -> dict`, which appends one JSON line: `name, exit_code, timed_out, supervised, head, tree, clean_before, clean_after, env (environment(...)), at`.
  - `latest_records(ctx: Ctx) -> dict[str, dict]`: the last parseable record per gate name. Lines that don't parse are skipped.
- Consumes: `Git(cwd).tree_state(refresh=False).semantic` for clean/dirty, `Git(cwd).rev("HEAD")`, `rev("HEAD^{tree}")`. `rustc --version` runs with `cwd`. The grammar is `cli._grammar(ctx)`, i.e. `<root>/tree-sitter-al`. The commit comes from `git rev-parse HEAD` there, and the dirty flag from `tree_state` there.

- [ ] **Step 1: Write the failing tests** in `test_identity.py`. Use a real temp git repo the way `test_cli_flow.py` builds its own (reuse its helpers via import if they are module-level, else a local `git init` fixture). Monkeypatch `identity._rustc` (a seam returning the version string) so no toolchain is needed.

```python
def test_record_names_tree_head_and_env(ctx, tmp_repo):      # one line, tree == rev(HEAD^{tree}), env.rustc == "rustc X"
def test_latest_record_wins(ctx, tmp_repo):                  # red then green -> green; green then red -> red
def test_torn_line_is_ignored(ctx, tmp_repo):                # append '{"name": "ci-st' ; latest_records still returns the good one
def test_dirty_grammar_is_recorded(ctx, tmp_repo):           # edit a tracked file in <root>/tree-sitter-al -> env.grammar.dirty is True
def test_cdo_identity_changes_when_an_app_changes(tmp_path): # .alpackages/a.app bytes edited -> closure differs; no CDO_WS -> path None
def test_cmd_run_writes_a_record(capsys, ...):               # `run --name noop --timeout 1 --cwd <repo> -- python -c pass` -> one record, clean_before True
def test_cmd_run_records_a_dirty_tree(capsys, ...):          # tracked file modified before run -> clean_before False
```

- [ ] **Step 2:** Run `python -m pytest scripts/agentflow/tests/test_identity.py -q`. Expected: FAIL (no module `agentflow.identity`).
- [ ] **Step 3:** Implement `identity.py` per the Interfaces block. In `cmd_run`, probe `clean_before` before `_run_gate`, then call `record_gate_run` after it returns. Skip the record on `GateQueued` (no child ran) and under `--dry-run`. `cdo=(args.name == cli.CDO_GATE_KEY)`.
- [ ] **Step 4:** Run `python -m pytest scripts/agentflow/tests -q`. Expected: all pass (existing `run` tests are unaffected apart from the new file in the run dir).
- [ ] **Step 5:** Commit `scripts/agentflow/identity.py scripts/agentflow/cli.py scripts/agentflow/tests/test_identity.py` with message `agentflow: record every gate run with its tree and environment`.

### Task 3: The identity check (conditions 1–6)

**Files:**
- Modify: `scripts/agentflow/identity.py`
- Test: `scripts/agentflow/tests/test_identity.py`

**Interfaces:**
- Consumes: `mergeops.read_attestation`, `mergeops.read_merge_record`, `mergeops.freeze_violations`, `cli._gate_failures`, `cli._docs_only` (import inside the function to avoid a cycle), and Task 2's `latest_records`, `environment`.
- Produces: `identity_refusals(ctx: Ctx, git: Git, issue: int, merge_sha: str, gate_names: list[str], docs_only: bool) -> list[str]`. An empty list means skip. It never raises: any exception becomes `"error:<ExcType>"`.

Refusal codes (exact strings, one per failed check; collect all, don't stop at the first):

| Code | Condition |
|------|-----------|
| `not-merged-by-this-run` | merge record missing, or its `source != "merge"` (recover never skips) |
| `no-attestation` / `attestation-other-issue` | condition 1 |
| `attested-gates-not-green` | `_gate_failures(att.gates, docs_only)` non-empty |
| `base-moved` | `rev(merge_sha^1) != att.B` |
| `tree-differs` | `rev(merge_sha^{tree}) != rev(att.final_head^{tree})` |
| `non-evidence-after-H` | `freeze_violations(git, att.H, att.final_head, issue)` non-empty |
| `no-record:<gate>` | no record for a name in `gate_names` |
| `record-red:<gate>` | latest `exit_code != 0`, or `timed_out`, or not `supervised` |
| `record-dirty:<gate>` | `clean_before` or `clean_after` false |
| `record-other-tree:<gate>` | record `tree != rev(att.H^{tree})` |
| `rustc-changed:<gate>` / `grammar-changed:<gate>` | differs from `environment(ctx, ctx.paths.root, cdo=False)`, or grammar dirty now or then |
| `cdo-identity-changed:<gate>` | for `cdo-gate`: record's `cdo` != current `cdo_identity(os.environ.get("CDO_WS"))`, or either side's `path` is None |

- [ ] **Step 1: Write the failing tests.** Build the happy state by hand: a repo with B, then H (code commit), then final_head (one evidence file `.agent/issue-8/ledger.md`), a squash commit on B with final_head's tree pushed to origin, an attestation, a `source="merge"` merge record, and one green clean record per gate on H's tree with env equal to the current env (monkeypatch `identity._rustc`). Then:

```python
def test_all_conditions_hold_skips(...):            # identity_refusals(...) == []
@pytest.mark.parametrize("breaker, code", [...])    # one case per row above, each breaking ONE thing:
def test_each_condition_alone_refuses(breaker, code, ...):   # assert code in refusals (prefix match for :<gate>)
def test_refuses_when_final_head_is_unknown(...):  # att.final_head = "f"*40 -> "tree-differs" or "error:*", no raise
def test_cdo_ws_unset_refuses_when_cdo_required(...)
def test_recovered_merge_never_skips(...):          # source="recover" -> "not-merged-by-this-run"
```

- [ ] **Step 2:** Run the new tests. Expected: FAIL (`identity_refusals` undefined).
- [ ] **Step 3:** Implement `identity_refusals`, with each row as one guarded check.
- [ ] **Step 4:** Run `python -m pytest scripts/agentflow/tests -q`. Expected: all pass.
- [ ] **Step 5:** Discrimination: with Edit, remove the `tree-differs` check, run the parametrized test and confirm its case fails, then revert with Edit and confirm it passes. Note both outcomes in the commit message.
- [ ] **Step 6:** Commit `identity.py` and `test_identity.py`: `agentflow: identity check for post-merge (fail-closed, six conditions)`.

### Task 4: Wire the skip into `post-merge`, with `--full`

**Files:**
- Modify: `scripts/agentflow/cli.py` (`cmd_post_merge` ~960-1260; parser line ~1743)
- Modify: `scripts/agentflow/incidents.py` (`mark_verified`, add `note: str | None = None`; default keeps today's text)
- Modify: `.claude/commands/orchestrate.md` step 9, `docs/superpowers/specs/2026-09-13-issue-orchestrator-design.md` (post-merge section): one paragraph each on the identity path, `verified_by`, `identity_refused` and `--full`
- Test: `scripts/agentflow/tests/test_cli_flow.py`

**Interfaces:**
- Consumes: `identity.identity_refusals` (Task 3).
- Produces: payload keys `verified_by: "identity"|"gates"`, `identity_refused: list[str]`. The flag is `post-merge --full` (store_true).

Behaviour: after `open_incident(... verifying=True)` and after `gate_list` and `docs_only` are computed, refusals are `["forced-full"]` if `--full`, else `identity_refusals(ctx, git, args.issue, args.merge_sha, [n for n, _, _ in gate_list], docs_only)`. On an empty list:
- create no worktree and run no gate
- `mark_verified(..., note=f"verified by identity: tree {tree} == attested final_head {att.final_head}; gates on H {att.H}")`
- payload `ok: True, gates: att.gates, timeouts: [], revert: None, halt: None, verify_worktree_removed: True`
- release the lane
- the `finally` checkout of `master` still runs

Otherwise the existing path runs unchanged, with `verified_by: "gates"`.

- [ ] **Step 1: Write the failing tests** next to `test_post_merge_happy_path_restores_master_and_reports_ok`. Reuse `record_merge` and `commit_file`. Patch `cli.gates`/`cli.cdo_gate` to a child that writes a marker file, so you can see whether a gate ran.

```python
def test_post_merge_identity_skip_runs_no_gate(...):
    # full happy state from Task 3 -> code 0, out["verified_by"] == "identity", out["identity_refused"] == []
    # marker file absent; no al-sem-verify-* dir; incidents[sha]["state"] == "resolved" and "identity" in note
def test_post_merge_falls_back_to_gates_and_says_why(...):
    # same state minus the attestation -> verified_by == "gates", "no-attestation" in identity_refused, marker present
def test_post_merge_full_forces_the_gates(...):
    # happy state + "--full" -> verified_by == "gates", identity_refused == ["forced-full"], marker present
```

- [ ] **Step 2:** Run them. Expected: FAIL (`KeyError: 'verified_by'` / unknown `--full`).
- [ ] **Step 3:** Implement the code, then the two doc paragraphs. Don't rename any existing payload key or halt reason: `test_docs_drift.py` reads the docs against the code.
- [ ] **Step 4:** Run `python -m pytest scripts/agentflow/tests -q`. Expected: all pass, including every existing `test_post_merge_*` (they have no attestation, so they take the gates path).
- [ ] **Step 5:** Commit `cli.py incidents.py test_cli_flow.py .claude/commands/orchestrate.md docs/superpowers/specs/2026-09-13-issue-orchestrator-design.md`: `agentflow: post-merge skips the gates when the merge is provably the gated tree`.

### Task 5: Measure nextest against cargo test, per gate script (no code change)

**Files:** results file `<scratchpad>/nextest-measure.md` (not committed; the numbers go into Task 6's CHANGELOG entry).

Each pair runs on the same warm tree. Do one warm-up run of each first, then two timed runs per side. Peak memory comes from a psutil sampler (psutil 7.2.2 is installed): a scratch script that runs the command and polls, every 0.5 s, the summed RSS of the child process tree, then prints `wall_s` and `peak_rss_gb`.

| Script | `cargo test` side | nextest side |
|--------|-------------------|--------------|
| `ci-steps test` | `cargo test --workspace` (env unsets as the script) | `cargo nextest run --workspace` + `cargo test --workspace --doc` |
| `check-goldens` | `cargo test --no-fail-fast <targets>` | `cargo nextest run --no-fail-fast <targets>` |
| `cdo-gate` | each of its 4 `cargo test --profile cdo …` lines | the same with `cargo nextest run --cargo-profile cdo …` (the harness with `-j 1`); `CDO_WS` + `ENFORCE_CDO_WS=1` per command |

- [ ] **Step 1:** Count tests on both sides for each script (`cargo test … -- --list` vs `cargo nextest list …`) and confirm the counts match, doc-tests aside. A mismatch stops the task: report it.
- [ ] **Step 2:** Take the timings and fill the table with wall time and peak memory for both sides.
- [ ] **Step 3:** Verdict per script: switch only if nextest's median wall time is ≤ cargo test's AND its peak memory is ≤ 110% of cargo test's. Report the table to the owner before Task 6.

### Task 6: Switch the scripts that passed; nextest in CI

**Files (only for scripts Task 5 cleared):**
- Modify: `scripts/ci-steps` (`run_test`), `scripts/check-goldens` (both `cargo test` lines), `scripts/cdo-gate`
- Modify: `.config/nextest.toml`: add `[profile.goldens]` with `fail-fast = false`. Add `[test-groups] serial-cdo = { max-threads = 1 }` plus an override for `binary(program_resolve_harness)`, only if `cdo-gate` switches.
- Modify: `.github/workflows/ci.yml`: before the `test` step, add `uses: taiki-e/install-action@v2` with `tool: cargo-nextest`. Pin the action by the commit SHA of its current release tag, not a moving tag.
- Modify: `CHANGELOG.md` (Changed: the identity skip, gate records, nextest per script with the measured table; machine note: sccache moved to H:), `CLAUDE.md` "Test tiers" timings. CLAUDE.md is the owner's file, and the change is made at the owner's request.

- [ ] **Step 1:** Add a `require_nextest` helper to `scripts/ci-steps`, beside `require_grammar`:

```bash
require_nextest() {
    cargo nextest --version >/dev/null 2>&1 && return 0
    echo "ci-steps: cargo-nextest is required (cargo install cargo-nextest --locked)" >&2
    return 1
}
```

Copy it into `check-goldens` and `cdo-gate` (the scripts do not source each other today).
- [ ] **Step 2:** Replace the cleared `cargo test` lines. In `ci-steps test`: `cargo nextest run --workspace && cargo test --workspace --doc`, with the same `env -u` prefix and the stamp written only after both. In `check-goldens`: `cargo nextest run --profile goldens "${targets[@]}"` on both the regen and the verify line. Any script not cleared keeps `cargo test`, with the comment `# cargo test, not nextest: measured 2026-10-18 <wall/mem both sides> (per-process shared setup)`.
- [ ] **Step 3:** Run every changed script and redirect to a log: `bash scripts/ci-steps test > <scratchpad>/t.log 2>&1; echo $?`. Expected: `0`, with the test count from Task 5 in the log's summary line. Same for `check-goldens`, and for `cdo-gate` if switched (with `CDO_WS` given per command).
- [ ] **Step 4:** Prove the missing-nextest message: temporarily rename `~/.cargo/bin/cargo-nextest.exe`, run `bash scripts/ci-steps test` and check that it exits non-zero with the message, then restore the file and check that it passes. Record both outcomes.
- [ ] **Step 5:** Commit the scripts with `.config/nextest.toml`, then `ci.yml`, then CHANGELOG/CLAUDE.md, as three commits.
- [ ] **Step 6:** With the owner's ok, push the branch and open a PR; the merge waits for the owner's explicit go. Confirm the GitHub CI test step runs nextest and is green.

### Task 7: Measure the result

- [ ] **Step 1:** After merge, on the next real `/orchestrate` merge, take post-merge wall time from the run log and check `verified_by`. Target: identity path under one minute (was 15–28 min).
- [ ] **Step 2:** Get sccache's hit rate on a fresh worktree's first build (`sccache --zero-stats` before, `--show-stats` after) and compare with the 2026-10-18 C: numbers.
- [ ] **Step 3:** Record both in the CHANGELOG entry (a follow-up commit).
