//! Print the tree-sitter-al parse tree of an AL file (or stdin) as an
//! S-expression with field names: the shape the lowerer reads. Use it to check
//! a grammar-shape claim against real output when the `tree-sitter` CLI is not
//! installed (the `grammar-upgrade` skill):
//!
//! `cargo run -q -p al-syntax --example sexp -- path/to/File.al`

use std::io::Read;

fn main() {
    let source = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path).expect("read the AL file"),
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).expect("read stdin");
            s
        }
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&al_syntax::language::language())
        .expect("load the AL grammar");
    let tree = parser.parse(&source, None).expect("parse");
    println!("{}", tree.root_node().to_sexp());
}
