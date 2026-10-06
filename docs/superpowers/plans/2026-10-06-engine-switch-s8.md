# Engine switch S8 — Phase C: cross-app analyze (G11b, G15b-new)

Spec: `docs/superpowers/specs/2026-10-06-engine-switch-design.md`, S8.
Branch: `engine-switch/s8-phase-c`, from master `fb3f9631` (S7 merged).

## Owner decisions (2026-10-06)

1. **`alsem analyze` is cross-app by default**; `--single-app` keeps today's
   workspace-only model.
2. **d45 with a dependency publisher**: the publisher is a d45 root when at least one
   primary routine is in its subscriber chain; its own writes come from its
   dependency summary; an opaque summary is stated as `unknown` coverage.

## What exists (after S7)

- Two detector pipelines: `run_detectors` + `build_detector_context` (single-app,
  demand-gated substrates) and `run_detectors_cross_app` +
  `build_detector_context_cross_app` (over `R3a5CrossAppBase`, builds every
  substrate). On dependency-free workspaces they agree on every finding (S7.6
  contract test).
- Baseline, release-fast, 2026-10-06: single-app `analyze` CDO 632 MiB / 6 s, DO
  613 MiB / 6 s. Cross-app findings (`aldump --r4-findings-cross-app`) CDO ~6.0 GB /
  ~120 s, DO ~143 s.
- Cross-app-only false positives triaged in S7 (bindings correct, detector limits):
  d43 26 (a subscriber that replaces Base App `Send` for one channel), d35 2 (a
  Commit on an inactive-app branch behind a UI hook), d44 2 (a write only for another
  notification id).
- A dependency's own edges reach the cone only as admitted intra-app edges;
  `CODEUNIT.Run`, triggers and cross-dependency events inside dependencies are not
  followed.

## Steps (one commit each, measured)

- **S8.0 Where the cross-app cost goes.** Span-profile the cross-app run on CDO
  (FULL build, dependency resolution, model assembly, base, context, detectors) with
  peak memory per phase. No behaviour change.
- **S8.1 One detector pipeline.** The cross-app base's extras (dependency routine
  ids, fixed leaves, injected edges, declared dependencies, versions) become inputs of
  `build_detector_context`, demand-gated like the single-app path;
  `build_detector_context_cross_app` is deleted. Contract: every cross-app and
  single-app golden and the S7.6 contract test unchanged; CDO/DO cross-app findings
  unchanged.
- **S8.2 Cost.** Cut what S8.0 shows dominates (demand: only dependency routines
  reachable from the workspace, forward, and the event raisers of subscribed
  dependency events, reverse — the spec's demand policy). Gate: findings equal the
  full run on CDO/DO.
- **S8.3 `alsem analyze` cross-app by default**, `--single-app` flag. The analyze
  projections, scope filter, coverage and fingerprints read the cross-app model; goldens
  of fixtures with dependencies are triaged.
- **S8.4 d45 dependency publishers** (decision 2), with a hand-stated fixture.
- **S8.5 Detector precision for the cross-app default**: the d43/d35/d44 classes above,
  fixed at the root or, per doctrine (> 30% FP on the sample), made opt-in in cross-app
  mode with the evidence recorded.
- **S8.6 Propagation inside dependencies**: whether the cone follows a dependency's
  runs, triggers and events (demand-limited), measured against S8.2's budget.
- **S8.7 Docs**: CHANGELOG, spec as-built, CLAUDE.md, OUTSTANDING, memory.

## Out of scope

- The other consumers (`prove`, `digest`, `fingerprint`, `diff`, `events`, `policy`,
  `query`, LSP) stay single-app; making them cross-app is a follow-up decision.
- Deleting L3 (S9).
