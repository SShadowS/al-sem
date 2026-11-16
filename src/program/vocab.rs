//! ENG-96: small vocabularies stored in a few bytes instead of a `String`.
//!
//! A field that holds one of a handful of words (a temp-state kind, a variable scope, a
//! control context) was a 24-byte `String` plus a heap copy per row. [`open_vocab!`] makes
//! an enum of the known words that still keeps ANY other word: `Other(u16)` indexes a
//! process-wide table of the words seen so far (leaked, one copy each; none occur on real
//! input, so the table stays empty). Equality, ordering, hashing, `Display` and serde all go
//! through the word, so every comparison, sort and output is the one the `String` gave.

use std::sync::Mutex;

/// The words no vocabulary knows, one leaked copy each, in first-seen order.
static OTHER_WORDS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// The index of `word` in the table, added on first sight.
#[doc(hidden)]
pub fn other_index(word: &str) -> u16 {
    let mut words = OTHER_WORDS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(i) = words.iter().position(|w| *w == word) {
        return i as u16;
    }
    let i = u16::try_from(words.len()).expect("under 65,536 unknown vocabulary words");
    words.push(Box::leak(word.to_owned().into_boxed_str()));
    i
}

/// The word at `index` in the table.
#[doc(hidden)]
pub fn other_word(index: u16) -> &'static str {
    OTHER_WORDS.lock().unwrap_or_else(|p| p.into_inner())[index as usize]
}

/// `open_vocab!(Name { Variant => "word", ... })`: a `Copy` enum of the known words plus
/// `Other(u16)` for any other word, compared, ordered, hashed, printed and (de)serialized as
/// its word.
#[macro_export]
macro_rules! open_vocab {
    ($(#[$meta:meta])* $vis:vis $name:ident { $($variant:ident => $word:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy)]
        $vis enum $name {
            $($variant,)+
            /// A word this vocabulary does not know, kept ([`crate::program::vocab`]).
            Other(u16),
        }

        impl $name {
            /// The word.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word,)+
                    Self::Other(i) => $crate::program::vocab::other_word(i),
                }
            }
        }

        impl From<&str> for $name {
            fn from(word: &str) -> Self {
                match word {
                    $($word => Self::$variant,)+
                    other => Self::Other($crate::program::vocab::other_index(other)),
                }
            }
        }

        impl From<String> for $name {
            fn from(word: String) -> Self {
                Self::from(word.as_str())
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                self.as_str()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.as_str() == other.as_str()
            }
        }
        impl Eq for $name {}

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }
        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }
        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.as_str() == other.as_str()
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for $name {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.as_str().cmp(other.as_str())
            }
        }

        impl std::hash::Hash for $name {
            fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
                self.as_str().hash(h)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(self.as_str(), f)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let word = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
                Ok(Self::from(word.as_ref()))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    crate::open_vocab!(Color { Red => "red", Green => "green" });

    /// A known word is its variant; any other word is kept and reads back as itself; both
    /// compare, order and hash as their word (what the `String` they replace did).
    #[test]
    fn known_and_other_words_behave_as_their_text() {
        let red = Color::from("red");
        assert!(matches!(red, Color::Red));
        let blue = Color::from("blue");
        assert!(matches!(blue, Color::Other(_)));
        assert_eq!(blue.as_str(), "blue");
        assert_eq!(
            Color::from("blue"),
            blue,
            "the same unknown word, the same value"
        );
        assert_eq!(blue, "blue");
        let mut v = vec![Color::Red, blue, Color::Green];
        v.sort();
        assert_eq!(
            v.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            vec!["blue", "green", "red"],
            "ordered as their words"
        );
        assert_eq!(serde_json::to_string(&blue).unwrap(), "\"blue\"");
        let back: Color = serde_json::from_str("\"green\"").unwrap();
        assert!(matches!(back, Color::Green));
        assert_eq!(std::mem::size_of::<Color>(), 4);
    }
}
