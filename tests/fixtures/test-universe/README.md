# test-universe fixture

A production app and its sibling test apps (agent-tasks design §4, milestone C). The tests
copy this folder into a temporary folder with a `.git` marker, so auto-discovery searches only
these apps.

| Folder | What it pins |
| --- | --- |
| `App` | the production app (the workspace): `TU Core` with `Post` (calls `Calc`, `Leaf`, then `Item.Modify()`), `Calc`, `Leaf` (called only by `Post`), `Untested` (no caller), `Audited` (called only by a subscriber to the platform's `OnAfterModifyEvent` of `TU Item`, which `Post`'s `Item.Modify()` raises) and `Logged` (called only by a subscriber to the platform's `OnAfterInsertEvent` of `TU Log`, a table no code writes: an unraised platform table event) |
| `Test` | a test app depending on `App` with a `Subtype = Test` codeunit: `PostUpdatesQty` calls `Post`, `CalcAddsOne` calls `Calc`. It also declares `Library Assert` (Microsoft), which is not loaded: the missing-library next step |

The tests add a third sibling, `TestFR` (an `app.json` depending on `App`, no `.al` file: skipped
`no-source`), to their temporary copy only: an app folder with no source builds no model, and
the fixture oracles build every app folder under `tests/fixtures`.

Ids 50800-50899. Nothing here is compiled; the engine reads the source only.
