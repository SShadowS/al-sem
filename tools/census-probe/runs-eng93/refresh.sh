#!/usr/bin/env bash
# ENG-93: session refresh --verify on a CDO copy, both modes, WITHOUT --file (the harness's own
# file choice, which found nothing on CRLF CDO before), with the ENG-93 aldump.
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-engine/494fcd35-e29c-4995-b44d-bcb017c905d2/scratchpad/eng93
ALDUMP=$S/aldump
for mode in default low-mem; do
  rm -rf "$S/cdo-copy" && cp -r /work/src/cdo-baseline/Cloud "$S/cdo-copy"
  extra=""; [ "$mode" = low-mem ] && extra="--low-mem"
  start=$(date -u +%T)
  "$ALDUMP" --session-refresh "$S/cdo-copy" --verify $extra > "$S/refresh-$mode.out" 2> "$S/refresh-$mode.err"
  echo "$mode rc=$? $start-$(date -u +%T)"
done
rm -rf "$S/cdo-copy"
