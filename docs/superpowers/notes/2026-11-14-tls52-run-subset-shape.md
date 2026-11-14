# TLS-52: a run that starts only part of an object (edge shape for `alsem graph`)

Status: **approved by the owner 2026-11-14** ("Yes, build it", coord question
`q-20261114T103029-2c027eca`), as proposed below.
Task: TLS-52 (coord). Related: ENG-19, engine limits L20, spec
`docs/superpowers/specs/2026-10-13-callgraph-export-design.md` (rule 1, §4.1, §4.2).

## The problem

`Report.RunRequestPage` runs only part of a report: `OnInitReport` and the request page
(its `OnInit`, `OnOpenPage`, `OnQueryClosePage`, `OnClosePage` and record triggers). It
does not run `OnPreReport`, any data item or `OnPostReport`. This was measured on the
BC 28.4 Linux sidecar and confirmed on Windows (L20). ENG-19 taught the engine this:
`resolver::start_runs_trigger(kind, method, trigger, member_level)` decides which triggers
a start method runs, and `retain_started_triggers` drops the others from the call's routes.

`alsem graph` (and the sited graph that `alsem impact` walks) loses that subset. The
projection (`impact_graph/project.rs`, `engine_route`) turns every `run` route into one
`Run` edge to the report's **object node**. Rule 1 (lifecycle) then gives that object node
a `Trigger` edge to **every** trigger of the report and its extensions. A walk backwards
from `OnPreReport` therefore reaches every caller of `RunRequestPage`. The result
over-selects tests; it never hides one. L20 records it as an over-approximation.

Today `RunRequestPage` is the only partial start: for every other (kind, method),
`start_runs_trigger` is true for every trigger. On the pinned CDO workspace the method
appears 7 times in 4 files (grep, 2026-11-14), so CDO's output moves very little. The
shape matters for correctness and for any later partial start.

## Proposal: a start node per (object, partial start method)

1. **A new node kind, `ObjectStart`.** There is one per (run object, start method) pair
   where the method runs a strict subset of the object's triggers, and only when some
   site uses that pair. Its fields are those of the `Object` node (`appId`, `objectType`,
   `objectId`, `objectName`, `dependency`, `symbolOnly`) plus `start`: the method,
   folded (`"runrequestpage"`; as built, the call site's own spelling is not kept). Its id is `s:`, a hash over `(app guid, object kind,
   object key, start method folded)`. Like every id it stays opaque (§4.1).
2. **The run edge from such a site goes to the start node** instead of the object node.
   The edge is unchanged otherwise: `kind: "Run"`, `via: "run"`, the same confidence,
   `line` and conditions. A full start (`Run`, `RunModal`, `Execute`, `Print`, `SaveAs*`,
   …) still goes to the object node.
3. **The start node's lifecycle is the object node's lifecycle, filtered by
   `start_runs_trigger`.** Each trigger of the object and its extensions gets a
   `Trigger` / `lifecycle` edge only when `start_runs_trigger(trigger's object kind,
   method, trigger name, enclosing member is some)` is true. That function stays the only
   place that knows the subset (principle 19). **As built:** the filter applies only to
   the triggers a run runs (`reverse::run_reaches_trigger`, the `run_triggers` list). In
   the engine's use, `start_runs_trigger` only ever sees those, and its member-level rule
   rejects data-item triggers. The lifecycle also holds UI triggers such as a request page
   field's `OnLookup`, which that rule would reject too, and the user may start them on
   the shown page. They are kept (found on CDO's Report 25, 2026-11-14). Everything else
   in the object's lifecycle is kept as it is: the parts and pages it opens, its own
   platform events, and its app's callbacks hub when it is symbol-only. Each of these may still run during the request
   page, and keeping them can only over-select, never hide. (A report has no `tables`
   entry: only an XmlPort's table elements fill it.)
4. **"Is this start partial?" comes from `start_runs_trigger` itself.** The method is
   partial for an object when the function rejects at least one of that object's triggers.
   No second list of partial methods is added.
5. **Unchanged:**
   - The hubs. `h:run:report` (a runtime report id) still expands to object nodes, so a
     dynamic `RunRequestPage` still over-selects.
   - `TestRequestPage` declarations (`declaredRunnable`). They still point at the object
     node: the test code runs the report itself elsewhere.
   - The engine's own answers. ENG-19 already filters them.

### What the walk sees

- Backwards from `OnPreReport`: the only edge into it comes from the object node, and only
  full starts point at the object node. `RunRequestPage` callers drop out. This is the fix.
- Backwards from `OnInitReport` or a request page trigger: the object node and the start
  node both point at it, so every caller of either kind of start is reached. Nothing is
  hidden.

## The schema and bc-dev-mcp

- **Additive in callgraph v1; no version bump.** §4.2 already says: "an unknown node
  `kind` [is read] as a node that only carries the walk". A start node is exactly that.
  Rule 1's sentence "Every `Run` edge targets the run object's node" becomes "the run
  object's node, or its start node for a partial start". No field changes meaning. A
  reader that ignores the new kind still gets a correct, narrower answer.
- **bc-dev-mcp needs no code change.** Checked at `bd6543be` (GitHub, 2026-11-14):
  - `parseAlSemCallGraphJsonl` (`src/core/call-graph.ts:182`) keeps a node's `kind` as a string and
    rejects nothing for it.
  - `reverseReachable` (`:269`) walks every edge backwards and never looks at node kinds.
  - `indexAlSemGraph` (`src/core/test-selection.ts:146`) joins changed code only to
    workspace `Method`/`Trigger` nodes, so it never joins to a start node.

  Two optional changes: name `ObjectStart` in the node-kind comment (`call-graph.ts:32`),
  and note the narrower selection in its changelog.
- **al-sem's own readers** of the sited graph must treat a start node as an object node
  where they ask "is this an object run?":
  - the object set and `Boundary` detection in `Builder::add` (`project.rs:290-296`);
  - `impact::up::object_run_reaches` (lifecycle out of a start node is code, as from an
    object node);
  - `test_select`, `reverse.rs` and the graph's object counts and hidden-callers logic.

  This is implementation work in this repo, listed so that review can check each one.

## Alternatives rejected

- **A. Run edges straight to each started trigger** (no new node). This repeats the
  lifecycle fan-out at every partial site, one edge per started trigger per site. It also
  loses the rest of the lifecycle (opened pages, callbacks hub) unless that is copied too,
  and leaving it out would hide tests. It also breaks rule 1's "a run targets an object
  node" with no node that names the start.
- **B. Keep the edge to the object node and add `start: "RunRequestPage"` to it.** The walk
  would then need state ("I came in through a partial start, so skip `OnPreReport`").
  Every consumer would have to write that, and one that does not still over-selects with
  no sign that it does. bc-dev-mcp's walk is stateless.
- **C. Reuse `kind: "Object"` with an extra `start` field.** A consumer that counts or
  keys on object nodes (al-sem's own object counts, a future consumer) would take the start
  node for a second copy of the object. A separate kind fails safe under §4.2.

## Tests (when approved)

- The task brief's fixture: a codeunit calls `Rep.RunRequestPage()` and another calls
  `Rep.Run()`.
  - The impact set (`alsem impact`) and the graph's reverse reach of `OnPreReport`, a data
    item trigger and `OnPostReport` contain the `Run` caller and not the `RunRequestPage`
    caller.
  - `OnInitReport` and a request page trigger reach both callers.
  - A report extension's `OnPreReport` behaves like the base report's.
  - Discrimination: send the partial site to the object node, and the exclusion assert
    fails.
- A test that a full start never makes a start node. Discrimination: make every start
  method partial.
- The CDO graph before and after: only the edges from the 7 `RunRequestPage` sites move,
  and each moved one points at a start node.

## Decision asked

Approve the `ObjectStart` node (proposal 1–5), additive in callgraph v1, with no bc-dev-mcp
change required. Recommendation: yes. Principles used: 1, 5, 13, 19 and 22 (additive after
release; never break a reader).
