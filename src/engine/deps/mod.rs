//! R2.5a `.app` symbol reader — Rust port of al-sem's dependency `.app` ingestion.
//!
//! Pipeline (symbol-only, no cache/summary layer — the Rust side does ONLY this):
//! ```text
//! .app bytes
//!   → strip_app_header (≤4096 scan for PK\x03\x04)
//!   → extract NavxManifest.xml  → parse_app_manifest_xml  (dep app identity + includesSource)
//!   → extract SymbolReference.json → parse_symbol_reference (neutral ABI DTO)
//!   → project_abi_to_index (manifest appGuid) → dependency model entities
//! ```
//!
//! The dependency app identity used for ENTITY ENCODING comes from the MANIFEST
//! `<App>` element, NOT `SymbolReference.json`'s `AppId` (R2.5a Rev 2 #2).
//!
//! Everything here is additive and isolated from the LSP binary; it must not
//! depend on or alter the LSP method surface. (The LSP has its own, simpler
//! `crate::app_package` reader — this module is the parity-faithful port and is
//! intentionally separate.)

pub mod app_manifest;
pub mod app_package_zip;
pub mod dep_artifact_l4;
pub mod projection;
pub mod r3a4_projection;
pub mod symbol_reference;
