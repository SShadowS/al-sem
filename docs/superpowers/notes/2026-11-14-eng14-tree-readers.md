# ENG-14 Task 1: what each reader does for a routine with no control tree

Spec `docs/superpowers/specs/2026-11-14-eng14-310-remainder-design.md` §4.2 D: an untrusted routine has no
`statement_tree`. This lists every production reader of `statement_tree`, `unreachable_statements` and the
control contexts on master `a4072928`, and what it does with no tree. Read from the code; nothing was run.

**Changes needed** (the no-tree path makes a claim):

| reader | no-tree behaviour today | change |
|---|---|---|
| `ir_walk` unreachable scan (ir_walk.rs:953-990) → `unreachable_statements` → d20 (d20.rs:53-65), `DeadSites` | filled from the IR before the tree is built | clear the vector for an untrusted routine |
| `control_context::apply_control_contexts` (control_context.rs:467-501) | empty maps: sites keep no context | set every call and operation site `conditional` |
| `cfg_walker` (cfg_walker.rs:255) | `walk_flat`: straight-line, computes exit facts and `dirty_at_exit` | untrusted: path facts Unknown |
| `param_guard::frame_guards` (param_guard.rs:98-115) | a frame with `alias_error_ops`'s requirements | untrusted: no frame (`None`) |
| `pending_writes` (pending_writes.rs:659) | `all = true` (whole body) but `committed` starts true for non-empty targets | to settle in Task 2 against the code after line 662: "a Commit runs before every target" must not hold without a tree |
| d43 `classify_setter` (d43.rs:320-333) | `is_assignment_nested_in_tree(None)` false and `exit_before(None)` false → `MustSetTrue` | untrusted: `MaySetTrue` |

**Safe** (no tree, no claim):

| reader | no-tree behaviour |
|---|---|
| `return_summary` (return_summary.rs:431) | normal return, `all_paths_error` false, coverage partial |
| `dep_artifact_l4` (dep_artifact_l4.rs:772) | the same, `TriBool::False` |
| `operation_order::compute_operation_order` (operation_order.rs:623) | empty order: no dominance, no success-path claim |
| `literal_flow` (:297), `literal_values` (:450), `explain` (:791), `literal_reach` (:905, :4723) | `None` / empty: no pruning, no guards |
| `recref_origin` (:252) | `every()`: every origin (the conservative side) |
| `recref_flow` (:1645, :1963), `field_guard` (:31) | empty |
| d56 `unconditional_before`, `cross_rules::runs_before` (:1126) | `is_some_and` → false: no "runs before" claim |
| d62 `mutually_exclusive` | false → "not exclusive" (conservative) |
| d39 / `detectors::receiver_reset_after` (`load_point_after` with `None`) | reads as a reset after (suppresses the finding: conservative for a finding) |
| d61 (:97) | not found → `true`; a finding gate, to confirm in Task 2 |
| d60 (:163) | no branch found → the loop is not skipped: a possible extra finding, not a proof (precision only) |
| `snapshot` (:1534), `dep_artifact_l4` (:508) | copy the tree as it is |

Constructors with `statement_tree: None` (ABI rows, test literals, `format_pr_summary`, `root_classification`,
`fingerprint`, `d1_temp`, `d17`, `db_effect_solver`) build rows with no body, so they make no mark.
