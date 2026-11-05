//! The `alsem` binary's global allocator: mimalloc, and in the resident engine
//! child (`alsem engine-host`) the interior of every large block it frees is
//! handed back to the OS (context-server design §7, the per-refresh RSS creep).
//!
//! **Why.** Every refresh builds the next revision's model and artifacts (about
//! 1.8 GB on CDO) while the published one is still alive, then frees the old
//! one. The live heap stays flat (`tools/census-probe refresh-seq`: within
//! 1 MiB over 30 refreshes), but mimalloc keeps a page resident while any
//! block in it is live, and a freed block's memory stays resident inside such
//! a page: the pages fragment. On CDO the bytes mimalloc held beyond its
//! blocks in use grew from 451 MiB after the cold build to 835 MiB after
//! three B and three S refreshes (`PROBE_MI_AREAS`), most of it in the larger
//! size classes (10 KiB to 512 KiB, many of them a growing `Vec`'s abandoned
//! buffers), where a 512 KiB or 4 MiB page holds only a few blocks, so one
//! live block keeps a lot of freed memory resident. mimalloc 3 has no option
//! to release freed memory inside a live page (v2's `page_reset` is gone), and
//! a collect on every thread frees none of it (measured).
//!
//! **What.** While the release is on (only the engine child turns it on,
//! through [`set_resident`]), freeing a block of [`RELEASE_MIN`] to [`RELEASE_MAX`] bytes first
//! discards every whole OS page strictly inside it (`madvise(MADV_DONTNEED)`,
//! Linux), then frees it. The first OS page of the block is never touched: mimalloc
//! keeps its free-list link in the block's first word. A discarded page reads
//! as zeros when the block is used again, which no allocation may rely on
//! anyway (`alloc_zeroed` zeroes through mimalloc as before). A `realloc` of
//! such a block that cannot stay in place allocates the new block, copies,
//! releases and frees the old one (mimalloc would copy too). Off (every other
//! command, and every platform but Linux), this is plain `mimalloc::MiMalloc`.
//!
//! **Huge pages.** With transparent huge pages on (`always`, as on the machine
//! measured, and `khugepaged/max_ptes_none` 511, the kernel default), the
//! kernel's `khugepaged` collapses a 2 MiB range back into one huge page even
//! when most of its 4 KiB pages were released, so a release is undone over
//! time. Turning them off for the process keeps the released pages released,
//! at a cost in speed (fewer TLB hits), so only `--low-mem`, the mode that
//! trades speed for memory, does ([`set_resident`]); the default mode, which
//! the owner keeps the fastest, releases with huge pages on. Measured on CDO
//! (`front-seq`, one child, 40 B then 20 S refreshes, each with the four
//! asks; the RSS after the cold build, after the first refresh, at most over
//! the 60, and the band over the first refresh):
//!
//! | | cold | first | max | band |
//! |---|---:|---:|---:|---:|
//! | before | 4.81 GB | 5.06 GB | 5.46 GB | 0.40 GB |
//! | release (the default mode) | 4.58 GB | 4.78 GB | 5.07 GB | 0.29 GB |
//! | release, huge pages off | 4.24 GB | 4.44 GB | 4.66 GB | 0.22 GB |
//!
//! `--low-mem` (20 B then 10 S): before 3.81 / 4.03 / 4.37 GB (band 0.34),
//! after 3.50 / 3.73 / 3.95 GB (band 0.22). Every series flattens after about
//! ten refreshes of each kind. Speed (two interleaved runs of 6 B and 2 S):
//! the four asks 36.0 s before, 36.6 s with the release, 38.2 s with huge
//! pages off too. Huge pages off without the release only lower the level
//! (cold 4.42 GB) and leave the creep.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

/// The engine child's threshold: the smallest freed block whose interior is
/// discarded. Measured on CDO (`front-seq`, 6 B and 2 S refreshes, each with
/// the four asks, huge pages on): RSS after them 4.86 GB at 16 KiB, 5.00 GB at 64 KiB,
/// 5.15 GB off; 8 KiB released no more than 16 KiB but made twice the
/// calls (about 50,000 per B refresh at 16 KiB).
pub const RELEASE_MIN: usize = 16 * 1024;

/// The threshold in force; 0 = off (`usize::MAX` would be off too).
static RELEASE_FROM: AtomicUsize = AtomicUsize::new(0);
static RELEASES: AtomicU64 = AtomicU64::new(0);
static RELEASED_BYTES: AtomicU64 = AtomicU64::new(0);

/// How many freed blocks were released so far, and the bytes discarded.
#[must_use]
pub fn released() -> (u64, u64) {
    (RELEASES.load(Relaxed), RELEASED_BYTES.load(Relaxed))
}

/// Release freed blocks of at least `min` bytes (see the module doc); `0`
/// turns it off.
pub fn set_release_on_free(min: usize) {
    RELEASE_FROM.store(min, Relaxed);
}

/// The threshold in force (0: off).
#[must_use]
pub fn release_on_free() -> usize {
    RELEASE_FROM.load(Relaxed)
}

static THP_DISABLED: AtomicBool = AtomicBool::new(false);

/// The resident engine child's memory settings: release freed blocks of at
/// least `min` bytes (`0`: off) and, when `thp_off` and the release is on,
/// turn transparent huge pages off for the process
/// (`prctl(PR_SET_THP_DISABLE)`, Linux; see the module doc: with them on, the
/// kernel fills part of the released pages back in, and with them off the
/// asks are about 4 % slower than with the release alone). Returns whether
/// huge pages are now off.
pub fn set_resident(min: usize, thp_off: bool) -> bool {
    set_release_on_free(min);
    #[cfg(target_os = "linux")]
    if min != 0 && thp_off {
        // SAFETY: a plain prctl on this process; it only changes how the
        // kernel backs anonymous memory.
        let r = unsafe { libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0) };
        THP_DISABLED.store(r == 0, Relaxed);
    }
    // Off Linux there are no transparent huge pages to turn off.
    #[cfg(not(target_os = "linux"))]
    let _ = thp_off;
    thp_disabled()
}

/// Whether [`set_resident`] turned transparent huge pages off.
#[must_use]
pub fn thp_disabled() -> bool {
    THP_DISABLED.load(Relaxed)
}

/// The largest block a release covers: mimalloc's largest size class that
/// shares a page (`MI_LARGE_MAX_OBJ_SIZE`, 512 KiB in mimalloc 3). A larger
/// block has a page of its own, which mimalloc purges itself when it is
/// freed (purge delay 0).
pub const RELEASE_MAX: usize = 512 * 1024;

/// Whether a freed block of `size` bytes is released. Never off Linux: there
/// the release is a no-op, so `realloc` keeps mimalloc's own path too.
#[inline]
fn releases(size: usize) -> bool {
    let from = RELEASE_FROM.load(Relaxed);
    cfg!(target_os = "linux") && from != 0 && size >= from && size <= RELEASE_MAX
}

/// mimalloc, releasing the interior of freed large blocks when asked.
pub struct ReleasingMiMalloc;

// SAFETY: every method forwards to mimalloc's matching call; the release
// only discards the contents of pages inside a block this call owns and is
// about to free, never the block's first OS page (mimalloc's link).
unsafe impl GlobalAlloc for ReleasingMiMalloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { libmimalloc_sys::mi_malloc_aligned(l.size(), l.align()).cast() }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { libmimalloc_sys::mi_zalloc_aligned(l.size(), l.align()).cast() }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if releases(l.size()) {
            unsafe { release_interior(p) };
        }
        unsafe { libmimalloc_sys::mi_free(p.cast()) }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if !releases(l.size()) {
            return unsafe { libmimalloc_sys::mi_realloc_aligned(p.cast(), new, l.align()).cast() };
        }
        // In place exactly when mimalloc's own realloc keeps the block (the
        // new size fits its usable size and wastes at most half of it), else
        // a new block: copy, release the old one, free it.
        let usable = unsafe { libmimalloc_sys::mi_usable_size(p.cast()) };
        if new <= usable && new >= usable / 2 {
            return p;
        }
        let q: *mut u8 = unsafe { libmimalloc_sys::mi_malloc_aligned(new, l.align()).cast() };
        if q.is_null() {
            return q;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(p, q, l.size().min(new));
            release_interior(p);
            libmimalloc_sys::mi_free(p.cast());
        }
        q
    }
}

/// Discard every whole OS page strictly after the first one of the block at
/// `p` (its usable size, as mimalloc rounds it).
///
/// # Safety
/// `p` is a live mimalloc block the caller owns and frees right after.
#[cfg(target_os = "linux")]
unsafe fn release_interior(p: *mut u8) {
    let usable = unsafe { libmimalloc_sys::mi_usable_size(p.cast()) };
    if let Some((start, len)) = interior(p as usize, usable) {
        // A failure (e.g. a larger OS page size, where `start` is not
        // aligned) leaves the memory resident: the same as not asking.
        let ok =
            unsafe { libc::madvise(start as *mut libc::c_void, len, libc::MADV_DONTNEED) } == 0;
        if ok {
            RELEASES.fetch_add(1, Relaxed);
            RELEASED_BYTES.fetch_add(len as u64, Relaxed);
        }
    }
}

#[cfg(not(target_os = "linux"))]
unsafe fn release_interior(_p: *mut u8) {}

/// The 4 KiB pages a release discards for a block at `p` of `usable` bytes:
/// from the first page boundary after `p` (so the page holding the block's
/// first word is kept) to the last boundary inside it, as `(start, len)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn interior(p: usize, usable: usize) -> Option<(usize, usize)> {
    const PAGE: usize = 4096;
    let start = (p + PAGE) & !(PAGE - 1);
    let end = (p + usable) & !(PAGE - 1);
    (end > start).then(|| (start, end - start))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// How many of the `len / 4096` pages at `start` are resident.
    fn resident(start: usize, len: usize) -> usize {
        let mut v = vec![0u8; len / 4096];
        // SAFETY: `start` is page-aligned and `v` has one byte per page.
        let r = unsafe { libc::mincore(start as *mut libc::c_void, len, v.as_mut_ptr()) };
        assert_eq!(r, 0, "mincore");
        v.iter().filter(|b| **b & 1 == 1).count()
    }

    /// Allocate `l` through `a`, write every usable byte (mimalloc rounds the
    /// size up to its size class, and the interior a release discards runs to
    /// the usable end), and return it with that interior.
    unsafe fn touched(a: &ReleasingMiMalloc, l: Layout) -> (*mut u8, usize, usize) {
        let p = unsafe { a.alloc(l) };
        assert!(!p.is_null());
        let usable = unsafe { libmimalloc_sys::mi_usable_size(p.cast()) };
        unsafe { std::ptr::write_bytes(p, 0xA5, usable) };
        let (start, len) = interior(p as usize, usable).expect("a large block has an interior");
        (p, start, len)
    }

    /// The mechanism (and its switch): with the release off, a freed large
    /// block that shares its mimalloc page with a live block stays resident;
    /// with it on, every page of its interior is discarded on `dealloc`, and
    /// on a `realloc` that moves it, the contents moving with the block. A
    /// block allocated again afterwards is usable, and `alloc_zeroed` still
    /// gives zeros. One test: the switch is process-wide.
    ///
    /// Transparent huge pages are off for the test (`PR_SET_THP_DISABLE`).
    /// With them on, residency is the kernel's to decide, not the release's:
    /// a 2 MiB range can be (re)mapped as one huge page, which `mincore`
    /// reports wholly resident, and `khugepaged` collapses released ranges
    /// back (the module doc). On CI (Ubuntu 26.04, kernel 7.0 azure) the old
    /// block of the moving `realloc` read as all 79 pages resident after its
    /// release, 16 of them never written since the first huge-page fault.
    /// With 4 KiB pages only, a page is resident exactly when it was written
    /// and not discarded since, on any kernel.
    #[test]
    fn freed_large_blocks_are_released_only_when_on() {
        // SAFETY: a plain prctl on this process; it only changes how the
        // kernel backs anonymous memory.
        let thp_off = unsafe { libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0) };
        assert_eq!(thp_off, 0, "PR_SET_THP_DISABLE");
        let a = ReleasingMiMalloc;
        let l = Layout::from_size_align(256 * 1024, 16).unwrap();
        unsafe {
            // A live neighbour keeps the 4 MiB page (and its commit) alive.
            let keep = a.alloc(l);
            set_release_on_free(0);
            let (p, start, len) = touched(&a, l);
            assert_eq!(resident(start, len), len / 4096, "precondition: touched");
            let before = released();
            a.dealloc(p, l);
            assert_eq!(
                resident(start, len),
                len / 4096,
                "off: the freed block stays resident"
            );
            assert_eq!(released(), before, "off: nothing released");

            set_release_on_free(RELEASE_MIN);
            let (p, start, len) = touched(&a, l);
            assert_eq!(resident(start, len), len / 4096, "precondition: touched");
            let before = released();
            a.dealloc(p, l);
            assert_eq!(resident(start, len), 0, "on: dealloc releases the interior");
            assert_eq!(
                released(),
                (before.0 + 1, before.1 + len as u64),
                "on: dealloc counts one release of the interior"
            );

            // A realloc that moves the block: the contents move, the old
            // block's interior is released.
            let (q, start, len) = touched(&a, l);
            assert_eq!(resident(start, len), len / 4096, "precondition: touched");
            let before = released();
            let big = 1024 * 1024;
            let r = a.realloc(q, l, big);
            assert_ne!(r, q, "precondition: the block moved");
            assert!((0..l.size()).all(|i| *r.add(i) == 0xA5), "contents kept");
            assert_eq!(
                resident(start, len),
                0,
                "on: a moving realloc releases the old block"
            );
            assert_eq!(
                released(),
                (before.0 + 1, before.1 + len as u64),
                "on: the moving realloc counts one release of the old interior"
            );
            a.dealloc(r, Layout::from_size_align(big, 16).unwrap());

            // Reuse after a release: usable, and zeroed when asked.
            let z = a.alloc_zeroed(l);
            assert!((0..l.size()).all(|i| *z.add(i) == 0));
            std::ptr::write_bytes(z, 1, l.size());
            a.dealloc(z, l);
            set_release_on_free(0);
            a.dealloc(keep, l);
            // SAFETY: as above; under `cargo test` the lib tests share this
            // process, so huge pages go back on for the rest.
            libc::prctl(libc::PR_SET_THP_DISABLE, 0, 0, 0, 0);
        }
    }
}
