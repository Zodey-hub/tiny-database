//! Encoding of index values into bytes that sort the way the values do.
//!
//! - Unsigned integers: the big-endian bytes.
//! - Signed integers: the big-endian bytes with the sign bit flipped.
//! - `bool`: `0` or `1`.
//! - `str` and `String`: the UTF-8 bytes with every `0x00` escaped as `0x00 0xFF`, then the terminator `0x00 0x01`.
//!
//! No encoding is a prefix of another one of the same type, so the encodings of several values can be concatenated into one multi-field key.
//! `usize` and `isize` have no encoding, their width depends on the platform: cast them to a fixed-width integer.

/// The bytes that replace a `0x00` byte of a string.
const ESCAPED_ZERO: [u8; 2] = [0x00, 0xFF];

/// The bytes that end an encoded string.
const TERMINATOR: [u8; 2] = [0x00, 0x01];

/// Encodes an index value into bytes that sort the way the values do.
///
/// The encoding depends on the type, so an integer literal needs its suffix: `enc(18u8)` and `enc(18u32)` are different keys.
pub fn enc<K: IndexKey>(value: K) -> Vec<u8> {
    let mut encoded = Vec::new();
    value.encode(&mut encoded);
    encoded
}

/// A value that can be used as an index key.
pub trait IndexKey {
    /// Appends the sortable encoding of the value to `out`.
    fn encode(&self, out: &mut Vec<u8>);
}

impl<K: IndexKey + ?Sized> IndexKey for &K {
    fn encode(&self, out: &mut Vec<u8>) {
        (**self).encode(out);
    }
}

impl IndexKey for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl IndexKey for str {
    fn encode(&self, out: &mut Vec<u8>) {
        out.reserve(self.len() + TERMINATOR.len());

        for byte in self.bytes() {
            if byte == 0x00 {
                out.extend_from_slice(&ESCAPED_ZERO);
            } else {
                out.push(byte);
            }
        }

        out.extend_from_slice(&TERMINATOR);
    }
}

impl IndexKey for String {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_str().encode(out);
    }
}

/// Implements [`IndexKey`] for unsigned integers: their big-endian bytes.
macro_rules! impl_index_key_for_unsigned {
    ($($integer:ty),*) => {$(
        impl IndexKey for $integer {
            fn encode(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_be_bytes());
            }
        }
    )*};
}

/// Implements [`IndexKey`] for signed integers: their big-endian bytes with the sign bit flipped.
macro_rules! impl_index_key_for_signed {
    ($($integer:ty),*) => {$(
        impl IndexKey for $integer {
            fn encode(&self, out: &mut Vec<u8>) {
                // without the flip the negative values would sort after the positive ones
                let sign_flipped = self ^ <$integer>::MIN;
                out.extend_from_slice(&sign_flipped.to_be_bytes());
            }
        }
    )*};
}

impl_index_key_for_unsigned!(u8, u16, u32, u64, u128);
impl_index_key_for_signed!(i8, i16, i32, i64, i128);

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    /// Asserts that sorting by the encoded keys gives the same order as sorting the values.
    fn assert_sorts_like_values<K: IndexKey + Ord + Clone + Debug>(values: &[K]) {
        let mut sorted_by_value = values.to_vec();
        let mut sorted_by_key = values.to_vec();

        sorted_by_value.sort();
        sorted_by_key.sort_by_key(|value| enc(value));

        assert_eq!(sorted_by_key, sorted_by_value);
    }

    #[test]
    fn unsigned_integers_sort_like_values() {
        assert_sorts_like_values(&[u8::MAX, 0, 128, 1, 127]);
        assert_sorts_like_values(&[256u16, 1, u16::MAX, 255, 0, 257]);
        assert_sorts_like_values(&[u32::MAX, 65_536, 1, 0, 255, 256]);
        assert_sorts_like_values(&[1u64 << 40, u64::MAX, 0, 1, 1 << 32, 255]);
        assert_sorts_like_values(&[u128::MAX, 1 << 64, 0, 1, 1 << 100]);
    }

    #[test]
    fn signed_integers_sort_like_values() {
        assert_sorts_like_values(&[0i8, i8::MIN, -1, 1, i8::MAX]);
        assert_sorts_like_values(&[256i16, -256, i16::MAX, -1, 0, i16::MIN, 1, 255, -255]);
        assert_sorts_like_values(&[i32::MIN, 65_536, -65_536, 0, -1, 1, i32::MAX]);
        assert_sorts_like_values(&[1i64 << 40, i64::MIN, -1, 0, 1, -(1 << 40), i64::MAX]);
        assert_sorts_like_values(&[i128::MAX, -1, 0, i128::MIN, 1 << 100, -(1 << 100)]);
    }

    #[test]
    fn bools_sort_like_values() {
        assert_sorts_like_values(&[true, false]);
    }

    #[test]
    fn strings_sort_like_values() {
        // prefixes, embedded 0x00 bytes, the bytes next to the escape and multibyte characters
        let values = [
            "b", "", "ab", "a", "a\0", "a\0\0", "a\0b", "a\u{1}", "a\u{1}b", "\0", "\0\0", "\u{1}",
            "abc", "é", "a\u{7f}", "a\u{ff}", "日本",
        ];

        assert_sorts_like_values(&values);
    }

    #[test]
    fn owned_strings_encode_like_string_slices() {
        let owned = String::from("a\0b");

        assert_eq!(enc(&owned), enc("a\0b"));
        assert_eq!(enc(owned), enc("a\0b"));
    }

    /// Asserts that sorting by the concatenated keys of the fields gives the same order as sorting the pairs.
    fn assert_pairs_sort_like_values<A, B>(pairs: &[(A, B)])
    where
        A: IndexKey + Ord + Clone + Debug,
        B: IndexKey + Ord + Clone + Debug,
    {
        let mut sorted_by_value = pairs.to_vec();
        let mut sorted_by_key = pairs.to_vec();

        sorted_by_value.sort();
        sorted_by_key.sort_by_key(|(first, second)| [enc(first), enc(second)].concat());

        assert_eq!(sorted_by_key, sorted_by_value);
    }

    #[test]
    fn multi_field_keys_sort_like_tuples() {
        // without the terminator ("a", "bc") and ("ab", "c") would be the same key
        assert_pairs_sort_like_values(&[
            ("ab", "c"),
            ("a", "bc"),
            ("a", "b"),
            ("a\0", ""),
            ("a", "\0"),
            ("", "ab"),
        ]);
        assert_pairs_sort_like_values(&[
            ("ab", 1u16),
            ("a", 256),
            ("a", 1),
            ("", 7),
            ("a\0", 0),
            ("a", u16::MAX),
            ("b", 0),
        ]);
        assert_pairs_sort_like_values(&[(-1i8, "b"), (1, "a"), (-1, "a"), (i8::MIN, "z")]);
    }

    #[test]
    fn encoding_matches_the_stored_format() {
        // the keys are stored in the database file, a change of the format orphans them
        assert_eq!(enc(0x0102u16), [0x01, 0x02]);
        assert_eq!(enc(1u64), [0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(enc(-1i8), [0x7F]);
        assert_eq!(enc(0i16), [0x80, 0x00]);
        assert_eq!(enc(i32::MIN), [0, 0, 0, 0]);
        assert_eq!(enc(false), [0]);
        assert_eq!(enc(true), [1]);
        assert_eq!(enc(""), [0x00, 0x01]);
        assert_eq!(enc("a\0b"), [b'a', 0x00, 0xFF, b'b', 0x00, 0x01]);
    }
}
