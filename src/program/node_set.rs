//! A sorted node list in two sorted parts, read as ONE sorted list.
//!
//! `shared` is the dependency tier: built once, `Arc`-held, identical for
//! every workspace root that loads the same dependencies (see
//! `program::dep_cache`). `own` is this root's part: its workspace nodes
//! (always `AppRef(0)`, so a prefix) plus synthetic platform publishers,
//! which carry a dependency `AppRef` and so interleave with `shared`.
//! Iteration, indexing and binary search all see the merged order, and on an
//! id tie the `shared` element comes first — exactly what the old
//! `Vec::extend` + stable `sort_by` produced. Nothing is copied.

use std::cmp::Ordering;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

use crate::program::node::{ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{ObjectNode, RoutineNode};

/// The key a node list is sorted by.
pub trait SortKey {
    type Key: Ord;
    fn sort_key(&self) -> &Self::Key;
}

impl SortKey for ObjectNode {
    type Key = ObjectNodeId;
    fn sort_key(&self) -> &ObjectNodeId {
        &self.id
    }
}

impl SortKey for RoutineNode {
    type Key = RoutineNodeId;
    fn sort_key(&self) -> &RoutineNodeId {
        &self.id
    }
}

#[derive(Debug)]
pub struct NodeSet<T> {
    shared: Arc<Vec<T>>,
    own: Vec<T>,
    /// Merged position of each `own` element, ascending (one per element).
    own_pos: Vec<usize>,
}

impl<T> Default for NodeSet<T> {
    fn default() -> Self {
        NodeSet {
            shared: Arc::new(Vec::new()),
            own: Vec::new(),
            own_pos: Vec::new(),
        }
    }
}

impl<T: SortKey> NodeSet<T> {
    /// `shared` and `own` must each already be sorted by `sort_key`.
    pub fn layered(shared: Arc<Vec<T>>, own: Vec<T>) -> Self {
        Self::debug_assert_sorted(&shared);
        Self::debug_assert_sorted(&own);
        let mut set = NodeSet {
            shared,
            own,
            own_pos: Vec::new(),
        };
        set.reindex();
        set
    }

    /// `run_by` and every routine lookup depend on each part being sorted.
    /// (Checked where sortedness is promised, not in `reindex`: `push` and
    /// `extend` legitimately leave `own` unsorted until `sort_by`.)
    fn debug_assert_sorted(part: &[T]) {
        debug_assert!(
            part.windows(2).all(|w| w[0].sort_key() <= w[1].sort_key()),
            "NodeSet part is not sorted by its sort key"
        );
    }

    /// Recompute `own_pos`: an own element lands after every shared element
    /// whose key is <= its own (shared first on ties).
    fn reindex(&mut self) {
        let mut pos = Vec::with_capacity(self.own.len());
        let mut k = 0;
        for (i, o) in self.own.iter().enumerate() {
            while k < self.shared.len() && self.shared[k].sort_key() <= o.sort_key() {
                k += 1;
            }
            pos.push(i + k);
        }
        self.own_pos = pos;
    }

    pub fn len(&self) -> usize {
        self.shared.len() + self.own.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        match self.own_pos.binary_search(&i) {
            Ok(j) => self.own.get(j),
            Err(j) => self.shared.get(i - j),
        }
    }

    pub fn iter(&self) -> NodeSetIter<'_, T> {
        NodeSetIter {
            shared: &self.shared,
            own: &self.own,
            s: 0,
            o: 0,
        }
    }

    /// `slice::binary_search_by`'s exact algorithm (Rust 1.96 core), over the
    /// merged order, so it returns the SAME index the old flat `Vec` did —
    /// including which element of a run of equal keys it lands on (a
    /// synthetic publisher can tie with a real dependency routine).
    pub fn binary_search_by<F: FnMut(&T) -> Ordering>(&self, mut f: F) -> Result<usize, usize> {
        let mut size = self.len();
        if size == 0 {
            return Err(0);
        }
        let mut base = 0usize;
        while size > 1 {
            let half = size / 2;
            let mid = base + half;
            if f(&self[mid]) != Ordering::Greater {
                base = mid;
            }
            size -= half;
        }
        match f(&self[base]) {
            Ordering::Equal => Ok(base),
            Ordering::Less => Err(base + 1),
            Ordering::Greater => Err(base),
        }
    }

    /// The contiguous run of elements `f` maps to `Equal`, in merged order
    /// (shared first on ties), multiplicity kept. `f` must agree with the sort
    /// order: `Less` before the run, `Greater` after it. Two binary searches
    /// per part; no allocation.
    ///
    /// Restricting the merge to the run gives the full merge's order: every
    /// element before the run (in either part) is strictly smaller than every
    /// run element, and every element after it strictly larger.
    pub fn run_by<F: Fn(&T) -> Ordering>(&self, f: F) -> NodeSetIter<'_, T> {
        let run = |part: &[T]| {
            let lo = part.partition_point(|x| f(x) == Ordering::Less);
            let len = part[lo..].partition_point(|x| f(x) == Ordering::Equal);
            lo..lo + len
        };
        NodeSetIter {
            shared: &self.shared[run(&self.shared)],
            own: &self.own[run(&self.own)],
            s: 0,
            o: 0,
        }
    }

    /// Append to the own part. Like `Vec::push` on a sorted list, the caller
    /// re-sorts afterwards (`sort_by`) before relying on the order.
    pub fn push(&mut self, item: T) {
        self.own.push(item);
        self.reindex();
    }

    pub fn extend<I: IntoIterator<Item = T>>(&mut self, items: I) {
        self.own.extend(items);
        self.reindex();
    }

    /// Stable-sort the own part (the shared part is sorted by construction).
    pub fn sort_by<F: FnMut(&T, &T) -> Ordering>(&mut self, f: F) {
        self.own.sort_by(f);
        Self::debug_assert_sorted(&self.own);
        self.reindex();
    }

    pub fn shared(&self) -> &Arc<Vec<T>> {
        &self.shared
    }

    pub fn own(&self) -> &[T] {
        &self.own
    }
}

impl<T: SortKey> Index<usize> for NodeSet<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        self.get(i).expect("NodeSet index out of bounds")
    }
}

/// Mutating the shared part copies it (`Arc::make_mut`, copy-on-write).
/// Production code never mutates graph nodes after assembly; these exist for
/// tests.
impl<T: SortKey + Clone> IndexMut<usize> for NodeSet<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        match self.own_pos.binary_search(&i) {
            Ok(j) => &mut self.own[j],
            Err(j) => &mut Arc::make_mut(&mut self.shared)[i - j],
        }
    }
}

impl<T: SortKey + Clone> NodeSet<T> {
    /// Mutation order is unspecified; mutating a node's sort key requires
    /// `sort_by` afterwards, as with a `Vec`. Copies the shared part
    /// (copy-on-write).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        Arc::make_mut(&mut self.shared)
            .iter_mut()
            .chain(self.own.iter_mut())
    }
}

impl<T> NodeSet<T> {
    pub fn clear(&mut self) {
        self.shared = Arc::new(Vec::new());
        self.own.clear();
        self.own_pos.clear();
    }
}

impl<T: SortKey> From<Vec<T>> for NodeSet<T> {
    fn from(own: Vec<T>) -> Self {
        NodeSet::layered(Arc::new(Vec::new()), own)
    }
}

impl<'a, T: SortKey> IntoIterator for &'a NodeSet<T> {
    type Item = &'a T;
    type IntoIter = NodeSetIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Merged in-order iterator: shared first on ties.
#[derive(Clone)]
pub struct NodeSetIter<'a, T> {
    shared: &'a [T],
    own: &'a [T],
    s: usize,
    o: usize,
}

impl<'a, T: SortKey> Iterator for NodeSetIter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        let take_shared = match (self.shared.get(self.s), self.own.get(self.o)) {
            (Some(s), Some(o)) => s.sort_key() <= o.sort_key(),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return None,
        };
        if take_shared {
            self.s += 1;
            Some(&self.shared[self.s - 1])
        } else {
            self.o += 1;
            Some(&self.own[self.o - 1])
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.shared.len() - self.s + self.own.len() - self.o;
        (n, Some(n))
    }
}

impl<T: SortKey> ExactSizeIterator for NodeSetIter<'_, T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct N(u32, &'static str);
    impl SortKey for N {
        type Key = u32;
        fn sort_key(&self) -> &u32 {
            &self.0
        }
    }

    fn flat(set: &NodeSet<N>) -> Vec<N> {
        set.iter().cloned().collect()
    }

    #[test]
    fn layered_reads_as_one_sorted_list() {
        // own = workspace prefix (0, 1) + one interleaved synthetic (5).
        let set = NodeSet::layered(
            Arc::new(vec![N(3, "s"), N(4, "s"), N(7, "s")]),
            vec![N(0, "o"), N(1, "o"), N(5, "o")],
        );
        let want = vec![
            N(0, "o"),
            N(1, "o"),
            N(3, "s"),
            N(4, "s"),
            N(5, "o"),
            N(7, "s"),
        ];
        assert_eq!(flat(&set), want);
        assert_eq!(set.len(), 6);
        for (i, w) in want.iter().enumerate() {
            assert_eq!(&set[i], w, "index {i}");
        }
        assert_eq!(set.get(6), None);
        assert_eq!(set.binary_search_by(|n| n.0.cmp(&5)), Ok(4));
        assert_eq!(set.binary_search_by(|n| n.0.cmp(&6)), Err(5));
        assert_eq!(set.binary_search_by(|n| n.0.cmp(&9)), Err(6));
    }

    /// Review Focus 3: on an id tie the SHARED element comes first — what
    /// the old `extend` + stable `sort_by` produced for a synthetic that
    /// ties with an existing dependency routine.
    #[test]
    fn tie_puts_shared_first() {
        let set = NodeSet::layered(Arc::new(vec![N(2, "s")]), vec![N(2, "o")]);
        assert_eq!(flat(&set), vec![N(2, "s"), N(2, "o")]);
        assert_eq!(set[0], N(2, "s"));
        assert_eq!(set[1], N(2, "o"));
    }

    #[test]
    fn push_then_sort_matches_vec_semantics() {
        let mut set: NodeSet<N> = vec![N(1, "a"), N(4, "a")].into();
        set.push(N(2, "b"));
        set.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(flat(&set), vec![N(1, "a"), N(2, "b"), N(4, "a")]);
        assert_eq!(set[1], N(2, "b"));
    }

    #[test]
    fn index_mut_hits_merged_index_and_copies_shared_on_write() {
        let original = Arc::new(vec![N(3, "s"), N(4, "s"), N(7, "s")]);
        let mut set =
            NodeSet::layered(Arc::clone(&original), vec![N(0, "o"), N(1, "o"), N(5, "o")]);
        // merged: 0o 1o 3s 4s 5o 7s. Index 1 and 4 are own; 3 and 5 shared/own mix.
        set[1].1 = "x"; // own
        set[4].1 = "y"; // own (interleaved)
        assert_eq!(set[1], N(1, "x"));
        assert_eq!(set[4], N(5, "y"));
        // shared untouched so far: no copy yet.
        assert!(Arc::ptr_eq(set.shared(), &original));
        set[3].1 = "z"; // shared element -> copy-on-write
        assert_eq!(set[3], N(4, "z"));
        assert_eq!(set[2], N(3, "s"));
        assert_eq!(set[5], N(7, "s"));
        assert!(!Arc::ptr_eq(set.shared(), &original));
        assert_eq!(*original, vec![N(3, "s"), N(4, "s"), N(7, "s")]);
    }

    #[test]
    fn iter_mut_and_clear() {
        let mut set = NodeSet::layered(Arc::new(vec![N(3, "s")]), vec![N(1, "o")]);
        for n in set.iter_mut() {
            n.1 = "m";
        }
        assert_eq!(flat(&set), vec![N(1, "m"), N(3, "m")]);
        set.clear();
        assert!(set.is_empty());
        assert_eq!(set.get(0), None);
    }

    /// Discrimination for the sortedness `debug_assert!`: an unsorted part
    /// panics in debug builds, a sorted one passes.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "not sorted")]
    fn unsorted_own_part_panics_in_debug() {
        let _ = NodeSet::layered(Arc::new(vec![N(1, "s")]), vec![N(4, "o"), N(2, "o")]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "not sorted")]
    fn unsorted_shared_part_panics_in_debug() {
        let _ = NodeSet::layered(Arc::new(vec![N(5, "s"), N(3, "s")]), Vec::new());
    }

    #[test]
    fn sorted_parts_with_ties_pass() {
        let _ = NodeSet::layered(
            Arc::new(vec![N(1, "s"), N(1, "s")]),
            vec![N(1, "o"), N(2, "o")],
        );
    }

    #[test]
    fn empty_parts_are_fine() {
        let empty: NodeSet<N> = NodeSet::default();
        assert!(empty.is_empty());
        assert_eq!(empty.iter().count(), 0);
        let only_shared = NodeSet::layered(Arc::new(vec![N(1, "s")]), Vec::new());
        assert_eq!(flat(&only_shared), vec![N(1, "s")]);
    }

    /// `run_by` yields exactly the flattened list's elements of one key, in
    /// order: duplicates in both parts, a shared/own tie (shared first), and
    /// interleaving across the parts.
    #[test]
    fn run_by_matches_flat_filter() {
        // Sort key is the number; the run key is the tens digit.
        let set = NodeSet::layered(
            Arc::new(vec![
                N(5, "s"),
                N(10, "s"),
                N(10, "s"),
                N(13, "s"),
                N(21, "s"),
            ]),
            vec![N(9, "o"), N(10, "o"), N(12, "o"), N(12, "o"), N(30, "o")],
        );
        let flat = flat(&set);
        for tens in 0..5u32 {
            let got: Vec<N> = set.run_by(|n| (n.0 / 10).cmp(&tens)).cloned().collect();
            let want: Vec<N> = flat.iter().filter(|n| n.0 / 10 == tens).cloned().collect();
            assert_eq!(got, want, "tens {tens}");
        }
        let one: Vec<N> = set.run_by(|n| (n.0 / 10).cmp(&1)).cloned().collect();
        assert_eq!(
            one,
            vec![
                N(10, "s"),
                N(10, "s"),
                N(10, "o"),
                N(12, "o"),
                N(12, "o"),
                N(13, "s")
            ]
        );
    }

    /// `binary_search_by` must return exactly what `slice::binary_search_by`
    /// returns on the flattened list — the same `Ok` index inside a run of
    /// equal keys, not just "some" equal element. Resolver sites read the
    /// found node's fields, so a different tie pick is a different answer.
    #[test]
    fn binary_search_matches_slice_on_ties() {
        let shapes: Vec<(Vec<N>, Vec<N>)> = vec![
            (vec![N(1, "s"), N(2, "real")], vec![N(2, "synthetic")]),
            (
                vec![N(1, "s"), N(3, "s"), N(3, "s")],
                vec![N(0, "o"), N(3, "o"), N(4, "o")],
            ),
            (
                vec![N(2, "s"), N(2, "s"), N(2, "s"), N(5, "s")],
                vec![N(2, "o"), N(2, "o"), N(2, "o")],
            ),
            (
                vec![N(0, "s"), N(1, "s"), N(1, "s"), N(6, "s"), N(6, "s")],
                vec![N(1, "o"), N(6, "o"), N(6, "o"), N(6, "o"), N(8, "o")],
            ),
        ];
        for (shared, own) in shapes {
            let set = NodeSet::layered(Arc::new(shared), own);
            let flat = flat(&set);
            for key in 0..10u32 {
                assert_eq!(
                    set.binary_search_by(|n| n.0.cmp(&key)),
                    flat.binary_search_by(|n| n.0.cmp(&key)),
                    "key {key} over {flat:?}"
                );
            }
        }
    }
}
