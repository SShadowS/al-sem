# isolated-events fixture

Subscribers of isolated and normal events (engine limits B3). An isolated event
(`[IntegrationEvent(_, _, true)]`, `[BusinessEvent(_, true)]`,
`[InternalEvent(_, true)]`) runs each subscriber in a transaction of its own. When a
subscriber fails, the platform rolls back that subscriber's database changes and the
publisher goes on. This does not hold when the raising transaction has a write pending,
or while an extension is installed or upgraded: then the event runs as a normal event
(Microsoft Learn, "Isolated events in AL").

- `ws/`: the workspace (`Iso Publisher`, `Iso Subscriber`).
- `dep/SymbolReference.json`: a symbol-only dependency, `Iso Dep`. The tests pack it
  into a `.app` in a temporary copy's `.alpackages`; it has no source, so its
  publishers are read from their symbol attributes only.

| Subscriber | Event | Isolated | `callers-intercept-errors` |
| --- | --- | --- | --- |
| `HandleIso` | workspace `OnIsoEvent`, `[IntegrationEvent(false, false, true)]` | yes | `yes`, witness `isolated-event` |
| `HandlePlain` | workspace `OnPlainEvent`, `[IntegrationEvent(false, false)]` | no | `no` (with `--no-dependents`) |
| `HandleDepIso` | dependency `OnDepIso`, `[BusinessEvent(False, True)]` | yes | `yes`, witness `isolated-event` |
| `HandleDepPlain` | dependency `OnDepPlain`, `[BusinessEvent(False)]` | no | no `isolated-event` witness |

Ids 53000-53099. Nothing here is compiled; the engine reads the source and symbols only.
The app sits in `ws/` (no root `app.json`), so `ids_parent_baseline` does not enumerate it.
