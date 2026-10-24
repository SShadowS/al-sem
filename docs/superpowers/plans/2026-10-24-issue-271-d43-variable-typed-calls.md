# d43 follows variable-typed calls — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** d43 finds a guarded IsHandled dispatch through every exact, resolved call kind (bare calls, codeunit / record / page variables, `this.`, dependency publishers through a variable).

**Architecture:** Admit the `variable-typed-call` edge kind in `d43.rs::enumerate_dispatch_sites` next to `direct-call` and `object-run-resolved`; drop d43's dead `dependency-export` arm. No other detector or layer changes.

**Tech Stack:** Rust; cargo-nextest; `tests/cli` and `tests/r4` umbrellas.

**Spec:** the `## Design` section of `.agent/issue-271/ledger.md` (bounded; converged after 2 spec rounds; acceptance matrix items 1–7 with their breaks; evidence and fixtures A–I under `/work/bench/271/ws/`).

## Global Constraints

- `interface-dispatch` stays OUT (non-goal, item 5).
- Only d43's own `dependency-export` arm is removed; the other dead arms (capability_cone, digest, ordering_inter) are untouched (Discovery D1).
- No golden may move; a moved golden is a discovery to explain, never a regen.
- `rustfmt <file>` only; Edit/Write only; protected paths `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/`.

## Review Focus

1. Item 3 must NOT reuse `ishandled_workspace` (its dependency already has a guarded bare dispatch); its own helper, evidence path asserted to start at `Ws Caller`, `[]` under the break.
2. The `this.` fixture declares runtime 14.0.
3. The IsHandled binding for member calls is index-based over call arguments only; a regression where the receiver shifts the index would mis-bind.
4. Item 5 uses the INVERSE break.
5. Update the comment at `cli_loose_fingerprint.rs:1350-1351`, the d43 module doc (Substrate; the deliberate departure from the al-sem port, `d43.rs:1-2`, `:69-70`), and CHANGELOG (Fixed).

---

### Task 1: admit variable-typed-call in d43

**Files:**
- Modify: `src/engine/l5/detectors/d43.rs` (`enumerate_dispatch_sites` ~:85-91; module doc)
- Modify: `tests/cli/cli_loose_fingerprint.rs` (items 1, 2, 4, 5; comment ~:1350)
- Modify: `tests/r4/r4_cross_app_program.rs` (item 3, own helper)
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Write the failing tests** — matrix items 1–5 exactly as the Design states (fixtures from `/work/bench/271/ws/{A,B,C,E,F,G,H,I}`).
- [ ] **Step 2: Run red** — items 1–4 red on the base; item 5 green (control) until its inverse break.
- [ ] **Step 3: Implement** the allow-list change and the doc/comment/CHANGELOG updates.
- [ ] **Step 4: Green** — targeted tests, then `scripts/ci-steps task-affected`; `scripts/check-goldens` (no regen) must pass (item 6).
- [ ] **Step 5: Discrimination proofs** — removal break (items 1–4), inverse break (item 5), each made and reverted with Edit; record test | mutation | fail | pass.
- [ ] **Step 6: Commit** after `check-diff`.
