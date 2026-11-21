//! The IR codec (ENG-110): postcard over `al-syntax`'s serde derives, and the publish
//! gate's round trip.

use al_syntax::ir::AlFile;

/// One file's IR as bytes.
pub(crate) fn encode_file(f: &AlFile) -> Vec<u8> {
    // The IR holds only strings, numbers, sequences and enums, which postcard always encodes.
    postcard::to_stdvec(f).expect("postcard encodes every IR value")
}

/// One file's IR back from exactly `bytes`: a decode that leaves a byte unread fails.
pub(crate) fn decode_file(bytes: &[u8]) -> anyhow::Result<AlFile> {
    let (f, rest): (AlFile, &[u8]) = postcard::take_from_bytes(bytes)?;
    anyhow::ensure!(
        rest.is_empty(),
        "{} trailing bytes after the IR",
        rest.len()
    );
    #[cfg(test)]
    let f = test_hook::on_decode(f);
    Ok(f)
}

/// The publish gate (spec Guarantee (b)): `encoded` (the fresh encoding of `fresh`) decodes,
/// the decoded IR equals `fresh`, and re-encoding it gives `encoded` byte for byte. The byte
/// compare sees what `==` cannot, such as an `Origin` kind id read back as another id of the
/// same name.
pub(crate) fn round_trip_ok(fresh: &AlFile, encoded: &[u8]) -> bool {
    match decode_file(encoded) {
        Ok(d) => d == *fresh && encode_file(&d) == encoded,
        Err(_) => false,
    }
}

/// A broken decoder for tests: a file whose first object is named [`test_hook::ALIAS_ON_DECODE`]
/// decodes with its first routine's origin moved to another grammar id of the same kind name,
/// equal by `==`, different in bytes. Keyed by the name, so no other test sees it.
#[cfg(test)]
pub(crate) mod test_hook {
    use al_syntax::ir::{AlFile, Origin};

    pub(crate) const ALIAS_ON_DECODE: &str = "Eng110AliasOnDecode";

    pub(super) fn on_decode(mut f: AlFile) -> AlFile {
        if f.objects.first().is_some_and(|o| o.name == ALIAS_ON_DECODE)
            && let Some(r) = f.objects[0].routines.first_mut()
        {
            r.origin = alias_of(&r.origin).expect("the routine origin's kind has an alias id");
        }
        f
    }

    /// An origin equal to `o` by `==` (same kind name, bytes and points) with another kind id.
    pub(crate) fn alias_of(o: &Origin) -> Option<Origin> {
        let bytes = postcard::to_stdvec(o).ok()?;
        (1..u16::MAX).find_map(|id| {
            // Ids past the grammar's symbol table panic in `kind_text`; every name has its
            // ids below that, and the search stops at the first alias.
            let a = Origin::from_kind_id(id, o.byte(), o.start, o.end);
            (a.kind_text() == o.kind_text() && postcard::to_stdvec(&a).ok()? != bytes).then_some(a)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED_IF: &str = "codeunit 50120 \"Nested\"\n{\n    procedure P()\n    begin\n#if A\n        X();\n#if B\n        Y();\n#else\n        Z();\n#endif\n#endif\n    end;\n}\n";
    const RECOVERED: &str =
        "codeunit 50121 \"Broken\"\n{\n    procedure P()\n    begin\n        X(;\n    end;\n}\n";
    const NON_ASCII: &str = "codeunit 50122 \"Blåbærgrød\"\n{\n    procedure Æble(Øl: Integer)\n    var\n        Å: Text;\n    begin\n        Å := 'æøå';\n    end;\n}\n";
    const PAGE_ACTION: &str = "page 50123 \"Acts\"\n{\n    actions\n    {\n        area(Processing)\n        {\n            action(Go)\n            {\n                trigger OnAction()\n                begin\n                end;\n            }\n        }\n    }\n}\n";

    #[test]
    fn round_trips_nested_if_recovered_and_non_ascii() {
        for src in [NESTED_IF, RECOVERED, NON_ASCII, PAGE_ACTION] {
            let f = al_syntax::parse(src);
            assert!(round_trip_ok(&f, &encode_file(&f)), "{src}");
        }
        assert_eq!(
            al_syntax::parse(RECOVERED).parse_status,
            al_syntax::ir::ParseStatus::Recovered
        );
        assert_eq!(
            al_syntax::parse(PAGE_ACTION).objects[0].actions[0].kind,
            "action"
        );
    }

    #[test]
    fn trailing_byte_is_refused() {
        let enc = encode_file(&al_syntax::parse(NESTED_IF));
        assert!(decode_file(&enc).is_ok());
        assert!(decode_file(&[enc, vec![0]].concat()).is_err());
    }

    /// The keyword decodes only into its static set (`action` → `actiox`, same length).
    #[test]
    fn unknown_action_keyword_is_refused() {
        let mut enc = encode_file(&al_syntax::parse(PAGE_ACTION));
        assert!(decode_file(&enc).is_ok());
        let at = enc
            .windows(7)
            .position(|w| w == b"\x06action")
            .expect("the keyword is encoded as its text");
        enc[at + 6] = b'x';
        // postcard keeps no custom message, so only the refusal is seen.
        assert!(decode_file(&enc).is_err(), "not a keyword");
    }

    /// An `Origin` kind id read back as another id of the same name passes `==` and is caught
    /// only by the re-encoded bytes.
    #[test]
    fn aliased_kind_id_is_caught_by_bytes() {
        let src = format!(
            "codeunit 50124 \"{}\"\n{{\n    procedure P()\n    begin\n    end;\n}}\n",
            test_hook::ALIAS_ON_DECODE
        );
        let fresh = al_syntax::parse(&src);
        let enc = encode_file(&fresh);
        let decoded = decode_file(&enc).expect("decodes");
        assert_eq!(decoded, fresh, "the alias is equal by ==");
        assert_ne!(encode_file(&decoded), enc, "and different in bytes");
        assert!(!round_trip_ok(&fresh, &enc));
    }

    /// The codec stores every field: no `serde(skip` or `serde(default` in the IR.
    #[test]
    fn no_serde_skip_or_default_in_ir() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/al-syntax/src/ir");
        let mut seen = 0;
        for e in std::fs::read_dir(&dir).expect("ir dir") {
            let p = e.expect("entry").path();
            let text = std::fs::read_to_string(&p).expect("read");
            seen += 1;
            for bad in ["serde(skip", "serde(default"] {
                assert!(!text.contains(bad), "{} has `{bad}`", p.display());
            }
        }
        assert!(seen >= 4, "read the IR files");
    }
}
