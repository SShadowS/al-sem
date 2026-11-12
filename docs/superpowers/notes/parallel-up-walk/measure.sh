#!/usr/bin/env bash
# TLS-21 measurement (spec 2026-11-12-parallel-up-walk.md section 5), on the pinned CDO baseline,
# release-fast binaries. Each run is one process at a time.
#   diff  <counts...>  composite `impact --diff` (TLS-16 input): a traced run (spans, walk
#                      window) and an untraced timeit run (wall, CPU, process peak) per count
#   base  <counts...>  composite `impact --base bc3ccb18~5 --no-tests`, the same two runs
#   partup <counts...> `impact --part up` over the 699 roots of the count-1 diff run, traced and
#                      untraced; stdout kept for the byte comparison with master
#   master             master's `--part up` over the same roots (the outside reference)
# Env: AL_SEM_REACH_WORKERS sets the count (one knob for the literal pass and the up walk).
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls21
B=$S/bins
TIMEIT=/work/alsem-wt/lane-tools/docs/superpowers/notes/crate-split-phase0/timeit.py
TOP=/work/src/cdo-baseline
WS=$TOP/Cloud
O=$S/m
mkdir -p "$O"
[ -f "$S/range.diff" ] || git -C "$TOP" diff --no-color --no-ext-diff --diff-algorithm=histogram \
  --src-prefix=a/ --dst-prefix=b/ bc3ccb18~5 bc3ccb18 -- Cloud > "$S/range.diff"
env_line() { { date +%T; uptime; grep MemAvailable /proc/meminfo; } | tr '\n' ' '; echo; }
# The orchestrator's rule: at least 35 GB available before each run (waits up to 10 min).
# Also waits (up to 10 min) for the 1-minute load to fall below 10: the traced runs have no
# idle wait of their own. Runs anyway after that, and the env log records the load.
mem_ok() {
  local i
  for i in $(seq 120); do
    [ "$(awk '/MemAvailable/ {print $2}' /proc/meminfo)" -ge 36700160 ] && break
    sleep 5
    [ "$i" = 120 ] && { echo "MemAvailable stayed below 35 GB: stopping" | tee -a "$O/env.log"; exit 3; }
  done
  for i in $(seq 120); do
    awk '{exit !($1 < 10)}' /proc/loadavg && return 0
    sleep 5
  done
  echo "load stayed at or above 10 for 10 min: running anyway" >> "$O/env.log"
}

composite() { # kind count
  local kind=$1 n=$2 args
  if [ "$kind" = diff ]; then args="--diff - --diff-root $TOP --format json"
  else args="--base bc3ccb18~5 --no-tests --format json"; fi
  mem_ok; echo "$kind-w$n traced $(env_line)" >> "$O/env.log"
  local cache; cache=$(mktemp -d "$O/base-cache.XXXX")
  AL_SEM_REACH_WORKERS=$n AL_SEM_IMPACT_BASE_CACHE=$cache ALSEM_TRACE=1 AL_SEM_UP_WALK_WINDOW=1 \
    ALSEM_TRACE_FILE="$O/trace-$kind-w$n.json" \
    "$B/alsem-head" impact "$WS" $args < "$S/range.diff" > "$O/$kind-w$n.traced.out" 2> "$O/$kind-w$n.traced.err"
  echo "$kind-w$n traced exit=$?"
  rm -rf "$cache"; cache=$(mktemp -d "$O/base-cache.XXXX")
  mem_ok; echo "$kind-w$n untraced $(env_line)" >> "$O/env.log"
  python3 "$TIMEIT" "$kind-w$n" "$O/runs.jsonl" -- env AL_SEM_REACH_WORKERS=$n \
    AL_SEM_IMPACT_BASE_CACHE=$cache bash -c "\"$B/alsem-head\" impact \"$WS\" $args < \"$S/range.diff\" > \"$O/$kind-w$n.out\" 2> \"$O/$kind-w$n.err\""
  echo "$kind-w$n untraced exit=$?"
  rm -rf "$cache"
}

roots_args() { jq -r '.roots[].stableId | "--root-id=" + .' "$O/diff-w1.out"; }

partup() { # exe label count
  local exe=$1 label=$2 n=$3
  mapfile -t R < <(roots_args)
  mem_ok; echo "$label traced $(env_line)" >> "$O/env.log"
  AL_SEM_REACH_WORKERS=$n ALSEM_TRACE=1 AL_SEM_UP_WALK_WINDOW=1 ALSEM_TRACE_FILE="$O/trace-$label.json" \
    "$exe" impact --part up "${R[@]}" "$WS" > "$O/$label.traced.out" 2> "$O/$label.traced.err"
  echo "$label traced exit=$?"
  mem_ok; echo "$label untraced $(env_line)" >> "$O/env.log"
  python3 "$TIMEIT" "$label" "$O/runs.jsonl" -- env AL_SEM_REACH_WORKERS=$n bash -c \
    "\"$exe\" impact --part up $(printf '%q ' "${R[@]}") \"$WS\" > \"$O/$label.out\" 2> \"$O/$label.err\""
  echo "$label untraced exit=$?"
}

phase=$1; shift
case "$phase" in
  diff|base) for n in "$@"; do composite "$phase" "$n"; done ;;
  partup) for n in "$@"; do partup "$B/alsem-head" "partup-w$n" "$n"; done ;;
  master) partup "$B/alsem-master" "partup-master" 1 ;;
  # Master's composite `impact --diff` at the default 8, untraced: the spec's memory bar
  # (the head's peak at 8 against master's on the same input).
  masterdiff)
    mem_ok; echo "masterdiff-w8 untraced $(env_line)" >> "$O/env.log"
    python3 "$TIMEIT" "masterdiff-w8" "$O/runs.jsonl" -- env AL_SEM_REACH_WORKERS=8 bash -c \
      "\"$B/alsem-master\" impact \"$WS\" --diff - --diff-root $TOP --format json < \"$S/range.diff\" > \"$O/masterdiff-w8.out\" 2> \"$O/masterdiff-w8.err\""
    echo "masterdiff-w8 untraced exit=$?" ;;
esac
