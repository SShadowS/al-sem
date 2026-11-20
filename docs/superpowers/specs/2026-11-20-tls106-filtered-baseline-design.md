# TLS-106: `--update-baseline` must not lose fingerprints to a filter

Status: draft for review and an owner decision (a released command changes behaviour).

## 1. The bug

`alsem analyze --baseline B --update-baseline` replaces B's fingerprint set with the findings
the run kept after its filters (`src/engine/gate/run.rs`, the baseline block: it serializes the
post-`--limit` findings). So a filtered update loses every fingerprint the filter left out:

| Filter | What the update drops from B |
|---|---|
| `--min-severity X` | every finding below X |
| `--detector a,b` | every other detector's findings |
| `--preset P` | every detector outside P |
| `--limit N` | every finding after the first N (in detector order) |
| `--scope primary` (the default) | every dependency-anchored finding |
| `--focus` (ENG-168) | refused already |

The next run with a wider view reports every dropped finding as NEW: a CI gate fails, or a
team triages old findings again. A user who updated with any of these filters has a damaged
baseline today.

## 2. Why a merge is not possible with today's file

The file (`baseline.rs` `serialize_baseline`) holds only fingerprints:
`{"schemaVersion": "1", "generatedAt": ..., "fingerprints": [...]}`. When a `--detector d1`
update does not see an old fingerprint F, F is either a d1 finding that was fixed (drop it) or
another detector's finding outside the filter (keep it). The file cannot say which.

## 3. Options

**A. Refuse every filter with `--update-baseline`** (as ENG-168 does for `--focus`): exit 3
naming the filters, and how to update (run without them). Small and correct. It breaks
commands users run today, e.g. a CI step `analyze --min-severity error --baseline B
--update-baseline`. `--scope primary` is the default, so A also needs `--scope all` on every
update, or the default scope accepted as "the baseline's scope" (then a later `--scope all`
run still reports dependency findings as new).

**B. Union for filtered updates.** A filtered update keeps every old fingerprint and adds the
new ones; only an unfiltered update replaces. No data loss, no format change. But a fixed
finding's fingerprint stays until the next unfiltered update, so the same finding reintroduced
later is silently suppressed: a wrong answer (principle 1).

**C. Schema 2: each entry records its scope facts, and a filtered update merges exactly.**
Recommended.
- The file becomes `"schemaVersion": "2"` with `"entries": [{"fingerprint", "detector",
  "severity", "dependency": bool}]`, sorted by fingerprint. `"fingerprints"` is kept as well
  (the same sorted list), so a reader of version 1 still reads it (principle 22: after release,
  never break users).
- A filtered update replaces exactly the entries inside the run's view: an old entry is
  dropped when its detector is selected (`--detector`, `--preset`, the default set), its
  severity passes `--min-severity`, and its dependency flag passes `--scope`; every other old
  entry is kept. Then the run's findings are added. So the update is what an unfiltered update
  would have written, restricted to the view, and the rest of the file is untouched.
- `--limit` is refused with `--update-baseline` (exit 3): a limit is not a scope (it drops
  findings by position), so no merge is defined.
- `--focus` stays refused for now (ENG-168); with schema 2 its concern comes from the detector,
  so a later task can allow it the same way.
- A filtered update of a version 1 file cannot merge (no facts): refused (exit 3) with the fix
  "run `--update-baseline` once without filters (and with `--scope all`) to rebuild the file as
  version 2". An unfiltered update of a version 1 file writes version 2.
- Reading (`--baseline` without update) accepts both versions, unchanged in meaning.

## 4. Release note (CHANGELOG)

"If you ever ran `--update-baseline` together with `--min-severity`, `--detector`, `--preset`,
`--limit` or the default `--scope primary`, your baseline is missing the findings that run
left out, and later runs report them as new. Rebuild it once: `alsem analyze <ws> --scope all
--baseline <file> --update-baseline` (no other filters). The new file (schema 2) is merged
exactly by later filtered updates."

## 5. Tests (each with a red proof)

- Per filter (min-severity, detector, preset, scope): a filtered update keeps every
  out-of-view entry byte-for-byte and replaces the in-view ones; a fixed in-view finding's
  entry is dropped.
- `--limit` with update refused; a filtered update of a v1 file refused with the rebuild hint;
  an unfiltered update of v1 writes v2.
- v1 and v2 files read identically for suppression.
- An unfiltered update writes the same fingerprints as today (the `fingerprints` list).

## 6. Owner question

C changes released behaviour in two places: `--limit` with `--update-baseline` is refused, and
a filtered update of an old (v1) file is refused until it is rebuilt once. A refuses every
filtered update. B never refuses but can hide a reintroduced finding. Recommendation: C.
