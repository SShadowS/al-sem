# Issue 11 — three ROOT_KIND_VALUES are declared but never emitted by the AST pass

## Claim

```json
{"run_id":"20260916-082325-37ceb4","issue":11,"attempt":1,
 "branch":"issue/11-agent-discovery-l5-page-action-a1",
 "worktree":"U:\\Git\\al-sem-issue-11-a1",
 "body_hash":"f612215f35833d889c3721e2d43ec7f24919e8a2ab85053a433931da5c8f6612"}
```

Worktree from `master` @ `df42d1e4`.

## Classification

**BOUNDED** -- and, after the panel, bounded in the way the issue's own option (a) proposed.

An earlier revision of this section said option (a) "is NOT bounded" and option (b) "would be
false". That rested on probe P4, which the panel refuted (below). Option (a) is a few lines
inside a branch that already exists. The summary is corrected here rather than left to
contradict the design, because a reader who stops at the classification would otherwise reject
the implementation the design now demonstrates. (astra r2, N6)

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 exactly 3 of the 12 kinds are never inserted | **holds** | `root_classification.rs` inserts nine: api-page, event-subscriber, install-codeunit, public-procedure, report-trigger, test-procedure, trigger-page, trigger-table, upgrade-codeunit. `page-action`, `web-service-exposed`, `job-queue-entrypoint`: zero insertions |
| P2 the trigger-page branch could distinguish an action | **holds, at the branch** | `root_classification.rs:98-100` inserts `trigger-page` for `Page`/`PageExtension` with no further discrimination |
| P3 the lowerer DOES know a member is an action | **holds** | `RoutineDecl.enclosing_member: Option<(String, Origin)>` (`al-syntax/src/ir/decl.rs:183`), set from `(ident_text, origin_of(member))` (`lower/mod.rs:711-719`); `Origin.kind_text` is "the raw grammar kind string" (`al-syntax/src/ir/mod.rs:42-48`) |
| ~~P4 the member KIND is DROPPED at the L3 boundary~~ | **FALSE — refuted by the spec panel** | See below |
| P5 real fixtures with page actions already exist | **holds** | `ws-policy-api-ui/src/Page.al:24-30` has `actions { action(DoAction) { trigger OnAction() } }`, as do three sibling `ws-policy-api-*` fixtures |
| **P6 the member kind DOES reach the classifier** | **holds — this replaces P4** | `L3Routine.enclosing_member_range: Option<PAnchor>` (`l3_workspace.rs:533`) and `PAnchor.syntax_kind: String` (`l2/features.rs:44-45`). `kinds_for` takes `&L3Routine`, so it can read it today |
| P7 the grammar has a distinct node for an action declaration | **holds** | `RawKind::ActionDeclaration` (`kind_policy.rs:202`, `raw/generated/nodes.rs`) |

### P4 was my error, and it inverted the design

I recorded that the enclosing member's kind is dropped at the L3 boundary, and built the whole
design on it: option (a) "is not effort-S", option (b) "would be false". The spec panel (astra)
rejected the premise and it is right.

`L3Routine` has TWO enclosing-member fields. I read `enclosing_member: Option<String>` at
`:521`, saw it carried only the name, and concluded the kind was lost — without reading
`enclosing_member_range: Option<PAnchor>` twelve lines below at `:533`, whose `syntax_kind`
is exactly the thing I declared missing. The classifier has had access all along.

This is the same failure mode this session has been correcting on another issue all night: a
confident claim about the code that a careful read refutes. It is recorded rather than quietly
replaced, because the design that follows is the opposite of the one it produced.

## Design

`page-action` is derivable NOW, in the branch that already exists, with no cross-layer change.

### The discriminator

Inside the existing `"Page" | "PageExtension"` arm (`root_classification.rs:98-100`), a trigger
whose enclosing member wrapper is an ACTION declaration also gets `page-action`:

```rust
"Page" | "PageExtension" => {
    set.insert("trigger-page".to_string());
    if routine
        .enclosing_member_range
        .as_ref()
        .is_some_and(|a| is_page_action_wrapper(&a.syntax_kind))
    {
        set.insert("page-action".to_string());
    }
}
```

Route, every link read directly rather than taken from a reviewer (see "Two reviewers, opposite
answers" below for why that mattered):

| step | evidence |
|---|---|
| the grammar gives an action declaration a `name` field | `grammar.js:2379-2386` |
| the lowerer makes a named, non-object, non-`_body` node the enclosing member | `lower/mod.rs:638-659` |
| it captures that member's origin | `lower/mod.rs:711-719` |
| the origin carries the raw grammar kind | `lower/mod.rs:1903-1919` sets `kind_text: n.kind_str()`; `raw/node.rs:29-32` |
| the kind string is the grammar's own snake_case name | `raw_kind.rs:1009` maps `RawKind::ActionDeclaration` to `"action_declaration"` |
| L3 assembly forwards the WRAPPER's origin, not the trigger's | `l3_workspace.rs:1260-1266` |
| the anchor keeps the kind verbatim | `l3_workspace.rs:625-638` sets `syntax_kind: origin.kind_text.to_string()` |
| the classifier receives it | `l3_workspace.rs:533`; `l2/features.rs:44-45`; `root_classification.rs:159-174` |

### Which wrappers qualify -- and what is actually PROVEN about each

The grammar has four action-declaration rules plus two neighbours, all six sharing the same
optional `declaration_body` (`grammar.js:2379-2445`). **That the grammar accepts a trigger in a
position is not proof that real AL permits one there** (astra r2, N1), so each is recorded with
the strength of evidence behind it rather than lumped together as "coverage":

| wrapper | matched? | evidence |
|---|---|---|
| `action_declaration` | YES | Real and exercised by the corpus (`ws-sibling-member-triggers`) |
| `systemaction_declaration` | YES | Real: Microsoft's PromptDialog page type documents `OnAction()` under `systemaction(Generate)` |
| `fileuploadaction_declaration` | YES | Real: documented trigger `OnAction(Files: List of [FileUpload])` -- note the non-empty signature |
| `customaction_declaration` | YES, **defensive only** | NOT verified as trigger-bearing. Microsoft documents customaction as a client-invoked Power Automate flow (`CustomActionType = Flow`, `FlowId`), a shape with no AL routine to classify. Matched because a false NEGATIVE on a real form costs more than dead code, but **no coverage is claimed**, and its test is labelled a parser-shape contract, not a real-AL witness |
| `separator_action` | NO | A visual separator is not invokable. REACHABLE as a member when written `separator(Name)` -- its `name` field is optional (`grammar.js:2390-2397`) -- so this exclusion is a live path worth an executable test |
| `actionref_declaration` | NO | Promotes an action declared elsewhere; that action's own trigger is already the root. Additionally UNREACHABLE as a member: it has `promoted_name`/`action_name` and no `name` field (`grammar.js:2399-2409`), so the lowerer's `FieldName::Name` gate never captures it. Its test pins intent, not a reachable path -- stated so nobody reads it as end-to-end coverage |

### KNOWN GAP -- action modifications are out of scope, deliberately

A page extension can add a trigger to an action it did not declare:

```al
actions { modify(SomeExistingAction) { trigger OnAfterAction() begin end; } }
```

That routine does **not** gain `page-action` under this design, and could not be fixed by adding
a fifth string to `is_page_action_wrapper`. The wrapper is `modify_action_modification`, which
carries `target` and no `name` (`grammar.js:2488-2496`), so the lowerer's generic member gate
never captures it at all; the lowerer's special case covers `ModifyModification` only
(`lower/mod.rs:628-637`). Fixing it means teaching the lowerer to capture that wrapper, which is
a cross-layer change and a separate issue.

This issue is therefore scoped to action DECLARATIONS. The gap is filed as a discovery rather
than left for a reader to mistake for an oversight. (astra r2, N4)

### Additive, not exclusive

An action's trigger keeps `trigger-page` AND gains `page-action`. The issue's acceptance wording
is ambiguous; additive is chosen because it only ever ADDS a kind. Verified against a real
consumer: `fingerprint_query.rs:705-735` filters on ANY intersection with the requested kinds
and emits one block per classification, so `--roots trigger-page` keeps selecting these actions
and selecting both kinds does not double-emit. Exclusive would REMOVE a kind from two existing
golden entries. (One consumer checked, not all.)

`canonical_kinds` sorts by `ROOT_KIND_VALUES` order (`:31-58`), so the moved line reads exactly
`["trigger-page", "page-action"]`.

### The overlay interaction -- three cases, not one

An earlier revision said the `[roots-config/kinds-mismatch]` warning "disappears". That is a
blanket claim and it is wrong (astra r2, N3). `overlay_config_roots` computes the symmetric
difference BEFORE unioning (`root_classification.rs:447-458`), so deduplication never suppresses
a mismatch, and the change moves warnings in both directions:

Each cell is the diagnostic's **(ast-only, config-only)** pair, spelled in full: an earlier
revision wrote the first two "before" cells as `warning (config-only)`, which is incomplete
shorthand -- the first one ALSO carries `ast-only=["trigger-page"]`, and a test written from the
shorthand would fail against the existing implementation. (astra r3)

| `roots.config.json` kinds for an ordinary action | before (AST `{trigger-page}`) | after (AST `{trigger-page, page-action}`) |
|---|---|---|
| `["page-action"]` | warning `(["trigger-page"], ["page-action"])` | **still warning**, `(["trigger-page"], [])` |
| `["trigger-page", "page-action"]` | warning `([], ["page-action"])` | **no warning** |
| `["trigger-page"]` | no warning | **NEW warning**, `(["page-action"], [])` |

All three rows produce the same deduped kind list after the change:
`["trigger-page", "page-action"]`. Warning objects keep `severity: "warning"` and
`stage: "discover"`.

The third row is a user-visible new diagnostic on a config that was previously silent. The
equality-based policy stays as it is; the three cases are documented and tested.

The union itself is benign: `BTreeSet` dedup (`:477-479`), so an overlay asserting `page-action`
never double-counts.

### The other two kinds -- what the comment may honestly say

`web-service-exposed` and `job-queue-entrypoint` stay unemitted. The comment must NOT say "not
derivable from AL source at all, by their nature" -- AL source declares the capability
(`PageType = API`, `[ServiceEnabled]`, `TableNo = "Job Queue Entry"` + `OnRun`), and the
classifier already derives `api-page` from exactly that (`:123-134`). Nor may it say the overlay
supplies verified deployment state: config-only roots carry `confidence: "user-asserted"`
(`:407-435`). The wording is therefore:

> Currently supplied only by the config overlay. Source may indicate capability; the overlay
> supplies user-asserted publication or scheduling information.

### Where the tests go -- and why NOT a new corpus fixture

A new `tests/r0-corpus/` directory moves three golden families (CLAUDE.md:612-621) for a change
whose real blast radius is otherwise four files. So:

1. **Unit tests through `classify_roots`, hand-stating the precondition.** Construct `L3Routine`
   values with `enclosing_member_range: Some(PAnchor { syntax_kind: ... })` by ASSIGNMENT
   (CLAUDE.md's doctrine). They must exercise `kinds_for` via `classify_roots`, not
   `is_page_action_wrapper` alone -- a helper-only test does not pin the production use, which is
   the exact failure mode CLAUDE.md records five instances of.
2. **The existing `ws-sibling-member-triggers` fixture is the end-to-end witness.** A `page
   50920` whose two `action(Alpha)`/`action(Bravo)` members each declare `trigger OnAction()`.
   `cli_b_snapshot_differential.rs:185-208` genuinely assembles and resolves the AL before
   serializing, so it witnesses the real lowerer/L3 route for the plain-action form. It proves
   nothing about the system/fileupload/custom forms, and it carries no negative case -- both its
   triggers are actions -- which is why the unit matrix above must supply both.

**Blast radius -- CORRECTED (astra r2, N2).** An earlier revision named three serializations plus
`tests/cli/cli_b_fingerprint_oracles.rs`. Both halves were wrong:

- There are **four** serializations, not three: `.raw.json`, `.envelope.json`, `.cbor` and
  `.cbor.gz` (all four on disk; `cli_b_snapshot_differential.rs` byte-compares each for every
  `SNAPSHOT_CORPUS` entry, and `ws-sibling-member-triggers` is member `:136`). Missing `.cbor.gz`
  would have left a stale golden.
- `cli_b_fingerprint_oracles.rs` does **NOT** move. Its only `trigger-page` occurrence is inside
  a valid-kind error message at `:188` that already lists `page-action` -- a vocabulary string,
  not a classification.
- The earlier "hashes over the payload move too" explanation was also wrong: `workspaceFingerprint`
  hashes input triples plus driver version, not classification payloads
  (`l5/snapshot_full.rs:776-865`). The envelope and CBOR move because they CONTAIN the changed
  classifications; the gzip moves because it compresses the changed CBOR. **No identity or
  fingerprint should change**, and one that does is an unexplained line, not a pre-authorized
  "id-shaped" movement.

`ws-d51-jobqueue`'s `kinds-mismatch` golden is unaffected: its mismatch is
`ast-only=["public-procedure"]` / `config-only=["job-queue-entrypoint"]` on a codeunit with no
page or action. No corpus config asserts `page-action` at all.

A text grep cannot establish the full output blast radius; `check-goldens` is what settles it.

## Acceptance matrix

| # | item | proof |
|---|---|---|
| A1 | An action trigger classifies as `page-action` | Through `classify_roots`, hand-stated `L3Routine` with `syntax_kind = "action_declaration"` on a `Page`; assert kinds == `["trigger-page", "page-action"]` |
| A2 | The other matched wrappers qualify | One case each for `systemaction_`/`fileuploadaction_declaration` (real forms) and `customaction_declaration` (labelled in the test name and a comment as a parser-shape contract, NOT real-AL coverage) |
| A3 | `PageExtension` behaves identically | A1 repeated with the owning **`L3Object.object_type`** set to `"PageExtension"` -- NOT `L3Routine`'s similarly-named field: `classify_roots` looks the object up and `kinds_for` branches on the OBJECT's type (`root_classification.rs:90-105,159-174`). A test that moves the routine field would pass while testing nothing (astra r3) |
| A4 | A non-action page trigger does NOT | Three negatives: `enclosing_member_range: None` (object-level `OnOpenPage`); `Some` with `"page_field"` (a field trigger); `Some` with `"separator_action"` (a named separator -- a REACHABLE exclusion). All assert `["trigger-page"]` |
| A5 | Non-trigger routines are unaffected | A procedure with an action-shaped anchor gets no `page-action` -- the `routine.kind == "trigger"` gate still holds |
| A6 | `actionref` is excluded by contract | Asserts `["trigger-page"]`, with a comment recording that the lowerer cannot produce this anchor (no `name` field), so it pins intent rather than a reachable path |
| A7 | Discrimination | Delete the `page-action` insertion, ASSERT the deletion applied (`s.count(old) == 1`), watch A1/A2/A3 fail and A4/A5/A6 stay green, byte-restore with a hash check, re-run |
| A8 | These witnesses emit exactly the ten derivable kinds -- NOT "the pass can never emit more", which no test establishes (see the `ROOT_KIND_VALUES` note) | Unfiltered union of `classify_roots` over witnesses covering all ten INCLUDING a real action trigger, against a literal ten-element set. Mutation is the REAL action-specific insertion. An earlier revision added "an equality test whose witnesses contain no action would stay green otherwise" -- that is FALSE now that `DERIVABLE_KINDS` lists `page-action`: omitting the action witness fails A8 immediately, before any deletion. The witness is required for A8 to pass at all; it is not what makes the deletion detectable (astra, final panel r3, non-blocking) |
| A9 | The complement invariant is ASSERTED, not just documented | `ROOT_KIND_VALUES` minus the derivable ten == `["web-service-exposed", "job-queue-entrypoint"]`, executably. Without this, a thirteenth declared-but-unemitted kind leaves A8 green (astra r2, N5) |
| A10 | The comment states the right reason | `ROOT_KIND_VALUES` carries the "capability vs user-asserted publication" wording above, not an impossibility claim |
| A11 | The three overlay cases behave as tabled | Executable test per row, asserting the exact diagnostic and the deduped kind list |
| A12 | Golden movement is exactly the four serializations, purely additive | Every changed `kinds` line gains `page-action` and loses nothing; `cli_b_fingerprint_oracles.rs` does NOT move; no identity or fingerprint changes. `golden-diff-triager` classifies every line before any regen; an unexplained line is a discovery, not a rebaseline |

## Two reviewers, opposite answers -- and why the record says so

On the one decisive fact, the panel SPLIT. astra said the member kind reaches the classifier;
flash said it is "completely discarded at the L3 boundary" and deriving `page-action` locally is
"structurally impossible", rated Critical with "confidence certain". Both cannot be right, and a
reviewer's confidence was worth nothing here.

It was settled by reading `anchor_from_origin` and its call site directly. astra was right. In
round 2, flash re-read the same lines and retracted its own finding without being told what to
conclude.

This is recorded because the design was ALSO wrong in the same direction first, for the same
reason: a claim about the code made without reading the five lines that refute it.

**A limit on this section, since the rest of this ledger is written to be checkable and this part
is not.** The reviews themselves live under `.agent/runs/<run-id>/`, which is NOT committed, so a
reader of this repository cannot verify the account of who said what. What IS checkable from the
repo is the only thing the account turns on: `anchor_from_origin`
(`src/engine/l3/l3_workspace.rs:625-638`) carries `kind_text` through, and `:1260-1266` passes the
member wrapper's origin. Verify those two and the narrative is beside the point — the
design either follows from them or it does not. The panel is
worth its cost only when its output is checked rather than counted, and on this issue the two
reviewers' round-2 outputs differed sharply in value -- flash accepted all eleven register
entries; astra rejected the blast-radius enumeration, the overlay claim and the coverage claim,
and every one of those rejections was verified correct.

## Timeline

| when | phase | outcome |
|---|---|---|
| 2026-09-16T08:25Z | claim | attempt 1, from `master` @ `df42d1e4` |
| | classify | BOUNDED |
| | probes | 5 recorded; **P4 REFUTED by the panel**, replaced by P6/P7 |
| | spec panel r1 | `charge spec_rounds` -> 2 left. astra rejected the premise; flash confirmed it. Split |
| | design v2 | inverted: derive in the existing branch, additive, four wrappers, no new corpus fixture |
| | spec panel r2 | `charge spec_rounds` -> 1 left. flash retracted and accepted all 11. astra re-raised F5/F7/F9/F10 and added N1-N6; all six verified correct |
| | design v3 | coverage claims qualified, blast radius corrected to 4 files, overlay matrix added, action-modification gap scoped out and filed |
| | spec panel r3 | `charge spec_rounds` -> 0 left. **CONVERGED**: both reviewers accepted all 18 entries (F1-F11, N1-N7) |
| | design v3.1 | two precision edits applied AFTER adjudication -- see below |

### The two post-adjudication edits, and why the register's hash moved

Both reviewers adjudicated against ledger hash
`50b4d49e...`. Two edits were then applied, so the register records a NEWER hash than the one
the reviewers saw. Both edits came from astra's own round-3 review and neither changes a
decision:

1. The overlay table's "before" cells were incomplete shorthand. `warning (config-only)` omitted
   that the first row's warning ALSO carries `ast-only=["trigger-page"]`; astra gave the full
   `(ast-only, config-only)` oracle and warned that a test written from the shorthand would fail
   against the existing implementation. The table now spells every cell out.
2. A3 said "repeated with `object_type = PageExtension`" without saying WHICH `object_type`.
   `classify_roots` looks the object up and `kinds_for` branches on the OBJECT's type, so moving
   `L3Routine`'s similarly-named field would leave a test that passes while testing nothing. A3
   now names `L3Object.object_type` explicitly.

Recorded rather than silently re-hashed, because "accepted against hash X" stops meaning
anything if the document can move afterwards without a note.

## Discrimination proofs

### A7 — the `page-action` insertion

| field | value |
|---|---|
| test target | `cargo test -p al-sem --lib root_classification` |
| mutation | delete the whole `if routine.enclosing_member_range … { set.insert("page-action") }` block from `kinds_for`'s Page arm |
| break asserted? | YES — anchor matched exactly once, and the script additionally asserted `set.insert("page-action"` is absent from the production half of the file after the edit. An unasserted scripted break proves nothing; its green run reads exactly like a passing test |
| fail output | exit 101 — **9 failed, 7 passed** |
| pass output | exit 0 — **16 passed, 0 failed** |
| restore | byte-restore verified by sha256: `7cd1b653…0f53` before and after, MATCH |

An earlier revision of this table said 9/6 and 15 passed. That was accurate when written and went
stale when the final panel added a sixteenth test (`the_two_root_kind_vocabularies_are_identical`,
which asserts over constants and so survives the break, like A9). Re-run rather than re-derived on
paper, because a proof table carrying numbers nobody re-measured is exactly the artifact this
repo distrusts. (flash, final panel)

**Which 9 failed — every test that asserts `page-action` is PRODUCED:** `action_trigger_is_page_action`
(A1), `systemaction_trigger_is_page_action` and `fileuploadaction_trigger_is_page_action` and
`customaction_wrapper_is_matched_parser_shape_contract_only` (A2),
`page_extension_action_trigger_is_page_action` (A3), `ast_pass_emits_exactly_the_ten_derivable_kinds`
(A8), and all three A11 overlay rows.

The three overlay rows fail by TWO DIFFERENT mechanisms, which an earlier revision of this
paragraph flattened into one (found by the final code review): rows 1 and 2 fail on their
DIAGNOSTIC assertion, because the AST half of the symmetric difference moves when the kind stops
being emitted; row 3 fails on its KINDS assertion, because the union collapses back to
`["trigger-page"]`. Same count, different guards — and a reader told only "the
diagnostics change" would not know row 3 covers the union.

**Which 6 held — every negative:** the three A4 negatives (`None`, `page_field`, `separator_action`),
A5's non-trigger gate, A6's `actionref` contract, and A9.

### A3 -- proving the FIX, after the panel showed the test proved nothing

astra's final-panel finding 5: A3 set the owning object to `PageExtension` and the routine's own
`object_type` to `"Page"`, then asserted the routine field had stayed `"Page"` -- but
`"Page" | "PageExtension"` is ONE match arm, so `kinds_for` reading the wrong field took the same
branch and produced identical kinds. **A3 passed whichever field was read**, while its comment
claimed it proved the object's field was the one consulted. It proved nothing, and I had earlier
praised that assertion in exactly the terms astra refuted.

The routine now carries `"Report"`, which takes a DIFFERENT arm. Proof of the fix:

| field | value |
|---|---|
| mutation | `match object.object_type.as_str()` -> `match routine.object_type.as_str()` in `kinds_for` |
| break asserted? | YES -- anchor matched once, and the script asserted the original expression is absent afterwards |
| fail output | exit 101, `page_extension_action_trigger_is_page_action` **FAILED** (it would have PASSED before the fix) |
| restore | sha256 MATCH |

This is the CLAUDE.md rule biting in the direction it was written for: a test that passes is
evidence about the test, not the code, until you break the thing and watch it fail.

### A9 passed under the break, and that is correct — but it is a stated LIMIT

A9 asserts a property of the CONSTANTS (`ROOT_KIND_VALUES` minus `DERIVABLE_KINDS`), not of the
classifier's behaviour, so deleting the insertion cannot move it. It guards a different failure:
someone declaring a thirteenth kind and never emitting it. A8 is what guards the classifier
ceasing to emit one, and A8 DID fail.

This is recorded rather than glossed because a reader could otherwise take A9's green run under
the break as evidence the proof was weak. The two rows are complementary, and neither alone
covers both directions. CLAUDE.md's rule applies exactly here: a discrimination proof that PASSES
is evidence about the TEST, not the code — in this case it told us truthfully what A9 is for.

## Findings register summary

18 entries, all `fixed` or `refuted`, every one `accepted` by BOTH reviewers. None `open`; no
blocking entry left `deferred`.

| severity | count | ids |
|---|---|---|
| critical | 3 | F1, F2, F5 |
| important | 9 | F3, F4, F8, F9, F10, N1, N2, N3, N4 |
| minor | 6 | F6, F7, F11, N5, N6, N7 |

| raised by | count | ids |
|---|---|---|
| astra | 12 | F1, F3, F4, F6, F7, N1, N2, N3, N4, N5, N6, N7 |
| flash | 5 | F2, F5, F8, F9, F11 |
| me | 1 | F10 |

### What the two reviewers were actually worth, measured rather than asserted

This is recorded because "the panel approved it" is not evidence, and on this issue the two
reviewers' value differed sharply and in a way that reversed between rounds.

- **Round 1, flash was wrong about the load-bearing fact** and rated it `critical` with
  "confidence certain" (F2). Had I counted votes instead of reading `anchor_from_origin`, the
  issue would have shipped the ORIGINAL design -- documentation asserting a non-existent
  information loss, plus a needless cross-layer follow-up issue.
- **Round 2, flash accepted all eleven entries**, including three claims that were false: the
  blast radius (wrong file, missing `.cbor.gz`), the overlay warning behaviour, and "no gaps
  identified". astra re-raised four entries and found all six of those defects.
- **Round 3, after being shown its three wrong CONFIRMEDs**, flash's verification changed
  character -- it decoded the four serializer test names and quoted the oracle's line 188 rather
  than affirming. It agreed with astra, having actually checked.

The generalisable part: a reviewer's CONFIRMED is worth nothing unless it names the line it read.
Both reviewers were useful, but only after their output was checked rather than counted -- and
the one finding neither caught (F10, four action wrappers rather than one) came from reading the
grammar myself.


### Convergence, stated precisely

27 entries, all `fixed` or `refuted`, all marked accepted by both reviewers. Two honest caveats,
because "the panel accepted it" should mean something checkable:

1. **The final F4 correction is UNREVIEWED by either reviewer.** astra re-raised F4 in the last
   round as an explicitly NON-BLOCKING record imperfection -- the A8 proof cell still carried a
   rationale the source comment had already shed -- and said to land it as a stated limit. I fixed
   it instead, after the last commit either reviewer saw and after the final round cap was spent.
   It is a one-line record correction, it makes the ledger agree with the code rather than
   disagree, and nobody has checked it but me.
2. **Flash's round-3 acceptance arrived after I had already marked the register converged.** It
   then confirmed all five at `6069e657` with file:line citations, so the state is accurate -- but
   the marking preceded the evidence by a few minutes rather than following it.

Neither changes a disposition. Both are recorded because the alternative is a register that looks
uniformly verified when two cells are not.

## Golden triage -- A12

Triaged BEFORE any regen, which is the whole point: the regen is licensed by the triage, never
the other way round. Done twice independently -- once by me at byte level against the committed
side, once by a dedicated triager that reconstructed the NEW side.

### What moved

All nine `check-goldens` targets ran; script exit 101, so the failure is not swallowed. Exactly
one target failed -- `--test cli`, 227 passed / 4 failed -- and the four are precisely
`cli_b_snapshot_differential::{raw_json,envelope_json,cbor,cbor_gz}_matches_goldens`, each naming
`ws-sibling-member-triggers`. `check-goldens --verify-coverage` also passes (44 golden dirs
declared), so the changed path is genuinely covered by the gate rather than merely un-failing.

Two green targets carry real information and are named rather than lumped into "everything else
passed":

- **`--test r4` includes `r4f_root_classifications::r4f_root_classifications_match_goldens`,
  and it PASSED.** The R4-F projection copies `rc.kinds` directly, so this was the family most
  likely to move besides the snapshot. It does not, because its fixture list does not include
  this fixture.
- **`--test l2_ir` (70 passed) did not move either**, though
  `tests/ir-l2-goldens/l2_features.snapshot` is one of only three files repo-wide that mention
  this fixture. CLAUDE.md records that family as the one that goes stale when someone regenerates
  only what they were looking at. It stayed green because this change is above L2.

### Every changed line, classified

**Two** classification entries change -- the two `OnAction` routines of page 50920
(`…#38bc8c9b92b2aa8c…` and `…#d1d921fd1b581c59…`). The same two appear in all four
serializations, so it is 2 entries x 4 files, not 8 distinct changes. Every delta reconciles to
exact arithmetic, which is what turns "looks additive" into "is additive":

| file | delta | change | class | arithmetic |
|---|---|---|---|---|
| `.raw.json` | +46 B | `"trigger-page"` -> `"trigger-page",` + a new `"page-action"` line, x2 | content-shaped | 2 x 23 (`,` + newline + 8 spaces + `"page-action"`) |
| `.envelope.json` | +50 B | same two entries, two levels deeper indent | content-shaped | 2 x 25 |
| `.cbor` | +24 B | `kinds` array header `0x81` -> `0x82`, one `0x6b` + `page-action` appended, x2 | content-shaped | 2 x 12 |
| `.cbor.gz` | +7 B | none of its own | derived | proven `gzip(.cbor)` on both sides; first diff at offset 12, the deflate stream |

| class | count |
|---|---|
| content-shaped | 2 entries (x4 serializations) |
| **id-shaped** | **0** |
| **unexplained** | **0** |

### The CBOR proves a NEGATIVE about every id in the tree

A line diff only shows what a serializer chose to print. The CBOR is a whole-tree byte
comparison, decoded structurally rather than eyeballed:

- Across all 20 top-level keys the decoded structural diff is **exactly four differences**, every
  one of the form "`kinds` grew from 1 to 2, gaining `page-action` at index 1". Key order
  preserved everywhere.
- In the committed bytes, `0x6c` + `trigger-page` occurs **exactly twice** and `0x6b` +
  `page-action` **zero** times -- checked directly against the file, not inferred. So the only
  places that string exists ARE those two entries, and nothing else in the snapshot could be
  affected by appending to them. After: `page-action` 0 -> 2, `trigger-page` 2 -> 2.
- Every other byte is identical -- every routine id, object id, signature fingerprint and
  `workspaceFingerprint`.

### The mask trap, checked rather than assumed

A hex mask is the obvious way to compare "ids" and it is exactly how an identity change hides.
The preceding-byte histogram is identical on both sides (45 hex runs each, identical value
multiset), and it shows why a mask would have been wrong here:

| length | preceded by | count | what it is |
|---|---|---|---|
| 64 | `#` | 10 | `{stableObjectId}#{hash}` -- id-positioned |
| 64 | `/` | 2 | id-positioned |
| 64 | `"` | **5** | **NOT id-positioned**: 3x `signatureFingerprint`, 1x `workspaceFingerprint`, 1x `contentHash` |

Those five are precisely the population a bare hex mask would have swallowed, and they are
byte-identical. The triage did not rely on masking at all; it compared full values.

### The probe was validated before its output was trusted

The reconstructed "new" bytes come from a throwaway crate outside the repo that replays the
test's own `compose_for` / `tree_for` verbatim, with the version override and the 34-detector
diagnostics list extracted programmatically from the test file rather than retyped. It was run
over **all 21** `SNAPSHOT_CORPUS` fixtures.

**80 of its 84 outputs are byte-identical to the committed goldens.** That does two jobs at once:
it proves the probe reproduces the test's computation, and it independently establishes that
exactly one fixture moved -- without leaning on the suite's panic-on-first-mismatch behaviour,
which can only ever name the first failure it hits. `.cbor.gz` files were confirmed to decompress
to exactly their `.cbor` sibling on both sides.

Without that validation step the whole triage would rest on an unvalidated instrument, which is
the failure this repo has hit before: an unasserted probe's output reads exactly like a verified
one.

### Four files moved -- but EIGHT fixture directories changed behaviour

"Exactly four files move" is true of FILES and would be badly misleading as a summary of the
behaviour change. The triage found, and I verified, that the classification also moves for four
more fixtures:

`ws-policy-api-ui`, `ws-policy-api-dynamic-dispatch`, `ws-policy-api-isolated-storage` and
`ws-policy-api-ledger-write` -- each present in BOTH `tests/r0-corpus/` and
`tests/fixtures/cli-c-policy/`, so **8 directories**. Every one is a `PageType = API` page with an
`action(...)` carrying `trigger OnAction()`, so each goes
`["trigger-page", "api-page"]` -> `["trigger-page", "page-action", "api-page"]`.

**No golden family projects root classifications for any of them.** That is consistent with the
independent evidence already in this ledger -- a repo-wide `grep trigger-page tests/` returns
only the `ws-sibling-member-triggers` goldens and the fingerprint oracle's vocabulary string -- so
four of the five affected fixture NAMES change behaviour with no byte-compared golden watching.
Nothing is wrong, but the record should not let a future reader infer from "four files moved"
that two routines were affected.

### Why the consumers stay green -- verified, not assumed, and it turns on the ADDITIVE choice

Two consumers read root kinds, and both were checked at the call site rather than by reputation:

- `d50.rs:107-117`: `for kind in &rc.kinds { if D50_UNTRUSTED_ROOT_KINDS.contains(kind) { return false } }`
- `ordering_engine.rs:105-108`: `!s.kinds.iter().any(|k| is_untrusted_root_kind(k))`

Both are ANY-quantified, and both lists (`d50.rs:34-44`, `ordering_engine.rs:86-99`) contain
`trigger-page` and do NOT contain `page-action`. Since the rule is additive, every routine that
gains `page-action` still carries `trigger-page`, which already made it untrusted. Adding a kind
absent from both lists therefore cannot change either result.

**That inertness is a CONSEQUENCE of choosing additive, not a property of the change.** Under the
exclusive reading the issue's wording also permitted, an action trigger on a non-API page would
carry `page-action` ALONE -- a kind in neither list -- and `is_trusted_commit_root` would flip
from false to TRUE, with d50's Cap 2 ceasing to reject it. `ws-sibling-member-triggers` is exactly
that shape (`PageType = Card`), so the flip would have been real, not hypothetical, and no golden
would have shown it because no golden projects d50 output for that fixture.

The design chose additive on consumer-compatibility grounds and cited `fingerprint_query`. This
is a second, stronger reason found only by checking the other consumers: the additive choice
protected two detectors from a silent behaviour change.

The policy engine is inert for the SHIPPED policies, and the reason is narrower than an earlier
revision of this paragraph claimed. `root.kinds` is a `FieldValueShape::EnumList`, which
`predicate_compiler.rs` compiles to `PredicateOperator::In` (any-overlap), and no rule in
`policy-default.yaml` or the `ws-policy-custom` fixture names `page-action`.

**"An added kind can only add matches" is FALSE as a general statement** (astra, final panel,
finding 1). It holds for a positive applicability predicate. It does NOT hold for an `except:`
clause: a user policy carrying `except: {root.kinds: [page-action]}` can newly become TRUE and
SUPPRESS findings that previously reported. For arbitrary user policies this change can therefore
move findings in BOTH directions. The honest claim is the narrow one -- the shipped policies are
unaffected because none names the kind.

The same correction applies to the Report request-page note in `is_page_action_wrapper`'s doc:
the cost is not only that `alsem fingerprint --roots page-action` misses them, but that ANY
`page-action`-based selection does, policy applicability and exceptions included.


**Verdict: regen licensed.** Every changed byte is content-shaped and accounted for; zero
unexplained lines; zero identity movement.

## Base / Head SHAs

| | |
|---|---|
| B (`origin/master`) | `df42d1e4d94f146f045d4edf88964602f257992e` |
| H (`HEAD`) | `6069e6575f3156289a64f677110a59a39706592d` |

`master` did not move between claim and freeze, so the rebase in step 11 is a no-op and no
re-gate was triggered by it.

## Gate results

All four green on H. Toolchain: the repo's pinned Rust; grammar submodule at the committed
`tree-sitter-al` pointer; `CDO_WS` = the pinned baseline worktree, detached at `bc3ccb18` with a
clean tree.

| gate | exit | seconds | evidence |
|---|---|---|---|
| `ci-steps all` | **0** | 723.8 | `f3-ci-steps-all.log` -- fmt, clippy, gen-syntax, test, build, perf-bounds all ran; 23 `test result: ok`; no FAILED marker |
| `check-goldens --verify-coverage` | **0** | 0.6 | 44 golden dirs declared |
| `check-goldens` | **0** | 399.9 | full nine-target run |
| `cdo-gate` | **0** | 62.4 | `f2-cdo-gate.log` read end to end: `ENFORCE_CDO_WS=1` against the pinned baseline, `program_resolve_harness` 190 passed, `program_graph`+`snapshot_robustness` 2, `l4_summary_differential` 20, `--lib` **1788 passed / 0 failed**, ending `cdo-gate: PASS` |

### Why the gate logs are named `f2-`/`f3-`, and a failure that was mine

This is recorded because the run numbering otherwise looks like a cover-up.

Two earlier gate runs were killed mid-flight, deliberately: each was gating a HEAD that panel
fixes were about to supersede, and letting them finish would have burned ~30 minutes validating
code that no longer existed. The kill was the right call; its consequences were not anticipated.

1. **The killed runs' children survived and kept writing.** The supervisor derives each log and
   result path from `--name`, so two live runs shared them. The resulting summary interleaved:
   a `ci-steps-all` header wrapped a JSON naming `check-goldens.log`, a `cdo-gate` header wrapped
   one naming `ci-steps-all.log`, and `grep` reported the log as binary. One file said
   `exit_code: 1` and there was no way to tell a real failure from a kill artifact. Those results
   were DISCARDED rather than interpreted.
2. **The fix was structural, not more force.** Re-running with unique `--name` values (`f2-`)
   made collision impossible. Two rounds of killing stray `cargo`/`rustc` had already failed to
   converge -- the count went UP -- and with 106 `python` processes on the machine and no clean
   attribution, continuing would have risked another session's work for no gain.
3. **`f2-ci-steps-all` then failed for real, and the cause was also mine:**
   `rust-lld: error: failed to write output 'program_resolve_harness-...exe': permission denied`.
   A test binary from a killed run was still executing and holding its own `.exe` open. Root-caused
   by reading the linker error, not guessed; the process had exited by the time I looked, and the
   `f3-` re-run passed clean.

`f2-cdo-gate`'s 62.4s against an earlier 389.6s looked like a short-circuit and was checked
rather than accepted: it ran the complete suite, warm binaries account for the difference. A gate
number is not a gate result until someone reads what produced it.

## Discoveries

```json
[
  {
    "subsystem": "al-syntax",
    "locator": "crates/al-syntax/src/lower/mod.rs:628-659,711-719",
    "symptom": "A page extension's action-extension trigger has no enclosing member at all. `modify(SomeExistingAction) { trigger OnAfterAction() }` lowers with `enclosing_member: None`, because the wrapper node `modify_action_modification` carries `field('target', ...)` and no `name` field (grammar.js:2488-2496), so the lowerer's generic member gate (`child.field(FieldName::Name).is_some()`) never captures it, and the lowerer's special case at :628-637 covers `RawKind::ModifyModification` only. The consequence found while implementing issue #11 is that such a trigger cannot be classified `page-action`, but the loss is broader than that one kind: the member NAME is missing too, so `L3Routine.enclosing_member` and `enclosing_member_range` are both `None` for every action-extension trigger.",
    "kind": "gap",
    "origin_issue": 11,
    "reproducer": "A pageextension whose `actions` section contains `modify(SomeAction) { trigger OnAfterAction() begin end; }`. Assert the lowered routine's `enclosing_member` is `Some`; it is `None`.",
    "pre_existing": true,
    "capability": "Action-extension triggers carry their enclosing member, so they classify and identify like any other member trigger.",
    "acceptance": "Two halves, and a fifth string in the classifier fixes NEITHER: (a) the lowerer captures `modify_action_modification` as a member wrapper, and (b) its name is extracted from the `target` field, as `ModifyModification` already is. Note the blast radius before starting: enclosing-member names participate in stable routine identity (l3_workspace.rs:1211-1235), so giving these routines a member name CHANGES their stable ids and will move goldens. That identity consequence is the reason this was scoped out of #11 rather than folded into it."
  },
  {
    "subsystem": "l5",
    "locator": "tests/r0-corpus/ws-policy-api-{ui,dynamic-dispatch,isolated-storage,ledger-write}",
    "symptom": "Root classifications for the four ws-policy-api-* fixtures are projected by NO byte-compared golden, so a change to how those routines classify is invisible to the whole golden suite. Found while measuring issue #11's blast radius: that change moves these four fixtures' OnAction triggers from [\"trigger-page\",\"api-page\"] to [\"trigger-page\",\"page-action\",\"api-page\"] in BOTH tests/r0-corpus/ and tests/fixtures/cli-c-policy/ (8 directories), and every golden stayed green. A repo-wide `grep -rl trigger-page tests/` returns only the ws-sibling-member-triggers serializations and one vocabulary string in cli_b_fingerprint_oracles.rs, confirming a single fixture carries the entire golden witness for root classification.",
    "kind": "coverage-gap",
    "origin_issue": 11,
    "reproducer": "Change kinds_for so any Page trigger gains an extra kind, then run scripts/check-goldens. Only ws-sibling-member-triggers fails; the four ws-policy-api-* fixtures change classification silently.",
    "pre_existing": true,
    "capability": "A change to root classification is witnessed by a golden for more than one fixture shape -- in particular for an API page, which is the shape four corpus fixtures use and the one whose kinds list is longest.",
    "acceptance": "Either add one ws-policy-api-* fixture to cli_b_snapshot_differential.rs's SNAPSHOT_CORPUS (it already has 21 entries; this is a one-line addition plus seeded goldens), or add an r4f rootclass golden covering an API page. Prefer whichever costs less golden surface. NOTE: no seed file is needed for this family, despite CLAUDE.md's general rule that the regen env var REWRITES but cannot MINT. cli_b_snapshot_differential's check_or_regen creates the parent directory and WRITES before any read, and the CBOR/gzip branches do the same, so regeneration mints these goldens. An earlier revision of this discovery imported the r4 family's restriction and would have sent the follow-up author to manufacture placeholder files for no reason (astra, final panel, finding 8)."
  }
]
```

TWO discoveries. The first came from issue #11's spec panel (astra, round 2, finding N4); the
second from measuring this change's real reach during the golden triage. Both are genuine
PRE-EXISTING gaps, neither a consequence of #11's change -- #11 merely made them visible.

**Evidence strength, stated precisely:** this is a STATIC trace — the grammar rule, the lowerer's
member gate and its `ModifyModification` special case were each read directly, and the inherited
member is `None` because action sections carry no `name` either. No parse of such a pageextension
was executed, and no test was run against one. Whoever picks this up should reproduce it first;
the acceptance above is written so that a failed reproduction closes the discovery honestly
rather than sending someone to build a fix for a gap that is not there.
