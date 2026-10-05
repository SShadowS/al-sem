# B3 detector diff: do

DO (`U:/Git/DO/Cloud`) is at the same commit as the CDO baseline (`bc3ccb18`); its only local
change is `AppSourceCop.json`, which the engine does not read. The harness output for DO is
identical to [cdo.md](cdo.md) apart from the title (checked on 2026-10-05 by running
`aldump --b3 U:/Git/DO/Cloud --b3-triage <file>` and diffing). Triage DO in `cdo.md`.
Regenerate a separate DO table only when DO moves off `bc3ccb18`.
