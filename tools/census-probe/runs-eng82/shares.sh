#!/usr/bin/env bash
# ENG-82: inclusive CPU share of each exact frame name in pprof-probe's folded.txt
# (one line per stack, root first, `;`-joined, then the sample count).
# Usage: shares.sh <folded.txt> [frame...]
# resolve_recorded's 44.4% is from pprof.out: its rayon frames carry the name inside generics,
# so no frame is named exactly `resolve_recorded`.
set -euo pipefail
f=$1
shift
frames=("$@")
[ ${#frames[@]} -gt 0 ] || frames=(byte_to_pos lower_file ts_parser_parse \
  fold_identifier eq_fold_identifier from_raw field named_children kind note closure)
for name in "${frames[@]}"; do
  awk -v f="$name" '{n=$NF; t+=n; s=$0; sub(/ [0-9]+$/,"",s); k=split(s,a,";");
    for(i=1;i<=k;i++) if(a[i]==f){c+=n;break}}
    END{printf "%-22s %6.2f%% (%d of %d)\n",f,100*c/t,c,t}' "$f"
done
