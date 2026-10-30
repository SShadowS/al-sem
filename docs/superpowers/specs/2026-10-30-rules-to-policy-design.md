# From evidence to policy: pinning rules (design, 2026-10-30)

Step 7 of the intent-evidence spec (`2026-10-29-intent-evidence-design.md`, Part E,
§4.6). Steps 1–6 are on `docs/intent-evidence-spec` at `c54ef49e`; every citation
below is against that commit.

## 1. Goal

A rule the writers of a field follow can be **pinned**: written into
`al-sem.policy.yaml`, so `alsem policy check` reports every writer of that field that
does not apply it, on every later run, like the bundled rules.

1. `alsem explain <ws> --table T --field F --emit-policy` writes the majority rules of
   `T.F` as policy entries. `alsem explain <ws> --emit-policy` does the same for every
   field the workspace writes.
2. `alsem policy check <ws> --policy <file>` enforces those entries.
3. A team can also write or edit an entry by hand: the triage on Continia DO found a
   majority rule on only 6 of the 166 fields with 3 or more writers, so an
   auto-mined majority must not be the only way in.

**Terms.** A **pin** is a policy entry with a `requires:` block (2.2). A **write
fact** is one writer of one field: one routine and one table field it writes
(`RulesIndex`'s `Writer` under its `FieldWriters`, `src/engine/l5/rules_index.rs`).
A **rule key** is d65's: the rule's kind plus its condition atoms on the written
table (`src/engine/l5/detectors/d65.rs:141`, `rule_key`).

## 2. Design

### 2.1 One key, one "applied", one index

A pin uses exactly what d65 uses, never a second copy:

- **the key**: d65's `rule_key`, split into its two parts, `kind` and `atoms` (the
  sorted atoms on the table, joined with ` and `). The key text d65 prints is
  `"<kind>: <atoms>"`; a pin names the same two parts.
- **"applies"**: d65's `applied` (`d65.rs:171`): a writer applies a key when every
  write it makes of the field has a rule with that key in force, or every known caller
  of it applies one.
- **the effective rules**: the `substrate::RULES` writer index (`RulesIndex`), which
  keeps only rules that always stop (`rules_index.rs:155`). A `MayNotStop` rule is
  never applied, in d65 and in a pin alike.
- **the majority bar**: d65's `MIN_FOLLOWERS = 3` and `MIN_SHARE = 0.75`
  (`d65.rs:67-69`). d65's `majorities` (`d65.rs:201`) is split so both read one
  function: it lists every key any writer applies, with its followers; d65 keeps the
  majority keys with a writer missing; the emitter keeps every majority key, a
  unanimous one included (the most important rule to pin is the one nobody skips
  yet).

### 2.2 The entry

```yaml
version: 2
rules:
  # Guard Order.Amount: 4 of 5 writer(s) apply it (for example Guard Writers::FollowAssign, Guard Writers::FollowValidate).
  # Without it: Guard Writers::BypassAmount.
  - id: pin-guard-order-amount-testfield-851b4754
    title: "Writers of Guard Order.Amount apply: TestField: status <> ::open"
    description: "rejects when O.Status <> O.Status::Open (TestField)"
    severity: medium
    when:
      write.table: "Guard Order"
      write.field: "Amount"
    requires:
      rule.kind: "TestField"
      rule.atoms: "status <> ::open"
```

- **`when`** selects write facts, as it selects capability facts in a fact rule. It
  may use the routine fields (`routine.name`, `object.name`, …) and the new write
  fields. `except` works as today: a routine-only `except` exempts a whole routine,
  which is how a team records a deliberate exception by hand.
- **`requires`** (new) is a predicate over one applied rule key. A write fact that
  `when` selects, and `except` does not, is **satisfied** when at least one key its
  writer applies makes `requires` true. Otherwise the writer **violates** the pin.
- **`id`**: `pin-<table-field slug>-<kind>-<8 hex>`, the hex the start of the SHA-256
  of the folded table name, field name and key. It depends only on the key, never on
  the counts, so fixing a bypass and emitting again gives the same ids. The slug is
  cut so the id fits the loader's `^[a-z][a-z0-9-]{2,80}$`.
- **`description`**: d65's `rule_text` of the first follower's rule ("rejects when C
  (Kind): "message""), the text d65's finding shows.
- **`severity`**: `medium` (a d65 finding is `low`; a pinned rule is one a human
  confirmed).
- **Counts and examples** go in a YAML comment above the entry: they change when the
  code changes, the entry does not.

### 2.3 Predicate fields

New in `src/engine/gate/policy/predicate_fields.rs` (`PREDICATE_FIELDS`, `:181`), with
two new scopes in `FieldScope` (`:22`):

| Field | Scope | Shape | Value |
|---|---|---|---|
| `write.table` | write | glob | the written table's name |
| `write.field` | write | glob | the field's name, as the table declares it |
| `rule.kind` | rule | enum | `Error`, `TestField`, `FieldError`, `ConfirmError`, `ConfirmExit` |
| `rule.atoms` | rule | string | the key's atoms, exactly as d65 forms them |

`rule.atoms` is compared **exactly** (`==`, or `in` a list). That is deliberate: a
rule with one more atom fires in fewer cases, so it is a weaker rule, and matching
on "contains this atom" would count it as applied and hide a bypass. Exact text is
d65's own test. To find the text for a hand-written pin, run
`explain --table T --field F --emit-policy`: it also prints every key that any
writer applies (2.5).

`FieldEvalContext` (`:74`) gains `write` and `rule`. In applicability mode the write
and rule fields read unknown, as fact fields do today
(`predicate_evaluator.rs:220`).

### 2.4 Format version

The policy format grows, so its version goes from 1 to **2**. The loader accepts both
(`policy_loader.rs:145`, which today accepts only 1, and `:312`, which hard-codes the
1 it returns). A version 1 file cannot use `requires` or a write or rule field: the
loader rejects it with "needs policy version 2", so an older reader's
"policy version must be 1 (got 2)" and this one's agree on what a file needs. The
bundled default and the fixtures' policies stay version 1 and load unchanged, so no
existing golden moves. `policyVersion` in the `policy.check` output is the file's
own version, as today.

The loader also checks a pin's shape: `when` names at least one write field and no
capability field; `except` names no capability or rule field; `requires` names only
rule fields; a non-pin entry names no write or rule field; `facts` and
`requireCoverage` are refused on a pin (the coverage gate is about capability facts;
a pin reads writes and rules). `onUnknown` is kept: when the writer index was cut
short (`RulesIndex::truncated`), a writer with no matching key is unknown, not a
violation.

### 2.5 `alsem explain --emit-policy`

- With `--table T --field F`: that field. Unknown table or field: exit 2, the
  diagnostic on stderr, nothing on stdout (as `explain`'s own exit 2). A field with no
  majority rule is an answer: a valid policy with `rules: []`, exit 0.
- Alone: every field the primary app writes, majority rules only.
- With `--table/--field`, each key that some writer applies but that is below the bar
  is written as a **commented-out** entry with its counts, ready to be confirmed by
  removing the `#`. That is the hand-confirmed path (1.3).
- `--routine` and `--diff` with it are flag misuse (exit 3), and so is a `--format`
  other than the default: the output is always YAML. `--out` works as for `explain`.
- Declared needs: `ViewNeeds { rules, ..MODEL }`, no `card_text`, and no plan at all
  (no tree, no run): the emitter builds the index itself with
  `RulesIndex::build_where`, as `explain --table/--field` does (`explain.rs`).

### 2.6 `policy check` reads the rules only for a pin

The declared-needs rule: a policy with no pin costs exactly what it costs today.

- `PolicyDoc::has_pins()`: some rule has `requires`. The loader guarantees a write or
  rule field appears only in a pin.
- **Level 1.** `PolicyJob::demands` (`pipeline.rs:299`, today `ViewNeeds::MODEL`)
  sets `rules` only when the policy has a pin.
- **Level 2.** `Plan` gains `own: u32`, the substrate bits a command reads off its
  view itself, outside any detector run. `prepare` fills the `RULES` cell when the
  runs' union or `own` holds the bit (`session.rs:776`, fill at `:876-878`), before the
  runs, so a batch's analyze-with-d65 and policy check share one index.
  `Plan::merge` ORs `own`. `policy_plan` (`session.rs:485`) takes `pins` and sets
  `own = RULES` only then; `PolicyJob::reads` (`pipeline.rs:337`) keeps `Cell::Rules`
  only then. The new `View::rules()` panics "rules cell not filled: declare
  substrate::RULES"; filling it from a model built without rules already panics
  "rules not built: declare ViewNeeds::rules".
- A test builds the view from `PolicyJob::demands` for the bundled policy and asserts
  no routine has rules, and for a pin policy that every one has.

### 2.7 Output and exit codes

Unchanged: `policy check` exits 0 with findings (it has no fail-on gate), 1 on a load
error or under `--strict`. A pin finding is a normal policy finding
(`policy-<rule id>/<routine id>`, detector `policy-<rule id>`), with three
differences: it is anchored at the writer's first write of the field (where d65
anchors), not at the routine's declaration; `affectedTables` names the table; and
its evidence has one step per violating write ("writes T.F (assign) without a rule
that matches requires"). Root cause: the entry's `message`, else "R writes T.F
without the rule this policy pins: <description>". JSON, SARIF and human output
are unchanged in shape. `policy explain` prints a `Requires:` block for a pin (and
"Coverage gate: none (a pin)").

A d65 inline suppression (`// al-sem-ignore d65-guard-bypass`) is d65's, not the
policy's: a policy records an exception with `except`.

## 3. Out of scope

- Writers outside the primary app (d65's blind spot, intent spec §4.5): a pin on a
  Base Application field checks only the workspace's own writers.
- Partial keys: "applies at least these atoms" or "a stronger rule counts". Exact
  keys only, as d65.
- Pinning a rule of a routine that is not a field writer (a pin is about writes).
- Emitting a pin from a single rule of one routine (`explain --routine`): the key is
  per field.
- Merging a new emission into an existing policy file: the emitter writes a new
  document; a team copies the entries it confirms.
- Literal table names in `write.table`: the field is a glob, like the policy's other
  name fields, so a table whose name contains `*` or `?` matches more tables than
  itself. The emitter writes the name as it is. No table in the Continia DO
  workspace uses either character (checked 2026-10-30); a pin on such a table should
  be checked by hand.

## 4. Tests

- `explain --emit-policy` goldens in `tests/cli-explain-goldens/` (the `cli`
  umbrella, already in `scripts/check-goldens`): `ws-d65` whole workspace, `ws-d65`
  `Ref` (no majority, one commented candidate), and the new
  `tests/fixtures/policy-pins/bypass` fixture. The last one is also the policy the
  check tests read.
- `policy check` goldens in `tests/cli-c-policy-goldens/`: the pins on `bypass`
  (the plain bypass and the `MayNotStop` writer are found) and on `fixed` (the same
  code with the checks added: no finding).
- Round trip: on `ws-d65`, the pins `explain --emit-policy` writes, checked by
  `policy check`, flag exactly the writes d65 flags (compared by anchor, through the
  shipped binary). Emitting `fixed` gives the same rule ids as `bypass`.
- Needs: the bundled policy's view builds no rules; a pin policy's builds them.
- Loader: version 1 with a pin, a write field outside a pin, a rule field in
  `when`, `facts` on a pin: each refused with its message.
- Discrimination proofs (Edit, run, fail, revert, pass): drop the conditional
  needs; count `MayNotStop` as applied; change the emitted key away from d65's.

## 5. As built (2026-10-30)

Built as designed. Where to find it:

- d65 split: `RuleKey`, `table_key`, `applied`, `KeyCount` / `key_counts`,
  `rule_text` (`src/engine/l5/detectors/d65.rs`).
- Emitter and its job: `src/engine/l5/emit_policy.rs` (`EmitPolicyJob`,
  `emit_policy`, `pin_id`); `explain::find_field` is the table/field lookup both
  modes share; CLI `plan_emit_policy` (`src/bin/alsem.rs`).
- Fields and scopes: `predicate_fields.rs` (`FieldScope::{Write, Rule}`,
  `WriteFact`, `RuleFact`); applicability reads them unknown
  (`predicate_evaluator.rs`).
- Loader: `policy_loader.rs` (`check_pin_shape`, versions 1–2;
  `POLICY_FORMAT_VERSION` in `policy_types.rs`, with `Rule::requires` and
  `PolicyDoc::has_pins`).
- Engine: `policy_engine.rs` (`PinIndex`, `run_pin`, `emit_pin_finding`).
- Needs: `PolicyJob::needs` / `reads` (`pipeline.rs`); `Plan::own`,
  `policy_plan(.., pins)`, `View::rules` (`src/engine/session.rs`); `prepare`
  refuses an `own` bit other than `ORDERING_FACTS` / `RULES`.
- Tests: `tests/cli/cli_policy_pins.rs`. Discrimination proofs, each made with
  Edit and reverted: `needs()` with `rules: false` failed 4 tests ("rules not
  built: declare ViewNeeds::rules" and `only_a_pin_policy_builds_rules`), with
  `rules: true` failed `only_a_pin_policy_builds_rules` ("the bundled policy built
  rules"); `policy_plan` with `own: 0` failed 3 ("rules cell not filled: declare
  substrate::RULES"); keeping `MayNotStop` rules in the index failed 4 (the
  `Collect` writer no longer found; d65's own count drops to 4); emitting the
  whole key text as `rule.atoms` failed the round trip (every writer flagged) and
  2 goldens.
