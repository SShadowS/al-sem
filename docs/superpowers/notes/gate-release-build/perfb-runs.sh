#!/usr/bin/env bash
# TLS-09 (a): perf_bounds headroom, release vs release-fast, 3 runs each, alternating.
# Runs the already-built test binaries (target3, master d887bb47) directly, the way
# `cargo test --test perf_bounds` runs them (whole binary, no filter, libtest default
# threads), with --nocapture so every row's `[perf_bounds] ... min=... bound=...` line prints.
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls09
R=/work/alsem-wt/lane-tools
REL=$R/target3/release/deps/perf_bounds-5a21aeead109756b
FAST=$R/target3/release-fast/deps/perf_bounds-15d2401aa377451a
cd "$R" || exit 1
"$R/scripts/require-rttime" || exit 1
for i in 1 2 3; do
  for p in release release-fast; do
    bin=$REL; [ "$p" = release-fast ] && bin=$FAST
    out=$S/perfb-$p-$i.log
    { date +%T; uptime; } > "$out"
    "$bin" --nocapture >> "$out" 2>&1
    echo "$p run $i exit=$?" | tee -a "$out"
  done
done
