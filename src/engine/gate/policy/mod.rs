//! cli-c/c2 — POLICY CLI (`policy check` + `policy explain`).
//!
//! A byte-parity port of al-sem's `src/policy/*` + `src/cli/{policy,format-policy}.ts`.
//! Policy is a PURE QUERY over already-byte-parity model data (rootClassifications +
//! capabilityFacts + coverage). The modules:
//!   - [`policy_types`]     — the compiled `Predicate` AST + `Rule`/`PolicyDoc` + AST→JSON.
//!   - [`policy_loader`]    — YAML → validated `PolicyDoc` (verbatim al-sem errors).
//!   - [`predicate_compiler`] — raw YAML node → typed `Predicate` (4 kinds, operator derivation).
//!   - [`predicate_fields`] — the field registry (al-sem's 16 + the 4 pin fields of
//!     policy version 2) + per-field evaluation.
//!
//! Version 2 adds pins (`requires:`; rules-to-policy spec
//! `docs/superpowers/specs/2026-10-30-rules-to-policy-design.md`): a rule every
//! writer of a table field must apply, checked over d65's writer index.
//!   - [`predicate_evaluator`] — 4 operators + Kleene tristate (full / applicability).
//!   - [`policy_engine`]    — per rule×routine eval, coverage gate, 3 finding variants.
//!   - [`format_policy`]    — `policy.check` envelope + human/json/sarif.
//!   - [`pipeline`]         — `run_policy_check` / `run_policy_explain` CLI drivers.

pub mod format_policy;
pub mod pipeline;
pub mod policy_engine;
pub mod policy_loader;
pub mod policy_types;
pub mod predicate_compiler;
pub mod predicate_evaluator;
pub mod predicate_fields;
