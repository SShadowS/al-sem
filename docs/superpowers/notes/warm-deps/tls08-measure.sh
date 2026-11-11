#!/usr/bin/env bash
# TLS-08: first test build (the task gate's `cargo nextest run --workspace`, build only)
# in a fresh worktree of REV, cold vs seeded, alternating, N rounds.
# Usage: tls08-measure.sh <rev> <rounds> <seed-script> <out.jsonl>
set -uo pipefail
rev=$1 rounds=$2 seed=$3 out=$4
logdir=$(dirname "$out")/tls08-logs
mkdir -p "$logdir"
for i in $(seq 1 "$rounds"); do
    for mode in cold seeded; do
        wt=/work/alsem-wt/tls08-$mode-$i
        git -C /work/alsem worktree add -q --detach "$wt" "$rev" || exit 1
        secs() { awk -v a="$1" -v b="$2" 'BEGIN { print b - a }'; }
        seed_s=0
        if [[ $mode == seeded ]]; then
            t0=$EPOCHREALTIME
            "$seed" "$wt" || exit 1
            seed_s=$(secs "$t0" "$EPOCHREALTIME")
        fi
        t0=$EPOCHREALTIME
        (cd "$wt" && cargo nextest run --workspace --no-run) > "$logdir/$mode-$i.log" 2>&1
        rc=$?
        wall=$(secs "$t0" "$EPOCHREALTIME")
        compiled=$(grep -c "^ *Compiling " "$logdir/$mode-$i.log")
        units=$(grep "^ *Compiling " "$logdir/$mode-$i.log" | awk '{print $2}' | sort -u | tr '\n' ' ')
        printf '{"round":%d,"mode":"%s","rc":%d,"seed_s":%.1f,"build_s":%.1f,"compiled":%d,"units":"%s"}\n' \
            "$i" "$mode" "$rc" "$seed_s" "$wall" "$compiled" "${units% }" >> "$out"
        rm -rf "$wt" && git -C /work/alsem worktree prune
    done
done
