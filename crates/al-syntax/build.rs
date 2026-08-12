use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Compile the tree-sitter-al grammar C into this crate. `al-syntax` is the ONLY
/// crate that links the grammar; the rest of the workspace reaches it through
/// `al_syntax::language::language()`.
fn main() {
    // Default to the repo-root submodule (build.rs CWD is this crate dir).
    let tree_sitter_al = PathBuf::from(
        std::env::var("TREE_SITTER_AL_PATH").unwrap_or_else(|_| "../../tree-sitter-al".to_string()),
    );
    let src_dir = tree_sitter_al.join("src");

    // GRAMMAR HASH GUARD: the generated raw vocabulary (RawKind/FieldName) was
    // produced from a specific node-types.json; assert the grammar being compiled
    // matches it, so a silent grammar swap/bump fails the build instead of
    // producing a vocabulary that disagrees with the parser. Sidecar is written by
    // `cargo run -p xtask -- gen-syntax`.
    let node_types = src_dir.join("node-types.json");
    let sidecar = PathBuf::from("src/raw/generated/node-types.sha256");
    if let (Ok(bytes), Ok(expected)) = (
        std::fs::read(&node_types),
        std::fs::read_to_string(&sidecar),
    ) {
        let actual = hex(&Sha256::digest(&bytes));
        let expected = expected.trim();
        if actual != expected {
            panic!(
                "al-syntax: node-types.json hash mismatch.\n  grammar:   {actual}\n  generated: {expected}\n\
                 The pinned grammar changed but the generated vocabulary did not. Regenerate:\n  \
                 cargo run -p xtask -- gen-syntax"
            );
        }
        println!("cargo:rerun-if-changed={}", node_types.display());
        println!("cargo:rerun-if-changed={}", sidecar.display());
    }

    let mut build = cc::Build::new();
    build
        .include(&src_dir)
        .file(src_dir.join("parser.c"))
        // C11: scanner.c has a file-scope _Static_assert (its depth-counter width
        // gate). Since grammar v4.0.1 the guard is `__STDC_VERSION__ >= 201112L`
        // alone, so builds work WITHOUT this flag too (a negative-array fallback
        // fires instead) — but under C11 MSVC/gcc/clang take the _Static_assert
        // branch, which prints the gate's actual MESSAGE on failure. cc maps this
        // to /std:c11 (MSVC) / -std=c11 (GNU-likes).
        .std("c11")
        .warnings(false);

    let scanner_c = src_dir.join("scanner.c");
    if scanner_c.exists() {
        build.file(scanner_c);
    }
    let scanner_cc = src_dir.join("scanner.cc");
    if scanner_cc.exists() {
        build.cpp(true).file(scanner_cc);
    }

    build.compile("tree-sitter-al");

    println!("cargo:rerun-if-changed={}", src_dir.display());
    println!("cargo:rerun-if-env-changed=TREE_SITTER_AL_PATH");
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
