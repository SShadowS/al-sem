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
use std::ops::Index;
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
        let mut set = NodeSet {
            shared,
            own,
            own_pos: Vec::new(),
        };
        set.reindex();
        set
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

    /// Same contract as `slice::binary_search_by`, over the merged order.
    pub fn binary_search_by<F: FnMut(&T) -> Ordering>(&self, mut f: F) -> Result<usize, usize> {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match f(&self[mid]) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(mid),
            }
        }
        Err(lo)
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
    fn empty_parts_are_fine() {
        let empty: NodeSet<N> = NodeSet::default();
        assert!(empty.is_empty());
        assert_eq!(empty.iter().count(), 0);
        let only_shared = NodeSet::layered(Arc::new(vec![N(1, "s")]), Vec::new());
        assert_eq!(flat(&only_shared), vec![N(1, "s")]);
    }
}
