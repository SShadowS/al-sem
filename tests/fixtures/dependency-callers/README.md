# dependency-callers fixture

Callers through dependency code (design r2,
`docs/superpowers/specs/2026-11-06-dependency-callers-design.md`). Two apps:

- `dep/`: a dependency that carries **Base Application's app id and name**, so the
  production configured-dispatch catalog entry
  (`impact_graph::configured_dispatch::CATALOG`) is the one under test;
- `ws/`: the workspace. `ws/al-sem.dependencies.json` loads `dep/` as a **source
  folder** (trust tier local source). The tests also mint the same `dep/` source into
  a **package** (`.app` with embedded source) in a copy's `.alpackages`, and every
  expectation holds in both forms: dependency code is decided by app identity, not
  by R0's trust tier.

The root is `DC Target.Target` (it inserts `DC Log`).

| Workspace routine | Path to `Target` | Class |
| --- | --- | --- |
| `DC Direct.CallTarget` | calls `Target` | ordinary code call |
| `DC Mid.Step` | calls `Target` | ordinary code call |
| `DC Sub.HandleAfterRun` | subscriber of dep `DC Engine.OnAfterRun`; calls `Mid.Step` | ordinary code call |
| `DC Sub.HandleAfterPostIt` | subscriber of dep `Workflow Response Handling.OnAfterPostIt`; calls `Target` | ordinary code call |
| `DC Caller.Start` | calls dep `DC Engine.Run` -> `OnAfterRun` -[event]-> `HandleAfterRun` -> `Step` -> `Target` (5 hops; enters the workspace two hops before the root) | through dependency code, `code` |
| `DC Outer.Outer` | calls `DC Caller.Start` (a workspace hop BEFORE the dependency) | through dependency code, `code` (6 hops) |
| `DC Toucher.Touch` | `Insert(true)` on dep table `DC Dep Entry`, whose `OnInsert` calls `Engine.Run` (first hop: a record trigger) | through dependency code, `code` |
| `DC Approver.Approve` | calls dep `ExecuteResponse`, whose `case WorkflowResponse."Function Name"` arm `PostCode()` calls `PostIt` -> `OnAfterPostIt` -[event]-> `HandleAfterPostIt` -> `Target` | through dependency code, `configured` (listed after the `code` callers) |

`Workflow Response Handling.ExecuteResponse` has the catalogued signature `(var
Variant: Variant; ResponseWorkflowStepInstance: Record "Workflow Step Instance";
xVariant: Variant)`. The configured region is its case's arm labels (`DoNothingCode()`,
`PostCode()`), arm bodies and `else` body (`OnExecuteWorkflowResponse`); its first call,
`OnBeforeExecuteResponse`, is outside the case and not labelled.

Ids 52800-52999. Nothing here is compiled; the engine reads the source only. The apps
sit in `ws/` and `dep/` (no root `app.json`), so `ids_parent_baseline` does not
enumerate them.
