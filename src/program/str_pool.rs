//! Strings stored once (engine-switch S10.2).
//!
//! Engine string fields hold a [`SharedStr`]: an `Arc<str>` that compares,
//! hashes, orders, prints (`{}` and `{:?}`) and serializes exactly as its text,
//! so changing a `String` field to it changes no output. [`StrPool`] makes equal
//! texts one allocation: a dependency tier runs one pool over its nodes when it is
//! built ([`ShareStrings`]), and every later clone of a tier value (event links,
//! `incoming`, a second root's copies of dependency ids) shares that text. A text
//! is freed with its last holder, so nothing grows in a long-running server.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// A shared, immutable string. See the module doc.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct SharedStr(Arc<str>);

impl SharedStr {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Both hold the same allocation (what [`StrPool`] guarantees for equal texts).
    pub fn ptr_eq(a: &SharedStr, b: &SharedStr) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

impl Deref for SharedStr {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SharedStr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lookups by `&str` in maps and sets keyed by `SharedStr` (`Hash` and `Eq` are
/// those of the text, as `Borrow` requires).
impl Borrow<str> for SharedStr {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl fmt::Display for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.0, f)
    }
}

impl From<&str> for SharedStr {
    fn from(s: &str) -> Self {
        SharedStr(Arc::from(s))
    }
}

impl From<String> for SharedStr {
    fn from(s: String) -> Self {
        SharedStr(Arc::from(s))
    }
}

impl From<&String> for SharedStr {
    fn from(s: &String) -> Self {
        SharedStr(Arc::from(s.as_str()))
    }
}

impl From<SharedStr> for String {
    fn from(s: SharedStr) -> Self {
        String::from(&*s.0)
    }
}

impl PartialEq<str> for SharedStr {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for SharedStr {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl PartialEq<String> for SharedStr {
    fn eq(&self, other: &String) -> bool {
        &*self.0 == other.as_str()
    }
}

impl PartialEq<SharedStr> for str {
    fn eq(&self, other: &SharedStr) -> bool {
        self == &*other.0
    }
}

impl PartialEq<SharedStr> for &str {
    fn eq(&self, other: &SharedStr) -> bool {
        *self == &*other.0
    }
}

impl PartialEq<SharedStr> for String {
    fn eq(&self, other: &SharedStr) -> bool {
        self.as_str() == &*other.0
    }
}

/// One allocation per distinct text.
#[derive(Default)]
pub struct StrPool {
    texts: HashSet<SharedStr>,
    seen: usize,
    merged: usize,
}

impl StrPool {
    /// Replace `s` with the pool's copy of its text (adding it when new).
    pub fn share(&mut self, s: &mut SharedStr) {
        self.seen += 1;
        match self.texts.get(s.as_str()) {
            Some(pooled) => {
                if !SharedStr::ptr_eq(pooled, s) {
                    self.merged += 1;
                    *s = pooled.clone();
                }
            }
            None => {
                self.texts.insert(s.clone());
            }
        }
    }

    /// Strings passed to [`Self::share`], distinct texts among them, and how
    /// many were a second allocation of a text the pool already held.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.seen, self.texts.len(), self.merged)
    }

    pub fn share_opt(&mut self, s: &mut Option<SharedStr>) {
        if let Some(s) = s {
            self.share(s);
        }
    }
}

/// A value whose string fields can be pooled. Each impl names every
/// [`SharedStr`] field it holds, so a new field is a visible decision.
pub trait ShareStrings {
    fn share_strings(&mut self, pool: &mut StrPool);
}

impl<T: ShareStrings> ShareStrings for [T] {
    fn share_strings(&mut self, pool: &mut StrPool) {
        for item in self {
            item.share_strings(pool);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_str_compares_orders_prints_and_serializes_as_its_text() {
        let s = SharedStr::from("Ab \"c\"");
        let t = String::from("Ab \"c\"");
        assert_eq!(format!("{s:?}"), format!("{t:?}"));
        assert_eq!(format!("{s}"), t);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            serde_json::to_string(&t).unwrap()
        );
        let back: SharedStr = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, s);
        assert!(s == "Ab \"c\"");
        assert!(s == t);
        assert!(t == s);
        let mut v = vec![
            SharedStr::from("b"),
            SharedStr::from("B"),
            SharedStr::from("a"),
        ];
        v.sort();
        assert_eq!(v, ["B", "a", "b"]);
        let set: HashSet<SharedStr> = v.into_iter().collect();
        assert!(set.contains("a"));
    }

    #[test]
    fn a_pool_makes_equal_texts_one_allocation() {
        let mut pool = StrPool::default();
        let mut a = SharedStr::from("x");
        let mut b = SharedStr::from(String::from("x"));
        let mut c = SharedStr::from("y");
        assert!(!SharedStr::ptr_eq(&a, &b));
        pool.share(&mut a);
        pool.share(&mut b);
        pool.share(&mut c);
        assert!(SharedStr::ptr_eq(&a, &b));
        assert!(!SharedStr::ptr_eq(&a, &c));
        assert_eq!(pool.counts(), (3, 2, 1));
        pool.share(&mut b);
        assert_eq!(
            pool.counts(),
            (4, 2, 1),
            "an already shared text is no merge"
        );
    }
}
