#!/usr/bin/env bash
# ENG-97 round 2: cdo-speed itself, 801cc6af (base) vs 96d49041 (ENG-85 head), RUNS runs of
# --rounds 1. Each run's warm-up times the real `build` call twice (base first, then head
# first), release-fast, through `alsem mcp` + the engine child. Before each run, outside the
# slot, wait for the slot to be free and the host's `some avg60` to be at most 15 %; cdo-speed
# then applies its own per-pass admission and labels.
set -u
S=$(cd "$(dirname "$0")" && pwd)
REPO=/work/alsem-wt/lane-engine
WS=/work/src/cdo-baseline/Cloud
RUNS=${1:-3}
export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al
cd "$REPO" || exit 2

wait_free() {
  until "$REPO/scripts/cdo-slot" status | grep -q '^free'; do sleep 20; done
}
wait_calm() {
  local waited=0
  while awk '/^some/ { split($3, a, "="); exit !(a[2] > 15) }' /proc/pressure/cpu; do
    [ "$waited" -ge 1800 ] && { echo "host pressure stayed above 15 % for 30 min; running anyway"; return; }
    sleep 15; waited=$((waited + 15))
  done
}

wait_free
echo "building head 96d49041 ($(date -u +%T))"
# In the slot: a build beside another session's timed run skews it (20:45, TLS-77).
scripts/cdo-slot run engine "ENG-97 untimed release-fast build of 96d49041 (~4 min, load only)" -- \
  scripts/cdo-speed --warm 96d49041 > "$S/warm-head.log" 2>&1 || { echo "head build failed"; tail -20 "$S/warm-head.log"; exit 2; }

for r in $(seq 1 "$RUNS"); do
  wait_free; wait_calm
  echo "run $r start $(date -u +%T), host $(head -1 /proc/pressure/cpu)"
  scripts/cdo-slot run engine "ENG-97 cdo-speed run $r/$RUNS (801cc6af vs 96d49041, rounds 1, ~6 min)" -- \
    scripts/cdo-speed "$WS" --base 801cc6af --head 96d49041 --rounds 1 --max-rounds 1 --no-build \
      --out "$S/speed-run$r" > "$S/speed-run$r.log" 2>&1
  echo "run $r exit $? $(date -u +%T)"
  grep -E "build  |peak memory|stall share|PASS|FAIL" "$S/speed-run$r.log" | cut -c1-220
done
