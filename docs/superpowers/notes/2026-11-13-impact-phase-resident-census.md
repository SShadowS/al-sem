# CDO gate: would the impact phase gain from a resident engine? (TLS-38 census)

TLS-38 asked whether `cdo-gate`'s impact phase (`output_cdo::cdo_impact*`, 363-393 s in
the 2026-11-13 serial gates) would get faster if its `alsem impact` calls asked one resident
engine instead of each rebuilding everything. **Result: no. It would get slower. TLS-38 stops
at the census.** This was decided from the code and from existing measurements, with no new
CDO run.

## What the phase runs

Two tests, run at the same time, start five one-shot `alsem impact` processes at once:

| test | calls (run at once) |
|---|---|
| `cdo_impact_ratchets` (`tests/cli/cli_impact.rs`) | `--diff` JSON, `--diff` human, `--routine CDO Telemetry::UpdateLastUpload` JSON |
| `cdo_impact_base_ratchets` (`tests/cli/cli_impact_base.rs`) | `--base bc3ccb18~5 --no-tests` JSON, `--base bc3ccb18~5` human |

TLS-28's note (`2026-11-12-impact-shared-sited.md`) gives each call alone (`release-fast`,
CDO): `--diff` 135 s wall and 561 s CPU, `--base` 207 s and 777 s, `--routine` 41 s and 242 s.
Run one after another, the five would take ~725 s of wall time even on `release-fast`. Run
together on the `cdo` profile, they take 363 s. **The phase is fast because its five processes
share the host's cores.** It is CPU-bound, not build-bound.

## What a resident engine would change

- **The asks would run one at a time.** `src/mcp/pool.rs` keeps one worker thread per
  (workspace, universe), and that worker owns one engine child. `engine-ask` and `alsem mcp`
  both go through it, so asks for one workspace queue up. Five asks would run in a row:
  about the ~725 s sum, minus the builds saved.
- **Only the builds would be shared, not the per-root work.** A resident `impact` keeps the
  model and the sited graph (`SERVED_COMMANDS` in `src/engine/resident/revision.rs`: "main
  34-50 s, up 13-43 s" per part when not served). But `WITNESS_MEMO_MAX_ROOTS` is 64, and the
  diff and base calls have 699 roots. An effects read over that many roots computes its
  entries itself and publishes none. The second ask of a JSON/human pair would redo nearly all
  of the first one's work. Most of an `impact` is that per-root work (digest's batches, ~70 s
  of the 161 s in `2026-11-13-raw-half-census.md`).
- **The resident pass is serial** (the TLS-22 owner rule), while the one-shot CLI uses 8
  workers. Each ask would get slower too.
- **Memory:** a 699-root impact once had its trees' reservation refused at 7.6 GB of the 8 GiB
  cross-app budget (the comment on `WITNESS_MEMO_MAX_ROOTS`).

Estimate: saving four of the five builds (~25 s each alone) buys ~100 s of wall time.
Losing the five-way overlap costs ~360 s or more. **Net: several minutes slower.**

## What else the census shows (not done)

- Each JSON/human pair computes the same impact twice, so the human answer's work duplicates
  the JSON answer's. A one-shot option that writes both formats from one run would cut roughly
  two of the five processes' CPU. But that is a product feature (a new CLI option) made for a
  test, and the human path would then no longer be checked through its own run. Not proposed.
- The orchestrator asked that at least one ratchet keep the real one-shot path. Nothing changes
  here, so that holds.
