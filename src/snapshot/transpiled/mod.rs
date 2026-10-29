//! Synthesized AL source for a runtime package that ships no AL: its
//! declarations from `SymbolReference.json` and the per-object metadata
//! (`bin/*.xml`), its bodies from the C# BC transpiled the AL into
//! (`bin/*.cs`). Spec: `docs/superpowers/specs/2026-10-28-transpiled-runtime-bodies-design.md`.

pub(crate) mod cs;
pub(crate) mod synth;
pub(crate) mod translate;
