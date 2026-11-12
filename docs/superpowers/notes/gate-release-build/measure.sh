#!/usr/bin/env bash
# TLS-09 step 1: where the gate's release build time goes, per profile.
# For each profile: a warm-up build (deps compiled, not timed as the gate's case), then the
# gate's case: touch src/lib.rs, `cargo build --<profile> --timings` (every bin, as
# `ci-steps build`), then the perf_bounds test build (`cargo test --<profile> --test
# perf_bounds --no-run`, as `ci-steps perf-bounds` builds it). Wall, CPU, peak child RSS via
# timeit.py; cargo's --timings HTML kept per run. Own target dir (target3), so no other
# build's lock or artifacts interfere.
# Usage: measure.sh <profile>...   (e.g. release release-fast)
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls09
R=/work/alsem-wt/lane-tools
TIMEIT=$R/docs/superpowers/notes/crate-split-phase0/timeit.py
export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al
export CARGO_TARGET_DIR=$R/target3
cd "$R" || exit 1
flag() { [ "$1" = release ] && echo --release || echo "--profile $1"; }
for p in "$@"; do
  # shellcheck disable=SC2046
  cargo build $(flag "$p") > "$S/warm-$p.log" 2>&1 || { echo "warm-up $p failed"; exit 1; }
  touch src/lib.rs
  # shellcheck disable=SC2046
  python3 "$TIMEIT" "build-$p" "$S/runs.jsonl" -- cargo build $(flag "$p") --timings
  echo "build-$p exit=$?"
  cp "$CARGO_TARGET_DIR"/cargo-timings/cargo-timing.html "$S/timing-build-$p.html" 2>/dev/null
  # shellcheck disable=SC2046
  python3 "$TIMEIT" "perfb-$p" "$S/runs.jsonl" -- cargo test $(flag "$p") --test perf_bounds --no-run --timings
  echo "perfb-$p exit=$?"
  cp "$CARGO_TARGET_DIR"/cargo-timings/cargo-timing.html "$S/timing-perfb-$p.html" 2>/dev/null
done
