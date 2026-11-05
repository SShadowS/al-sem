# caller-relations fixture

Two definitions of "the callers of `CR Target.Target`" meet here
(`program::impact_graph::caller_agreement_tests`):

- the **code-call relation**: what the program engine resolves for code that
  runs, the solve graph's (`query touches` up, the detectors' caller walks);
- the **may-execute relation**: R0's reverse walk over the sited graph, which
  also follows what a user starts on a page the code opens.

On local code the first equals R0 restricted to code-call sites
(`reverse::is_code_call_site`); the second adds the openers below.

| File | Caller of `Target` | Code-call | May-execute |
| --- | --- | --- | --- |
| `Callers.al` | `CR Direct.CallIt` (a call) | yes | yes |
| `Callers.al` | `CR Runner.RunIt` (`Codeunit.Run` -> `OnRun`) | yes | yes |
| `Callers.al` | `CR Raiser.RaiseIt` (an event, subscriber calls `Target`) | yes | yes |
| `Callers.al` | `CR Modifier.ModifyIt` (`Modify(true)` -> `OnModify`) | yes | yes |
| `Pages.al` | `CR Openers.OpenLifecycle` (`Page.Run`; the page's `OnOpenPage` calls it) | yes | yes |
| `Pages.al` | `CR Openers.OpenExtended` (`Page.Run`; a page EXTENSION's `OnOpenPage` calls it) | yes | yes |
| `Pages.al` | `CR Openers.OpenAction` (`Page.Run`; only an action's `OnAction` calls it) | no | yes |
| `Pages.al` | `CR Openers.OpenWithPart` (`Page.Run`; only its part's `OnOpenPage` calls it) | no | yes |

The page extension's action (`ExtGo`) calls `Target` too: R0-3's
`select::direct_callers` counts it apart, as explain counts page actions
(F16j); `tests/cli/cli_tests_selection.rs`' `explain_callers_equal_the_cones_direct_callers`
runs over this fixture as well.

Ids 51000-51099. Nothing here is compiled; the engine reads the source only.
The app sits in `ws/` (no root `app.json`), so `ids_parent_baseline`, whose
record lists every fixture with a root `app.json`, does not enumerate it.
