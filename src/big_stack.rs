//! Shared big-stack execution for anywhere the `al_syntax` lowerer runs.
//!
//! `src/snapshot/parse.rs` documented an OBSERVED stack overflow lowering real
//! BaseApp source on rayon's default worker stack (~1 MiB on Windows) and worked
//! around it with a local 32 MiB rayon thread pool — the ONLY hardened
//! `al_syntax::parse` call site in the repo before T2.1. Every OTHER site running
//! the same recursive lowerer was unhardened: the LSP indexer (global rayon
//! pool), `didSave` (the LSP main thread, ~1 MiB on Windows), the file-watcher
//! thread, CLI `--analyze`, and the engine's sequential per-workspace parse loops
//! (`aldump` / `alsem`, also CLI-main-thread-driven). This module generalizes the
//! one working mitigation into the ONE place every such call site routes through.
//!
//! `crates/al-syntax`'s lowerer ALSO carries its own depth budget now
//! (`MAX_LOWER_DEPTH`, fails closed to a `SyntaxIssue` past ~128 nesting levels),
//! as does the engine's CFG walker (`MAX_CFG_WALK_DEPTH`) — the big stack here is
//! belt-and-suspenders defense in depth, not load-bearing.

/// Worker stack size for anywhere the `al_syntax` lowerer runs. ~32x the
/// smallest OS default thread stack this process runs on (the Windows main
/// thread, ~1 MiB), leaving generous headroom for the deepest known-real
/// nesting (Base Application source). The exact maximum required depth is
/// unmeasured; revisit if BC codebase complexity grows substantially.
pub const BIG_STACK_SIZE: usize = 32 * 1024 * 1024;

/// Run `f` on a dedicated thread with a [`BIG_STACK_SIZE`] stack and return its
/// result.
///
/// Uses a scoped thread (`std::thread::scope`), so `f` may freely borrow from
/// the caller's stack — no `'static` bound needed, and no risk of moving a
/// large owned value's eventual DROP onto the constrained thread. Use this at
/// every SEQUENTIAL (non-rayon) call site that lowers real AL source
/// (`al_syntax::parse`) on a thread whose stack size is not already guaranteed
/// generous: the LSP main thread (`didSave`), the file-watcher thread, and the
/// engine's sequential per-workspace parse loops. Rayon-PARALLEL call sites
/// should instead use [`big_stack_pool`] — spawning one big-stack OS thread PER
/// FILE here would be wasteful; wrap the whole sequential loop in ONE call here
/// instead of calling this per-file.
///
/// # Panics
/// Propagates a panic from `f` (via [`std::panic::resume_unwind`]) rather than
/// swallowing it, so callers observe the same panic behavior as calling `f()`
/// directly.
pub fn run_with_big_stack<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(BIG_STACK_SIZE)
            .spawn_scoped(scope, f)
            .expect("spawn big-stack worker thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// A local rayon thread pool with [`BIG_STACK_SIZE`] per worker, for PARALLEL
/// (`par_iter`) call sites that lower real AL source.
///
/// Building a dedicated LOCAL pool (rather than configuring the global pool)
/// avoids a footgun: rayon's global pool is lazily initialized with default
/// settings on first use from ANYWHERE in the process, and a later
/// `build_global()` call errors if that already happened — a local pool works
/// regardless of what else in the process has touched rayon.
///
/// Its size is rayon's default, so `RAYON_NUM_THREADS` sets it (#277 F18);
/// the `pool.threads` trace instant reads the built pool's own size.
pub fn big_stack_pool() -> rayon::ThreadPool {
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(BIG_STACK_SIZE)
        .build()
        .expect("build big-stack rayon pool");
    crate::engine::perf_trace::instant_lazy(
        "pool",
        "pool.threads",
        || serde_json::json!({ "threads": pool.current_num_threads() }),
    );
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_with_big_stack_returns_the_closures_value() {
        let x = 41;
        let result = run_with_big_stack(|| x + 1);
        assert_eq!(result, 42);
    }

    #[test]
    fn run_with_big_stack_can_borrow_non_static_data() {
        let owned = String::from("borrowed, not moved");
        let len = run_with_big_stack(|| owned.len());
        assert_eq!(len, owned.len());
    }

    #[test]
    #[should_panic(expected = "boom")]
    fn run_with_big_stack_propagates_panics() {
        run_with_big_stack(|| panic!("boom"));
    }

    #[test]
    fn big_stack_pool_runs_parallel_work() {
        use rayon::prelude::*;
        let pool = big_stack_pool();
        let sum: i32 = pool.install(|| (1..=100).into_par_iter().sum());
        assert_eq!(sum, 5050);
    }
}
