# Provenance

The 10 workspace fixtures under this directory are byte-for-byte copies, not
a live oracle:

- ws-policy-commit-in-subscriber
- ws-policy-commit-in-trigger
- ws-policy-api-ui
- ws-policy-api-dynamic-dispatch
- ws-policy-trigger-http
- ws-policy-install-business-write
- ws-policy-api-isolated-storage
- ws-policy-api-ledger-write
- ws-policy-custom
- ws-policy-clean

- **Source path:** `U:\Git\al-sem-OBOLETE\test\fixtures\<name>`
- **al-sem HEAD:** `cfea6149c1ed912f1a10fa45eb4a755302327c60`
- **Copy date:** 2026-07-05
- **Verification:** each file's SHA-256 matches
  `.superpowers/sdd/alsem-witness/fixture-listings/cli-c-policy-fixtures.sha256.txt`

`al-sem-OBOLETE` is a frozen, read-only archive checkout — it is never a live
oracle for this repo. This tree is vendored so `tests/cli_c_policy_differential.rs`
is self-contained and requires no sibling al-sem checkout.

## `ws-policy-commit-inherited-trigger` — NOT vendored

Added by the C1 Task 3 review fix wave (finding I-1). Authored fresh for this
repo, not copied from al-sem — every one of the 10 fixtures above matches its
rule on the routine's own **direct** fact; none exercises the **inherited**
half of a capability cone (a fact that reaches the matched root only through a
callee). `no-commit-in-triggers` (`root.kinds: trigger-table`, `capability.op:
commit`) matches here purely via `Table.al`'s `OnInsert` trigger calling
`Helper.al`'s `PostAndCommit`, which is the one that calls `Commit()` — without
`select_facts` reading the inherited half (`policy_engine.rs`'s `raw`, the
view's witness pass since #160 step 6), this fixture's golden would go from a
match to a pass.
