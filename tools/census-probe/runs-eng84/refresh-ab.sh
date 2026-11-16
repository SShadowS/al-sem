#!/usr/bin/env bash
# ENG-84: session refresh step timings, master e708f1f6 (base) vs ENG-84 995c75b1 (head),
# default mode, no --verify (the step timers do not need it). PASSES passes, order flipped each
# pass; each pass waits outside the slot for host `some avg60` <= 15 %, then holds the slot.
# Output: decl-ab.tsv (pass order side step class programMs refreshMs)
set -u
S=$(cd "$(dirname "$0")" && pwd)
REPO=/work/alsem-wt/lane-engine
PASSES=${1:-6}
BIN=~/.cache/al-sem/cdo-speed/bin
BASE=$(ls -d $BIN/e708f1f6*/aldump)
HEAD=$(ls -d $BIN/995c75b1*/aldump)
OUT=$S/decl-ab.tsv
[ -f "$OUT" ] || printf 'pass\torder\tside\tstep\tclass\tprogramMs\trefreshMs\n' > "$OUT"

one() { # pass order side exe
  local copy=$S/ab-copy
  rm -rf "$copy" && cp -r /work/src/cdo-baseline/Cloud "$copy"
  "$4" --session-refresh "$copy" --file "src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al" \
    > "$S/ab-$1-$3.out" 2>&1 || { echo "run failed: $1 $3"; return 1; }
  grep '^{' "$S/ab-$1-$3.out" | jq -r --arg p "$1" --arg o "$2" --arg s "$3" \
    '[$p, $o, $s, .step, .class, .programMs, .refreshMs] | @tsv' >> "$OUT"
  rm -rf "$copy"
}

for p in $(seq 1 "$PASSES"); do
  until "$REPO/scripts/cdo-slot" status | grep -q '^free'; do sleep 20; done
  waited=0
  while awk '/^some/ { split($3, a, "="); exit !(a[2] > 15) }' /proc/pressure/cpu; do
    [ "$waited" -ge 600 ] && break; sleep 15; waited=$((waited + 15))
  done
  if [ $((p % 2)) -eq 1 ]; then order=base-first; a=base; ae=$BASE; b=head; be=$HEAD
  else order=head-first; a=head; ae=$HEAD; b=base; be=$BASE; fi
  export -f one; export S OUT
  "$REPO/scripts/cdo-slot" run engine "ENG-84 timed refresh A/B pass $p/$PASSES ($order, ~3 min)" -- \
    bash -c "one $p $order $a $ae && one $p $order $b $be" || { echo "pass $p failed"; exit 1; }
  echo "pass $p done ($order, waited ${waited}s, host $(awk '/^some/ {print $3}' /proc/pressure/cpu))"
done
