# TLS-106: `--update-baseline` must not lose fingerprints to a filter

Status: revision 2, after review r1 (Fable, sol). Needs an owner decision (a released file
format and command change behaviour).

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
  - its detector ran in this run and did not error;
  - the run's universe (cross-app or `--single-app`) is the one the file records;
  - the run's coverage is not degraded (the preflight passed);
  - its fingerprint is absent from the run's findings BEFORE `--min-severity`, the allow-list,
    `--scope`, `--focus` and `--limit`.
- **Keep** every other old entry, byte for byte.
- **Add** the findings the run shows (after its filters), as today.

So severity drift (review, sol and Fable: F moves from high to medium) cannot drop F, a
dependency finding is never dropped by a primary-scope update, a detector outside the view
keeps its entries, and an error or a degraded run drops nothing. `--limit` needs no refusal:
it only limits what is added.

## 4. The file, schema 2

```json
{
  "schemaVersion": "2",
  "generatedAt": "...",
  "universe": "cross-app",
  "fingerprints": ["...", "..."],
  "entries": [{"fingerprint": "...", "detector": "d1-db-op-in-loop"}, ...]
}
```

- `entries` is sorted by (fingerprint, detector) and deduped on the pair (several findings
  share a fingerprint: 2,308 findings, 2,301 fingerprints on CDO).
- `fingerprints` is the sorted, deduped list of the entries' fingerprints: what suppression
  reads, so a version-1 reader still works. When the two disagree (a hand edit, a merge), the
  file is refused as inconsistent (exit 2), never repaired silently.
- `universe` is `cross-app` or `single-app`. An update in the other universe is refused
  (exit 3): the two models find different sets, so presence in one says nothing about the
  other. Use one baseline per universe.
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
- **An old binary** updating a version-2 file writes version 1 and loses `entries`; the
  newer binary then reads it as version 1 (keeps everything). Not a loss, only a downgrade.

## 6. Release note (CHANGELOG)

"`--update-baseline` no longer drops findings that were outside the run's view. Before, an
update with `--min-severity`, `--detector`, `--preset`, `--limit`, `--single-app`, the default
`--scope primary`, a failing detector or degraded coverage removed every finding that run did
not show, and later runs reported them as new. If that happened to your baseline, rebuild it:
delete the file and run `alsem analyze <ws> --scope all --baseline <file> --update-baseline`
with the detectors your gate uses (`--preset`, opt-in detectors), then any narrower updates you
like: they now keep what they do not see."

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
- An unfiltered update of a clean run writes the same `fingerprints` as today.

## 8. Owner question

Recommended: the rule in §3 with schema 2 (§4). It refuses nothing a released command does
today except an update in a different universe than the file records, and a malformed or
inconsistent file. Its one trade: a fixed finding's old v1 entry stays until the file is
rebuilt (counted on stderr). Alternatives: refuse every filter with `--update-baseline` (breaks
the plain command, whose default scope is a filter) or keep every old fingerprint (hides a
reintroduced finding: a wrong answer).
