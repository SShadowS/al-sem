//! ENG-176: one typed key for a variable a routine names, from one resolver that applies AL's
//! scoping.
//!
//! Readers of RecordRef and Variant holders used to key a variable by its text, folded and
//! stripped on their own: a declared name (already logical: the lowerer strips its quotes), an
//! assignment side from the walk (logical too), or raw source text (a call's receiver or
//! argument, a record operation's receiver: quotes, `this.`, parentheses and indexes kept). So one
//! variable could get two keys (a local named `"this.G"`: `g` declared, `this.g` as a receiver;
//! sol r3 on ENG-162, a false `no` since ENG-159) and two variables one key (a global and the
//! local that hides it), and ENG-162 needed a sentinel for a hidden global.
//!
//! A [`HolderKey`] is the variable's position: the routine's own variables (parameters, then
//! locals; `RoutineVariables::own`) or the object's globals (`RoutineVariables::globals`, every
//! one, shadowed or not). It is exact by construction: no folding or stripping can make two
//! variables equal, and a global hidden by a local keeps its own key. Within a routine it is
//! `Copy` and cheap to hash.
//!
//! [`HolderScope`] resolves both kinds of text, the way alc binds them:
//! - [`HolderScope::logical`]: a declared or logical name: the routine's own variable of that
//!   name, else the object's global of that name.
//! - [`HolderScope::text`]: raw source text: parentheses around the whole are looked through, a
//!   leading unquoted `this.` names the object's global even when a local hides it (ENG-12's
//!   `extract::this_receivers`), quotes are stripped (`""` unescaped), and an index (`X[1]`)
//!   names an element of the array variable `X` ([`HolderRef::indexed`]). A member of anything
//!   else (`Rec.Field`), a call or an expression is no variable.

use crate::program::model::workspace::{ModelRoutine, ModelVariable};
use al_syntax::IdentifierFoldExt;

/// A variable of a routine, by position (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HolderKey {
    /// The routine's own variable at this index: parameters in order, then locals.
    Own(u32),
    /// The owning object's global at this index (shadowed or not).
    Global(u32),
}

/// A variable named by raw source text, and whether the text names an element of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolderRef {
    pub key: HolderKey,
    /// `X[1]`: an element of the array variable `X`, not `X` itself.
    pub indexed: bool,
}

/// One routine's scope, to resolve the variables its text names (module doc).
#[derive(Debug, Clone, Copy)]
pub struct HolderScope<'a> {
    own: &'a [ModelVariable],
    globals: &'a [ModelVariable],
}

impl<'a> HolderScope<'a> {
    /// The scope of routine `r`.
    #[must_use]
    pub fn of(r: &'a ModelRoutine) -> Self {
        Self::new(r.variables.own(), r.variables.globals())
    }

    /// The scope over these own variables and object globals.
    #[must_use]
    pub fn new(own: &'a [ModelVariable], globals: &'a [ModelVariable]) -> Self {
        Self { own, globals }
    }

    /// The variable a declared or logical name names: the routine's own variable of that name,
    /// else the object's global of that name. `None` for a name the routine does not declare.
    #[must_use]
    pub fn logical(&self, name: &str) -> Option<HolderKey> {
        let n = name.fold_identifier();
        self.own
            .iter()
            .position(|v| v.name == n)
            .map(|i| HolderKey::Own(i as u32))
            .or_else(|| self.global(&n))
    }

    /// The object's global of this (folded) name, whatever hides it.
    fn global(&self, folded: &str) -> Option<HolderKey> {
        self.globals
            .iter()
            .position(|v| v.name == folded)
            .map(|i| HolderKey::Global(i as u32))
    }

    /// The variable raw source text names, or `None` when the text is not a variable (module doc).
    #[must_use]
    pub fn text(&self, raw: &str) -> Option<HolderRef> {
        let s = crate::program::model::recref_alias::strip_parens(raw);
        let (s, this) = match s.get(..5) {
            Some(p) if p.eq_fold_identifier("this.") => (&s[5..], true),
            _ => (s, false),
        };
        // The first `[` outside quotes starts an index; a `.` outside quotes is a member.
        let mut quoted = false;
        let mut end = s.len();
        for (i, ch) in s.char_indices() {
            match ch {
                '"' => quoted = !quoted,
                '[' if !quoted => {
                    end = i;
                    break;
                }
                '.' | '(' | ' ' if !quoted => return None,
                _ => {}
            }
        }
        let indexed = end < s.len();
        let base = s[..end].trim();
        if base.is_empty() {
            return None;
        }
        let name = crate::program::body::node_util::strip_quotes(base)
            .replace("\"\"", "\"")
            .fold_identifier();
        let key = if this {
            self.global(&name)?
        } else {
            self.logical(&name)?
        };
        Some(HolderRef { key, indexed })
    }

    /// The variable a key names.
    #[must_use]
    pub fn variable(&self, key: HolderKey) -> &'a ModelVariable {
        match key {
            HolderKey::Own(i) => &self.own[i as usize],
            HolderKey::Global(i) => &self.globals[i as usize],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, scope: &str) -> ModelVariable {
        ModelVariable {
            name: name.fold_identifier(),
            declared_type: "RecordRef".to_string(),
            is_parameter: scope == "parameter",
            parameter_index: None,
            initializer: None,
            scope: Some(scope.to_string()),
            fixed_local: None,
        }
    }

    /// Every scoping case the module doc names, against one scope: a parameter `P`, locals `G`
    /// (hiding the global `G`), `this.G` (a quoted local's name) and `Slot`, and globals `G`, `H`
    /// and `Arr`.
    ///
    /// Discrimination (2026-11-21): `text` stripping `this.` like the old keys (reading `this.G`
    /// through `logical`) fails the `this.G` rows; stripping quotes before the `this.` test fails
    /// the quoted `"this.G"` rows; restored, passes.
    #[test]
    fn one_resolver_names_every_spelling_exactly() {
        let own = [
            var("P", "parameter"),
            var("G", "local"),
            var("this.G", "local"),
            var("Slot", "local"),
        ];
        let globals = [var("G", "global"), var("H", "global"), var("Arr", "global")];
        let s = HolderScope::new(&own, &globals);
        let own_k = |i| Some(HolderKey::Own(i));
        let glob = |i| Some(HolderKey::Global(i));
        let plain = |k: Option<HolderKey>| {
            k.map(|key| HolderRef {
                key,
                indexed: false,
            })
        };
        let elem = |k: Option<HolderKey>| k.map(|key| HolderRef { key, indexed: true });
        // Declared and logical names: the own variable first, then the global.
        assert_eq!(s.logical("p"), own_k(0));
        assert_eq!(s.logical("G"), own_k(1), "the local hides the global");
        assert_eq!(s.logical("this.g"), own_k(2), "a quoted local named this.G");
        assert_eq!(s.logical("h"), glob(1));
        assert_eq!(s.logical("nope"), None);
        // Raw text.
        assert_eq!(s.text("G"), plain(own_k(1)));
        assert_eq!(s.text("(G)"), plain(own_k(1)));
        assert_eq!(
            s.text("this.G"),
            plain(glob(0)),
            "this.G is the global, hidden or not"
        );
        assert_eq!(s.text("THIS.g"), plain(glob(0)));
        assert_eq!(s.text("((this.G))"), plain(glob(0)));
        assert_eq!(
            s.text("\"this.G\""),
            plain(own_k(2)),
            "the quoted local, not the global"
        );
        assert_eq!(s.text("this.H"), plain(glob(1)));
        assert_eq!(s.text("H"), plain(glob(1)));
        assert_eq!(s.text("Slot[1]"), elem(own_k(3)));
        assert_eq!(s.text("this.Arr[2]"), elem(glob(2)));
        assert_eq!(s.text("this.Slot[1]"), None, "no global Slot");
        // Not a variable: a member, a call, an expression, an unknown name.
        assert_eq!(s.text("Rec.Field"), None);
        assert_eq!(s.text("G.Duplicate()"), None);
        assert_eq!(s.text("A + B"), None);
        assert_eq!(s.text("Unknown"), None);
        assert_eq!(s.variable(HolderKey::Own(2)).name, "this.g");
    }
}
