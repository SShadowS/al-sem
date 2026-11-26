//! TLS-117: the IR cache's data-digest slot, written into a built Windows binary and checked.
//!
//! ```text
//! alsem-stamp stamp <file.exe> [<out.exe>]   # in place, or into <out.exe>
//! alsem-stamp verify <file.exe>
//! ```
//!
//! Stamp a binary after linking and before Authenticode signing (signing changes no section).
//! An unstamped Windows binary gets no IR cache key: the cache is off for it. Runs on any system.

use al_sem::program::ir_cache::stamp::{stamp_file, verify_file};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("stamp") if (3..=4).contains(&args.len()) => {
            let input = Path::new(&args[2]);
            stamp_file(input, args.get(3).map_or(input, Path::new))
        }
        Some("verify") if args.len() == 3 => verify_file(Path::new(&args[2])),
        _ => {
            eprintln!("usage: alsem-stamp stamp <file.exe> [<out.exe>] | verify <file.exe>");
            std::process::exit(2);
        }
    };
    match r {
        Ok(line) => println!("{line}"),
        Err(e) => {
            eprintln!("alsem-stamp: {e}");
            std::process::exit(1);
        }
    }
}
