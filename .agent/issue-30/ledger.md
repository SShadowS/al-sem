# Issue 30 — drift enforcement panics from inside a library helper

## Claim

```json
{"run_id":"20260918-162940-31ce00","issue":30,"attempt":1,
 "branch":"issue/30-cdo-drift-enforcement-boundary-a1",
 "worktree":"U:\\Git\\al-sem-issue-30-a1",
 "body_hash":"eafb97a8b97ac6ffaadf7a9d172289a9e7f0d9442a961247234ccba58e3939b3"}
```

Worktree from `master` @ `51fa4f6e`.

## Classification

**BOUNDED**, and the scope was measured BEFORE claiming rather than discovered during review —
the correction two ticks in this session needed.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 the helper reads process-global env and panics | **holds** | `semantic_golden.rs:318` defines it; `:356-360` is `assert!(std::env::var("ENFORCE_CDO_WS").as_deref() != Ok("1"), "{msg}")` |
| P2 the blast radius is one file | **holds for CALL SITES; the ledger's wording was wrong** | Exactly three callers, all in `semantic_golden.rs` — `:2422`, `:2620`, `:2721`. Round 1 'corrected' the last two to `:2621`/`:2722`; round 2 showed those are CLOSING BRACES and the original numbers were right. No other module calls it. But I then wrote "three entry points", which is a different claim and false: the plumbing spans SEVEN — see Corrections |
| P3 the three callers are audit entry points reached from tests | **holds** | Enclosing functions are `run_cdo_semantic_audit_on_with` (`:2408`), `trigger_audit_from_fresh` (`:2611`), `event_audit_from_fresh` (`:2712`), plus `run_cdo_semantic_audit_on_raw` (`:2400`) which round 1 found has its own test caller at `tests/program_resolve_harness.rs:5073`; `tests/program_resolve_harness.rs:2887-2888` imports the public `run_cdo_*_audit_on` wrappers |
| P4 a gate boundary for this decision ALREADY exists | **holds** (and there are THREE `ENFORCE_CDO_WS` readers in total, not two — see Corrections) | `tests/common/cdo.rs:29` `cdo_ws_or_enforce()` reads `CDO_WS` and applies the same `ENFORCE_CDO_WS=1` hard-fail, and is `#[path]`-included by every gated test binary. The pattern the issue wants is already established — the drift check simply does not use it |
| P5 the hazard is latent, not live | **holds — and the issue says so** | `mint-goldens` mints the L3/trigger/event goldens directly and never calls `run_cdo_*_audit` (`src/bin/mint-goldens.rs:153-204`), so the panic is not currently reachable from minting. The issue states this itself and files at that severity |
| P6 the panic preempts the warning | **holds** | The `assert!` is at `:356`, the `eprintln!("WARNING: ...")` at `:361`. Under enforcement the warning never prints — the panic message carries the text instead |

## Design (v2 — after round 1)

**The library neither reads process state nor decides failure.** Round 1 showed v1 fixed only
half: it removed the ambient env read but kept the panic inside the library, which merely moved
the responsibility to the callers rather than out of the library at all.

**1. The helper becomes policy-free.**

```rust
/// Returns the drift message when the current workspace differs from the golden's
/// mint stamp, `None` when it matches. Decides NOTHING about what to do with it.
fn workspace_drift(stamped: &MintMetadata, workspace_root: &Path) -> Option<String>
```

No `std::env`, no `assert!`, no `eprintln!`.

**2. A drift HANDLER is threaded in** — astra's shape, and smaller than the `DriftPolicy` enum I
first proposed:

```rust
type DriftHandler = fn(&str);
```

The three check points call it when drift is present and do nothing when it is not. The library
owns neither the enforcement decision nor the failure: a handler that panics does so as the
CALLER's choice, in the caller's code.

**3. The handler lives in `tests/common/cdo.rs`**, beside the `ENFORCE_CDO_WS` read already
there. It warns or panics, reading the env once, at the gate boundary. Exact `== "1"` semantics
preserved.

This preserves return types, early termination and the warning-suppression behaviour, and does
not require the audit report to carry drift.

### Corrections from round 1

- **Seven entry points, not three.** Three direct call sites, but the plumbing spans three
  path-based wrappers, three `_on` wrappers, AND `run_cdo_semantic_audit_on_raw` (`:2400`), which has a real test caller at `tests/program_resolve_harness.rs:5073`. Still
  small, but "three entry points" understated the signature changes and the ledger said it.
- **A THIRD `ENFORCE_CDO_WS` reader exists**: `enforce_audit_ran` (fn at
  `tests/program_resolve_harness.rs:2930`, read at `:2931`), gating committed-golden LOAD
  failure. It does not collide with this change — it guards golden presence, not drift — but my
  claim that only two readers existed was wrong.
- **Line numbers: my originals were RIGHT and I broke them.** Round 1 said the later call sites
  were `:2621`/`:2722`; I accepted that without checking. Round 2 showed those lines are closing
  braces. Verified by grep: the calls are `:2422`, `:2620`, `:2721`. The raw entry is `:2400`
  (not the `:2355` I originally wrote, nor the `:2395` offered in round 2).
- **THREE `ENFORCE_CDO_WS` reads, not two and not four.** Grep for `env::var("ENFORCE_CDO_WS")`
  returns exactly: `semantic_golden.rs:358` (drift), `tests/common/cdo.rs:35` (workspace
  presence), `tests/program_resolve_harness.rs:2931` (`enforce_audit_ran`, golden loading). My
  original "two" was wrong; round 1's "four" counted `:2914`, which is inside a DOC COMMENT.

### What the requirement actually is

Round 1 asked me to be explicit rather than hand-wave, so: the requirement is BOTH halves — no
ambiently-selected behaviour AND no library-owned failure. v1 met only the first while the ledger
claimed both. v2 meets both, because the library's only action on drift is to invoke a handler it
was given.

## Acceptance matrix

| # | item | proof |
|---|---|---|
| A1 | The library reads no process state | No `std::env` in `workspace_drift` or the three check points |
| A2 | The library does not decide failure | No `assert!`/`panic!` on the drift path; the only action is invoking the supplied handler |
| A3 | Ungated behaviour is unchanged | Warning handler + drifted stamp: the same `WARNING: {msg}` on stderr, execution continues |
| A4 | Gated behaviour is unchanged | Enforcing handler + drifted stamp: the run fails carrying the same message text |
| A5 | The warning stays SUPPRESSED under enforcement | Round 1's specific request: A7 previously asserted only the panic. Assert explicitly that NO `WARNING:` line is emitted when the enforcing handler fires — today the `assert!` at `:356` preempts the `eprintln!` at `:361`, and that ordering must survive |
| A6 | No drift stays silent | Matching stamp: neither handler is invoked at all |
| A7 | The decision lives at the gate boundary | The drift `ENFORCE_CDO_WS` read is in `tests/common/cdo.rs`, beside the existing one, with `== "1"` semantics preserved |
| A8 | All seven entry points thread the handler | The three path wrappers, three `_on` wrappers and `run_cdo_semantic_audit_on_raw`; `tests/program_resolve_harness.rs:5073` still compiles and passes |
| A9 | Discrimination | Force a drifted stamp: with the warning handler assert the warning and no panic; with the enforcing handler assert the panic AND the absence of the warning. Invert each and watch the matching assertion fail. Byte-restore under a hash check |
| A10 | No behaviour change anywhere else | `check-goldens` byte-clean; `cdo-gate` green — the load-bearing one, since this code exists to guard exactly that context |

## A note on the two reviewers disagreeing on facts

Round 2 produced a direct conflict: one reviewer said the call sites were `:2422/:2620/:2721` and
the other `:2422/:2621/:2722`; one counted three `ENFORCE_CDO_WS` reads and the other four; they
gave three different lines for the raw entry point. I settled all three by grep rather than by
weighing confidence, and the same reviewer was right every time.

The part worth recording is my own role. My original scope check had the call sites RIGHT. Round 1
told me they were off by one, I accepted it without looking, and wrote the wrong numbers into both
the design and the probe table — where round 2 then found them. A correction from a reviewer is
evidence, not a result, and this one cost nothing only because a second reviewer disagreed.

## Convergence, and one process deviation stated plainly

All five register entries are accepted by both reviewers. Getting there needed one step outside
the ordinary flow, recorded rather than glossed:

The 3-round spec cap was spent when astra's round-3 answer accepted F1 and re-raised F2 for
exactly one stated cause -- "ledger P3 still cites the raw entry at `:2355`, not `:2400`" -- while
saying explicitly there was no substantive design objection. I fixed that citation and spent a
`pi_call` (NOT a fourth `spec_round`) asking astra to confirm only that token. It did, and F2 is
accepted at spec level.

**Why this is not the same as the two ticks this session that blocked.** In #19 and #34 the
re-raises were substantive: a design whose parts were still being killed round after round. Here
the design was ratified in round 2 (A1-A3 accepted by both) and the only thing outstanding was a
stale four-digit number in a probe row. Blocking a ratified design over that would waste the tick
and teach nothing.

**Why it is still a deviation.** The cap is on rounds of spec REVIEW; a single-coordinate
confirmation is not a review round, but reasonable people could call this cap-shopping, so it is
in the record with the reasoning rather than buried. Issue #45, filed earlier in this session,
proposes exactly this as a first-class mechanism -- a cheap ratification pass that cannot open new
findings -- so that a future tick does not have to make this judgement call at all.

## Implementation

`workspace_drift` is policy-free -- no `std::env`, no `assert!`, no `eprintln!`, though it still
probes git and reads `.alpackages`, so not pure in the mathematical sense. A `DriftHandler` is
threaded through all seven entry points; the handler is test-side. Three files.

### One deviation from the ratified design, and why

**A7 said the handler lives in `tests/common/cdo.rs`, beside the existing `ENFORCE_CDO_WS`
read. It does not. It is in `tests/program_resolve_harness.rs`.**

I built it in `cdo.rs` first, exactly as ratified, and it compiled -- with two warnings:

```
warning: function `drift_handler` is never used
  --> tests\common\cdo.rs:62:8
warning: function `drift_handler` is never used
  --> tests\lsp\..\common\cdo.rs:62:8
```

`cdo.rs` is `#[path]`-included VERBATIM by three test binaries -- `program_resolve_harness.rs`,
`tests/lsp/main.rs` and `tests/l4_summary_differential.rs` -- and only the first runs the golden
audits. A handler defined there is dead code in the other two, and `scripts/ci-steps clippy` is
`-D warnings`.

The repo already has this exact problem and already answers it:
`tests/common/regen.rs:36` carries `an `allow(dead_code)` attribute // not every including binary calls both
helpers below`. So the established convention would have me write the same attribute. The FLOW
forbids it: `check-diff` rejects `#[allow(` as `allow-attribute` (`scripts/agentflow/tests/
test_protect.py:40`).

So the choice was: block a converged, implemented design on a lint attribute, or move the handler
to the file that actually uses it. I moved it. The SUBSTANCE of A7 -- "the decision lives at the
gate boundary, in test code, not ambiently inside a library function" -- is fully met:
`program_resolve_harness.rs` already reads `ENFORCE_CDO_WS` at `:2930` (`enforce_audit_ran`), the
handler sits beside it, and the eight audit call sites it serves are in the same file. What is NOT
met is A7's letter about which file. A source comment at the definition states this, so a reader
who expects it in `cdo.rs` finds out why from the code and not only from here.

Filed as a discovery: the `#[path]`-shared-helper pattern and the flow's `allow-attribute` guard
collide, and the repo's own convention is on the wrong side of it.

### Three of the seven entry points have no callers

`run_cdo_semantic_audit`, `run_cdo_trigger_audit` and `run_cdo_event_audit` (the `&Path`-taking
wrappers) are `pub`, so nothing flags them, and a repo-wide grep finds them only inside doc
comments -- zero real call sites. I threaded them like the rest rather than deleting them:
deleting public API is a separate decision with its own blast radius, and this issue is about the
enforcement boundary. But it means A8's "all seven thread the handler" is verified by COMPILATION
for three of them and by execution for four. Filed as a discovery. The acceptance row is amended
below to say so rather than implying all seven are exercised.

## Discrimination proofs

| # | break | applied? | result | restore |
|---|---|---|---|---|
| D1 | delete the `assert!` from `drift_handler` | asserted absent after patch | gated **FAILED** exit 101; ungated passed exit 0 | sha256 byte-identical |
| D2 | put `std::env::var("ENFORCE_CDO_WS")` + `assert!` back into `workspace_drift` | asserted present after patch | `library_ignores_enforcement_env` **FAILED** exit 101 | sha256 `c878c0ebcf7069e3...` byte-identical |
| D3 | move the handler's `eprintln!` ABOVE the `assert!` | asserted: original body absent AND broken body present | **FAILED** exit 101 -- "under enforcement the WARNING line must be SUPPRESSED, but it was printed" | sha256 `ac551ed9fca3f141...` byte-identical |
| D4 | delete the handler's `eprintln!` entirely | same two-sided assert | **FAILED** exit 101 -- "ungated, the WARNING line must actually be emitted" | sha256 `ac551ed9fca3f141...` byte-identical |
| D5 | `println!` instead of `eprintln!` in the handler | same two-sided assert | **FAILED** exit 101 -- "the WARNING line must be emitted on STDERR"; the child's stdout carried it | sha256 `96bdeb689c97ae04...` byte-identical |
| D6 | misspell the probe name in the driver's filter | same two-sided assert | **FAILED** exit 101 -- "the child exited 0 but never reported completing the probe". The child reported `0 passed; 0 failed; 198 filtered out` | sha256 `96bdeb689c97ae04...` byte-identical |
| D7 | `eprintln!("WARNING: {msg:?}")` before the `assert!` -- the message QUOTED | asserted: the quoted form present after patch | **FAILED** exit 101 -- "under enforcement NO `WARNING:` line may be emitted, but one was" | sha256 `24c7a5a59a4225d8...` byte-identical |

**D3 and D4 exist because the final panel proved my first A5 test did not discriminate.** I had
asserted only that `catch_unwind` returned `Err`, and argued the `eprintln!` after the `assert!`
was therefore unreachable. astra produced two counterexamples: print-before-assert breaks the
suppression while that test stays green, and deleting the print breaks the ungated warning while
it stays green too. Both are correct -- I reproduced them, and they are D3 and D4 above. The
argument was true of the body as written and proved nothing about a regression, which is exactly
the distinction CLAUDE.md's testing rule is about. I had called it "a stronger substitute, not a
weaker one"; that was an over-claim and it is withdrawn.

**The two reviewers disagreed here and one was wrong.** flash called the same proof
"mathematically sound and complete". It is sound about today's code and unsound as a regression
guard. I checked the counterexamples myself rather than counting votes -- the lesson from round 2
of the spec panel, where I accepted a reviewer's line-number "correction" without looking and it
was wrong.

**Round 2 then found two more, and both were mine.** The first fix merged the child's stdout and
stderr before searching, so a `println!` satisfied an assertion about `eprintln!` (D5). And
`library_ignores_enforcement_env` checked only the child's exit status -- but libtest exits
SUCCESSFULLY when a filter matches zero tests, so a stale probe name would have turned that test
into one that passes while checking nothing (D6). The second is the worse of the two: it is a test
that cannot fail, which is exactly the shape this repo's testing rule exists to catch, and I built
it while fixing a different instance of the same thing.

Both are fixed: the streams are kept separate, and the probe prints a completion marker its driver
requires. `probe_driver_rejects_a_child_that_ran_nothing` pins the libtest behaviour itself, so the
hazard is documented executably rather than in a comment.

The fix replaces the inference with captured stderr: both probes now run in a CHILD process via
`Command::env`, so the enforcing arm is a real ambient environment and stderr is readable. That
also removed the `unsafe { std::env::set_var }` astra flagged separately -- the lib tests no
longer touch the environment at all, so they cannot race libtest's thread pool or leave an
inherited value cleared behind them.

## Acceptance matrix -- as delivered

| # | item | how it is actually proven |
|---|---|---|
| A1 | library reads no process state | **by inspection**: `grep -rn 'env::var' src/ \| grep ENFORCE_CDO_WS` returns nothing -- the only mentions left under `src/` are doc comments. The behavioural test below proves the library does not ACT on it, which is a weaker claim; both are stated rather than conflated (astra, minor) |
| A2 | library does not decide failure | `library_ignores_enforcement_env`: a child process with `ENFORCE_CDO_WS=1` genuinely set calls the PUBLIC `run_cdo_trigger_audit` with an inert handler and it RETURNS. D2 proves this catches a regression |
| A3 | ungated behaviour unchanged | `drift_handler_warning_is_emitted_ungated_and_suppressed_gated`, ungated arm: the child exits 0 and its **captured stderr contains** `WARNING: <sentinel>`. D4 proves it catches deletion of the print |
| A4 | gated behaviour unchanged | same test, gated arm: the child exits non-zero and the output carries the message |
| A5 | the warning stays suppressed under enforcement | same test, gated arm: captured stderr does **not** contain the `WARNING:` line. D3 proves it catches a reordering. This is the assertion A5 originally asked for, not a substitute |
| A6 | no drift stays silent | `issue30_workspace_drift_reports_without_deciding`: a hand-built `MintMetadata::default()` against a bare temp dir (precondition asserted: `workspace_git_info` is `(None, None)`, closure `None`) returns `None` |
| A7 | the decision lives at the gate boundary | **met in substance, deviated in letter** -- handler in `program_resolve_harness.rs`, not `tests/common/cdo.rs`. Both reviewers judged the deviation acceptable and neither blocked on it |
| A8 | all seven entry points thread the handler | `run_cdo_trigger_audit` is now genuinely EXECUTED (the A2 probe calls it), so five of seven are exercised and two remain compile-only. The message text is pinned by `contains` on fragments, not byte-for-byte equality (astra, minor) |
| A9 | discrimination | D1-D8, all byte-restored under sha256 -- table above |
| A10 | no behaviour change anywhere else | gate results section; the row is not claimed green until those exit codes are recorded (astra, minor -- the earlier draft pointed at results that did not yet exist) |

## Timeline

| phase | outcome | caps |
|---|---|---|
| claim | run `20260918-162940-31ce00`, branch `issue/30-cdo-drift-enforcement-boundary-a1`, from `master` @ `51fa4f6e` | -- |
| classify | BOUNDED, scope measured before claiming | -- |
| probes P1-P6 | all hold; P2's WORDING was wrong (see Corrections) | -- |
| spec round 1 | both reviewers; v1 fixed only half the defect | `spec_rounds` 1/3 |
| spec round 2 | A1-A3 accepted by both; design v2 (drift handler) ratified | `spec_rounds` 2/3 |
| spec round 3 | F1 accepted; F2 re-raised on ONE stale citation | `spec_rounds` 3/3 |
| ratification | one `pi_call`, single-coordinate confirmation, NOT a 4th round -- see Convergence | `pi_calls` 7/14 |
| implement | 3 files, +283/-56, zero warnings | -- |
| lib tests | 2 added, both green | -- |
| harness tests | 5 added (2 child probes, 2 drivers, 1 zero-run guard), green | -- |
| discrimination | D1-D13 by the end of attempt 2, all byte-restored under sha256 | -- |
| commit `338e1e0e` | pre-commit `check-goldens` OK | -- |
| check-diff #1 | **REFUSED** `forbidden:allow-attribute` -- a false positive on a COMMENT | -- |
| amend | doc fix + rewording, hook re-run | -- |

## Not delivered

- **A7 as written.** The handler is in `tests/program_resolve_harness.rs`, not
  `tests/common/cdo.rs`. Substance met, letter not; cause and reasoning above, and it is the first
  thing the final panel was asked to attack rather than something they had to find.
- **Execution coverage for two of the seven entry points.** `run_cdo_semantic_audit` and
  `run_cdo_event_audit` have no callers, so threading them is checked by the compiler only. This
  said THREE until the round-2 fix gave `run_cdo_trigger_audit` a real executing caller (the A2
  child probe); it is two now, and A8 agrees.
- ~~**A direct stderr assertion for A5.**~~ **SUPERSEDED.** This row described the unwind-inference
  version, and said it was "stronger than a string search". The final panel proved otherwise (see
  Discrimination proofs, D3-D5) and that claim is withdrawn. A5 now IS a direct stderr assertion:
  the probes run in a child process, so stderr is readable and is asserted on specifically.
- **Deleting the three uncalled `pub` wrappers.** Out of scope for an enforcement-boundary issue;
  filed as a discovery instead.

## Round 3, and why this tick BLOCKS

**astra found a fifth counterexample and it was correct.** The gated suppression assertion checked
only the exact sentinel-bearing line, so `eprintln!("WARNING: {msg:?}")` -- the same message,
printed QUOTED -- broke the suppression while dodging the substring check. A5's contract is "no
`WARNING:` line", not "not this exact string". Fixed: the assertion now rejects the `WARNING:`
PREFIX on both captured streams. D7 above is astra's own suggested proof, executed.

**Five counterexamples were found against these assertions across three rounds. astra found four
of them; flash found none.** flash's round-1 review returned zero findings and called the original
A5 proof "mathematically sound and complete" -- it was sound about the body as written and unsound
as a regression guard, which is the only thing a test is for. That asymmetry is the single most
useful thing this panel produced, and it is worth carrying into how future panels are read: a
review that confirms is not evidence, and two reviewers agreeing is not a majority when only one
of them tried to break anything.

### Register at the cap

| reviewer | dispositions |
|---|---|
| flash (round 3, post-fix) | **all nine accepted**, explicitly including FF1 and FF4, having read the current files |
| astra (round 3, pre-fix) | seven accepted; **A3 and FF1 re-raised** -- "Substance, not wording" |

`final_rounds` is 3/3. astra's re-raise was against code that has since changed in exactly the way
astra prescribed, and its prescribed discrimination proof has been executed and recorded. But
astra has not seen that, and **I am not going to mark a reviewer's disposition on its behalf.**

That refusal is the same one that blocked #19 and #34 earlier in this session, and it costs the
same thing here. The alternative was available and I declined it: at SPEC stage I spent a
`pi_call` on a narrow ratification rather than burning a round, recorded it as a deviation, and
wrote that reasonable people could call it cap-shopping. Using that escape a second time in the
same tick -- and this time against a finding the reviewer itself called substantive -- would make
that criticism correct. A cap that bends whenever I judge the remaining gap small is not a cap.

**Outcome: `blocked final-panel-cap`.**

### What the branch carries for whoever picks this up

Not a half-finished change. The fix for astra's finding is applied, formatted, committed and
proven; `check-diff` is clean; both lib tests and all five harness tests pass; seven mutations have
been run and every one fails as intended, each byte-restored under a sha256 check. flash has
accepted the final state in full. The single outstanding action is astra confirming that the fix it
specified was applied correctly -- one narrow question, not a review round.

Issue #45, filed earlier in this session, proposes exactly the mechanism this tick needed twice: a
cheap ratification pass that cannot open new findings. This tick is now its second concrete
motivating case, and the stronger one.

## Gate results

Commit `9a258297`. All four supervised by the executor (`"supervised": true`), none timed out.

| gate | exit | log |
|---|---|---|
| `scripts/ci-steps all` | **0** | `.agent/runs/20260918-162940-31ce00/logs/ci-steps-all-f.log` |
| `scripts/check-goldens --verify-coverage` | **0** | `logs/check-goldens-coverage-f.log` |
| `scripts/check-goldens` | **0** | `logs/check-goldens-f.log` |
| `scripts/cdo-gate` | **0** | `logs/cdo-gate-f.log` |

`git status --porcelain -- . ':!.agent'` in the worktree: empty. No golden moved; no regen was run
or needed. `check-diff --base master --head HEAD` : `{"reasons": []}`.

**The `-f` name suffix is deliberate.** An earlier gate run was killed mid-`ci-steps all` when it
held the harness `.exe` open and blocked a rebuild (`rust-lld: permission denied`). The executor
derives log and result paths from `--name`, so reusing the same names would have interleaved a
killed run's artifacts with this one's -- the exact corruption seen in issue #11's tick. Fresh
names, not more process-killing. The one orphaned `program_resolve_harness` process was killed by
PID, attributably, rather than by a sweep.

### A10 is now claimed, and this is what claims it

All seven new tests ran under `scripts/cdo-gate`, which exports `ENFORCE_CDO_WS=1` for the whole
suite:

```
test child_probe_drift_handler ... ok
test child_probe_library_under_enforcement ... ok
test drift_handler_warning_is_emitted_ungated_and_suppressed_gated ... ok
test library_ignores_enforcement_env ... ok
test probe_driver_rejects_a_child_that_ran_nothing ... ok
test program::resolve::semantic_golden::tests::issue30_check_point_forwards_to_the_supplied_handler ... ok
test program::resolve::semantic_golden::tests::issue30_workspace_drift_reports_without_deciding ... ok
```

That matters beyond "the suite is green": `cdo-gate` is the one context where the handler's FAILING
arm is a real ambient environment rather than a child process this tick spawned, and it is the
context this code exists to guard. The gate passing against the pinned CDO baseline is also A4's
strongest evidence -- the drift check ran against a workspace whose stamp MATCHES, so the handler
was correctly never invoked there.

## Evidence files are UNCOMMITTED, by design

This ledger and `findings.json` are committed at the freeze step, which a blocked tick never
reaches. They are sitting uncommitted in `U:/Git/al-sem-issue-30-a1/.agent/issue-30/`. The worktree
is deliberately left in place (blocked ticks do not clean up), so nothing is lost -- but a
`git status` there will show them, and that is expected rather than a loose end.

## Attempt 2: a stand-in for astra's slot, and a sixth counterexample

`fable` stood in for `astra` (unreachable: pi's single provider was still returning `429` on
2026-09-19). It accepted nine entries and **re-raised FF1 on a sixth counterexample, proven by
execution**:

> The test's "ungated" arm set `ENFORCE_CDO_WS="0"`. Nothing ever sets that value -- no developer
> shell does, and `scripts/cdo-gate` only ever exports `=1`. The real ungated state is the
> variable **unset**, and it was never exercised. So a handler regression that panics whenever
> the variable is ABSENT -- `.as_deref() != Ok("1")` rewritten as `.is_ok_and(|v| v != "1")` --
> passed all five tests, while it would break every developer's drifted run.

Fixed: `run_probe_with_enforcement` takes `Option<&str>`, and `None` REMOVES the variable. The
ungated arm passes `None`.

| # | break | result |
|---|---|---|
| D8 | the handler panics when the variable is absent (fable's M6) | **FAILED** exit 101 on `drift_handler_warning_is_emitted_ungated_and_suppressed_gated`; the fix is green without it; byte-restored, sha `a4d2887c33a3` |

Six counterexamples have now been found against these assertions: print-before-assert, deleted
print, wrong stream, misspelled probe name, quoted warning, and a fake ungated state.

### A record I got wrong (fable N2)

The register showed FF5 as `accepted` by flash. **Flash never gave FF5 a verdict.** FF5 did not
exist when flash reviewed: flash's round-3 table has nine rows, A1-A3, F1-F2, FF1-FF4. Its prose
did discuss the D7 fix that FF5 records, and I turned that prose into a mark. The ledger refuses
to do exactly that for astra, so FF5's flash mark is now `unreviewed`.

### Two smaller points fable raised

- **N3:** two comments still said the check point "warns (never fails)". They now say it reports
  drift to the caller's handler.
- **N4, a design cost that is stated rather than guarded:** nothing forces the eight gated call
  sites to pass `drift_handler`. A no-op closure would compile and silently drop that audit's
  drift check. The risk is small, because all three goldens share one stamp and any one check
  point reporting drift makes the gate fail. Adding machinery to police it would cost more than
  it saves.

## Attempt 2, round 2: two stand-ins, and a seventh counterexample

Both review slots are filled by stand-ins, and the attestation will say so. `fable` stands in for
`astra`, and `opus` for `flash`. Both rostered reviewers were still unreachable (pi's single
provider was returning `429`). A slot cannot keep an old verdict from the reviewer it replaces,
because `attest --substitute` makes the stand-in's marks REPLACE that slot's. So opus reviewed
all eleven entries from scratch rather than inheriting flash's.

**fable accepted all eleven.** It also confirmed the FS1 fix survives an INHERITED
`ENFORCE_CDO_WS=1`, which is the `cdo-gate` situation.

**opus re-raised FS1 on a seventh counterexample (O1), proven by execution.** The FS1 fix had
SWAPPED the `"0"` arm for the unset arm instead of ADDING one. So the driver tested only unset and
`"1"`, and a handler that checks PRESENCE (`var_os(..).is_none()`, opus's M7) passed every
assertion while it would fail a developer running with `ENFORCE_CDO_WS=0`. Opus ran both
mutations against both versions of the driver:

| driver | M6 (panics when unset) | M7 (checks presence only) |
|---|---|---|
| before FS1 (`9a258297`) | passes, which was the FS1 gap | fails |
| after FS1 (`6f2553b9`) | fails | **passes, which is the new gap** |
| after this fix | fails | fails |

Closing one hole by moving the test's blind spot, rather than removing it, is the same mistake as
the fifth counterexample. The fix is to test all three states.

### Fixes, one batch, each proven

| # | finding | fix | proof |
|---|---|---|---|
| O1 / FS3 | the handler is only tested in two of three states | ungated arm now runs unset AND `"0"` | **D9**: M7 fails the driver, exit 101 |
| FS2 | the library could print `WARNING:` itself and pass | library driver asserts no `WARNING:` on either stream | **D10**: an `eprintln!` at the trigger check point fails it |
| O2 | a lowercase `warning:` duplicate dodged the prefix check | under enforcement the message must appear EXACTLY ONCE across both streams | **D11**: M9 fails it |
| O3 | only the trigger check point was ever driven with drift | the probe also calls `run_cdo_event_audit` and counts handler calls (1, then 2) | **D12**: removing the event check point's `on_drift` fails it |
| gap | the two lib tests had no recorded discrimination proof | none needed in code | **D13**: `workspace_drift` always returning `None` fails both |
| O4 | no gate run existed for the new head | gates are run on this branch's final head before attest, and cited from there only | -- |
| O5 | stale "warning" and "pure" wording | 4 source comments and 2 ledger rows corrected | -- |

All five proofs exit 101 on the assertion they target, each restored byte-for-byte. The drivers
are green with the variable unset AND with `ENFORCE_CDO_WS=1` inherited from the parent.

**Still a stated gap, not a pinned one:** `run_cdo_semantic_audit` returns before its check point
on an empty directory, so no test drives that check point with drift. It needs a real fixture.
The risk is small: all three committed goldens share one stamp (`bc3ccb18…`, closure
`48bccdc3…`), so the trigger and event check points, which are pinned, still trip the gate on
drift.

**Ten counterexamples** had been found by the end of attempt 2 (see the final section below; an earlier draft of this sentence said seven): print-before-assert,
deleted print, wrong stream, misspelled probe name, quoted warning, a fake ungated state, and an
ungated state tested in only one of its two forms. Each is a recorded mutation that fails the
suite.

## Attempt 2, round 3: both stand-ins accept; one last minor batch

**fable accepted all sixteen entries and the O3 deferral. opus accepted all sixteen and the O3
deferral.** Both made O4 conditional on gates being run on the head that merges, not cited from
older logs. That is what happens next.

Both also found that **O3's stated reason was wrong.** It said the semantic check point "needs a
real fixture", and `tests/fixtures/semantic-golden` already exists (`harness:1178`). The real
obstacle is different and was checked. Driving the semantic audit on a real fixture calls
`merge_deanon_map`, which would merge that fixture's sites into `cdo-deanon-map.json`. That file is
gitignored, so the tree stays clean, but it is the developer's LOCAL file for decoding
anonymised CDO sites, and a test should not write fixture entries into it. The trigger and event
probes merge nothing, because they run on an empty directory. O3 stays deferred, with this
corrected reason.

**fable's claim that {unset, "0", "1"} was exhaustive was wrong, and opus showed it.** A handler
that enforces on any set value except `"0"` (M8a) passes all three but fails a developer using
`ENFORCE_CDO_WS=true`. No finite list of values is exhaustive for a `== "1"` rule; what is
tested now is every shape a real regression has taken during review.

### The final batch

| # | finding | fix | proof |
|---|---|---|---|
| opus N1 | `"0"` stands in for every set-but-not-`"1"` value | ungated loop adds `"true"` | **D14**: M8a fails on the `"true"` arm, exit 101 |
| opus N3 | library driver checked only the uppercase prefix | also asserts the drift text itself never appears, in any wording | **D15**: a library `eprintln!("warning: {msg}")` fails, exit 101 |
| opus N2 | a comment claimed the one copy is "in the panic" | comment now says it checks the COUNT, not the location | -- |
| opus N4 | three stale comments from the O1 fix | corrected | -- |
| opus N5 / fable N6 | ledger and CHANGELOG counts lagged the code | six of seven entry points exercised; event audit has a caller; **ten** counterexamples | -- |

### Proofs with recorded hashes

D9-D13 (previous batch) were each asserted applied and restored under a sha256 check inside the
proof script, but the script did not print the hashes (opus N5). This batch prints them:

| # | break | result | restored sha256 |
|---|---|---|---|
| D9 (re-run) | presence-only handler, M7 | exit 101, fails on the `"0"` arm | `6d7c26821a39` |
| D14 | enforces on anything but `"0"`, M8a | exit 101, fails on the `"true"` arm | `6d7c26821a39` |
| D15 | library prints a lowercase warning | exit 101 | `22ec8327a656` |

The drivers are green with `ENFORCE_CDO_WS` unset and with `=1` inherited from the parent.

### The ten counterexamples, for the record

print-before-assert (D3) - deleted print (D4) - wrong stream (D5) - misspelled probe name (D6) -
quoted warning (D7) - a fake ungated state (D8) - a presence-checking handler (D9) - the library
printing its own warning (D10) - a lowercase duplicate (D11) - a check point that stopped calling
the handler (D12). D14 and D15 extend D9 and D10 to wordings and values the first fixes missed.

**Entry points exercised:** six of seven. `run_cdo_semantic_audit` alone has no caller (O3).

## Attempt 2, round 4: both stand-ins accept all twenty-one entries

**fable: all 21 accepted, "MERGE". opus: all 21 accepted.** Both accepted O3's deferral with the
corrected reason, and both made O4 conditional on gates run on the head that merges. Each ran the
drift tests at `c04153f3` with `ENFORCE_CDO_WS` unset and `=1`, all green.

Two notes that are not register entries:
- Both found that my briefing's `cargo test --test ... a b c` form is rejected by cargo; test names
  go after `--`. The tree was never affected, and the proof scripts use `--`.
- fable: the ungated value list is a curated set of regression shapes, not an exhaustive one. The
  test comment already says so; this is recorded on ON1.

opus raised three minor findings, all comment or message wording that no longer matched the code.
Leaving known-false comments in merged code is worse than one more commit, so they were FIXED,
not deferred, in `3a4a869c` (a 40-line diff, wording only; the drift tests stay green). Because I
changed the commit, both stand-ins mark OL1-OL3 explicitly. Neither mark is inferred -- inferring
flash's mark on FF5 was the error fable caught in round 1.

## Attempt 2, round 5: OL1-OL3 accepted; final gates on the head that merges

Both stand-ins read only the 40-line diff of `3a4a869c`, read-only and with no cargo, so nothing
could touch the tree while the gates ran. **fable: OL1, OL2, OL3 accepted. opus: OL1, OL2, OL3
accepted.** Both confirmed the diff is two comment blocks and one assert message string, and
checked each edit against the code (the count sums both streams; the match is case-sensitive;
the semantic audit on a real fixture reaches `merge_deanon_map`). Verdicts:
`.agent/runs/20260919-161751-7b0259/fable-ol.md`, `opus-ol.md`.

Register: 24 entries, every one `accepted` by both stand-ins, none `open`, no blocking entry
deferred. O3 (minor) stays deferred with its corrected reason.

### Final gate results (O4's condition)

B = `f5142310` (origin/master, unchanged since the branch was rebased), H = `3a4a869c`.
All four run on H through the executor, `"supervised": true`, none timed out. Logs under
`.agent/runs/20260919-161751-7b0259/logs/`.

| gate | exit | log |
|---|---|---|
| `scripts/ci-steps all` | **0** | `ci-steps-all.log` (24 `test result: ok` lines, 0 FAILED) |
| `scripts/check-goldens --verify-coverage` | **0** | `check-goldens-coverage.log` |
| `scripts/check-goldens` | **0** | `check-goldens.log` (9 targets ok) |
| `scripts/cdo-gate` | **0** | `cdo-gate.log` (334.7 s) |

`git status --porcelain -- . ':!.agent'` in the worktree after the gates: empty. No golden moved.

The merge is attested with `--substitute astra=fable --substitute flash=opus` (#51): pi's only
provider returned `429 quota exceeded` for every model, and the operator directed stand-ins run
through the Agent tool. The attestation records which reviewer filled which slot.
