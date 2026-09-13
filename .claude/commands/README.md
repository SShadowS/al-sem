# Project slash commands

Versioned on purpose: each encodes project doctrine.

| Command | What it does |
|---------|--------------|
| `/triage-wave` | FP-triage a wave of new L5 detectors on a real workspace, one subagent per detector, then gate >30%-FP detectors to opt-in. |
| `/orchestrate [--dry-run] [--max-issues N]` | One autonomous tick over the GitHub issue queue: preflight, fetch, rank, claim, run `/issue`, post-merge check, file discoveries. Re-fire with `/loop`. Start with `--dry-run`. |
| `/issue N` | The per-issue pipeline: worktree, classify, probes, spec, two-model panel, acceptance tests, plan, TDD with discrimination proofs, gates, final panel, attested squash-merge. Requires a claim. |

The orchestrator's executor is `python scripts/agentflow <subcommand>`; its tests
run with `python -m pytest scripts/agentflow/tests -q`. Kill switch: create
`.agent/HALT` in the main checkout. Resume an issue a human has looked at with
`python scripts/agentflow unblock N`. Spec:
`docs/superpowers/specs/2026-09-13-issue-orchestrator-design.md`.
