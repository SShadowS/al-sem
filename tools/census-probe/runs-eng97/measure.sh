#!/usr/bin/env bash
# ENG-97: time one resident CDO model build, base (801cc6af) vs head (ENG-85, 96d49041),
# PASSES passes, the order flipped each pass. Admission as cdo-speed's (TLS-76): before each
# pass, outside the CDO slot, wait up to CALM_WAIT_S for the host's /proc/pressure/cpu
# `some avg60` to be at most CALM (%), then run anyway (labelled). Each run records the
# container's stall share (cpu.pressure `some total` delta over its wall time); a run above
# CALM is labelled under-load.
# Usage: measure.sh <passes>
# Output: runs.tsv (pass order side user sys wall maxrss host_avg60 stall_pct label)
set -u
S=$(cd "$(dirname "$0")" && pwd)
WS=/work/src/cdo-baseline/Cloud
PASSES=${1:-6}
CALM=${CALM:-15}
CALM_WAIT_S=${CALM_WAIT_S:-600}
REPO=/work/alsem-wt/lane-engine
OUT=$S/runs.tsv
[ -f "$OUT" ] || printf 'pass\torder\tside\tuser_s\tsys_s\twall_s\tmaxrss_kb\thost_avg60\tstall_pct\tlabel\n' > "$OUT"

host_avg60() { awk '/^some/ { split($3, a, "="); print a[2] }' /proc/pressure/cpu; }
own_total() { awk '/^some/ { split($5, a, "="); print a[2] }' /sys/fs/cgroup/cpu.pressure; }

one() { # pass order side admitted
  local log=$S/time-$1-$3.txt host t0 t1 u s w m stall label
  host=$(host_avg60)
  t0=$(own_total)
  /usr/bin/time -v "$S/bin-$3" "$WS" > "$S/out-$1-$3.txt" 2> "$log" || { echo "run failed: $1 $3"; exit 1; }
  t1=$(own_total)
  u=$(awk -F': ' '/User time/ {print $2}' "$log")
  s=$(awk -F': ' '/System time/ {print $2}' "$log")
  w=$(awk -F': ' '/Elapsed \(wall clock\)/ {print $2}' "$log" | awk -F: '{ if (NF==3) print $1*3600+$2*60+$3; else print $1*60+$2 }')
  m=$(awk -F': ' '/Maximum resident/ {print $2}' "$log")
  stall=$(awk -v a="$t0" -v b="$t1" -v w="$w" 'BEGIN { printf "%.1f", (b - a) / 1e6 / w * 100 }')
  label=calm
  awk -v x="$stall" -v c="$CALM" 'BEGIN { exit !(x > c) }' && label=under-load
  [ "$4" = timeout ] && label="$label,admitted-by-timeout"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$u" "$s" "$w" "$m" "$host" "$stall" "$label" >> "$OUT"
}

for p in $(seq 1 "$PASSES"); do
  waited=0; admitted=calm
  while awk -v x="$(host_avg60)" -v c="$CALM" 'BEGIN { exit !(x > c) }'; do
    if [ "$waited" -ge "$CALM_WAIT_S" ]; then admitted=timeout; break; fi
    sleep 15; waited=$((waited + 15))
  done
  if [ $((p % 2)) -eq 1 ]; then order=base-first; a=base; b=head; else order=head-first; a=head; b=base; fi
  export -f one host_avg60 own_total; export S WS OUT CALM
  "$REPO/scripts/cdo-slot" run engine "ENG-97 timed pass $p/$PASSES ($order, ~1 min)" -- \
    bash -c "one $p $order $a $admitted && one $p $order $b $admitted" || { echo "pass $p failed"; exit 1; }
  echo "pass $p done ($order, admitted $admitted after ${waited}s)"
done
column -t "$OUT"
