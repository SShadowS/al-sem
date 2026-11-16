#!/usr/bin/env bash
# ENG-84 after-profile on the model-build probe (the before-profile's shape): samply over one
# resident CDO model build, master (base) then ENG-84 (head), then head then base again, so each
# side is profiled once first and once second.
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-engine/494fcd35-e29c-4995-b44d-bcb017c905d2/scratchpad/eng84
WS=/work/src/cdo-baseline/Cloud
run() { # tag binary
  samply record --save-only --unstable-presymbolicate -o "$S/pp-$1.json.gz" -- "$S/target/release/$2" "$WS" \
    > "$S/pp-$1.log" 2>&1
  echo "$1 exit $? $(grep 'build wall' "$S/pp-$1.log")"
}
run base1 eng84-probe-base
run head1 eng84-build-probe
run head2 eng84-build-probe
run base2 eng84-probe-base
