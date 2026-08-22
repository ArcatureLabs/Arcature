//! Unpadded base64url (RFC 4648 §5), written out rather than pulled in.
//!
//! Two properties are wanted here that a general-purpose encoder does not
//! promise. The alphabet has to stay **stable forever**, because it is baked
//! into every token this module has ever issued and a token outlives the
//! release that minted it. And the decoder has to be **strict**: it rejects
//! padding, rejects a length that cannot have come from an encoder, and
//! rejects non-canonical trailing bits, so that a given byte string has
//! exactly one spelling. A lax decoder gives an attacker a family of distinct
//! strings that decode alike, which is how a token that was revoked by string
//! comparison comes back to life.
//!
//! Sixty lines of table lookup is a smaller thing to own than a dependency
//! whose behaviour on malformed input is a version-to-version decision, and
//! the crate already writes its own hex for the same reason (see
//! `crate::cli::commands::key_generate`).

/// The URL-safe alphabet: `-` and `_` for indices 62 and 63.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encode `bytes` as unpadded base64url.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut triple = u32::from(chunk[0]) << 16;
        triple |= u32::from(chunk.get(1).copied().unwrap_or(0)) << 8;
        triple |= u32::from(chunk.get(2).copied().unwrap_or(0));

        let digit = |shift: u32| char::from(ALPHABET[((triple >> shift) & 0b0011_1111) as usize]);
        out.push(digit(18));
        out.push(digit(12));
        if chunk.len() > 1 {
            out.push(digit(6));
        }
        if chunk.len() > 2 {
            out.push(digit(0));
        }
    }
    out
}

/// Decode unpadded base64url, or `None` if `text` is not something [`encode`]
/// could have produced.
///
/// Rejected: any character outside the alphabet (padding `=` included), a
/// length congruent to 1 modulo 4 (no byte string encodes to that), and a
/// final group whose unused low bits are not zero.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let input = text.as_bytes();
    if input.len() % 4 == 1 {
        return None;
    }

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    for group in input.chunks(4) {
        let mut packed = 0u32;
        for (index, byte) in group.iter().enumerate() {
            packed |= u32::from(value_of(*byte)?) << (18 - 6 * index);
        }
        // A group of n symbols carries n - 1 whole bytes off the top of the
        // 24-bit word; every bit below them is padding the encoder wrote as
        // zero, so a non-zero one means this string did not come from an
        // encoder.
        let bytes = group.len() - 1;
        let unused_bits = 24 - 8 * bytes;
        if packed & ((1u32 << unused_bits) - 1) != 0 {
            return None;
        }
        for index in 0..bytes {
            out.push(u8::try_from((packed >> (16 - 8 * index)) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// The alphabet index of an ASCII byte, or `None` if it is not in it.
fn value_of(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    // RFC 4648 §10, with the standard alphabet's `+/` never appearing because
    // none of these vectors reach indices 62 or 63.
    #[test]
    fn the_rfc_4648_vectors_round_trip_without_padding() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), encoded, "encoding {plain:?}");
            assert_eq!(
                decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "decoding {encoded:?}"
            );
        }
    }

    // The two bytes the URL-safe alphabet exists for. `0xff 0xef` is
    // `+/` in the standard alphabet and `-_` here.
    #[test]
    fn the_url_safe_alphabet_is_used_for_sixty_two_and_sixty_three() {
        assert_eq!(encode(&[0xfb, 0xef, 0xff]), "--__");
        assert_eq!(decode("--__"), Some(vec![0xfb, 0xef, 0xff]));
    }

    #[test]
    fn every_byte_value_round_trips() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&all)), Some(all));
    }

    #[test]
    fn padding_is_rejected() {
        assert_eq!(decode("Zg=="), None);
        assert_eq!(decode("Zm8="), None);
    }

    #[test]
    fn a_character_outside_the_alphabet_is_rejected() {
        assert_eq!(decode("Zm9v!"), None);
        // The standard alphabet's two, which this one does not use.
        assert_eq!(decode("Zm9+"), None);
        assert_eq!(decode("Zm9/"), None);
        assert_eq!(decode("Zm9 "), None);
    }

    #[test]
    fn an_impossible_length_is_rejected() {
        assert_eq!(decode("Z"), None);
        assert_eq!(decode("Zm9vY"), None);
    }

    // "Zg" and "Zh" both carry the byte 0x66 in their top eight bits; only the
    // first has zero trailing bits, so only the first is a spelling an encoder
    // could have produced.
    #[test]
    fn non_canonical_trailing_bits_are_rejected() {
        assert_eq!(decode("Zg"), Some(vec![0x66]));
        assert_eq!(decode("Zh"), None);
        assert_eq!(decode("Zm8"), Some(vec![0x66, 0x6f]));
        assert_eq!(decode("Zm9"), None);
    }
}
