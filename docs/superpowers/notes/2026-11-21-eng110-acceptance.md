# ENG-110 acceptance on CDO (2026-11-21)

The spec's Acceptance (`docs/superpowers/specs/2026-11-18-eng110-ir-cache-design.md`), run on
server1 with `scripts/ir-cache-acceptance` and the other tools named below. Artifacts:
`/work/.al-sem/engine-notes/eng110-acceptance/` (one folder per run) and the run logs
`/work/.al-sem/engine-notes/eng110-acceptance-*.log`.

**Pins.** master = the merge base `bbec5165`; head = `bcc94da4` (timing), `1213df68`
(identity, refresh), `5b1e0023` (concurrent), `f35f13a6` (census). The head commits differ only
in `scripts/`, `CLAUDE.md` and docs: their release-fast `alsem` and `aldump` are byte-identical
(sha256 `47ea5684…` and `53bf2e43…`, checked for `bcc94da4` against `1213df68`), so every head
run used one key. Workspace `bc3ccb18` (0 dirty paths); the driver's `.alpackages` stamp
`b720cdd1…` was the same before and after each run. Linux, release-fast.

## 1. Identity, byte for byte: PASS

`ir-cache-acceptance identity` (run 2, `identity2/identity.json`): fingerprint, analyze
(primary and `--scope all`), `aldump --program-call-graph-stats`, explain, query effects and
digest of `CDO Telemetry::UpdateLastUpload`: master, head-off, head-cold and head-warm give the
same bytes for all 7 outputs; no output differed, so none needed the nondeterminism re-run.
Every head process's report met its rule (2.).
Run 1 (`identity/`) gave the same bytes but failed one coverage rule: its warm root was filled
by `alsem` only, and `aldump` has another key (a miss). The driver now fills the warm root with
both binaries (commit `1213df68`).

Also: `cdo-gate --only lib` (Task 11, `ir_cache_cdo_decoded_equals_parsed`: 10 packages,
10,881 files, cold and warm equal to a fresh parse, no round-trip failure; and
`ir_cache_cdo_encoding_is_deterministic`) PASS; the refresh verify (below) PASS; goldens run
with the cache off in `ci-steps task`.

**Refresh verify, both modes, cache warm** (`refresh/`): `aldump --session-refresh <copy>
--verify` and `--low-mem`: 12 steps each, every step `equalToCold: true` against a cache-off
reference build (under `Suspend`). Each process: 32,643 disk hits (3 builds x 10,881), 0
misses, 0 refused.

## 2. Coverage: PASS

Every head sample's report met spec Acceptance 2: warm `disk_hits = eligible_files = 10,881`,
`skipped_unstored 0`, `refused 0`; cold `disk_hits 0`, `published_packages 10` = the eligible
packages and 10 entries on disk; off `skipped_off = seam_files`, nothing hit or published. No
`roundtrip_failed` anywhere, no start without an end.

## 3. One-shot timing (bars (a) and (c)): PASS

`ir-cache-acceptance timing` in a timed CDO slot (`timing/timing.json`), `alsem analyze`, user
instructions per PMU (cpu_core + cpu_atom), every sample covered; 6 rounds, rotating order,
none void (container stall share 4.0-5.0 %, no suspend).

| arm | median instructions / master | limit | median CPU s | median wall s |
|---|---:|---:|---:|---:|
| master | 1 | | 66.45 | 18.31 |
| head-off | **x0.9999** | 1.01 | 65.90 | 18.35 |
| head-cold | **x1.0354** | 1.05 | 70.84 | 19.58 |
| head-warm | **x0.5273** | 0.70 | 43.85 | 16.74 |

Per-round ratios: warm x0.5272-0.5273, cold x1.0352-1.0357, off x0.9997-1.0002. No median was
within 0.01 of its limit (off: 0.0101 below), so 6 rounds decided. Wall time gains little
(-9 % warm): the parse was parallel, and the rest of `analyze` is not.

## 4. cdo-speed

Two runs, `--ir-cold` (`cdo-speed-1/`, `cdo-speed-2/`). The judged rows run with the cache off.
- Run 1: PASS; judged `build` instructions x1.0000, round total x0.9994; peak memory x0.970.
  Reported IR rows (head with the cache against base, which predates it): `build (IR warm)` CPU
  x0.687, wall x0.959; `build (IR cold)` CPU x1.039, wall x1.007.
- Run 2: PASS; judged `build` instructions x0.9991, round total x1.0006; peak memory x0.965.
  `build (IR warm)` CPU x0.666, wall x0.891; `build (IR cold)` CPU x1.055, wall x1.049.
- The two runs' warm build CPU ratios agree (x0.687, x0.666): combined x0.68 ±6 % for the
  resident server's model build with the cache warm; cold x1.04-1.06. Instructions are not
  counted on the IR rows (they are reported only).

## 5. cdo-gate

Not run in full (the orchestrator's landing measures its wall time; a gate gains nothing: its
binaries are new). `cdo-gate --only lib` PASS (Task 11). `cdo-gate --only harness,l4,output` on
the final head: see the CHANGELOG entry's landing note.

## 6. Memory and disk

- **Census** (`census2/census.json`, `/usr/bin/time -v`): `aldump --snapshot-memory` max RSS
  master 3,798,980 KiB, head warm 3,718,556 KiB (x0.979). One-shot `alsem analyze` alone: master
  2,394,772 KiB, head cold 2,485,748 KiB (x1.038), head warm 2,398,624 KiB (x1.002).
- **Four cold one-shots on one fresh root at once** (`concurrent/concurrent.json`): peak RSS
  2,195,640-2,221,240 KiB each; most temp bytes at once 100.1 MiB; 10 publishes for 10 entries
  (write amplification x1.0); 5 `publish_skipped_busy`; the later processes hit entries the
  others had published (623-1,197 disk hits each).
- **Disk:** the CDO key folder is 144.3 MiB (151,276,500 bytes, 10 entries); the largest entry
  is Base Application's, 122.3 MiB of encodings (step 1).
- **Round trip** (step 1): 0.84 CPU-s single-thread for all of CDO, paid by a cold fill.
- **Image hash** (step 1): blake3 of the 49.7 MB release-fast `alsem`, 0.043 CPU-s cold, 0.027
  warm; the marker scan rides on the same read (its cost is inside the warm and cold ratios).
- **Small packages** (step 1): a hit costs 0.026-0.046 of a parse at every CDO size;
  `SMALL_PACKAGE_MIN_FILES = 8`, so all 10 CDO packages are cached.
