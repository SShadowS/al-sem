#!/usr/bin/env bash
# TLS-22 plan task 5, one-shot part: CDO impact --diff at AL_SEM_REACH_WORKERS 1/2/4/8/16,
# impact --base at 1 and 8, one at a time, traced, peak RSS/CPU/idle via timeit.py.
# Usage: measure.sh <alsem>
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls22
R=/work/alsem-wt/lane-tools
A=$1
TOP=/work/src/cdo-baseline
WS=$TOP/Cloud
TIMEIT=$R/docs/superpowers/notes/crate-split-phase0/timeit.py
DIFF=$S/../tls16/range.diff
memavail_mib() { awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo; }
# The orchestrator's guard: stop any run when the host has under ~10 GB available.
watch_mem() { # <label>
  local low=999999 m
  while sleep 2; do
    m=$(memavail_mib); [ "$m" -lt "$low" ] && low=$m && echo "$low" > "$S/memlow-$1"
    if [ "$m" -lt 10240 ]; then
      echo "STOP $1: MemAvailable ${m} MiB" >> "$S/memguard.log"
      pkill -f alsem-d9f4135f
    fi
  done
}
run() { # <call> <workers>
  local call=$1 w=$2 args cache wpid
  if [ "$(memavail_mib)" -lt 10240 ]; then
    echo "SKIP $call-w$w: MemAvailable $(memavail_mib) MiB" | tee -a "$S/memguard.log"
    return
  fi
  watch_mem "$call-w$w" & wpid=$!
  cache=$(mktemp -d "$S/base-cache.XXXX")
  if [ "$call" = diff ]; then
    args=(impact "$WS" --diff "$DIFF" --diff-root "$TOP" --format json)
  else
    args=(impact "$WS" --base bc3ccb18~5 --no-tests --format json)
  fi
  # stdout to its own file (timeit's log takes stderr only, so the JSON stays clean).
  AL_SEM_REACH_WORKERS=$w ALSEM_TRACE=1 ALSEM_TRACE_FILE="$S/trace-$call-w$w.json" \
    AL_SEM_IMPACT_BASE_CACHE="$cache" OUT="$S/$call-w$w.out" \
    python3 "$TIMEIT" "$call-w$w" "$S/runs.jsonl" -- bash -c 'exec "$@" > "$OUT"' _ "$A" "${args[@]}"
  echo "$call-w$w exit=$? memavail_low_mib=$(cat "$S/memlow-$call-w$w" 2>/dev/null)"
  kill "$wpid" 2>/dev/null
  rm -rf "$cache"
}
for w in 1 8 2 4 16; do run diff "$w"; done
for w in 1 8; do run base "$w"; done
cmp "$S/diff-w1.out" "$S/diff-w8.out" && echo "diff output: w1 == w8"
# The resident part's witness case: 40 of the diff's roots (under the memo's 64).
jq -r '.roots[0:40][].selector' "$S/diff-w1.out" > "$S/roots40.txt"
