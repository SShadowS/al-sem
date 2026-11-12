#!/usr/bin/env bash
# TLS-21 step 0: one traced one-shot `impact --diff` on CDO (the TLS-16 input: bc3ccb18~5..bc3ccb18),
# release-fast alsem of this worktree with the up part's spans. Where do the up part's 53-62 s go?
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls21
A=/work/alsem-wt/lane-tools/target3/release-fast/alsem
TOP=/work/src/cdo-baseline
WS=$TOP/Cloud
mkdir -p "$S"
git -C "$TOP" diff --no-color --no-ext-diff --diff-algorithm=histogram --src-prefix=a/ --dst-prefix=b/ \
  bc3ccb18~5 bc3ccb18 -- Cloud > "$S/range.diff"
{ date +%T; uptime; grep MemAvailable /proc/meminfo; } > "$S/step0.env"
t0=$(date +%s)
python3 /work/alsem-wt/lane-tools/docs/superpowers/notes/crate-split-phase0/timeit.py step0-diff "$S/runs.jsonl" -- env ALSEM_TRACE=1 ALSEM_TRACE_FILE="$S/trace-impact-diff.json" bash -c "\"$A\" impact \"$WS\" --diff - --diff-root \"$TOP\" --format json < \"$S/range.diff\" > \"$S/impact-diff.out\" 2> \"$S/impact-diff.err\""
echo "diff exit=$? wall=$(( $(date +%s) - t0 )) s"
{ date +%T; uptime; } >> "$S/step0.env"
