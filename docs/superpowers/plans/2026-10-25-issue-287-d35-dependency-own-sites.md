# d35 dependency-subscriber own sites (#287) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give detectors the dependency half of the uncertainty edges, keyed by `from`, so d35 can name a dependency subscriber's own unresolved sites.

**Architecture:** Add a second index on `DetectorArtifacts`, built by the same helper as `uncertainty_edges_by_from` but over `rel.dependency_uncertainty()`. Add an accessor that chains both halves, and point d35's own-site read at it. Nothing else reads the new field.

**Tech Stack:** Rust (`al-sem` crate), nextest CLI tests.

**Spec:** `## Design` and `## Acceptance matrix` in `.agent/issue-287/ledger.md` (ratified in 3 spec rounds; register `.agent/issue-287/findings.json`, R1–R13).

## Global Constraints

- `uncertainty_edges_by_from` keeps its contents exactly (order and duplicates included). `uncertainty_view()`, `uncertainties_by_node` and the L4 solver are not touched.
- Finding ids, fingerprints, severities, and the `confidence` object stay as they were on base 08c58d63.
- `rustfmt <file>` only. No edits under `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/` (the ledger proof table is the one exception, see Task 1).

## Review Focus

1. A dependency routine whose row is opaque or parse-incomplete must stay under "partly analysed": `drop_self` reads `incomplete_ids`, and this design does not change that.
2. In a single-app run the new map is empty, so the output is identical to today's (A3 check 5).
3. A primary subscriber reads the same edges in the same order as before (A2 and A3 check 4).

---

### Task 1: dependency uncertainty index + d35 reader

**Files:**
- Modify: `src/engine/l5/detector_context.rs`: the field near `:858`, the build near `:1624`, the census near `:1875`, the artifacts literal near `:1795`, and `empty_artifacts` if that is where the defaults live.
- Modify: `src/engine/l5/test_support.rs:442` (the literal).
- Modify: `src/engine/l5/detectors/d35.rs`: the module doc at `:4` and `:38-40`, and the read at `:318`.
- Modify: `src/engine/l4/exec_relation.rs`: docs at `:38-41`, `:132-134` and `:139-140`.
- Modify: `src/engine/l4/capability_cone.rs`: `step4_dependency_uncertainty_stays_out_of_primary`, and its fixture if needed.
- Modify: `tests/cli/cli_d35_unknown_commit.rs`: rename `dependency_subscriber_keeps_todays_text` to `dependency_subscriber_names_its_own_sites`, and add A5's census test.
- Modify: `CHANGELOG.md`: `[Unreleased]` `### Fixed`.

**Interfaces:**
- Produces:
  - `pub dependency_uncertainty_edges_by_from: HashMap<String, Vec<crate::engine::l4::combined_graph::Uncertainty>>` on `DetectorArtifacts`;
  - `fn index_uncertainty_by_from(edges: &[UncertaintyEdge]) -> HashMap<String, Vec<Uncertainty>>`, private;
  - `pub fn own_uncertainty_edges(&self, routine_id: &str) -> impl Iterator<Item = &Uncertainty>` on `DetectorArtifacts`, yielding Primary edges first, then dependency edges.

- [ ] **Step 1: Record base values.** On base 08c58d63, before any edit, run A6d's workspace (`dependency_subscriber_ws`) at `--scope all` and record three things for HandleRaised's d35 finding: its `confidence` object, its fingerprint, and the leading coverage-reason words of the reasons string (everything before `; partly analysed:`). These become literals in Step 2.
- [ ] **Step 2: Write the failing tests** exactly as the ledger's A1, A3 (checks 3, 4, 5) and A5 rows state.
- [ ] **Step 3: Run them and see each fail for the stated reason:**
  - A1: there is no `unresolved:` segment;
  - A3: the field does not exist (a compile failure is the stated red for (3), (4) and (5)); write (4) against today's map first, so it compiles and passes on its own;
  - A5: the bucket is missing.

  Record the outputs in the ledger's `## Discrimination proofs`.
- [ ] **Step 4: Implement** the field, the helper, the accessor and its invariant doc (P2's cites), the census row, d35's read, and every doc in the Design's Docs list.
- [ ] **Step 5: Run the tests and see them pass.** Run `scripts/ci-steps task-affected`.
- [ ] **Step 6: Discrimination proofs.** Make each break with Edit, run, revert with Edit:
  - point d35 back at `uncertainty_edges_by_from` → A1 fails;
  - build the new map from the Primary half → A3 (3) fails;
  - drop the Primary map's edges → A3 (4) fails;
  - skip the census row → A5 fails.
- [ ] **Step 7: Commit** the named files (never `git add -A`). Run `check-diff` first.
