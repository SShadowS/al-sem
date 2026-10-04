# census-probe

Counting-allocator byte census of the LSP snapshots, one DepCache shared across roots; `--with-updaters` also starts each root's real updater and measures the idle heap. Its accounting conventions are `.claude/skills/byte-census/SKILL.md`'s.

Build: `CARGO_TARGET_DIR=C:/lpt cargo build --release` here (own workspace; never build into the repo's `target/`). Run: `C:/lpt/release/census-probe.exe <embedded|symbols> <root>... [--with-updaters]`.

Corpora: CG harness = roots `Core Fleet Integration Leasing Rental Reporting Test` of `U:/Git/CentralGauge/harness-tasks/refapp`, copied to a scratch dir with `.alpackages` copied from `U:/Git/CentralGauge/infra/cg-test-harness/.alpackages` (rebuild: `cp -r` the refapp dir, then `cp -r` that `.alpackages` into the copy's top level); CDO = `U:/Git/DO-cdo-baseline/Cloud` (read-only). Raw outputs of the step 0 runs: `runs/`.
