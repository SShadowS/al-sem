#!/usr/bin/env bash
# TLS-09 (b): codegen-units under fat LTO for the shipped profile, against today's cu=1.
# Phase "build": per cu, own target dir; a cold `release-ship` build (deps), then touch
# src/lib.rs and a timed `release-ship` build with --timings (as a ship build after an edit);
# the alsem binary is copied out. Then the `release` perf_bounds test binary with the same cu
# (cargo builds tests with unwind, so perf_bounds cannot be release-ship; release = ship minus
# abort). The cu comes from CARGO_PROFILE_RELEASE_CODEGEN_UNITS, which release-ship inherits;
# Cargo.toml is not edited.
# Phase "speed": an A/A control (cu1 against a copy of itself), then each cu against cu1,
# all with scripts/cdo-speed --base-exe/--head-exe --rounds 5.
# Phase "perfb": perf_bounds 3 runs per cu, alternating.
# Usage: b-measure.sh build|speed|perfb <cu>...   (cu 1 always first for build)
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls09
R=/work/alsem-wt/lane-tools
TIMEIT=$R/docs/superpowers/notes/crate-split-phase0/timeit.py
CDO=/work/src/cdo-baseline/Cloud
export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al
B=$S/bins; mkdir -p "$B"
cd "$R" || exit 1
phase=$1; shift
case "$phase" in
build)
  for cu in "$@"; do
    export CARGO_TARGET_DIR=$R/target-cu$cu CARGO_PROFILE_RELEASE_CODEGEN_UNITS=$cu \
           CARGO_PROFILE_RELEASE_SHIP_CODEGEN_UNITS=$cu
    cargo build --profile release-ship > "$S/b-warm-cu$cu.log" 2>&1 || { echo "warm cu$cu failed"; exit 1; }
    touch src/lib.rs
    python3 "$TIMEIT" "ship-cu$cu" "$S/b-runs.jsonl" -- cargo build --profile release-ship --timings
    echo "ship-cu$cu exit=$?"
    cp "$CARGO_TARGET_DIR"/cargo-timings/cargo-timing.html "$S/timing-ship-cu$cu.html"
    cp "$CARGO_TARGET_DIR"/release-ship/alsem "$B/alsem-cu$cu"
    python3 "$TIMEIT" "perfb-build-cu$cu" "$S/b-runs.jsonl" -- cargo test --release --test perf_bounds --no-run
    echo "perfb-build-cu$cu exit=$?"
    ls -t "$CARGO_TARGET_DIR"/release/deps/perf_bounds-* | grep -v '\.d$' | head -1 | xargs -I{} cp {} "$B/perfb-cu$cu"
  done
  cp "$B/alsem-cu1" "$B/alsem-cu1-copy"
  ;;
speed)
  for cu in "$@"; do  # "1" means the A/A control
    head=$B/alsem-cu$cu; [ "$cu" = 1 ] && head=$B/alsem-cu1-copy
    out=$S/speed-cu$cu; rm -rf "$out"
    python3 scripts/cdo-speed "$CDO" --base-exe "$B/alsem-cu1" --head-exe "$head" --rounds 5 --out "$out" > "$S/speed-cu$cu.log" 2>&1
    echo "speed cu$cu exit=$?"
  done
  ;;
perfb)
  for i in 1 2 3; do
    for cu in "$@"; do
      out=$S/b-perfb-cu$cu-$i.log
      { date +%T; uptime; } > "$out"
      "$B/perfb-cu$cu" --nocapture >> "$out" 2>&1
      echo "perfb cu$cu run $i exit=$?" | tee -a "$out"
    done
  done
  ;;
esac
