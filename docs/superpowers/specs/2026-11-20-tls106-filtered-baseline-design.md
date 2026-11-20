# TLS-106: `--update-baseline` must not lose fingerprints to a filter

Status: revision 3, after review r1 and r2 (Fable, sol). Decided 2026-11-20
(q-20261120T181319-96d0a142, answered by kraken from the owner's principles, the owner may
overrule; relayed by the orchestrator): implement this revision.

**This revision supersedes option C** (revision 1's per-entry facts merge), on C's own
principles: 1 (correct first: a filtered update must not lose fingerprints, and a plain union
keeps stale ones), 5 (fail loudly and closed) and 22 (released: a release note, nothing users
run today breaks). C's "exact merge" was broken in review (severity drift, the scope rule,
`--single-app`, incomplete detectors, §3), so this revision keeps C's aim and replaces its
rule: drop only what the run proves gone.

## 1. The bug

`alsem analyze --baseline B --update-baseline` replaces B's fingerprint set with the findings
the run kept after its filters (`src/engine/gate/run.rs` 913-933: it serializes the post-`--limit`
findings). So an update loses every fingerprint its view left out:

| Cause | What the update drops from B |
|---|---|
| `--min-severity X` | every finding below X |
| `--detector a,b` / `--preset P` | every other detector's findings |
| `--limit N` | every finding after the first N (in detector order) |
| `--scope primary` (the default) | every finding the primary scope excludes |
| `--single-app` | every finding only the cross-app model finds (review, Fable: the event detectors give 0 single-app on CDO against 195 cross-app) |
| a detector that errors | all of its findings (it contributes none: `registry.rs` 697-736; review, sol) |
| degraded coverage | findings the run could not reach; it still writes (review, sol) |
| `--focus` (ENG-168, not yet on master) | refused there |

The next wider run reports every dropped finding as NEW: a CI gate fails, or a team triages
old findings again. Inline `// al-sem-ignore` is not a hole: it runs after the baseline is
built (`run.rs` 920 against 946-962).

## 2. Why the file cannot be merged today

The file (`baseline.rs` `serialize_baseline`) holds only fingerprints. When an update does not
see an old fingerprint F, F is either fixed (drop it) or merely outside the view (keep it). The
file cannot say which.

## 3. The rule: an entry is dropped only when its finding is provably gone

A fingerprint is a hash over the detector's name and the finding's identity, never its
severity or scope (`fingerprint.rs` 262-271). So the question "is F still there?" can be asked
of the run's COMPLETE output, before any filter, and needs only F's detector:

- **Drop** an old entry only when all hold:
  - its detector ran in this run COMPLETELY: it has a `detector_stats` row (a detector that
    errors leaves only a diagnostic, `registry.rs` 711, 717-735) and reported nothing
    incomplete (a truncation such as d65's `crossCallTruncated`, a substrate warning such as
    d43's; review r2, sol). Each detector's incompleteness signals are listed in one place
    in the registry, and a detector lists none only when it has none;
  - the run's universe and primary app are the ones the file records;
  - the run's inputs were whole: no unreadable or encrypted dependency, no recovered (parse
    error) file, no failed coverage check. Unknown call edges do NOT stop pruning: DO's every
    run reports 1,142 of them, so a rule that stopped on any `degraded` preflight would never
    prune on real workspaces and decay into keeping everything (review r2, Fable);
  - the entry's scope is inside the run's scope: a primary-scope run (the default) prunes only
    entries recorded as primary; dependency entries are pruned only by a `--scope all` run
    (the registry drops dependency findings itself at `--scope primary`, `registry.rs` 565-582,
    so the gate never sees them; review r2, Fable and sol);
  - its fingerprint is absent from the run's findings BEFORE the gate's `--min-severity`,
    allow-list, `--focus` and `--limit`.
- **Keep** every other old entry, byte for byte.
- **Add** the findings the run shows (after its filters), as today, each with its detector and
  scope.

So severity drift (review, sol and Fable: F moves from high to medium) cannot drop F, a
dependency finding is never dropped by a primary-scope update, a detector outside the view
keeps its entries, and an incomplete detector or input drops nothing. `--limit` needs no
refusal: it only limits what is added.

What the rule promises is about fingerprints, not issues: a detector whose fingerprint includes
a sample anchor (d17 takes the first caller's) gives the same issue a new fingerprint when that
anchor moves; the old one is then pruned and the new one is new (review r2, sol).

Each update states how many entries it kept unchecked, and why, by reason: detector not run,
detector incomplete, inputs not whole, out of scope, no detector known (version 1), detector
no longer registered. On stderr (`al-sem: baseline: kept N entries unchecked: ...`) AND, for
`--format json`, as `payload.summary.baselineUpdate` (`{"written": n, "dropped": n, "added":
n, "keptUnchecked": {"<reason>": n, ...}}`), so keeping is never silent, for a person or a
reader of the JSON (decision 2026-11-20).

## 4. The file, schema 2

```json
{
  "schemaVersion": "2",
  "generatedAt": "...",
  "universe": "cross-app",
  "primaryApp": "<app id>",
  "fingerprints": ["...", "..."],
  "entries": [{"fingerprint": "...", "detector": "d1-db-op-in-loop", "scope": "primary"}, ...]
}
```

- `entries` is sorted by (fingerprint, detector, scope) and deduped (several findings share a
  fingerprint: 2,308 findings, 2,301 fingerprints on CDO). `scope` is `primary` or
  `dependency`, decided by the registry's own primary predicate (one shared function), never a
  second reading of the object role.
- `fingerprints` is what suppression reads, so a version-1 reader still works. A fingerprint
  listed with no entry is a version-1 entry (no detector known), kept until a run sees it: that
  covers a version-1 file, a hand edit and an old binary's rewrite alike. Only an entry whose
  fingerprint is not listed is inconsistent: the update refuses (exit 2) and writes nothing; a
  plain read still reads `fingerprints` (review r2, Fable).
- `universe` (`cross-app` or `single-app`) and `primaryApp` bind the file: an update in another
  universe or for another primary app is refused (exit 3). A version-1 file has neither; its
  first update stamps them.
- An unknown `schemaVersion` is refused (exit 2).

## 5. Old files and the update path

- **Version 1** (fingerprints only): read as entries with no detector. An entry with no
  detector is kept by every update until a run sees its fingerprint; then it gets the detector
  that produced it. Nothing is refused: the plain documented command keeps working (review,
  Fable: refusing would break `analyze --baseline B --update-baseline` on every v1 file). The
  first update writes version 2. A fixed finding's v1 entry that no run sees again stays until
  the file is rebuilt; the update says how many such entries it kept, on stderr.
- **Missing file**: an update writes a new one (as today).
- **Malformed or inconsistent file**: the update refuses and writes nothing (today the old
  file is never read, so a malformed one is overwritten).
- **Batch**: the job's baseline is now read and written; its plan declares both
  (`src/bin/alsem.rs` 1863-1867).
- **An old binary** updating a version-2 file writes version 1 with its old rule, so a
  filtered update by an old binary still loses fingerprints, as today (review r2, sol). Mixed
  readers are fine; mixed writers are not: the release note says to update every writer.
- **Entries that are never pruned:** a detector no longer registered (removed or renamed)
  never runs, so its entries stay; they are harmless (its name is hashed) and counted on
  stderr. Unseen version-1 entries are cleared only by rebuilding the file, which accepts
  every current finding untriaged: the release note says so.

## 6. Release note (CHANGELOG)

"`--update-baseline` no longer drops findings that were outside the run's view.

- **Before:** an update with `--min-severity`, `--detector`, `--preset`, `--limit`,
  `--single-app`, the default `--scope primary`, a failing detector or degraded coverage
  removed every finding that run did not show, and later runs reported them as new.
- **What a filtered update keeps now:** every entry the run did not prove gone. An entry is
  dropped only when its detector ran completely, the run's inputs were whole, the entry is
  inside the run's scope, and its fingerprint is absent before any filter. Everything kept
  unchecked is counted on stderr and in `--format json` (`payload.summary.baselineUpdate`).
- **Your old (schema 1) baseline moves to schema 2 by itself:** the first `--update-baseline`
  reads it, keeps every fingerprint, records a detector for each one the run sees, and writes
  schema 2. Fingerprints no run sees stay (counted as 'no detector known') until you rebuild.
  Reading a baseline (`--baseline` alone) works with both schemas. Update every machine that
  writes the file: an old binary still writes schema 1 with the old, lossy rule.
- **If an earlier filtered update damaged your baseline,** rebuild it once: delete the file
  and run `alsem analyze <ws> --scope all --baseline <file> --update-baseline` with the
  detectors your gate uses (`--preset`, opt-in detectors). This accepts every current finding
  as the new floor, untriaged: review the list first if that matters. Then any narrower
  updates you like: they now keep what they do not see."

## 7. Tests (each with a red proof)

- Per cause (severity, detector, preset, limit, primary scope, single-app refused, detector
  error, degraded): an update keeps every entry its run did not prove gone, byte for byte.
- A fixed finding (fingerprint gone, its detector ran, same universe, full coverage) is dropped.
- Severity drift: F high -> medium, `--min-severity high` update keeps F.
- Several findings with one fingerprint keep one entry per (fingerprint, detector).
- v1: read for suppression unchanged; an update keeps every unseen v1 fingerprint, gives seen
  ones their detector, writes v2, and reports the unseen count.
- Malformed, inconsistent, unknown-version files: refused, nothing written; missing: written.
- Batch: the plan declares the baseline as read and write.
- The kept-unchecked counts per reason, on stderr and in `payload.summary.baselineUpdate`.
- An unfiltered update of a clean run writes the same `fingerprints` as today.

## 8. Owner question

Recommended: the rule in §3 with schema 2 (§4). It refuses nothing a released command does
today except an update in a different universe than the file records, and a malformed or
inconsistent file. Its one trade: a fixed finding's old v1 entry stays until the file is
rebuilt (counted on stderr). Alternatives: refuse every filter with `--update-baseline` (breaks
the plain command, whose default scope is a filter) or keep every old fingerprint (hides a
reintroduced finding: a wrong answer).
