//! Encoding of index values into bytes that sort the way the values do.
//!
//! - Unsigned integers: the big-endian bytes.
//! - Signed integers: the big-endian bytes with the sign bit flipped.
//! - `bool`: `0` or `1`.
//! - `f32` and `f64`: the big-endian bits, with the sign bit flipped for a positive number and every bit flipped for a negative one.
//!   `-0.0` and `0.0` are different keys, a NaN sorts after the infinity of its sign.
//! - `char`: the big-endian bytes of its code point.
//! - `[u8; N]`: the bytes as they are.
//! - `[u8]`, `Vec<u8>`, `str` and `String`: the bytes with every `0x00` escaped as `0x00 0xFF`, then the terminator `0x00 0x01`.
//!
//! - Tuples of up to four fields: the encodings of the fields one after the other, a key of a multi-field index.
//!
//! No encoding is a prefix of another one of the same type, so the encodings of several values can be concatenated into one multi-field key.
//! `usize` and `isize` have no encoding, their width depends on the platform: cast them to a fixed-width integer.

use std::ops::Range;

/// The bytes that replace a `0x00` byte of a string or a byte slice.
const ESCAPED_ZERO: [u8; 2] = [0x00, 0xFF];

/// The bytes that end an encoded string or byte slice.
const TERMINATOR: [u8; 2] = [0x00, 0x01];

/// Encodes an index value into bytes that sort the way the values do.
///
/// The encoding depends on the type, so an integer literal needs its suffix: `enc(18u8)` and `enc(18u32)` are different keys.
#[must_use]
pub fn enc<K: IndexKey>(value: K) -> Vec<u8> {
    let mut encoded = Vec::new();
    value.encode(&mut encoded);
    encoded
}

/// The range of the encoded strings that start with `prefix`, for [`Database::range`](crate::Database::range).
///
/// `key::prefix_range("Al")` holds `key::enc("Al")` and `key::enc("Alice")`, but not `key::enc("Bob")` or `key::enc("A")`.
#[must_use]
pub fn prefix_range(prefix: &str) -> Range<Vec<u8>> {
    let mut start = Vec::with_capacity(prefix.len());
    push_escaped(prefix.as_bytes(), &mut start);

    // after the prefix comes a UTF-8 byte, an escaped zero or the terminator, all below 0xFF, which UTF-8 never contains
    let mut end = start.clone();
    end.push(0xFF);

    start..end
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

impl IndexKey for char {
    fn encode(&self, out: &mut Vec<u8>) {
        u32::from(*self).encode(out);
    }
}

impl<const N: usize> IndexKey for [u8; N] {
    fn encode(&self, out: &mut Vec<u8>) {
        // every key of the type has the same length, it needs no terminator
        out.extend_from_slice(self);
    }
}

impl IndexKey for [u8] {
    fn encode(&self, out: &mut Vec<u8>) {
        out.reserve(self.len() + TERMINATOR.len());
        push_escaped(self, out);
        out.extend_from_slice(&TERMINATOR);
    }
}

impl IndexKey for Vec<u8> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_slice().encode(out);
    }
}

impl IndexKey for str {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_bytes().encode(out);
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

/// Implements [`IndexKey`] for floating-point numbers: their big-endian bits, with the sign bit flipped for a positive number and every bit flipped for a negative one.
macro_rules! impl_index_key_for_float {
    ($($float:ty => $bits:ty),*) => {$(
        impl IndexKey for $float {
            fn encode(&self, out: &mut Vec<u8>) {
                // the bits of a positive number sort like the number, the ones of a negative number in reverse
                let bits = self.to_bits();
                let sign_bit: $bits = 1 << (<$bits>::BITS - 1);
                let sortable = if bits & sign_bit == 0 { bits ^ sign_bit } else { !bits };
                out.extend_from_slice(&sortable.to_be_bytes());
            }
        }
    )*};
}

/// Implements [`IndexKey`] for a tuple: the encodings of its fields one after the other.
macro_rules! impl_index_key_for_tuple {
    ($($field:ident $position:tt),*) => {
        impl<$($field: IndexKey),*> IndexKey for ($($field,)*) {
            fn encode(&self, out: &mut Vec<u8>) {
                $(self.$position.encode(out);)*
            }
        }
    };
}

impl_index_key_for_tuple!(A 0, B 1);
impl_index_key_for_tuple!(A 0, B 1, C 2);
impl_index_key_for_tuple!(A 0, B 1, C 2, D 3);
impl_index_key_for_unsigned!(u8, u16, u32, u64, u128);
impl_index_key_for_signed!(i8, i16, i32, i64, i128);
impl_index_key_for_float!(f32 => u32, f64 => u64);

/// Appends `bytes` to `out` with every `0x00` escaped, without the terminator.
fn push_escaped(bytes: &[u8], out: &mut Vec<u8>) {
    for &byte in bytes {
        if byte == 0x00 {
            out.extend_from_slice(&ESCAPED_ZERO);
        } else {
            out.push(byte);
        }
    }
}

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

    #[test]
    fn floats_sort_like_values() {
        // `f64` is not `Ord`, the expected order is the total order without the NaNs, with `-0.0` before `0.0`
        let values = [
            1.5,
            f64::NEG_INFINITY,
            -0.0,
            f64::MAX,
            0.0,
            -1.5,
            f64::MIN_POSITIVE,
            f64::INFINITY,
            f64::MIN,
            -f64::MIN_POSITIVE,
            1e-300,
            -2.0,
        ];
        let mut sorted_by_value = values.to_vec();
        let mut sorted_by_key = values.to_vec();

        sorted_by_value.sort_by(f64::total_cmp);
        sorted_by_key.sort_by_key(|value| enc(value));

        assert_eq!(sorted_by_key, sorted_by_value);
        assert!(
            enc(f64::INFINITY) < enc(f64::NAN),
            "a NaN sorts after the infinity"
        );
        assert!(
            enc(-1.5f32) < enc(-0.0f32) && enc(-0.0f32) < enc(0.0f32) && enc(0.0f32) < enc(1.5f32)
        );
    }

    #[test]
    fn chars_and_byte_arrays_sort_like_values() {
        assert_sorts_like_values(&['b', '\0', 'a', 'é', '日', char::MAX]);
        assert_sorts_like_values(&[[1u8, 0], [0, 255], [0, 0], [255, 255], [1, 1]]);
    }

    #[test]
    fn byte_slices_sort_like_values_and_encode_like_strings() {
        let values: [&[u8]; 9] = [
            b"b", b"", b"ab", b"a", b"a\0", b"a\0\0", b"a\x01", b"\xFF", b"a\xFF",
        ];

        assert_sorts_like_values(&values);
        assert_eq!(enc(b"a\0b".as_slice()), enc("a\0b"));
        assert_eq!(enc(b"a\0b".to_vec()), enc("a\0b"));
    }

    #[test]
    fn a_prefix_range_holds_exactly_the_strings_with_the_prefix() {
        let strings = [
            "", "a", "al", "Al", "Al\0", "Ala", "Alice", "Al\u{7f}", "Alé", "Al日本", "Am", "A",
            "Ak", "B", "\0", "A\0l",
        ];
        let prefixes = ["Al", "", "Al\0", "\0", "日"];

        for prefix in prefixes {
            let range = prefix_range(prefix);

            for string in strings {
                assert_eq!(
                    range.contains(&enc(string)),
                    string.starts_with(prefix),
                    "prefix {prefix:?}, string {string:?}"
                );
            }
        }
    }

    /// Asserts that sorting by the keys of the pairs gives the same order as sorting the pairs.
    fn assert_pairs_sort_like_values<A, B>(pairs: &[(A, B)])
    where
        A: IndexKey + Ord + Clone + Debug,
        B: IndexKey + Ord + Clone + Debug,
    {
        let mut sorted_by_value = pairs.to_vec();
        let mut sorted_by_key = pairs.to_vec();

        sorted_by_value.sort();
        sorted_by_key.sort_by_key(|pair| enc(pair));

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
    fn a_tuple_encodes_like_its_concatenated_fields() {
        // the key of a multi-field index was the concatenation before tuples were keys, the stored keys stay valid
        let name = String::from("Alice");

        assert_eq!(enc((&name, 30u8)), [enc("Alice"), enc(30u8)].concat());
        assert_eq!(
            enc(("a", -1i16, true)),
            [enc("a"), enc(-1i16), enc(true)].concat()
        );
        assert_eq!(
            enc((1u8, 'x', [7u8; 2], 0.5f32)),
            [enc(1u8), enc('x'), enc([7u8; 2]), enc(0.5f32)].concat()
        );
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
        assert_eq!(enc(1.0f32), [0xBF, 0x80, 0x00, 0x00]);
        assert_eq!(enc(-1.0f32), [0x40, 0x7F, 0xFF, 0xFF]);
        assert_eq!(enc(0.0f64), [0x80, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(enc('a'), [0, 0, 0, b'a']);
        assert_eq!(enc([0u8, 7]), [0, 7]);
        assert_eq!(enc([7u8, 0].as_slice()), [7, 0x00, 0xFF, 0x00, 0x01]);
    }
}
