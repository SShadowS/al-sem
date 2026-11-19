# ENG-20: the request page's member text (decision note, 2026-11-18)

Owner question: q-20261118T110644-48754843.

## The defect

A report procedure with a request-page trigger's name and parameters (`procedure OnOpenPage()`
beside the request page's `trigger OnOpenPage()`; alc accepts the pair) had the trigger's routine id.
The program node id (`RoutineNodeId`: object, name, member, parameter count and types) and the stable
id (`stable object id # normalized_signature_hash(name, params, return)`, discriminated only by the
enclosing member) both lack the routine kind. The build kept one body. Re-probed on master `a73b4bf6`,
both declaration orders: a run answered `writes-table: no` for the trigger's write, and a call of the
procedure read the trigger's body or answered `unknown`.

The fix gives a request page's own triggers an enclosing member, so both ids differ from the
procedure's. Measured on CDO: 0 findings and 0 fingerprints move (both scopes); 11 stable routine ids
move (its request-page triggers, in `alsem fingerprint` output only). Fixtures: 1 stable id
(`ws-report-dataitem`), no fingerprint, every parent-minted baseline still suppresses.

## The member text: two options

The member text is hashed into both ids, so changing it after landing moves those ids again.

**A. `RequestPage` (readable).** `enclosingMember: "RequestPage"` in analyze JSON and snapshots reads
naturally. Residual: a report dataitem (or a request-page control) named `RequestPage`, with a trigger
of a request-page trigger's name, collides again. The only shared names are the record triggers:
a dataitem's `OnAfterGetRecord` and a request page's `OnAfterGetRecord` (a request page with a
`SourceTable`). Rare, but a wrong answer of the same kind.

**B. A text no parsed name can produce (collision-free).** tree-sitter-al's `quoted_identifier` is
`"` (`[^"\n]+` | `""`)* `"` and `identifier` is letters, digits and `_`: no name contains a newline
(`"` is possible through `""`). A member text with a newline, e.g. `"RequestPage\n"`, can never equal
a dataitem's or control's member, so the collision is closed by construction in this engine. Cost:
`enclosingMember` in analyze JSON and snapshots shows `"RequestPage\n"` (snapshot readers read it
back as text, so it round-trips); a human reading the JSON sees the escape.

**C. Identity and display apart (built).** The member stays `RequestPage` for display, and a flag
the lowerer sets from the `requestpage` section itself (no name can set it) is its own part of every
id: `RoutineDecl::is_request_page_trigger`, `CanonicalRoutineKey::request_page` and
`to_stable_routine_id_from_parts`'s `request_page` (one more hash PART only when set, so every other
id is unchanged and no member text can collide), `RoutineNodeId::request_page` (the program graph;
pack schema 21). The display prints `Report X.requestpage::OnOpenPage`, and a field or control named
`RequestPage` prints quoted (`."RequestPage"`), so a pasted display selects exactly one routine.

**Decided: C**, owner, q-20261118T113518-4b94aa60 ("I want the best solution, not the fastest,
simplest or easiest. The result has to be correct, as this is about deterministic output."). Same
moves as A: on CDO 11 stable routine ids (its request-page triggers), 0 findings, 0 fingerprints.
Principles: 1 (correct first), 3 (root cause), 13 (no magic text).
