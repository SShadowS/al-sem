//! The retained byte census (#160 spec §4.1.5): `ALSEM_RETAINED_CENSUS=1`
//! (value-tested, like `REGEN_TEMP_GOLDENS`) prints one stderr line per
//! retained structure:
//!
//! `[retained-census] bucket=<name> bytes=<n> allocs=<n> kind=exact|shallow`
//!
//! Each site walks its own structure and prints through [`bucket`]. Bytes are
//! heap bytes by `len` (allocator overhead excluded; a shared allocation once).
//! `kind=exact` means the walk reaches and counts every heap allocation of the
//! structure, each by `len` (hash-table control bytes, B-tree node overhead —
//! a B-tree is charged as one buffer of its entries — and capacity slack are
//! not modeled; see [`Tally`]). One exception: the producer-solve buckets
//! (`arena.link_graph`, `arena.working`, via `pd_solve::arena_tally` /
//! `working_tally`, and `snapshot.pd_solution`, via `pd_arena_bytes`) count
//! every `Vec`, `String` and hash table at CAPACITY with one control byte per
//! hash slot. They keep the accounting of the pre-#160 `solve_bytes` figure
//! (525 MiB on CDO, epic §3), so step 0's numbers stay comparable with it.
//! The `inputs` bucket (`InputSet::census`) walks no `io::Error` held in a
//! slot, and needs none: a census runs only after a successful build, and the
//! one error that can carry a heap payload (a failed `.al` walk, `al_walk`)
//! fails the build first (`snapshot/provider.rs`); the file-read slots hold OS
//! errors, which own no heap. `kind=shallow` means top-level buffers by `len × size_of`, plus the
//! `String` / `Vec` heap one level down and nothing deeper. Every bucket is a
//! LOWER bound; the ground-truth live heap comes from `tools/census-probe`,
//! which records it at the `census_hook::mark` each site sets before its walk.
//!
//! Off, a site costs one `OnceLock` read. It replaces `C1_CONE_CENSUS=1`.

use std::mem::size_of;
use std::sync::OnceLock;

/// True when `ALSEM_RETAINED_CENSUS=1` (`=0`, absent or anything else is off).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("ALSEM_RETAINED_CENSUS").as_deref() == Ok("1"))
}

/// How completely a bucket's walk reaches its structure (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Exact,
    Shallow,
}

/// Print one bucket line on stderr (never stdout: the census changes no output).
pub fn bucket(name: &str, bytes: usize, allocs: usize, kind: Kind) {
    let kind = match kind {
        Kind::Exact => "exact",
        Kind::Shallow => "shallow",
    };
    eprintln!("[retained-census] bucket={name} bytes={bytes} allocs={allocs} kind={kind}");
}

/// A walk's running total. Hash tables and B-trees are charged as one buffer
/// of `len` `(K, V)` pairs (control bytes, load-factor slack and B-tree nodes
/// are not modeled: a further under-count).
#[derive(Clone, Copy, Debug, Default)]
pub struct Tally {
    pub bytes: usize,
    pub allocs: usize,
}

impl Tally {
    /// One heap buffer of `len` `T`s (nothing when empty).
    pub fn buf<T>(&mut self, len: usize) -> &mut Self {
        if len > 0 {
            self.bytes += len * size_of::<T>();
            self.allocs += 1;
        }
        self
    }

    /// One `Arc<T>` allocation (its strong and weak counts plus `T`).
    pub fn arc<T>(&mut self) -> &mut Self {
        self.bytes += 2 * size_of::<usize>() + size_of::<T>();
        self.allocs += 1;
        self
    }

    /// Owned string heap, by `len`.
    pub fn strs<S: AsRef<str>>(&mut self, it: impl IntoIterator<Item = S>) -> &mut Self {
        for s in it {
            let n = s.as_ref().len();
            if n > 0 {
                self.bytes += n;
                self.allocs += 1;
            }
        }
        self
    }

    /// The element buffers of nested `Vec`s (a map's values, one level down).
    pub fn bufs<'v, T: 'v>(&mut self, it: impl IntoIterator<Item = &'v Vec<T>>) -> &mut Self {
        for v in it {
            self.buf::<T>(v.len());
        }
        self
    }

    pub fn emit(&self, name: &str, kind: Kind) {
        bucket(name, self.bytes, self.allocs, kind);
    }
}

/// [`crate::engine::l4::combined_graph::CombinedGraph`], shallow (the base and
/// the detector context each hold one).
pub fn combined_graph(g: &crate::engine::l4::combined_graph::CombinedGraph) -> Tally {
    let mut t = Tally::default();
    t.buf::<String>(g.nodes.len()).strs(&g.nodes);
    t.buf::<(String, Vec<crate::engine::l4::combined_graph::CombinedEdge>)>(g.edges_by_from.len())
        .strs(g.edges_by_from.keys())
        .bufs(g.edges_by_from.values());
    t.buf::<String>(g.edges_from_order.len())
        .strs(&g.edges_from_order);
    t.buf::<crate::engine::l4::combined_graph::UncertaintyEdge>(g.uncertainty_edges.len());
    t.buf::<crate::engine::l4::combined_graph::TypedEdge>(g.typed_edges.len());
    t
}

/// [`crate::program::model::events::EventGraph`], shallow.
pub fn event_graph(g: &crate::program::model::events::EventGraph) -> Tally {
    let mut t = Tally::default();
    t.buf::<crate::program::model::events::EventSymbol>(g.events.len());
    t.buf::<crate::program::model::events::EventEdge>(g.edges.len());
    t
}
