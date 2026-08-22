//! What the encrypter refuses.
//!
//! The unit tests beside `src/crypt/encrypter.rs` pin the happy path: a token
//! goes in, the same bytes come out. These ask the opposite question, which is
//! the one an AEAD exists to answer -- **what happens when the token that
//! comes back is not the token that went out**.
//!
//! Two properties are asserted exhaustively rather than by example, because
//! an authenticated cipher that rejects most tampering is not an
//! authenticated cipher:
//!
//! * every single-bit flip anywhere in a token -- nonce, ciphertext or tag --
//!   is rejected, and
//! * the rejection is total. `decrypt` returns `Err`, so there is no code path
//!   on which a caller sees a prefix of a plaintext that failed its tag check.
//!   A cipher that streams plaintext out before verifying is one that lets an
//!   attacker use the application as a decryption oracle.
//!
//! The third property here is freshness: the same plaintext encrypted twice
//! must produce two different tokens. Without it, equal ciphertexts announce
//! equal plaintexts, and an attacker who can see a table of sealed values
//! learns which rows are the same without breaking anything.

#![cfg(feature = "crypt")]

use arcature::crypt::{AppKey, DecryptError, Encrypter};

/// The alphabet the crate's tokens use: RFC 4648 section 5, unpadded.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A deliberately independent base64url decoder.
///
/// The crate has one already, but reusing it would make these tests agree
/// with the implementation by construction rather than by evidence. This one
/// is written from the RFC, so a token that round-trips through it is a token
/// whose wire format is what the documentation claims.
fn decode(text: &str) -> Vec<u8> {
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut out = Vec::new();
    for symbol in text.bytes() {
        let value = ALPHABET
            .iter()
            .position(|candidate| *candidate == symbol)
            .unwrap_or_else(|| panic!("`{}` is outside the base64url alphabet", symbol as char));
        bits = (bits << 6) | u32::try_from(value).expect("a base64 index fits in a u32");
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push(u8::try_from((bits >> held) & 0xff).expect("masked to one byte"));
        }
    }
    out
}

/// The matching encoder.
fn encode(bytes: &[u8]) -> String {
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut out = String::new();
    for byte in bytes {
        bits = (bits << 8) | u32::from(*byte);
        held += 8;
        while held >= 6 {
            held -= 6;
            let index = usize::try_from((bits >> held) & 0x3f).expect("six bits fit in a usize");
            out.push(char::from(ALPHABET[index]));
        }
    }
    if held > 0 {
        let index = usize::try_from((bits << (6 - held)) & 0x3f).expect("six bits fit in a usize");
        out.push(char::from(ALPHABET[index]));
    }
    out
}

/// Split a token into its version tag and its raw bytes.
fn open(token: &str) -> (String, Vec<u8>) {
    let (version, body) = token
        .split_once('.')
        .expect("a token carries a version tag");
    (version.to_string(), decode(body))
}

/// Reassemble a token from a version tag and raw bytes.
fn seal(version: &str, raw: &[u8]) -> String {
    format!("{version}.{}", encode(raw))
}

fn encrypter(fill: u8) -> Encrypter {
    Encrypter::new(&AppKey::from_bytes(&[fill; 64]).expect("64 bytes is a valid APP_KEY"))
}

#[test]
fn every_single_bit_flip_anywhere_in_a_token_is_rejected() {
    let encrypter = encrypter(0x4a);
    let plaintext = b"transfer 250.00 to account 7781".as_slice();
    let token = encrypter.encrypt(plaintext).expect("encrypt");
    let (version, raw) = open(&token);

    // 24 nonce + 31 ciphertext + 16 tag.
    assert_eq!(
        raw.len(),
        24 + plaintext.len() + 16,
        "unexpected token layout"
    );

    let mut checked = 0usize;
    for index in 0..raw.len() {
        for bit in 0..8u8 {
            let mut tampered = raw.clone();
            tampered[index] ^= 1 << bit;
            let token = seal(&version, &tampered);

            assert_eq!(
                encrypter.decrypt(&token),
                Err(DecryptError::Authentication),
                "flipping bit {bit} of byte {index} produced something other than a \
                 clean authentication failure"
            );
            checked += 1;
        }
    }

    assert_eq!(checked, raw.len() * 8);
}

#[test]
fn a_failed_tag_check_yields_no_plaintext_at_all() {
    let encrypter = encrypter(0x4a);
    // Long enough that a streaming implementation would have had plenty of
    // plaintext to hand back before it reached the tag.
    let plaintext = b"A".repeat(4096);
    let token = encrypter.encrypt(&plaintext).expect("encrypt");
    let (version, mut raw) = open(&token);

    // Corrupt the very first ciphertext byte and leave the rest intact.
    raw[24] ^= 0xff;
    let token = seal(&version, &raw);

    let outcome = encrypter.decrypt(&token);
    assert_eq!(outcome, Err(DecryptError::Authentication));
    // `Result` has no "partial" case, and this is the assertion that says the
    // API shape is the security property: there is nowhere for a prefix to go.
    assert!(outcome.is_err());
}

#[test]
fn truncating_the_tag_is_rejected() {
    let encrypter = encrypter(0x4a);
    let token = encrypter.encrypt(b"invoice 4417").expect("encrypt");
    let (version, raw) = open(&token);

    for shorter in 1..=16 {
        let token = seal(&version, &raw[..raw.len() - shorter]);
        assert!(
            encrypter.decrypt(&token).is_err(),
            "a token missing its last {shorter} byte(s) was accepted"
        );
    }
}

#[test]
fn appending_bytes_is_rejected() {
    let encrypter = encrypter(0x4a);
    let token = encrypter.encrypt(b"invoice 4417").expect("encrypt");
    assert_eq!(
        encrypter.decrypt(&format!("{token}AAAA")),
        Err(DecryptError::Authentication)
    );
}

#[test]
fn swapping_the_nonce_between_two_tokens_is_rejected() {
    let encrypter = encrypter(0x4a);
    let (version, first) = open(&encrypter.encrypt(b"left").expect("encrypt"));
    let (_, second) = open(&encrypter.encrypt(b"right").expect("encrypt"));

    let mut spliced = first.clone();
    spliced[..24].copy_from_slice(&second[..24]);
    assert_eq!(
        encrypter.decrypt(&seal(&version, &spliced)),
        Err(DecryptError::Authentication)
    );
}

#[test]
fn a_token_sealed_under_another_key_is_rejected() {
    let mine = encrypter(0x4a);
    let theirs = encrypter(0x4b);
    let token = theirs.encrypt(b"invoice 4417").expect("encrypt");
    assert_eq!(mine.decrypt(&token), Err(DecryptError::Authentication));
}

#[test]
fn a_token_claiming_another_format_version_is_rejected() {
    let encrypter = encrypter(0x4a);
    let token = encrypter.encrypt(b"invoice 4417").expect("encrypt");
    let (_, raw) = open(&token);

    for version in ["v0", "v2", "v10", "", "V1"] {
        assert_eq!(
            encrypter.decrypt(&seal(version, &raw)),
            Err(DecryptError::UnknownVersion),
            "a token tagged `{version}` was not refused as an unknown version"
        );
    }
}

#[test]
fn the_same_plaintext_never_seals_to_the_same_token_twice() {
    let encrypter = encrypter(0x4a);
    let plaintext = b"the same message, every time";

    let mut seen = std::collections::HashSet::new();
    let mut nonces = std::collections::HashSet::new();
    for _ in 0..256 {
        let token = encrypter.encrypt(plaintext).expect("encrypt");
        assert_eq!(
            encrypter.decrypt(&token).expect("decrypt"),
            plaintext.as_slice()
        );

        let (_, raw) = open(&token);
        assert!(nonces.insert(raw[..24].to_vec()), "a nonce repeated");
        assert!(seen.insert(token), "a token repeated");
    }

    assert_eq!(seen.len(), 256);
    assert_eq!(nonces.len(), 256);
}
