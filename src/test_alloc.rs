//! The lib test binary's counting allocator (context-server design r4 §11
//! T-phase-alloc). It forwards every call to `System` and, only on a thread
//! that turned counting on, adds each allocation's count and bytes to that
//! thread's counters. Other threads (every other test running beside it)
//! pay one thread-local read per allocation and are never counted, so a
//! measured region must run on one thread (a one-thread rayon pool when the
//! code under it is parallel).
//!
//! The thread-locals are `const`-initialised `Cell`s with no destructor, so
//! reading them never allocates (no lazy registration) and cannot recurse.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

pub(crate) struct Counting;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

fn note(size: usize) {
    let _ = ON.try_with(|on| {
        if on.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            let _ = BYTES.try_with(|b| b.set(b.get() + size as u64));
        }
    });
}

// SAFETY: every call forwards to `System` with the same arguments; the
// counters only observe the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // A grow counts as one allocation of its new size: what a fresh
        // buffer of that size would cost.
        note(new);
        unsafe { System.realloc(p, l, new) }
    }
}

/// The allocations `f` made on this thread: `(count, bytes)`, with `f`'s
/// result. Not reentrant.
pub(crate) fn measure<T>(f: impl FnOnce() -> T) -> (T, u64, u64) {
    COUNT.with(|c| c.set(0));
    BYTES.with(|b| b.set(0));
    ON.with(|on| on.set(true));
    let t = f();
    ON.with(|on| on.set(false));
    (t, COUNT.with(Cell::get), BYTES.with(Cell::get))
}
