//! App dependency topology: an app may reference objects in its own dependency
//! closure (itself + transitively declared dependencies), never the whole world.

use crate::program::node::AppRef;
use std::collections::{BTreeSet, HashMap};

/// `Clone` (T3 Task 5, layered graph split): manifest-derived dependency
/// topology is part of [`crate::program::build::DepLayer`] — cloned into
/// each assembled `ProgramGraph` by `assemble_program_graph`.
#[derive(Default, Clone)]
pub struct DependencyGraph {
    direct: HashMap<AppRef, Vec<AppRef>>,
}

impl DependencyGraph {
    pub fn add_dependency(&mut self, from: AppRef, on: AppRef) {
        let deps = self.direct.entry(from).or_default();
        if !deps.contains(&on) {
            deps.push(on);
        }
    }

    /// `from` + all transitively reachable dependencies. Cycle-safe.
    pub fn closure(&self, from: AppRef) -> BTreeSet<AppRef> {
        crate::program::resolve::reads::probe();
        let mut seen = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(a) = stack.pop() {
            if seen.insert(a)
                && let Some(deps) = self.direct.get(&a)
            {
                stack.extend(deps.iter().copied());
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::node::AppRef;

    #[test]
    fn closure_includes_self_and_transitive_deps_cycle_safe() {
        let mut g = DependencyGraph::default();
        let (a, b, c) = (AppRef(0), AppRef(1), AppRef(2));
        g.add_dependency(a, b);
        g.add_dependency(b, c);
        g.add_dependency(c, a); // cycle — must not loop forever
        let cl = g.closure(a);
        assert!(cl.contains(&a) && cl.contains(&b) && cl.contains(&c));
        let only_c = g.closure(c);
        assert!(only_c.contains(&c) && only_c.contains(&a) && only_c.contains(&b));
    }
}
