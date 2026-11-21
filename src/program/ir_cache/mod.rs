//! ENG-110: a persistent cache of parsed dependency IR (spec
//! `docs/superpowers/specs/2026-11-18-eng110-ir-cache-design.md`).

pub(crate) mod codec;
pub(crate) mod envelope;
pub(crate) mod key;

/// The IR format, in every key and entry header. Bump it when the codec's bytes for an
/// unchanged IR change (the image key changes with any code change anyway).
pub(crate) const IR_FORMAT: u32 = 1;
