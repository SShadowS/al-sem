#!/usr/bin/env bash
# kraken setup (idempotent): the sibling repos al-sem builds against, on the work volume.
set -euo pipefail
mkdir -p /work/src
# The AL grammar: CI clones main too; build.rs refuses a grammar whose node-types.json hash drifts
if [ ! -d /work/src/tree-sitter-al/.git ]; then
  git clone https://github.com/SShadowS/tree-sitter-al.git /work/src/tree-sitter-al
fi
# agentflow's preflight looks only at <checkout>/tree-sitter-al (gitignored), not TREE_SITTER_AL_PATH
[ -e tree-sitter-al ] || ln -s /work/src/tree-sitter-al tree-sitter-al
# pi-mcp: /orchestrate's reviewer models (.mcp.json runs /work/src/pi-mcp/src/pi-server.ts)
if [ ! -d /work/src/pi-mcp/.git ]; then
  git clone https://github.com/SShadowS/pi-mcp.git /work/src/pi-mcp
else
  git -C /work/src/pi-mcp pull --quiet --ff-only || true
fi
(cd /work/src/pi-mcp && { [ -d node_modules ] || npm install --silent; } && npm run --silent --if-present build)
# agentflow's settings for this checkout: the CDO gate reads the baseline below; one heavy gate at a
# time inside the tentacle's 16g
mkdir -p .agent
[ -f .agent/config.json ] ||
  echo '{"slots": 2, "heavy_gates": 1, "cdo_ws": "/work/src/cdo-baseline/Cloud"}' > .agent/config.json
# The CDO baseline is data, copied from the owner's PC (private Azure DevOps; .alpackages gitignored):
# a clean depth-1 git checkout of DO at the commit the goldens stamp, autocrlf=true as minted on
# Windows. Agents pass /work/src/cdo-baseline/Cloud to scripts/cdo-gate.
[ -f /work/src/cdo-baseline/Cloud/app.json ] || echo "kraken-setup: /work/src/cdo-baseline is missing (data copy)" >&2
