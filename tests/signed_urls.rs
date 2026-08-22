//! What a signed URL refuses.
//!
//! A signed URL is a bearer token pasted into a query string. It travels
//! through mail clients that rewrite links, proxies that reorder parameters,
//! and browsers that keep it in history -- and the only thing standing between
//! "this link came from us" and "somebody typed a bigger number into it" is
//! the MAC. So the interesting tests are not the ones where it verifies.
//!
//! The central test here is exhaustive rather than illustrative:
//! [`every_single_character_change_anywhere_in_the_url_is_rejected`] rewrites
//! **every byte position** of a signed URL, one at a time, and requires an
//! error from each. That covers the origin, the path, every parameter name,
//! every parameter value, the expiry, the separators and the signature itself
//! without anyone having to remember to add a case for a new field.
//!
//! Expiry is tested against an injected clock. A test that proves a link dies
//! after sixty seconds by sleeping for sixty seconds is a test nobody runs
//! twice, and one that sleeps for a millisecond proves only that the
//! comparison is `>` rather than that it reads the right field.

#![cfg(feature = "signed-urls")]

use std::sync::Arc;
use std::time::Duration;

use arcature::config::AppConfig;
use arcature::crypt::{AppKey, Clock, SignedUrlError, UrlSigner};

/// A clock that says exactly what it was built with, so "an hour later" costs
/// nothing to test.
struct Frozen(u64);

impl Clock for Frozen {
    fn now_unix(&self) -> u64 {
        self.0
    }
}

const BASE: &str = "https://example.com";

fn config() -> AppConfig {
    AppConfig::new().url(BASE)
}

/// A signer holding the key every test here shares, reading a clock stopped at
/// `second`.
fn signer_at(second: u64) -> UrlSigner {
    let key = AppKey::from_bytes(&[0x4a; 64]).expect("64 bytes is a valid APP_KEY");
    UrlSigner::new(&key, &config()).with_clock(Arc::new(Frozen(second)))
}

/// A signer holding a *different* key, for the "signed by someone else" cases.
fn other_signer_at(second: u64) -> UrlSigner {
    let key = AppKey::from_bytes(&[0x4b; 64]).expect("64 bytes is a valid APP_KEY");
    UrlSigner::new(&key, &config()).with_clock(Arc::new(Frozen(second)))
}

#[test]
fn every_single_character_change_anywhere_in_the_url_is_rejected() {
    let signer = signer_at(1_000);
    let url = signer
        .sign_temporary(
            "/reports/7",
            &[("format", "csv"), ("page", "3")],
            Duration::from_secs(60),
        )
        .expect("sign");
    assert_eq!(
        signer.verify(&url),
        Ok(()),
        "the unmodified URL must verify"
    );

    let bytes = url.as_bytes();
    assert!(
        bytes.is_ascii(),
        "this test mutates bytes, so keep it ASCII"
    );

    let mut rejected = 0usize;
    for index in 0..bytes.len() {
        // One substitution that is guaranteed to differ from what is there.
        let replacement = if bytes[index] == b'a' { b'b' } else { b'a' };
        let mut mutated = bytes.to_vec();
        mutated[index] = replacement;
        let mutated = String::from_utf8(mutated).expect("ASCII in, ASCII out");
        assert!(
            signer.verify(&mutated).is_err(),
            "changing byte {index} of the URL was accepted:\n  {url}\n  {mutated}"
        );

        // And one deletion, which catches anything that survives substitution
        // by coincidence -- a shifted field, a shortened signature.
        let mut shortened = bytes.to_vec();
        shortened.remove(index);
        let shortened = String::from_utf8(shortened).expect("ASCII in, ASCII out");
        assert!(
            signer.verify(&shortened).is_err(),
            "deleting byte {index} of the URL was accepted:\n  {url}\n  {shortened}"
        );

        rejected += 2;
    }

    // A loop that ran zero times would pass every assertion above. This is the
    // assertion that the exhaustive test was actually exhaustive.
    assert!(bytes.len() > 100, "the fixture URL got shorter: {url}");
    assert_eq!(rejected, bytes.len() * 2);
}

#[test]
fn the_expiry_cannot_be_moved_forward() {
    let signer = signer_at(1_000);
    let url = signer
        .sign_temporary("/download/42", &[], Duration::from_secs(60))
        .expect("sign");
    assert!(url.contains("expires=1060"), "{url}");

    // The whole point of putting the expiry inside the MAC. Without it, this
    // edit would turn a one-minute link into a three-hundred-year one.
    let extended = url.replace("expires=1060", "expires=9999999999");
    assert_eq!(
        signer_at(2_000).verify(&extended),
        Err(SignedUrlError::Mismatch)
    );

    // Backwards is no better; it is still a URL nobody signed.
    let shortened = url.replace("expires=1060", "expires=1010");
    assert_eq!(signer.verify(&shortened), Err(SignedUrlError::Mismatch));
}

#[test]
fn an_expired_url_is_rejected_without_anyone_having_to_wait() {
    let url = signer_at(1_000)
        .sign_temporary("/download/42", &[("as", "pdf")], Duration::from_secs(3_600))
        .expect("sign");

    assert_eq!(signer_at(1_000).verify(&url), Ok(()));
    assert_eq!(signer_at(4_599).verify(&url), Ok(()));
    assert_eq!(
        signer_at(4_600).verify(&url),
        Ok(()),
        "inclusive at the deadline"
    );
    assert_eq!(signer_at(4_601).verify(&url), Err(SignedUrlError::Expired));
    assert_eq!(
        signer_at(u64::MAX).verify(&url),
        Err(SignedUrlError::Expired)
    );
}

#[test]
fn tampering_is_reported_as_tampering_even_after_the_deadline() {
    let url = signer_at(1_000)
        .sign_temporary("/download/42", &[], Duration::from_secs(60))
        .expect("sign");
    let edited = url.replace("/download/42", "/download/43");

    // The signature is checked first, so an expired *and* edited link reads as
    // an attack rather than as a slow reader.
    assert_eq!(
        signer_at(9_000).verify(&edited),
        Err(SignedUrlError::Mismatch)
    );
}

#[test]
fn reordering_the_query_still_verifies() {
    let signer = signer_at(1_000);
    let url = signer
        .sign_temporary(
            "/reports/7",
            &[("zebra", "1"), ("alpha", "2"), ("middle", "3")],
            Duration::from_secs(60),
        )
        .expect("sign");

    let (path, query) = url.split_once('?').expect("a signed URL has a query");
    let mut parts: Vec<&str> = query.split('&').collect();
    let original = parts.clone();

    // Every rotation of the parameter list, signature included: a mail client
    // or a redirect that rewrites the query must not break the link.
    for rotation in 1..parts.len() {
        parts.rotate_left(1);
        let reordered = format!("{path}?{}", parts.join("&"));
        assert_ne!(reordered, url, "rotation {rotation} changed nothing");
        assert_eq!(
            signer.verify(&reordered),
            Ok(()),
            "rotation {rotation} broke a link that only moved:\n  {reordered}"
        );
    }

    assert_eq!(parts.len(), original.len());
}

#[test]
fn swapping_two_values_between_their_keys_is_rejected() {
    let signer = signer_at(1_000);
    let url = signer
        .sign("/reports/7", &[("from", "alpha"), ("to", "omega")])
        .expect("sign");

    // A pure reorder verifies; this is not a reorder. The same multiset of
    // strings appears, attached to different names.
    let swapped = url
        .replace("from=alpha", "from=OMEGA")
        .replace("to=omega", "to=alpha")
        .replace("from=OMEGA", "from=omega");
    assert!(swapped.contains("from=omega") && swapped.contains("to=alpha"));
    assert_eq!(signer.verify(&swapped), Err(SignedUrlError::Mismatch));
}

#[test]
fn a_parameter_cannot_be_added_or_removed() {
    let signer = signer_at(1_000);
    let url = signer
        .sign("/reports/7", &[("format", "csv")])
        .expect("sign");

    let added = url.replace("?format=csv", "?format=csv&admin=1");
    assert_eq!(signer.verify(&added), Err(SignedUrlError::Mismatch));

    let removed = url.replace("format=csv&", "");
    assert_eq!(signer.verify(&removed), Err(SignedUrlError::Mismatch));
}

#[test]
fn a_signature_cannot_be_lifted_onto_another_path() {
    let signer = signer_at(1_000);
    let mine = signer.sign("/reports/7", &[]).expect("sign");
    let theirs = signer.sign("/reports/8", &[]).expect("sign");

    let signature = mine
        .split_once("signature=")
        .expect("a signature parameter")
        .1;
    let other_signature = theirs
        .split_once("signature=")
        .expect("a signature parameter")
        .1;
    assert_ne!(signature, other_signature);

    let forged = theirs.replace(other_signature, signature);
    assert_eq!(signer.verify(&forged), Err(SignedUrlError::Mismatch));
}

#[test]
fn a_url_signed_under_another_key_is_rejected() {
    let url = other_signer_at(1_000)
        .sign_temporary("/download/42", &[], Duration::from_secs(60))
        .expect("sign");
    assert_eq!(signer_at(1_000).verify(&url), Err(SignedUrlError::Mismatch));
}

#[test]
fn a_url_moved_to_a_lookalike_origin_is_rejected() {
    let signer = signer_at(1_000);
    let url = signer.sign("/reports/7", &[]).expect("sign");

    for origin in [
        "https://example.com.evil.test",
        "https://example.community",
        "http://example.com",
        "https://evil.test",
    ] {
        let moved = url.replace(BASE, origin);
        assert_eq!(
            signer.verify(&moved),
            Err(SignedUrlError::ForeignOrigin),
            "a link presented as `{origin}` was not refused on its origin"
        );
    }
}

#[test]
fn two_signatures_are_refused_rather_than_one_being_picked() {
    let signer = signer_at(1_000);
    let url = signer.sign("/reports/7", &[]).expect("sign");
    let signature = url
        .split_once("signature=")
        .expect("a signature parameter")
        .1
        .to_string();

    // A verifier that read the first and a router that read the last would
    // disagree about what was signed. There is no correct choice here, so
    // there is no choice.
    let doubled = format!("{url}&signature={signature}");
    assert_eq!(signer.verify(&doubled), Err(SignedUrlError::Malformed));
}

#[test]
fn a_signature_with_no_version_tag_is_refused() {
    let signer = signer_at(1_000);
    let url = signer.sign("/reports/7", &[]).expect("sign");

    let untagged = url.replace("signature=v1.", "signature=");
    assert_eq!(
        signer.verify(&untagged),
        Err(SignedUrlError::UnknownSignatureVersion)
    );

    let retagged = url.replace("signature=v1.", "signature=v2.");
    assert_eq!(
        signer.verify(&retagged),
        Err(SignedUrlError::UnknownSignatureVersion)
    );
}

#[test]
fn an_equivalent_percent_encoding_still_verifies() {
    let signer = signer_at(1_000);
    let url = signer
        .sign("/search", &[("q", "annual report")])
        .expect("sign");
    assert!(url.contains("q=annual%20report"), "{url}");

    // The MAC is over decoded bytes, so a client that re-encodes a character
    // the signer left alone -- or leaves alone one the signer encoded -- is
    // presenting the same query.
    let re_encoded = url.replace("q=annual%20report", "q=annual%20%72eport");
    assert_eq!(signer.verify(&re_encoded), Ok(()));

    // But a different value is a different value, however it is spelled.
    let different = url.replace("q=annual%20report", "q=annual%20reports");
    assert_eq!(signer.verify(&different), Err(SignedUrlError::Mismatch));
}

#[test]
fn a_value_cannot_smuggle_a_parameter_in() {
    let signer = signer_at(1_000);
    // If the value were pasted in unescaped, this would read as three
    // parameters, one of them called `admin`.
    let url = signer
        .sign("/reports/7", &[("note", "x&admin=1&y=")])
        .expect("sign");

    assert!(!url.contains("&admin=1"), "{url}");
    assert_eq!(signer.verify(&url), Ok(()));
}
