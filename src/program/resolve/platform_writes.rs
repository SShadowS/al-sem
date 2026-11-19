//! Which argument a platform method may write (ENG-135).
//!
//! An AL platform method writes an argument through a `var` parameter (`Evaluate(var X, ..)`,
//! `Dictionary.Get(K, var V)`, `InStream.Read(var X)`, `JsonToken.SelectToken(.., var T)`, ...).
//! The var-ness of every platform method's parameters comes from the AL compiler's own symbols
//! (`tools/gen-al-var-params`, generated into `platform_var_params.rs`): ONE table that every
//! reader asking "may this platform call write argument k" uses (the literal pruning's binding
//! and flow, and the guard frames), in place of the hand lists each kept before.
//!
//! The table keys by method NAME: a call site may not know its receiver's type or which overload
//! it calls, so an argument counts as written when ANY overload of that name, on ANY built-in
//! class, takes it as `var` (sound, and exact wherever a name has one shape). A name the compiler
//! does not list may write anything.

#[path = "platform_var_params.rs"]
mod table;

/// Whether a call of the platform method `method` (any case) may write its argument `arg`
/// (0-based, the receiver not counted).
#[must_use]
pub fn may_write_arg(method: &str, arg: u32) -> bool {
    let name = al_syntax::fold_identifier(method);
    match table::PLATFORM_VAR_PARAMS.binary_search_by(|(n, _)| (*n).cmp(name.as_str())) {
        Ok(i) => arg >= 64 || table::PLATFORM_VAR_PARAMS[i].1 & (1u64 << arg) != 0,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binary search needs the names sorted and unique, as the generator writes them.
    #[test]
    fn the_table_is_sorted_and_unique() {
        assert!(
            table::PLATFORM_VAR_PARAMS
                .windows(2)
                .all(|w| w[0].0 < w[1].0),
            "sorted and unique by name"
        );
        // Every platform method name alc 18.0.41 lists (the generator's header): a table that
        // lost names would only cost precision (an unlisted name writes), so pin the count to
        // make a shrink visible. A new alc: re-run the generator and update it.
        assert_eq!(
            table::PLATFORM_VAR_PARAMS.len(),
            796,
            "every platform method"
        );
    }

    /// The compiler's answers on the methods the old hand lists named, and on read-only ones.
    #[test]
    fn known_writers_and_readers() {
        // Writers, at their var position only.
        for (m, k) in [
            ("Evaluate", 0),
            ("Clear", 0),
            ("Get", 1), // Dictionary.Get(K, var V)
            ("Read", 0),
            ("ReadText", 0),
            ("SelectToken", 1),
            ("StartSession", 0),
        ] {
            assert!(may_write_arg(m, k), "{m} writes argument {k}");
        }
        assert!(!may_write_arg("Evaluate", 1), "Evaluate reads its text");
        // A variadic `var` parameter writes every position from it on (Fable: alc gives AL0130
        // for a literal at `Dialog.Open('#1## #2##', 1, 2)`'s positions 1 and 2).
        assert!(
            (1..5).all(|k| may_write_arg("Open", k)),
            "Dialog.Open's variables"
        );
        assert!(!may_write_arg("Open", 0), "Dialog.Open's text");
        assert!(
            !may_write_arg("SelectToken", 0),
            "SelectToken reads its path"
        );
        // Read-only.
        for m in [
            "StrLen",
            "CopyStr",
            "Format",
            "StrSubstNo",
            "Message",
            "Error",
            "Abs",
        ] {
            assert!((0..4).all(|k| !may_write_arg(m, k)), "{m} writes nothing");
        }
        // A name the compiler does not list may write.
        assert!(may_write_arg("NoSuchPlatformMethod", 0));
    }
}
