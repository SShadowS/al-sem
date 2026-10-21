#!/usr/bin/env bash
# kraken setup (idempotent): the sibling repos al-sem builds against, on the work volume.
set -euo pipefail
mkdir -p /work/src
# the target of the image's ~/.al-sem link (.kraken/Dockerfile): agentflow's runs and timings survive a rebuild
mkdir -p /work/.al-sem
# The AL grammar: CI clones main too; build.rs refuses a grammar whose node-types.json hash drifts
if [ ! -d /work/src/tree-sitter-al/.git ]; then
  git clone https://github.com/SShadowS/tree-sitter-al.git /work/src/tree-sitter-al
fi
# agentflow's preflight looks only at <checkout>/tree-sitter-al (gitignored), not TREE_SITTER_AL_PATH
[ -e tree-sitter-al ] || ln -s /work/src/tree-sitter-al tree-sitter-al
# .gitignore's `/tree-sitter-al/` matches only a directory, not this symlink
ex="$(git rev-parse --git-common-dir)/info/exclude"
grep -qx '/tree-sitter-al' "$ex" 2>/dev/null || echo '/tree-sitter-al' >> "$ex"
# pi-mcp: /orchestrate's reviewer models (.mcp.json runs /work/src/pi-mcp/src/pi-server.ts)
if [ ! -d /work/src/pi-mcp/.git ]; then
  git clone https://github.com/SShadowS/pi-mcp.git /work/src/pi-mcp
else
  git -C /work/src/pi-mcp pull --quiet --ff-only || true
fi
(cd /work/src/pi-mcp && { [ -d node_modules ] || npm install --silent; } && npm run --silent --if-present build)
# agentflow's settings for this checkout: the CDO gate reads the baseline below; one heavy gate at a
# time until two have been measured to fit inside the tentacle's 24g (then raise heavy_gates to 2)
mkdir -p .agent
[ -f .agent/config.json ] ||
  echo '{"slots": 2, "heavy_gates": 1, "cdo_ws": "/work/src/cdo-baseline/Cloud"}' > .agent/config.json
# The owner's Claude setup, gitignored on the PC, arrives as one tar through secret_files
# (.claude/kraken-claude.tar): project/ is this checkout's .claude (agents, skills, hooks), user/ is
# the owner's user-level CLAUDE.md and skills. kraken re-runs setup when the tar's bytes change.
if [ -f .claude/kraken-claude.tar ]; then
  t=$(mktemp -d)
  tar -xf .claude/kraken-claude.tar -C "$t"
  for d in agents skills hooks; do rm -rf ".claude/$d" && cp -r "$t/project/$d" ".claude/$d"; done
  cfg="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
  mkdir -p "$cfg/skills"
  cp "$t/user/CLAUDE.md" "$cfg/CLAUDE.md"
  for s in "$t"/user/skills/*; do rm -rf "$cfg/skills/$(basename "$s")" && cp -r "$s" "$cfg/skills/"; done
  rm -rf "$t"
fi
# The owner's plugins (on the Claude state volume, so this runs only when one is missing)
if command -v claude >/dev/null; then
  have=$(claude plugin list 2>/dev/null || true)
  for m in anthropics/claude-plugins-official DietrichGebert/ponytail; do
    claude plugin marketplace add "$m" >/dev/null 2>&1 || true
  done
  for p in superpowers@claude-plugins-official rust-analyzer-lsp@claude-plugins-official ponytail@ponytail; do
    echo "$have" | grep -q "${p%@*}" || claude plugin install "$p" ||
      echo "kraken-setup: plugin $p did not install" >&2
  done
fi
# The CDO baseline is data, copied from the owner's PC (private Azure DevOps; .alpackages gitignored):
# a clean depth-1 git checkout of DO at the commit the goldens stamp, autocrlf=true as minted on
# Windows. Agents pass /work/src/cdo-baseline/Cloud to scripts/cdo-gate.
[ -f /work/src/cdo-baseline/Cloud/app.json ] || echo "kraken-setup: /work/src/cdo-baseline is missing (data copy)" >&2
