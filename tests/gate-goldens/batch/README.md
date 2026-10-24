# `alsem batch` goldens (#277)

A subdirectory that holds a `RECORD` file holds base-binary records: bytes the `alsem` binary at
`1134bed1` (the branch base) printed. They state today's behaviour and are never regenerated.
`merged-streams/` (T20) holds three runs with stdout and stderr merged (`2>&1`), each with its exit
code in a `.exit` file: `degraded-analyze` is `analyze tests/r0-corpus/ws-txn-d47-pos-file
--deterministic` (the degraded-coverage warning comes before the report); `policy-diagnostics-out` is
`policy check tests/fixtures/cli-c-policy/ws-policy-commit-in-subscriber --format json --deterministic
--out <a writable file>` (the analyzer diagnostics come after the output); `policy-out-fails` is the
same with `--out tests/gate-goldens/batch/merged-streams/no-such-dir/p.json` (the write error stops
the diagnostics). `update-baseline/` (T23) is `analyze tests/r0-corpus/ws-d1 --deterministic
--baseline tests/gate-goldens/batch/update-baseline/no-such-dir/b.json --update-baseline`: stdout,
stderr and exit kept apart. Every record ran from the repository root with `RUST_LOG=off`.
`one-job/` (Task 7) holds the regenerable one-job manifest goldens.
