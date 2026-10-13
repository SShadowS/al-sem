---
description: FP-triage a wave of new L5 detectors on a real workspace by fanning out one subagent per detector, then gate >30%-FP detectors to opt-in.
---

Triage the new/changed L5 detectors' findings against REAL source, in parallel,
and decide each detector's tier. Use this after building a detector wave and
before shipping detectors as DEFAULT (see CLAUDE.md "New advisory L5 detector").

Arguments (`$ARGUMENTS`): the workspace to measure on (default: the pinned CDO
baseline, `U:/Git/DO-cdo-baseline/Cloud`; do not set `CDO_WS` globally) and optionally an explicit
list of detector ids. If none given, triage every detector that emits findings.

Steps:

1. **Measure.** `cargo build --release --bin alsem`, then run the detectors on the
   workspace to JSON — `target/release/alsem.exe analyze <ws> --preset <preset>
   --format json` (or `--detector d.. --detector d..`). Get per-detector counts
   from `.payload.summary.detectorStats` / `.byDetector`.
2. **Build the worklist.** `python .claude/skills/triage-findings/scripts/worklist.py
   <json> --scope primary` (absolute path — the skill lives in the MAIN checkout, not
   a worktree). Extract per-detector `file:line | routine | rootCause` (strip the
   `ws:` prefix; join to the workspace root; quote paths — they contain spaces).
3. **Fan out — ONE subagent per detector** (general-purpose), in a single message so
   they run concurrently. Give each: the detector's finding list, the workspace root,
   the detector source `src/engine/l5/detectors/dNN.rs` (read for the exact premise),
   and instruct it to open EACH finding's real AL source, verify the premise holds,
   and return REAL/FALSE-POSITIVE per finding WITH cited source evidence (loop bounds,
   var declaration, tempness, field class), plus a `dNN: REAL=n FP=m fp_rate=%` tally
   and any `DETECTOR-BUG:` flags. Ask for caveman-compressed output to save context.
   Batch tiny detectors (≤2 findings) into one shared agent.
4. **Gate.** For each detector: **> 30% FP on its sample → OPT-IN** (move the registry
   entry after the d51 block + the `presets.rs` name from DEFAULT_DETECTOR_NAMES to
   OPT_IN_DETECTOR_NAMES; regen the detectorStats goldens). If a detector is
   systematically wrong (a `DETECTOR-BUG:`), prefer FIXING THE ROOT CAUSE over
   demoting — write a fixture reproducing the FP shape, add the guard, `scripts/check-goldens`,
   re-measure. Only demote when the residual genuinely needs substrate you will not
   build now (record it in the module doc).
5. **Report** a per-detector table (pre-fix FP → fix → post-fix) and the final tiers.
   Append the outcome to the wave's session-notes doc.

Be honest both ways: never rubber-stamp a rootCause, never rationalize a real issue
away. Uncertain → mark REAL and let the human arbitrate.
