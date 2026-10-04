//! `cargo run -p xtask -- gen-syntax [--check]`
//!
//! Generates the al-syntax *raw layer* from the pinned tree-sitter-al
//! `node-types.json`:
//! - `raw_kind.rs` — `RawKind` (exhaustive over NAMED kinds + `Error`), `try_from_raw`
//!   (`None` on unknown), `from_raw` (panics on unknown), `as_str`, `ALL`,
//!   `GRAMMAR_NODE_TYPES_HASH`, `NAMED_KIND_COUNT`.
//! - `field.rs` — `FieldName` (exhaustive over field names), `as_raw`.
//! - `nodes.rs` — a typed wrapper struct per NAMED kind (`RawProcedure<'t>`) with
//!   `cast`/`node` + typed field accessors, plus deduplicated union enums for
//!   multi-type fields. Shape safety only; no AL semantics.
//! - `node-types.sha256` — sidecar the al-syntax build.rs hash-guards against.
//!
//! Output is checked in and never hand-edited. The drift guard CI actually runs
//! is `scripts/ci-steps gen-syntax`, which regenerates and `git diff
//! --exit-code`s the result.
//!
//! `--check` regenerates into memory and diffs against the committed files, and
//! is CURRENTLY BROKEN: it compares the RAW generator output against files the
//! write path has since run through `rustfmt`, so any construct rustfmt reflows
//! reports as false drift on a pristine tree (today: an over-100-column match
//! arm in `raw_kind.rs`, and `pub use` lists rustfmt sorts in `nodes.rs`/
//! `mod.rs`). Nothing depends on it. Fixing it means formatting the in-memory
//! content before the comparison, or deleting the flag.
//!
//! Design (owned-syntax-IR spec §3.2; reviewer-confirmed): only NAMED kinds become
//! variants/structs; single-type field → typed accessor; multi NAMED-type field →
//! deduplicated union enum (preserves compile-time wrapper-insertion safety); any
//! anonymous member type → `RawNode` fallback (cannot be typed).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sha2::{Digest, Sha256};

/// Multi-type fields with <= this many NAMED members get a typed union enum; larger
/// sets (the "any expression"/"any statement" unions, ~30 members) fall back to
/// `RawNode` — the lowerer dispatches those via `.kind()` anyway, and a 30-member
/// enum name is unusable. (Reviewer guidance: union enums only where materially useful.)
const MAX_UNION_MEMBERS: usize = 5;

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode != "gen-syntax" {
        eprintln!("usage: cargo run -p xtask -- gen-syntax [--check]");
        return ExitCode::FAILURE;
    }

    let ws = workspace_root();
    // Mirrors al-syntax/build.rs: TREE_SITTER_AL_PATH overrides the default
    // workspace-relative grammar clone. xtask used to ignore this var even
    // though every other tool that touches the grammar honours it — the clone
    // is gitignored, so a git worktree has none, and this is the ONLY way to point
    // gen-syntax at an already-checked-out grammar (e.g. the main checkout's).
    let grammar_dir = match std::env::var_os("TREE_SITTER_AL_PATH") {
        Some(p) => PathBuf::from(p),
        None => ws.join("tree-sitter-al"),
    };
    let node_types = grammar_dir.join("src/node-types.json");
    let out_dir = ws.join("crates/al-syntax/src/raw/generated");

    if !node_types.exists() {
        let via_env = std::env::var_os("TREE_SITTER_AL_PATH").is_some();
        eprintln!("xtask gen-syntax: {} not found.", node_types.display());
        eprintln!();
        if via_env {
            eprintln!("TREE_SITTER_AL_PATH is set but does not point at a checked-out");
            eprintln!("tree-sitter-al/ (expected `<TREE_SITTER_AL_PATH>/src/node-types.json`).");
            eprintln!("Fix the path and try again.");
        } else {
            eprintln!("This usually means gen-syntax is running from a git WORKTREE:");
            eprintln!("the tree-sitter-al/ clone is gitignored, so a worktree has none.");
            eprintln!("Run it in the MAIN checkout instead, or set TREE_SITTER_AL_PATH to an");
            eprintln!("already-checked-out grammar (e.g. the main checkout's tree-sitter-al/).");
            eprintln!("Note gen-syntax always WRITES generated files into THIS checkout's");
            eprintln!("crates/al-syntax/, regardless of where TREE_SITTER_AL_PATH points.");
        }
        return ExitCode::FAILURE;
    }

    let bytes = match std::fs::read(&node_types) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {}: {e}", node_types.display());
            return ExitCode::FAILURE;
        }
    };
    let hash = hex(&Sha256::digest(&bytes));
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("node-types.json parse");
    let entries = json.as_array().expect("node-types.json is an array");

    let model = Model::from_node_types(entries, &hash);
    let files: Vec<(String, String)> = vec![
        ("raw_kind.rs".to_string(), model.gen_raw_kind()),
        ("field.rs".to_string(), model.gen_field()),
        ("nodes.rs".to_string(), model.gen_nodes()),
        ("mod.rs".to_string(), model.gen_mod()),
        ("node-types.sha256".to_string(), format!("{hash}\n")),
    ];

    if check {
        let mut drift = false;
        for (name, content) in &files {
            let path = out_dir.join(name);
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if normalize(&on_disk) != normalize(content) {
                eprintln!(
                    "DRIFT: {} is stale — run `cargo run -p xtask -- gen-syntax`",
                    path.display()
                );
                drift = true;
            }
        }
        return if drift {
            ExitCode::FAILURE
        } else {
            println!(
                "gen-syntax --check: up to date ({} kinds, {} fields, {} structs, {} unions, hash {})",
                model.kinds.len(),
                model.fields.len(),
                model.nodes.len(),
                model.unions.len(),
                &hash[..12]
            );
            ExitCode::SUCCESS
        };
    }

    // Generate and format into a staging directory, then replace only the files whose
    // content changed. Rewriting an unchanged file still bumps its mtime, and cargo
    // fingerprints sources by mtime — so an unconditional write made every
    // `scripts/ci-steps` run (which calls gen-syntax before the tests) recompile
    // `al-syntax`, `al-sem` and every test executable from scratch.
    std::fs::create_dir_all(&out_dir).expect("create generated dir");
    let staging = out_dir.join(".gen-staging");
    std::fs::create_dir_all(&staging).expect("create staging dir");
    for (name, content) in &files {
        let path = staging.join(name);
        std::fs::write(&path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    // Format the generated Rust with rustfmt so the checked-in output is CANONICAL
    // and STABLE across regenerations: a developer's `cargo fmt` produces the same
    // bytes gen-syntax does, so there is no fmt/gen-syntax ping-pong. (Mirrors how
    // rust-analyzer formats its ungrammar-generated syntax nodes.) `node-types.sha256`
    // is not Rust, so it is excluded. The edition comes from the workspace
    // `rustfmt.toml` (single source of truth, found by walking up from the staging
    // dir) — NOT hardcoded here.
    let rs_paths: Vec<_> = files
        .iter()
        .filter(|(name, _)| name.ends_with(".rs"))
        .map(|(name, _)| staging.join(name))
        .collect();
    let status = std::process::Command::new("rustfmt")
        .args(&rs_paths)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "failed to run rustfmt on the generated files \
                 (install with `rustup component add rustfmt`): {e}"
            )
        });
    if !status.success() {
        panic!("rustfmt failed on the generated files (exit {status})");
    }
    println!("formatted {} generated Rust file(s)", rs_paths.len());

    // CR-insensitive, like `--check`: a Windows checkout may hold CRLF copies of
    // byte-identical content, and rewriting those would also bump the mtime.
    for (name, _) in &files {
        let (staged, path) = (staging.join(name), out_dir.join(name));
        let new = std::fs::read_to_string(&staged)
            .unwrap_or_else(|e| panic!("read {}: {e}", staged.display()));
        let old = std::fs::read_to_string(&path).ok();
        if old.as_deref().map(normalize) == Some(normalize(&new)) {
            println!("unchanged {}", path.display());
        } else {
            std::fs::write(&path, &new).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            println!("wrote {}", path.display());
        }
    }
    std::fs::remove_dir_all(&staging).expect("remove staging dir");

    println!(
        "gen-syntax: {} named kinds, {} fields, {} typed structs, {} union enums, hash {}",
        model.kinds.len(),
        model.fields.len(),
        model.nodes.len(),
        model.unions.len(),
        hash
    );
    ExitCode::SUCCESS
}

/// One member of a field's type set: (raw grammar type, is-named).
type Member = (String, bool);

struct FieldDef {
    raw: String,
    multiple: bool,
    members: Vec<Member>,
}

struct NodeDef {
    pascal: String,
    fields: Vec<FieldDef>,
}

struct Model {
    /// (raw type, Rust variant ident) for every NAMED ident-safe kind, sorted.
    kinds: Vec<(String, String)>,
    /// (raw field name, Rust variant ident), sorted.
    fields: Vec<(String, String)>,
    /// Typed node defs (named ident-safe nodes), sorted by raw.
    nodes: Vec<NodeDef>,
    /// Deduplicated union enums: sorted named-member raw types -> enum ident.
    unions: BTreeMap<Vec<String>, String>,
    hash: String,
}

impl Model {
    fn from_node_types(entries: &[serde_json::Value], hash: &str) -> Model {
        let mut kinds: Vec<(String, String)> = Vec::new();
        let mut field_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut nodes: Vec<(String, NodeDef)> = Vec::new();
        let mut unions: BTreeMap<Vec<String>, String> = BTreeMap::new();

        for e in entries {
            let named = e.get("named").and_then(|v| v.as_bool()).unwrap_or(false);
            let ty = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(fields) = e.get("fields").and_then(|v| v.as_object()) {
                for fname in fields.keys() {
                    field_set.insert(fname.clone());
                }
            }
            if !(named && is_ident_safe(ty)) {
                continue;
            }
            kinds.push((ty.to_string(), pascal(ty)));

            // Capture typed field defs for this node.
            let mut fdefs: Vec<FieldDef> = Vec::new();
            if let Some(fields) = e.get("fields").and_then(|v| v.as_object()) {
                for (fname, fval) in fields {
                    let multiple = fval
                        .get("multiple")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let members: Vec<Member> = fval
                        .get("types")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|t| {
                                    (
                                        t.get("type")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        t.get("named").and_then(|v| v.as_bool()).unwrap_or(false),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // Register a union enum for a small all-typed multi-member field.
                    if members.len() > 1 && members.iter().all(is_typed_member) {
                        let key = typed_key(&members);
                        if key.len() > 1 && key.len() <= MAX_UNION_MEMBERS {
                            unions
                                .entry(key.clone())
                                .or_insert_with(|| union_name(&key));
                        }
                    }
                    fdefs.push(FieldDef {
                        raw: fname.clone(),
                        multiple,
                        members,
                    });
                }
            }
            fdefs.sort_by(|a, b| a.raw.cmp(&b.raw));
            nodes.push((
                ty.to_string(),
                NodeDef {
                    pascal: pascal(ty),
                    fields: fdefs,
                },
            ));
        }

        kinds.sort();
        kinds.dedup();
        assert_no_collisions(&kinds, "RawKind");
        nodes.sort_by(|a, b| a.0.cmp(&b.0));
        let nodes: Vec<NodeDef> = nodes.into_iter().map(|(_, n)| n).collect();

        let fields: Vec<(String, String)> = field_set
            .into_iter()
            .map(|f| {
                let p = pascal(&f);
                (f, p)
            })
            .collect();
        assert_no_collisions(&fields, "FieldName");

        Model {
            kinds,
            fields,
            nodes,
            unions,
            hash: hash.to_string(),
        }
    }

    fn gen_raw_kind(&self) -> String {
        let mut s = header("raw_kind.rs");
        s.push_str(
            "/// Every NAMED node kind in the pinned tree-sitter-al grammar, plus `Error`.\n",
        );
        s.push_str(
            "///\n/// Exhaustive: a kind absent here cannot be produced by the pinned grammar's\n",
        );
        s.push_str("/// named children, so [`RawKind::from_raw`] panics on an unknown string.\n");
        s.push_str("#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]\n#[allow(non_camel_case_types)]\npub enum RawKind {\n");
        for (_raw, var) in &self.kinds {
            s.push_str(&format!("    {var},\n"));
        }
        s.push_str(
            "    /// tree-sitter error-recovery node (`kind() == \"ERROR\"`).\n    Error,\n}\n\n",
        );

        s.push_str("impl RawKind {\n");
        s.push_str("    /// Map a tree-sitter `node.kind()` string to a `RawKind`, or `None`.\n");
        s.push_str("    ///\n    /// The FALLIBLE entry point. `None` means the string is not a named kind of\n");
        s.push_str(
            "    /// the pinned grammar — an anonymous token kind (`kind_str()` returns those\n",
        );
        s.push_str(
            "    /// too, for any node) or a kind from a different grammar. Exists so a caller\n",
        );
        s.push_str(
            "    /// that must not crash on such a string — a persistence layer, which fails to\n",
        );
        s.push_str(
            "    /// STORE rather than abort (`src/program/pack`'s Origin codec) — has a total\n",
        );
        s.push_str(
            "    /// function to call. Prefer [`Self::from_raw`] wherever a non-named kind\n",
        );
        s.push_str("    /// would be a bug.\n");
        s.push_str("    pub fn try_from_raw(s: &str) -> Option<RawKind> {\n        match s {\n");
        for (raw, var) in &self.kinds {
            s.push_str(&format!("            {raw:?} => Some(RawKind::{var}),\n"));
        }
        s.push_str("            \"ERROR\" => Some(RawKind::Error),\n");
        s.push_str("            _ => None,\n        }\n    }\n\n");

        s.push_str("    /// Map a tree-sitter `node.kind()` string to a `RawKind`.\n");
        s.push_str("    ///\n    /// LOUD on unknown: only NAMED kinds are passed here (the lowerer classifies\n");
        s.push_str(
            "    /// named children); an unknown string means the binary was built against a\n",
        );
        s.push_str("    /// different grammar than it is parsing — a bug, not data.\n");
        s.push_str("    pub fn from_raw(s: &str) -> RawKind {\n");
        s.push_str("        match Self::try_from_raw(s) {\n            Some(k) => k,\n            None => panic!(\n                \"al-syntax: unknown node kind {s:?} — grammar/binary mismatch \\\n                 (regenerate with `cargo run -p xtask -- gen-syntax`)\"\n            ),\n        }\n    }\n\n");

        s.push_str("    /// The grammar kind string (round-trips `from_raw`).\n");
        s.push_str("    pub fn as_str(self) -> &'static str {\n        match self {\n");
        for (raw, var) in &self.kinds {
            s.push_str(&format!("            RawKind::{var} => {raw:?},\n"));
        }
        s.push_str("            RawKind::Error => \"ERROR\",\n        }\n    }\n\n");

        s.push_str("    /// Every variant, in DECLARATION order, so `ALL[k as usize] == k`\n");
        s.push_str("    /// for every `k` — the enum is fieldless and carries no explicit\n");
        s.push_str("    /// discriminants, so a variant's `as usize` IS its position here.\n");
        s.push_str(
            "    ///\n    /// Enables exhaustive iteration over the vocabulary, and a total\n",
        );
        s.push_str("    /// index → kind mapping for callers that encode a kind positionally.\n");
        s.push_str("    /// POSITIONAL, therefore NOT stable across grammar revisions: a kind\n");
        s.push_str("    /// added mid-alphabet shifts every later index. Anything persisting an\n");
        s.push_str("    /// index must key on the grammar identity too (`src/program/pack`\n");
        s.push_str("    /// relies on `crates/` being inside the pack fingerprint closure).\n");
        s.push_str("    pub const ALL: [RawKind; NAMED_KIND_COUNT + 1] = [\n");
        for (_raw, var) in &self.kinds {
            s.push_str(&format!("        RawKind::{var},\n"));
        }
        s.push_str("        RawKind::Error,\n    ];\n}\n\n");

        s.push_str(&format!(
            "/// sha256 of the `node-types.json` this file was generated from. al-syntax\n/// `build.rs` asserts the checked-in grammar matches, so a silent grammar swap\n/// fails the build.\npub const GRAMMAR_NODE_TYPES_HASH: &str = {:?};\n\n",
            self.hash
        ));
        s.push_str(&format!(
            "/// Count of NAMED kinds (excludes `Error`). Sanity anchor for the coverage test.\npub const NAMED_KIND_COUNT: usize = {};\n",
            self.kinds.len()
        ));
        s
    }

    fn gen_field(&self) -> String {
        let mut s = header("field.rs");
        s.push_str("/// Every field name in the pinned tree-sitter-al grammar.\n");
        s.push_str("///\n/// A renamed/removed field drops its variant here, so every `FieldName::X`\n/// call site fails to compile (the loud path for field drift).\n");
        s.push_str("#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]\n#[allow(non_camel_case_types)]\npub enum FieldName {\n");
        for (_raw, var) in &self.fields {
            s.push_str(&format!("    {var},\n"));
        }
        s.push_str("}\n\nimpl FieldName {\n    /// The grammar field-name string (for `child_by_field_name`).\n");
        s.push_str("    pub fn as_raw(self) -> &'static str {\n        match self {\n");
        for (raw, var) in &self.fields {
            s.push_str(&format!("            FieldName::{var} => {raw:?},\n"));
        }
        s.push_str("        }\n    }\n}\n");
        s
    }

    fn gen_nodes(&self) -> String {
        let mut s = header("nodes.rs");
        s.push_str("//! Typed CST wrappers over `RawNode` — shape safety only, no AL semantics.\n");
        s.push_str(
            "//! A field whose grammar type set changes (e.g. a v-next wrapper insertion)\n",
        );
        s.push_str(
            "//! changes the accessor's return type, breaking the lowerer at `cargo check`.\n\n",
        );
        s.push_str("#![allow(dead_code)]\n\n");
        s.push_str("use super::{FieldName, RawKind};\nuse crate::raw::RawNode;\n\n");

        // Union enums first (dedup'd).
        for (members, name) in &self.unions {
            s.push_str(&format!("/// One of: {}.\n", members.join(", ")));
            s.push_str("#[derive(Copy, Clone)]\n");
            s.push_str(&format!("pub enum {name}<'t> {{\n"));
            for m in members {
                s.push_str(&format!("    {}(Raw{}<'t>),\n", pascal(m), pascal(m)));
            }
            s.push_str("}\n");
            s.push_str(&format!("impl<'t> {name}<'t> {{\n"));
            s.push_str("    #[inline]\n    pub fn cast(n: RawNode<'t>) -> Option<Self> {\n        match n.kind() {\n");
            for m in members {
                s.push_str(&format!(
                    "            RawKind::{p} => Some(Self::{p}(Raw{p}(n))),\n",
                    p = pascal(m)
                ));
            }
            s.push_str("            _ => None,\n        }\n    }\n");
            s.push_str(
                "    #[inline]\n    pub fn node(self) -> RawNode<'t> {\n        match self {\n",
            );
            for m in members {
                s.push_str(&format!(
                    "            Self::{p}(x) => x.node(),\n",
                    p = pascal(m)
                ));
            }
            s.push_str("        }\n    }\n}\n\n");
        }

        // Structs.
        for node in &self.nodes {
            let p = &node.pascal;
            s.push_str("#[derive(Copy, Clone)]\n");
            s.push_str(&format!("pub struct Raw{p}<'t>(pub(super) RawNode<'t>);\n"));
            s.push_str(&format!("impl<'t> Raw{p}<'t> {{\n"));
            s.push_str(&format!(
                "    #[inline]\n    pub fn cast(n: RawNode<'t>) -> Option<Self> {{ if n.kind() == RawKind::{p} {{ Some(Self(n)) }} else {{ None }} }}\n"
            ));
            s.push_str("    #[inline]\n    pub fn node(self) -> RawNode<'t> { self.0 }\n");
            for f in &node.fields {
                s.push_str(&self.gen_field_accessor(f));
            }
            s.push_str("}\n\n");
        }
        s
    }

    fn gen_field_accessor(&self, f: &FieldDef) -> String {
        let method = rust_method_name(&f.raw);
        let fv = pascal(&f.raw); // FieldName variant
        let all_typed = !f.members.is_empty() && f.members.iter().all(is_typed_member);
        if all_typed && f.members.len() == 1 {
            let mp = pascal(&f.members[0].0);
            if f.multiple {
                format!(
                    "    pub fn {method}(self) -> Vec<Raw{mp}<'t>> {{ self.0.children_by_field(FieldName::{fv}).into_iter().filter_map(Raw{mp}::cast).collect() }}\n"
                )
            } else {
                format!(
                    "    pub fn {method}(self) -> Option<Raw{mp}<'t>> {{ self.0.field(FieldName::{fv}).and_then(Raw{mp}::cast) }}\n"
                )
            }
        } else if all_typed && typed_key(&f.members).len() <= MAX_UNION_MEMBERS {
            // small multi typed -> union enum
            let key = typed_key(&f.members);
            let u = self
                .unions
                .get(&key)
                .cloned()
                .unwrap_or_else(|| union_name(&key));
            if f.multiple {
                format!(
                    "    pub fn {method}(self) -> Vec<{u}<'t>> {{ self.0.children_by_field(FieldName::{fv}).into_iter().filter_map({u}::cast).collect() }}\n"
                )
            } else {
                format!(
                    "    pub fn {method}(self) -> Option<{u}<'t>> {{ self.0.field(FieldName::{fv}).and_then({u}::cast) }}\n"
                )
            }
        } else {
            // anon member(s) or large type-set -> RawNode fallback
            if f.multiple {
                format!(
                    "    pub fn {method}(self) -> Vec<RawNode<'t>> {{ self.0.children_by_field(FieldName::{fv}) }}\n"
                )
            } else {
                format!(
                    "    pub fn {method}(self) -> Option<RawNode<'t>> {{ self.0.field(FieldName::{fv}) }}\n"
                )
            }
        }
    }

    fn gen_mod(&self) -> String {
        let mut s = header("mod.rs");
        s.push_str("//! Generated raw grammar vocabulary + typed nodes. Regenerate with\n//! `cargo run -p xtask -- gen-syntax`; CI catches drift by REGENERATING and\n//! diffing (`scripts/ci-steps gen-syntax`), not with `--check`.\n\n");
        s.push_str("mod field;\nmod nodes;\nmod raw_kind;\n\n");
        s.push_str("pub use field::FieldName;\npub use raw_kind::{RawKind, GRAMMAR_NODE_TYPES_HASH, NAMED_KIND_COUNT};\n");
        s.push_str("#[allow(unused_imports)]\npub use nodes::*;\n");
        s
    }
}

fn header(file: &str) -> String {
    format!(
        "// @generated by `cargo run -p xtask -- gen-syntax` from tree-sitter-al\n// node-types.json. DO NOT EDIT — edit the generator or bump the grammar instead.\n// Source file: crates/al-syntax/src/raw/generated/{file}\n\n"
    )
}

/// A field member is "typed" (gets a Raw struct) iff it is named and ident-safe.
fn is_typed_member(m: &Member) -> bool {
    m.1 && is_ident_safe(&m.0)
}

/// Sorted, deduplicated member raw types (union keying/naming).
fn typed_key(members: &[Member]) -> Vec<String> {
    let mut k: Vec<String> = members.iter().map(|m| m.0.clone()).collect();
    k.sort();
    k.dedup();
    k
}

/// Deterministic union-enum name from a sorted member-type list.
fn union_name(members: &[String]) -> String {
    members
        .iter()
        .map(|m| pascal(m))
        .collect::<Vec<_>>()
        .join("Or")
}

/// snake_case / lowercase grammar string → PascalCase Rust ident.
fn pascal(s: &str) -> String {
    let mut out = String::new();
    for part in s.split('_') {
        let mut c = part.chars();
        if let Some(first) = c.next() {
            out.extend(first.to_uppercase());
            out.push_str(c.as_str());
        }
    }
    out
}

/// Field method name; raw-escape Rust keywords (e.g. `type` -> `r#type`).
fn rust_method_name(s: &str) -> String {
    const KW: &[&str] = &[
        "as", "break", "const", "continue", "else", "enum", "extern", "false", "fn", "for", "if",
        "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
        "static", "struct", "trait", "true", "type", "unsafe", "use", "where", "while", "async",
        "await", "dyn", "box",
    ];
    if KW.contains(&s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}

fn is_ident_safe(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn assert_no_collisions(pairs: &[(String, String)], what: &str) {
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for (raw, var) in pairs {
        if let Some(prev) = seen.insert(var, raw) {
            panic!("{what} ident collision: {prev:?} and {raw:?} both → {var}");
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Strip CR for line-ending-agnostic comparison (Windows checkouts may be CRLF).
fn normalize(s: &str) -> String {
    s.replace('\r', "")
}

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|p| p.join("Cargo.toml").exists() && p.join("crates").is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest.join("../.."))
}
