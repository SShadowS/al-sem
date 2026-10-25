# Issue #299 — d37 and d11 still fire on `Rec` in table procedures, where the caller owns the record

## Claim

Run `20261025-161234-1fe6d0`, attempt 1. Branch `issue/299-agent-discovery-l5-detectors-d3-a1`,
worktree `/work/al-sem-issue-299-a1`, base `ce5ddc3d`.
Issue body hash: `20f6227ed932aaf198f5415fd5a1ac61844b5fa9bdcf228b9c579a6d47f842da`.

## Classification

**Bounded.** One receiver flag on the record-variable row, set at the one place
`ir_walk` seeds the implicit `Rec`; two predicates and one extracted helper in
`src/engine/l5/detectors/mod.rs`; one new skip in each of `d11.rs` and `d37.rs`. No
serialized schema changes (the flag is `#[serde(skip)]`), and no golden finding moves. The
population is measured below and is one false-positive class. Revision 2 (after spec
round 1) replaces the revision-1 temp-state predicate, which `record_types` breaks (S1).

## Assumption probes

Data source: base `ce5ddc3d`, release-fast, measured on 2026-10-25 (see "Base
re-measure"). Its d11/d37 fingerprints are identical to #272's final-head outputs
(`/work/bench/272/final/`, head `91f62e0c`), which the first reading used. Extracts and
scripts: `/tmp/claude-1000/-work-alsem/e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c/scratchpad/299/`.
"Table `Rec`" below means the finding's receiver name is `Rec` (from `rootCause`) and the
object is a Table or TableExtension. Kind (procedure / trigger) comes from the nearest
`procedure`/`trigger` header above the finding line.

| probe | result | evidence |
|---|---|---|
| What d11 flags | `Modify` or `Validate` on a record variable with no earlier load op (`RECORD_LOAD_OPS`), no loading call (G-10/G-16a) and no assignment from a loaded var (G-16b). Skips every **parameter** record (`is_parameter`, by-var and by-value alike) and the G-9 trigger `Rec` (`is_platform_loaded_trigger_rec`: page trigger list; table field `OnValidate`; table `OnInsert/OnModify/OnDelete/OnRename`). Nothing else exempts `Rec` | `d11.rs:61-112`, `mod.rs:341-367` |
| What d37 flags | `Validate` on a record variable when the next op on it in source order is a reset-like op (Init/Reset/Get/Find*/Next/Copy/TransferFields) or none, not Modify/Insert, and no later call hands it to a helper that may persist. Skips temp records, every **parameter** record, and the G-9 trigger `Rec` | `d37.rs:68-152`, `:161-185` |
| How the engine models a table method's `Rec` | `ir_record_variables` seeds a `Rec` row for a Table/TableExtension routine (procedure AND trigger) with `temp_state = parameter-dependent(RECEIVER_PARAM_INDEX)`, unless a parameter, local or global of ANY type named `Rec` exists (`shadows_implicit_rec`). A named return value `Rec` is NOT checked there | `ir_walk.rs:2160-2187`, `:169-182`, `features.rs:250-256` |
| **S1: does the receiver's PD temp state survive?** | **No.** `record_types.rs` rewrites it to `Known(true)` in two passes: `TableType = Temporary` (`:384-401`, ops and variables) and the routine entry guard `if not X.IsTemporary then Error` (`:412-427`). So a temp-state predicate misses those receivers. Measured: the prototype (flag-based) removes **14** Base App d11 rows in 7 `TableType = Temporary` tables (`PaymentReportingArgument`, `VATAmountLine`, `AccountingPeriodBuffer`, `PurchInvLineAggregate`, `SalesInvoiceLineAggregate`, `ItemFromPictureBuffer`, `PriceLineFilters`) that the revision-1 predicate would have kept. Scratch fixture `TempModify` on a temporary table shows the same | `record_types.rs:369-427`, `scratchpad/299/gate/ba.tempfiles`, `runs-local/proto/alsem-*-owned.rows` |
| **S2/S4/OP6: which declared `Rec` shapes compile in a table?** | All three compile with alc 18.0.41 against BC 28.4 symbols, with no diagnostics: a local `Rec: Record "S Cust"`, a local `Rec: RecordRef`, and a named return `procedure NamedReturnRec() Rec: Record "S Cust"`. The first two get no seeded row (`shadows_implicit_rec`); the third DOES get one (return names are not checked), which is why the flag also excludes a named return `Rec` | `runs-local/proto/alc-shadow-S.al`, `alc-shadow-compile.log` |
| d11 / d37 on table `Rec`, **Base App** (single-app, scope all) | d11: **1220** of 2321 (Table 1199, TableExt 21). Procedures 1069, table field `OnLookup` 151, no other trigger. Ops: Validate 890, Modify 330. d37: **850** of 1400 (Table 829, TableExt 21). Procedures 701, table field `OnLookup` 149 | `ba-d11d37.jsonl`, `tabrec-kind.tsv` |
| Same, **CDO primary** (default mode) | d11: **97** of 112. d37: **9** of 12. (On `Rec` outside tables: d11 Page 6, d37 Page 1) | `runs-local/cdo-primary.json` |
| Same, **CDO all** (default mode) | d11: **480** of 729 (Table 473, TableExt 7). d37: **132** of 163 | `runs-local/cdo-all.json` |
| d37 table-`Rec` findings with a reset-like op on `Rec` after the `Validate` | **0 of 850** on Base App (textual scan to the routine end, `d37-reset-scan.tsv`). The prototype's model-based check (helper reloads included) keeps 0 of 850 too | scratchpad, `runs-local/proto/gate/` |
| Triage, d11, Base App table procedures | **18 / 18 FP** (every 60th): ValidateShortcutDimCode (Assembly Header, Customer), AssignNewOpportunity, CopyFromAttachment, UpdateApplyToAmount, GetVendorBalAccount, UpdateReasonCode, SetAnalysisReportName, SetCostApplication, ShowDocDim, CreateOrUpdateSelectedElement, UpdateRemainingQuantityFromUse, CalcInvDiscToInvoice, SetShipToCustomerAddressFieldsFromShipToAddr (Sales, Service), ValidateReturnReasonCode, UserExists, InitQtyToShip. Each is a method called on a record the caller holds (`SalesLine.InitQtyToShip()`, `Cust.ValidateShortcutDimCode(...)`, or from a field `OnValidate`); the entry state is the caller's responsibility. A routine may still load through another receiver method (CreateOrUpdateSelectedElement calls `Rec.CreateOrReadForCurrentUser`), so the claim is ownership, not proof that the caller loaded | `s-d11.tsv`, source |
| Triage, d37, Base App table procedures | **18 / 18 FP** (every 40th): SetDefaultLocation, CreateInteractionFromInteractLogEntry, GetFAVATSetup, GetVendorAccount, InitFromCRMIntegrationRecord, UpdateCurrencyCode, InitRevalJnlLine, UpdateExpectedQuantity, CopyFrom, UpdateQtyPickedForOptionalWhsePick, CopyFromVATPostingSetup, UpdateVendorItemNoFromItemReference, UpdateBillToCust, RestoreLookupSelection, ValidateVATProdPostingGroup, SetCustomerFromFilter, CalcInvDiscToInvoice (Service), InitItemFields. All set fields on the caller's record; the caller (or the platform after the field trigger that called it) inserts or modifies it | `s-d37.tsv`, source |
| Triage, table field `OnLookup` (d11 + d37) | **6 / 6 FP**: AssemblyHeader `Bin Code`, PriceListLine `Unit of Measure Code` / `Source No.`, SalesHeader `Bill-to Contact No.` (x2), InternalMovementHeader `To Bin Code` | `s-lookup.tsv` |
| Triage, CDO primary | d37: **9 / 9 table FP** (ReportNameSearch, TableNameSearch, FieldNameSearch: page-field helpers; 3 table `OnLookup`; `NewRecord` x3, called from page `OnNewRecord`). d11: **13 / 13 FP** (one per distinct routine, every 3rd): all methods on a caller-held record | `cdo-prim-rec.tsv`, source |
| Is the owner claim right **for d11** (who is responsible for the entry state)? | Yes, for every Table/TableExtension routine: a procedure runs on the caller's instance (`Cust.Foo()`), a trigger on the record the platform or caller fired it for. This differs from d39's question: d39 excludes tableext OnAfterInsert/OnAfterModify/OnAfterRename because the **write** is lost there, but `Rec` is still the platform's record there | source, `mod.rs:396-441` |
| Is it right **for d37** (is the Validate persisted)? | It is d39's question, so d39's Table arm answers it: every Table/TableExtension procedure and trigger except tableext OnAfterInsert/OnAfterModify/OnAfterRename (measured lost on BC 28.4, #272). A reload of `Rec` after the Validate loses the write whoever owns `Rec`, so the skip excludes it (Design) | `mod.rs:431-441`, `d39.rs:193-205` |
| BC behaviour: table field `OnLookup` | **Persisted, measured** on the BC 28.4 Linux sidecar (`http://bc`, instance BC, TestPage; app `P299` 1.0.0.1, source `scratchpad/299/bcprobe/src/Probe.al`, codeunit 71110; published, run, uninstalled with data deleted, unpublished, 2026-10-25). A TABLE field `OnLookup` doing only `Validate(Name, 'TLOOKUP')`, no Modify, no page trigger. Readings: `card-default editable=Yes inPage=TLOOKUP afterLookup=ORIG afterClose=TLOOKUP`; `card-openedit editable=Yes ... afterClose=TLOOKUP`; `rocard-openedit` (`Editable = false`) `editable=No ... afterClose=TLOOKUP`. The page writes the row at close, even on a read-only page under TestPage. **Windows Cronus confirmation owed** (no Cronus is reachable from this container; same status as #272's sidecar rows). Table-method `Rec` = caller's instance: standard AL, not re-probed | probe output above |
| Tests that pin today's firing on table `Rec` | `tests/cli/cli_implicit_rec_bindings.rs::readers_see_the_implicit_rec` (fixture `tests/fixtures/issue-272/ws-readers`, table `R Cust`) asserts 9 d11 + 4 d37 rows on `Rec`. The prototype run on that fixture keeps exactly the d21/d33 rows: `d21 D21WrongVar L93`, `d33 D33ByValue L108`, `d33 D33Unfiltered L113`. `tests/r3/r3a2_implicit_rec_pin.rs` pins `ws-readers`' R3a-2 summary core byte-for-byte with no regen path, so `ws-readers` must not change (OP1) | `runs-local/proto/` (prototype run), file :262-306 |
| Goldens that hold a d11/d37 finding on table `Rec` | None (grep over `tests/` for `calls (Modify\|Validate) on Rec`: only a d29 row). Stats goldens list a skip key only when its count is > 0 (`registry.rs:171-175`) | grep |
| Out of scope, measured on the way | (a) d21 (d11's read sibling): **952** Base App findings on table `Rec`, same class, same predicate. (b) d37 on page `Rec`: Base App 87 (36 page procedures, 34 `OnNewRecord`, 15 `OnAfterLookup`); sample 12 / 12 FP. (c) d11 on page `Rec`: Base App 208 (Page 207, PageExt 1; OnNewRecord 35, OnAfterLookup 15, OnOpenPage 8, OnQueryClosePage 5, the rest procedures), CDO primary 6, CDO all 10 (OP3). (d) d11 on `Rec` in a codeunit-`TableNo` `OnRun`: Base App 32, CDO all 2, sample 3 / 3 FP. This is the issue's Location line `ir_walk.rs (codeunit-TableNo Rec bound local)`: that `Rec` row is seeded `Known(false)` with a direct table and is not the receiver of a table method, so it needs its own rule, and the acceptance names table procedures only (OP2). (e) `is_platform_loaded_trigger_rec` matches by name only, so a declared `Rec` in a G-9 trigger is wrongly exempt | scratchpad, `runs-local/ba.json` |

## Design

### The rule

`Rec` in a Table or TableExtension routine is the **receiver**: the record the caller
called the method on, or the record the platform or caller fired the trigger for. The
engine already treats it as a reserved `var` parameter for temp state. d11 and d37
already skip parameter records; they skip the receiver too, each for its own question:

- **d11 (who is responsible for the entry state):** skip a `Modify`/`Validate` on the
  receiver in every Table/TableExtension routine. The caller (or platform) owns the
  record's state on entry, as for a parameter record. This is ownership, not proof that
  someone loaded it.
- **d37 (who persists it):** skip a `Validate` on the receiver when d39's Table owner
  rule says the owner persists it (every routine except tableext
  OnAfterInsert/OnAfterModify/OnAfterRename) **and** the routine does not put the
  receiver in a new state after the Validate. "New state" is:
  - d39's shared definition, `record_load_points(routine, ctx)["rec"]`: own
    Get/Find*/Next (`loadsFromDb`), Init (`initialises`), Copy/TransferFields
    (`copiesInto`), and every call that passes `Rec` by `var` to a helper whose role
    `puts_in_loaded_state` (L4-composed, so deeper helpers count). A point in the
    other arm of a branch holding the Validate is excluded (`branch_exclusive`), as in
    d39's `reset_after_call`;
  - a **bare whole-record copy** `Rec := Other` after the Validate: a `var_assignments`
    row with `lhs_name == "rec"` AND `rhs_identifier` set (both sides are bare
    identifiers). A member target `X.Rec := v` has `lhs_name == "rec"` too but never
    sets `rhs_identifier`, so it does not count (S14). Assignments are not leaves of
    `statement_tree`, so `branch_exclusive` cannot apply to them: an assignment in the
    other arm still counts, which only KEEPS a finding (conservative; OP12). Dropping
    the arm instead would hide the real loss `Validate; Rec := Other`, so the arm stays.
  - `Reset` is **not** a new state (it clears filters, keeps field values;
    `record_flow_role` = `resetsFilter`), so `Validate(F); Reset();` is skipped (OP5).

### Where it lives

1. **Receiver identity (S1, S2, S4).** New field `is_receiver: bool` on
   `program::body::features::PRecordVariable` (`#[serde(skip)]`) and on
   `program::model::workspace::ModelRecordVariable`, forwarded in `workspace.rs` where
   the model rows are built. Only `ir_record_variables` sets it to `true`, on the
   seeded `Rec` row, and only when the object is Table/TableExtension and the routine
   has no named return value `Rec`. All other rows (parameters, locals, report
   dataitems, promoted globals, ABI rows, test literals) carry `false`. This fails
   closed:
   - A declared `Rec` of any type (record, RecordRef, scalar) gets no seeded row
     (`shadows_implicit_rec`), so nothing carries the flag.
   - A named return `Rec` gets the seeded row but no flag.
   - A deserialized row reads `false`.
   - `record_types` never touches the flag, so temporary tables and entry guards keep
     it (S1).
2. **`mod.rs`:**
   - `is_table_receiver_rec(routine, name)`: `name` is `Rec`, and some row of the
     routine named `Rec` has `is_receiver`. The object-kind decision lives in `ir_walk`
     only (OP9: no redundant checks).
   - `table_rec_persisted_by_owner(routine)`: the Table arm of
     `implicit_rec_persisted_by_owner`, extracted. That function calls it, so d39 is
     unchanged.
   - `receiver_reset_after(routine, ctx, anchor, op_id)`: the "new state" check above.
3. **`d11.rs`:** the receiver skip is the **last** check before emitting (after the
   load-op, loading-call and assignment checks). Stats key `callerOwnedRec`.
4. **`d37.rs`:** the receiver skip is the **last** check before emitting (after the
   walk and `post_validate_helper_verdict`). Stats key `callerOwnedRec`.
5. **Counter placement (S7, OP4).** Because both skips come last, `callerOwnedRec`
   counts only would-be findings. No existing counter moves; `findingsEmitted` drops by
   the same number. Measured on all three runs (Base App d11 `callerOwnedRec` 1220,
   d37 850; CDO d11 480, d37 132; every other key identical).
6. **Docs (S11, OP7).**
   - The module docs of d11 and d37 state the receiver rule.
   - They say the parameter skip covers every parameter record, by-var and by-value,
     which is what the code does.
   - d37's opening line no longer lists `ModifyAll` as a persist.
   - d37's doc says that the G-9 trigger skip comes before the reset check.
   - `is_platform_loaded_trigger_rec` and `implicit_rec_persisted_by_owner` point to the
     new helpers.
7. **The serde-skipped flag (S12, S13).**
   - `PRecordVariable` loses its derived `PartialEq` and gets a manual one over the
     serialized fields only (`is_receiver` excluded), as `PVarAssignment` does
     (`features.rs:527-539`). Otherwise a freshly walked table receiver compares
     unequal to its deserialized baseline vector.
   - `features.rs`'s INTERNAL-ONLY list gains `PRecordVariable.is_receiver` (in file
     order, after `PLoop.exhausting_advance`), and `serde_skip_list_is_complete`
     asserts 10.
   - New lib test `receiver_flag_is_not_serialized_and_not_compared`: a table
     receiver row round-trips through JSON with no flag key, reads back `false`, and
     compares equal. Discrimination proof, already run on the prototype: adding
     `is_receiver` to the manual `eq` fails it (`assertion left == right failed`,
     `runs-local/proto/proof-eq-break.log`); reverted, all 3 features tests pass
     (`proof-eq-revert.log`).

Prototype of exactly this (uncommitted, in the worktree), revision 3:
`/work/al-sem-issue-299-a1/.agent/runs-local/proto/proto-r3.diff` (sha256
`bd1419ece38a7ff19417efb5bbb5579eb3117eff8d662b30edd70f53dd591bfe`). Revision 2's
diff (`proto.diff`, sha256 `2000948967ac…`) is the one the three heavy runs measured;
r3 differs only by items 7 and the narrowed assignment arm.

### Tests

**New fixture `tests/fixtures/issue-299/ws-owned/`** (not `tests/r0-corpus`, which would
demand goldens). Its content is the dry-run fixture
`/work/al-sem-issue-299-a1/.agent/runs-local/proto/ws-owned/src/O.al`. **New test file
`tests/cli/cli_caller_owned_rec.rs`**, added as `mod cli_caller_owned_rec;` in
`tests/cli/main.rs` (OP10). It drives `run_analyze_with_exit` as
`cli_implicit_rec_bindings.rs` does, and asserts the EXACT
`(detector | object | routine | line)` set for
`d11-modify-without-get,d37-validate-without-persist`.

Preconditions, all hand-stated in the fixture:

| routine | shape | expected |
|---|---|---|
| table `O Cust` `ProcModify` | bare `Modify()`, no load | no finding |
| `ProcValidate` | `Validate`, nothing after | no finding |
| field `Name` `OnLookup` | `Validate` | no finding |
| `ProcValidateThenReset` | `Validate; Reset()` | no finding (OP5) |
| `ProcValidateThenNext` | `Validate; Next()` | **d37** (own reload) |
| `ProcValidateThenHelperLoad` | `Validate; H.Loads(Rec)` (var helper that `Get`s) | **d37** (S3) |
| `ProcValidateThenAssign` | `Validate; Rec := Other` | **d37** (whole-record replacement) |
| `ProcShadow` | local `Rec: Record "O Cust"`, explicit `Rec.Validate` / `Rec.Modify` | **d11** x2 (Validate and Modify); no d37, because `Modify` follows (OP6) |
| `ProcNamedReturn` | `procedure ... Rec: Record "O Cust"`, `Rec.Validate` | **d11** and **d37** (fail closed, S2) |
| `ProcLocal` | local `Other` record | **d11** x2 (control); no d37, `Modify` follows |
| `ProcShadowRecRef` | local `Rec: RecordRef`, `Rec.Open(...)`, `Rec.Modify()` | **d11** (no seeded row; not the receiver; fires on base too) (S15) |
| `ProcValidateOppositeBranchAssign(B)` | `if B then Validate(...) else Rec := Other` | **d37** kept: the assignment in the other arm still counts (conservative, OP12) |
| table `O Named` (has a field named `Rec`) `MemberRecAssign` | `Validate(Name, 'x'); Other.Rec := 'v'` | no finding: a member target is not a receiver reset (S14). Revision 2 kept d37 here |
| table `O Temp` (`TableType = Temporary`) `TempModify` | receiver `Modify()` | no finding (S1) |
| tableext `ExtProc` | `Validate` | no finding |
| tableext `OnAfterModify` | `Validate` | **d37** only (after-write trigger) |
| page `O Card` | page copies of #272's reader shapes (D37Persists / NoPersist / ByValue / Unresolved / Arity / WrongVar / Order, D11Loaded / ByValue / Order), explicit `Rec.` calls | the same rows #272 asserts on its table today: d37 NoPersist / ByValue / WrongVar / Order; d11 on every D37* routine and on D11ByValue / D11Order |
| codeunit `O H` `LocalControl` | local record | **d11** x2 (control) |

The dry run gives exactly the column above. The r3 prototype is a dev build with
`CARGO_BUILD_JOBS=4`. The outputs:
- `runs-local/proto/alsem-base-owned.rows`: 44 rows (base binary);
- `alsem-proto-owned.rows`: 28 rows (revision 2);
- `alsem-proto3-owned.rows`: 27 rows (revision 3).

r2 and r3 differ only by `d37 | O Named | MemberRecAssign | L96`, which r3 correctly
drops. A field named `Rec` and the opposite-branch shape compile with alc
(`runs-local/proto/alc-shadow-S.al`, `compile2.log`). The test pins the 27 rows of the
r3 file.

Discrimination proofs, each break made and reverted with Edit, both outcomes recorded,
and each asserted to have applied (the edited text occurs once):

1. `is_table_receiver_rec` returns `false`: the "no finding" rows appear, so the test
   fails.
2. `ir_walk` sets `is_receiver: true` regardless of a named return: the
   `ProcNamedReturn` rows vanish.
3. The predicate reads the PD temp state instead of the flag (the revision-1
   predicate): the `TempModify` row appears.
4. Drop `table_rec_persisted_by_owner` from d37: the `OnAfterModify` d37 row vanishes.
5. Drop the `record_load_points` part of `receiver_reset_after`: the
   `ProcValidateThenHelperLoad` and `ProcValidateThenNext` rows vanish.
6. Drop the assignment part: the `ProcValidateThenAssign` and
   `ProcValidateOppositeBranchAssign` rows vanish.
7. Drop the `rhs_identifier.is_some()` condition: the `MemberRecAssign` d37 row
   appears. The r2-vs-r3 dry run above is this break's outcome, already observed.

`ProcShadow` is not a discriminating guard for the flag. It has no seeded row today,
so no break of the flag reaches it; the test doc says so.

**Existing test (OP1):**
- `tests/fixtures/issue-272/ws-readers` and the r3a2 pin stay byte-unchanged.
- `readers_see_the_implicit_rec` is updated to the post-change rows, measured with the
  prototype: `d21 | R Cust | D21WrongVar | L93`, `d33 | R Cust | D33ByValue | L108`,
  `d33 | R Cust | D33Unfiltered | L113`.
- Its doc says why the 9 d11 and 4 d37 rows left: the table receiver rule, #299. It
  points to the page copies in `ws-owned` that now carry the d37 helper-walk and d11
  helper-load coverage on an implicit `Rec`.
- This second fixture also pins the new skip.

### Expected moves (exact fingerprint gate, S5 / OP8)

Measured with the prototype binary on the three base runs (Base re-measure).

**Gate.** For each run:
- the set of removed `(detector, fingerprint)` pairs **equals** the base set of d11/d37
  findings on table `Rec`;
- **0** findings are added;
- no other detector moves.

| run | removed (d11 + d37) | = table-`Rec` base set | added | after: d11 / d37 |
|---|---|---|---|---|
| Base App single-app, scope all | 2070 (1220 + 850) | yes, 0 extra, 0 kept | 0 | 1101 / 550 |
| CDO primary | 106 (97 + 9) | yes | 0 | 15 / 3 |
| CDO all | 612 (480 + 132) | yes | 0 | 249 / 31 |

The sets are in `/work/al-sem-issue-299-a1/.agent/runs-local/proto/gate/`
(`{ba,cdo-primary,cdo-all}.{removed,added,tabrec}`). No table-`Rec` row is kept on
these workspaces: none is a shadowed or named-return `Rec`, a tableext after-write
trigger, or a Validate followed by a receiver reload.

The implementation must reproduce this gate on its final head, byte-exact on the removed
sets and with 0 added. Any difference is a stop: triage every differing fingerprint
before going on. CDO primary moves, and it is triaged above (acceptance).

**Goldens:**
- No d11/d37 finding golden moves.
- Stats goldens gain only a `callerOwnedRec` key, and only where a fixture has such a
  row, with `findingsEmitted` dropping by the same number. No other key may move.
- Regen all eight targets with `scripts/check-goldens --regen`, check that rule, then
  run `scripts/check-goldens`.

**CHANGELOG:** one `Changed` entry with the measured numbers. It also states that the
table field `OnLookup` persistence was measured on the BC 28.4 Linux sidecar and that a
Windows Cronus confirmation is owed.

### Retained findings (still reported; these are real losses or out of scope)

- tableext OnAfterInsert/OnAfterModify/OnAfterRename `Validate` on `Rec` (d37):
  measured lost (#272).
- A receiver `Validate` followed by its own reload, a `var` helper reload, or `Rec := X`
  (d37): the write is lost.
- `Rec` that is a declared variable or a named return value: not the receiver.
- A receiver `Validate` in one arm of a branch whose other arm does `Rec := Other`
  (d37): kept, because assignments carry no `statement_tree` leaf for
  `branch_exclusive`. This is a conservative false positive. It was 0 rows on the three
  workspaces, because the r2 gate kept no table-`Rec` row (OP12).
- **Out of scope, filed as discovery issues with the evidence above:**
  - d21 on table `Rec` (952 Base App);
  - d37 on page `Rec` (87);
  - d11 on page `Rec` (208);
  - d11 on codeunit-`TableNo` `OnRun` `Rec` (32);
  - the name-only G-9 check.

### Known false negatives (real problems now missed)

- **Caller never loaded the record.** The caller calls a table method on a record it
  never loaded (`Cust.ProcModify()` on a blank `Cust`). d11 used to fire inside the
  method, in the wrong place; now nothing fires. d11 has the same blind spot for
  parameter records today.
- **Caller never persists.** The caller calls `Cust.SetX()` (Validate only) and never
  persists `Cust`. A method call on a local is not an argument binding, so d39 does not
  see it either. This is a stated limit, as for a parameter record.
- **Non-persisting context.** A table method called from a tableext OnAfterModify, or
  from another context that does not persist, and that only Validates `Rec`. The owner
  rule cannot see the call context; d39 has the same limit.
- **Reload through a receiver method.** A receiver reload through a method called on
  the receiver itself (`ReloadMe()` / `Rec.ReloadMe()`, where `ReloadMe` does
  `Get(...)`): the receiver is not an argument binding, so `record_load_points` does
  not see it, and d37 skips. The textual scan found 0 such shapes on Base App
  (receiver reloads of any kind after a Validate: 0).
- **Bare call under a shadowing local.** In a table method that declares a local `Rec`,
  a bare `Modify()` binds by name to the local in the engine, while AL runs it on the
  receiver. This mis-binding exists today and is not changed here (OP6).
- **Expression or quoted replacement.** A right-hand side or target that is not a bare
  unquoted identifier (`Rec := GetSomeRecord()`, `Rec := "Other Cust"`, `"Rec" := Other`)
  sets no `rhs_identifier`, so it does not count as a reset (OP14), and a receiver
  `Validate` before it is skipped. The model carries no "bare target" marker for an
  expression assignment; adding one is out of scope. A textual scan of the 850 Base
  App d37 table-`Rec` rows found 0 `Rec :=` of any kind after the Validate (rev 3,
  same script shape as `d37-reset-scan.tsv`).

### Fallbacks, stated in advance

1. **Gate mismatch on the final head** (removed set differs from the prototype's, or
   anything is added): triage every differing fingerprint against source. If more than
   30% of the differing removals are real, stop and go back to design. Do not ship the
   skip.
2. **Page arm (S9).** If the panel asks for one answer to "who persists `Rec`" across
   pages too, d37 calls the full `implicit_rec_persisted_by_owner`, which needs
   `SymbolTable::build`, as d39 does. The page sample (12 / 12 FP, 87 Base App rows) is
   extended to 15 or more and triaged before the change. Stated remaining exception:
   d37's G-9 skip runs first, so page triggers in `PAGE_TRIGGERS_REC_LOADED`
   (OnAction, OnLookup, OnValidate, ...) stay skipped even on a read-only page that the
   owner rule rejects. Changing that precedence is a separate design and triage.
   Default: tables only, page arm filed as a discovery.
3. **Windows Cronus disagrees on table `OnLookup` (S8).** If a Windows Cronus run shows
   that a table field `OnLookup` write is not persisted, d37 keeps reporting it: the
   d37 skip excludes `kind == "trigger" && name == OnLookup` on a Table. d11 keeps
   skipping it, because `Rec` is the page's row either way. This is tied to the owed
   Cronus confirmation recorded in the CHANGELOG.

(Revision-1 fallbacks 2 and 4 are dropped: S4, S6.)

## Base re-measure (ce5ddc3d)

Binary `/work/al-sem-issue-299-a1/target/release-fast/alsem` at `ce5ddc3d` (copy:
`scratchpad/299/alsem-base`), runs one at a time (2026-10-25), outputs in
`/work/al-sem-issue-299-a1/.agent/runs-local/{ba,cdo-primary,cdo-all}.{json,err,time}`.
Peak RSS from `getrusage(RUSAGE_CHILDREN).ru_maxrss` via `/work/bench/272/final/tm.py`
(`/usr/bin/time` is not installed in this container).

| run | command | wall s | peak RSS MB | exit |
|---|---|---|---|---|
| Base App | `analyze /work/bench/272/ws-baseapp --format json --deterministic --single-app --scope all` | 65.3 | 5923 | 0 |
| CDO primary | `analyze <CDO_WS> --format json --deterministic` | 19.4 | 3085 | 0 |
| CDO all | `analyze <CDO_WS> --format json --deterministic --scope all` | 19.9 | 3094 | 0 |

| run | d11 total | d11 table Rec | d37 total | d37 table Rec |
|---|---|---|---|---|
| Base App | 2321 | 1220 | 1400 | 850 |
| CDO primary | 112 | 97 | 12 | 9 |
| CDO all | 729 | 480 | 163 | 132 |

The d11 + d37 fingerprint sets are identical to #272's final outputs (0 added, 0
removed in each run).

## Prototype measure (revision 2, gate unchanged in revision 3)

Prototype = the uncommitted worktree diff `runs-local/proto/proto.diff` on `ce5ddc3d`,
built `cargo build --profile release-fast --bin alsem` with `CARGO_BUILD_JOBS=4` (copy:
`scratchpad/299/alsem-proto`). Same three commands, one at a time, outputs in
`/work/al-sem-issue-299-a1/.agent/runs-local/proto/`.

| run | wall s | peak RSS MB | exit | d11 | d37 |
|---|---|---|---|---|---|
| Base App | 94.8 | 5917 | 0 | 1101 | 550 |
| CDO primary | 21.0 | 3091 | 0 | 15 | 3 |
| CDO all | 21.4 | 3089 | 0 | 249 | 31 |

The fingerprint gate result is in Expected moves. Detector stats: only `callerOwnedRec`
is new; every other d11/d37 counter is identical to base.

**Wall time is not a claim (OP13).** These runs, the base runs and #272's runs share a
box with other lanes, so wall seconds are indicative only. The Base App 65 → 95 s
difference is not attributed to the change; peak RSS (5923 vs 5917 MB) is the comparable
number. The final head's re-measure records both again, and the CHANGELOG quotes neither.

**Revision 3 and the gate.** The three heavy runs were not repeated for r3. r3 changes
two things:
- the manual `PartialEq` and the skip list (no detector behaviour);
- the assignment arm, which now requires `rhs_identifier`.

The narrowed arm can only turn a kept table-`Rec` d37 row into a skipped one, never the
reverse, and it touches no other row. The r2 gate already removed every table-`Rec` row
and kept none, so the r3 removed sets are the same. The dry-run fixture confirms that
only `MemberRecAssign` moves. The final head's gate re-measure checks this.

## Implementation

Plan: `docs/superpowers/plans/2026-10-25-issue-299-caller-owned-rec.md`. Commits:
- T1 `2478d03a`: the rule, the lib test, `readers_see_the_implicit_rec` post-change rows
  (exactly the Design's 3: d21 D21WrongVar L93, d33 D33ByValue L108, d33 D33Unfiltered
  L113). `ws-readers` and the r3a2 pin are byte-unchanged.
- T2 `b274b13e`: fixture `tests/fixtures/issue-299/ws-owned` (the dry-run fixture; the
  one comment change is `LocalControl`, which said "d11 and d37 fire" and now says
  "d11 x2; no d37 (Modify follows)", as OP11 fixed for ProcShadow/ProcLocal),
  `tests/cli/cli_caller_owned_rec.rs` (27 exact rows, the r3 file), docs, CHANGELOG,
  and `FIXTURE_ROOTS` `fixtures` 43 -> 44 (the intentional new-app-dir tripwire in
  `capability_cone.rs`, read by two oracle tests).

Gates, per task: `ci-steps task-affected` green (T1 4426 passed, T2 4427 passed; whole
suite, "a change outside the map"); `check-goldens` green (T1 1263, T2 1264);
`check-diff` `reasons: []` after each commit.

**Golden move not in the Design: `tests/ir-l2-goldens/l2_features.snapshot`.** It stores
an fnv1a hash of each routine's `PFeatures` Debug text, and the new field prints there:
479 changed lines over 477 distinct routine keys, 0 added, 0 removed (1027 lines on
both sides, keys identical, hashes only; `diff` of `git show ce5ddc3d:` against the head,
F1). The test's drift report said "476 changed"; it counts through a map keyed by routine
id, and the snapshot holds repeated ids, so it under-counts. Triage: a
temporary manual `Debug` for `PRecordVariable` that omits only `is_receiver` made
`cargo test --test l2_ir ir_l2_snapshot::` pass against the unchanged snapshot; reverted,
then regenerated with `check-goldens --regen` (receipt `.agent/golden-triage.md`,
gitignored). No stats golden moved: no golden fixture has a table-`Rec` d11/d37 row.

### Proofs

Each break made and reverted with Edit; "applied" = the grep count of the edited text
(1, or 0 for a deletion of the only occurrence); reverted = `git diff --quiet HEAD --
src/`. Logs in `scratchpad/299/` (`p*.log`, `p*.rows`, `t1-*.log`).

| test | red before | break | under break | after revert |
|---|---|---|---|---|
| `receiver_flag_is_not_serialized_and_not_compared` | FAILED with `is_receiver` in `eq` (left `false`, right `true`) | add `&& self.is_receiver == other.is_receiver` to the manual `eq` (applied 1) | FAILED, 2 passed 1 failed (`t1-red.log`) | 3 passed (`t1-green.log`) |
| `cli_caller_owned_rec::table_receiver_rec_is_caller_owned` | FAILED: 44 rows, identical to the base binary's `alsem-base-owned.rows` (break 1 = base behaviour) | 1. `is_table_receiver_rec` returns `false` (`false &&` prefix, applied 1) | FAILED, 44 rows: every "no finding" row appears (`p1.log`) | passed (`p0-green.log`) |
| same | (as above) | 2. `ir_walk` flag ignores the named return (applied 1) | FAILED: only `d11` + `d37 ProcNamedReturn L61` vanish (`p2.log`) | src clean |
| same | | 3. predicate reads PD temp state (`parameter-dependent` + `RECEIVER_PARAM_INDEX`) instead of the flag (applied 1) | FAILED: `d11 O Temp TempModify L109` appears (and ProcNamedReturn's 2 rows vanish, the temp state not knowing return names) (`p3.log`) | src clean |
| same | | 4. drop `table_rec_persisted_by_owner` from d37 (count 0) | FAILED: only `d37 O Cust Ext OnAfterModify L122` vanishes (`p4.log`) | src clean |
| same | | 5. drop the `record_load_points` part of `receiver_reset_after` (applied 1) | FAILED: only `d37 ProcValidateThenHelperLoad L35` and `ProcValidateThenNext L28` vanish (`p5.log`) | src clean |
| same | | 6. drop the assignment part (count 0) | FAILED: only `d37 ProcValidateThenAssign L42` and `ProcValidateOppositeBranchAssign L82` vanish (`p6.log`) | src clean |
| same | | 7. drop `rhs_identifier.is_some()` (applied 1) | FAILED: only `d37 O Named MemberRecAssign L96` appears (`p7.log`) | passed after all reverts (`p-after.log`) |
| `readers_see_the_implicit_rec` | FAILED with the old 16 rows once the rule existed (T1's first `task-affected`) | n/a (its rows are the Design's; break 1 restores the old 16 by the same predicate) | | passed |

### Final panel round 1 (commit `38f1dab5`)

Fixes: C3 (d11 keeps the receiver when the routine replaced `Rec` itself before the op;
`bare_rec_assignments` shared with d37), C6 + C8 (`load_point_after` is the one
"reset after" check of d37 and d39, over `LoadPointCache`, which builds
`record_load_points` once per routine; d39 output byte-identical on all three runs, see
below), C9 and F3 (docs), F1 (the count above), F2 + C1 + C2 + C4 + C5 (CHANGELOG known
limits).

**C5 was not fixable inside the receiver rule.** The model does record `Clear(Rec)`, as
a call site (bare callee `Clear`, one argument text; d3's `is_clear_call_on` reads it).
But d37's `post_validate_helper_verdict` runs before the receiver rule and returns
`suppress-unknown` for any later call that takes the record through an unresolved
binding, which a builtin `Clear(Rec)` is. So `Validate; Clear(Rec)` was suppressed on
base too, for every record, and a reset arm in `receiver_reset_after` would be dead code.
I tried that arm: with it in place, the fixture's `ProcValidateThenClear` still had no
d37 row. Under break 1 (receiver rule off, `r1-p1.log`), it has a d11 row but no d37
row, which shows the helper rule hides it. The arm was removed. `ProcValidateThenClear`
is pinned as a no-row limit, and the CHANGELOG states it. Changing the helper rule
would move d37 for local records too, so it is out of scope.

**C7 refuted** (reuse the `RECEIVER_PARAM_INDEX` temp state instead of a flag): spec
finding S1. `record_types.rs:384-427` rewrites that temp state to `Known(true)` for
`TableType = Temporary` tables and `IsTemporary` entry guards. Proof 3 above shows the
temp-state predicate bringing back `TempModify`.

New fixture rows (table `O Repl`, appended so earlier lines keep their numbers):
`ProcAssignThenModify` (d11 L248), `ProcModifyThenAssign` (no row),
`ProcValidateThenClear` (no row, the C5 limit). Test: 28 rows.

| test | red before | break | under break | after revert |
|---|---|---|---|---|
| `table_receiver_rec_is_caller_owned` | FAILED: break 8 is the pre-C3 code | 8. d11's assignment check made `any(\|_\| false)` (applied 1) | FAILED: only `d11 O Repl ProcAssignThenModify L248` vanishes (`r1-p8.log`) | src clean |
| same | | 9. d11's order flipped, `before_anchor(&op.source_anchor, a)` (applied 1) | FAILED: L248 vanishes, `d11 ProcModifyThenAssign L254`, `ProcValidateThenAssign L42` and `ProcValidateOppositeBranchAssign L82` appear (`r1-p9.log`) | src clean |
| same | | 1 again, `is_table_receiver_rec` returns `false` | FAILED, 47 rows; `ProcValidateThenClear` has d11 but no d37 (the C5 evidence, `r1-p1.log`) | src clean |
| same | | 5 again, on the refactored code: `load_point_after(...) && false` (applied 1) | FAILED: only `d37 ProcValidateThenHelperLoad L35` and `ProcValidateThenNext L28` vanish (`r1-p5.log`) | src clean |
| same | | 7 again, on the shared `bare_rec_assignments`: drop `rhs_identifier.is_some()` (applied 1) | FAILED: only `d37 O Named MemberRecAssign L96` appears (`r1-p7.log`) | passed in the gate below |

Gates: `ci-steps task-affected` green (4427 passed, whole suite; fmt, clippy-dev,
gen-syntax), `check-goldens` green (1264), `check-diff` `reasons: []`.

## Final-head measure

### Round 1 head `38f1dab5`

Tree clean apart from the untracked `.agent/issue-299/`. `heavy-ok` was present. The
build, commands and method are the same as below. Outputs are in
`scratchpad/299/final-r1/`.

| run | wall s | peak RSS MB | exit | d11 | d37 | d39 before / after |
|---|---|---|---|---|---|---|
| Base App | 65.7 | 5918 | 0 | 1101 | 550 | 363 / 363 |
| CDO primary | 19.0 | 3077 | 0 | 15 | 3 | 3 / 3 |
| CDO all | 19.7 | 3099 | 0 | 249 | 31 | 3 / 3 |

Removed sets: 2070 / 106 / 612, each byte-equal to `proto/gate/{run}.removed`, with 0
added. So C3 kept no row on these workspaces, and no fingerprint of any other detector
moved, d39 included. Against base, the only detector-stat changes are d11/d37
`findingsEmitted` and `callerOwnedRec` (1220 / 850; CDO 480 / 132). Every d39 counter is
identical. The CHANGELOG numbers stand.

### First head `b274b13e`

Head `b274b13e`, tree clean apart from the untracked `.agent/issue-299/`. `heavy-ok`
present. `cargo build --profile release-fast --bin alsem`, `CARGO_BUILD_JOBS=4` (copy:
`scratchpad/299/final/alsem-final`). The three commands of "Base re-measure", one at a
time, each through `/work/bench/272/final/tm.py`; outputs in `scratchpad/299/final/`.

| run | wall s | peak RSS MB | exit | d11 | d37 |
|---|---|---|---|---|---|
| Base App | 66.5 | 5913 | 0 | 1101 | 550 |
| CDO primary | 19.8 | 3087 | 0 | 15 | 3 |
| CDO all | 20.5 | 3095 | 0 | 249 | 31 |

Gate, `(detector, fingerprint)` over ALL findings, base (`runs-local/{run}.json`) vs
final:

| run | before | after | removed | added | removed == proto `gate/{run}.removed` | removed == base table-`Rec` set `gate/{run}.tabrec` |
|---|---|---|---|---|---|---|
| Base App | 74027 | 71957 | 2070 (d11 1220, d37 850) | 0 | yes, byte-exact | yes |
| CDO primary | 2323 | 2217 | 106 (d11 97, d37 9) | 0 | yes | yes |
| CDO all | 30618 | 30006 | 612 (d11 480, d37 132) | 0 | yes | yes |

No other detector moved. Detector stats (`payload.summary.detectorStats`): the only
changes are d11/d37 `findingsEmitted` and the new `callerOwnedRec` (Base App 1220 / 850;
CDO 480 / 132), every other key identical to base. Wall seconds are indicative only
(shared box, OP13).

## Timeline

- 2026-10-25 16:12 UTC: claimed (fallback from alsem-main; #77 is alsem-1b's). Heavy probes were held twice while alsem-1b re-ran cdo-gate (the OOM incident that led to #326).
- Spec panel: r1 both DO NOT APPROVE (S1-S11, OP1-OP10); r2 sol DO NOT APPROVE (S12-S15), Opus APPROVE (OP11-OP13); r3 both APPROVE (OP14). 3 of 5 rounds.
- Implementation: T1 2478d03a, T2 b274b13e; task gate green; draft PR #331.
- Final panel: r1 sol ship (no findings), Opus ship (F1-F3), code-review high (C1-C9) → fix 38f1dab5, final-head gate re-run exact; r2 both ship (Opus OF1, OF2) → 868e6974 (CHANGELOG only); r3 both ship. Register: 43 entries, all accepted by both.
- Opus r3 nit, not re-raised: d11 also judges `Validate`, so "holds the `Modify`" in the OF1 sentence is slightly narrow. Left as is, to keep the gated head.
- Gates on 868e6974 all green, but master moved (#279 09854085, #326 0794b35a). Rebased onto origin/master 09854085 with no conflict; subjects unchanged.
- Gates on H = 9f65abc6 (B = 09854085): check-goldens-coverage 0, ci-steps-all 0, check-goldens 0, cdo-gate 0 (gate-runs.jsonl).
- Re-measure on H (release-fast, three runs one at a time): every finding set is identical to the pre-rebase head 38f1dab5's. Base App d11/d37/d39 1101/550/363, CDO primary 15/3/3, CDO all 249/31/3; the CHANGELOG numbers stand.
- CI on 01ed4100 failed for every PR: tree-sitter-al main moved past c89c2ecb (build.rs node-types hash mismatch). This lane made the grammar bump as side PR #333 (940ba71b; no test outcome, golden or CDO number moved), merged as 7374c420; then #334 (CLAUDE.md only) landed as ba6561be.
- Rebased onto 7374c420 (CHANGELOG conflict: kept both entries), gates all 0 on e696bcc2, and the re-measure gave finding sets identical to 38f1dab5 on all three workspaces. Rebased again onto ba6561be (CLAUDE.md only): gates on H = 54876421 (B = ba6561be) all 0.
