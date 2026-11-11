#!/usr/bin/env bash
# TLS-16: the two one-shot impact calls cdo-gate's output_cdo makes, traced, one after
# the other (one CDO job at a time), with this worktree's cdo-profile alsem (master).
set -u
S=/tmp/claude-1000/-work-alsem-wt-lane-tools/d2bfaa88-a1b8-4b55-8bc1-15ddd941c4a9/scratchpad/tls16
A=/work/alsem-wt/lane-tools/target/cdo/alsem
TOP=/work/src/cdo-baseline
WS=$TOP/Cloud
git -C "$TOP" diff --no-color --no-ext-diff --diff-algorithm=histogram --src-prefix=a/ --dst-prefix=b/ \
  bc3ccb18~5 bc3ccb18 -- Cloud > "$S/range.diff"
base=$(mktemp -d "$S/base-cache.XXXX")
t0=$(date +%s)
ALSEM_TRACE=1 ALSEM_TRACE_FILE="$S/trace-impact-base.json" AL_SEM_IMPACT_BASE_CACHE="$base" \
  "$A" impact "$WS" --base bc3ccb18~5 --no-tests --format json > "$S/impact-base.out" 2> "$S/impact-base.err"
echo "base exit=$? wall=$(( $(date +%s) - t0 )) s"
t0=$(date +%s)
ALSEM_TRACE=1 ALSEM_TRACE_FILE="$S/trace-impact-diff.json" \
  "$A" impact "$WS" --diff - --diff-root "$TOP" --format json < "$S/range.diff" > "$S/impact-diff.out" 2> "$S/impact-diff.err"
echo "diff exit=$? wall=$(( $(date +%s) - t0 )) s"
# P4: prove's first call on RunReportID (the speed phase's round-1 routine), one-shot.
t0=$(date +%s)
ALSEM_TRACE=1 ALSEM_TRACE_FILE="$S/trace-prove.json" \
  "$A" prove "$WS" "CDO E-Mail Handler::RunReportID" may-commit --format json > "$S/prove.out" 2> "$S/prove.err"
echo "prove exit=$? wall=$(( $(date +%s) - t0 )) s"
# P5: a cold first ask (mostly the model build), one-shot explain of the same routine.
t0=$(date +%s)
ALSEM_TRACE=1 ALSEM_TRACE_FILE="$S/trace-explain.json" \
  "$A" explain "$WS" --routine "CDO E-Mail Handler::RunReportID" --format json > "$S/explain.out" 2> "$S/explain.err"
echo "explain exit=$? wall=$(( $(date +%s) - t0 )) s"
