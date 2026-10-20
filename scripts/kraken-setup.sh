#!/usr/bin/env bash
# kraken setup (idempotent): the sibling repos al-sem builds against, on the work volume.
set -euo pipefail
mkdir -p /work/src
# The AL grammar: CI clones main too; build.rs refuses a grammar whose node-types.json hash drifts
if [ ! -d /work/src/tree-sitter-al/.git ]; then
  git clone https://github.com/SShadowS/tree-sitter-al.git /work/src/tree-sitter-al
fi
# The CDO baseline is data, copied from the owner's PC (private Azure DevOps; .alpackages gitignored):
# a clean depth-1 git checkout of DO at the commit the goldens stamp, autocrlf=true as minted on
# Windows. Agents pass /work/src/cdo-baseline/Cloud to scripts/cdo-gate.
[ -f /work/src/cdo-baseline/Cloud/app.json ] || echo "kraken-setup: /work/src/cdo-baseline is missing (data copy)" >&2
