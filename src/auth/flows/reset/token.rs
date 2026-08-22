//! The value types: what a reset token looks like on the wire, and what the
//! caller gets back exactly once.

use std::fmt;

use chrono::{DateTime, Utc};
use zeroize::Zeroize;

use crate::crypt::base64url;

/// How many bytes of the public half of a reset token.
pub(crate) const ID_BYTES: usize = 16;

/// How many bytes of the secret half of a reset token.
///
/// 256 bits of uniform randomness, which is why the stored digest is a fast
/// hash rather than a slow one. The reasoning is spelled out in full at the
/// hashing site in [`crate::tokens`] and is unchanged here.
pub(crate) const SECRET_BYTES: usize = 32;

/// The fixed opening of every password-reset token this crate mints.
///
/// A distinct prefix from [`TOKEN_PREFIX`](crate::tokens::TOKEN_PREFIX), and
/// deliberately so: the two credentials have opposite lifetimes and opposite
/// blast radii, and a secret scanner that finds one in a log should be able to
/// say which it found. An API token in a paste is a standing grant to revoke;
/// a reset token in a paste is a live path to a password change, expiring
/// within the hour.
///
/// ```
/// use arcature::auth::flows::RESET_TOKEN_PREFIX;
///
/// // A scanner rule is one literal.
/// assert_eq!(RESET_TOKEN_PREFIX, "arcpwr_");
/// ```
pub const RESET_TOKEN_PREFIX: &str = "arcpwr_";

/// Separates the two halves of the plaintext.
///
/// A `.`, which is outside the base64url alphabet and inside RFC 3986's
/// unreserved set. The first property makes the split unambiguous without a
/// length assumption; the second makes the whole token a path segment or a
/// query value with no escaping, which is where a reset token lives.
const SEPARATOR: char = '.';

// ---------------------------------------------------------------------------
// ResetTokenId
// ---------------------------------------------------------------------------

/// The public half of a reset token: the 16 bytes the row is looked up by.
///
/// Crate-internal, unlike [`ApiTokenId`](crate::tokens::ApiTokenId). An API
/// token id is shown in a management screen and accepted from a revocation
/// request, so it has to be a type an application can hold. A reset token is
/// one opaque string that goes into one link and comes back once; splitting it
/// into halves is an implementation detail of the lookup, and exposing that
/// detail would only invite a caller to build a query out of the public half.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResetTokenId([u8; ID_BYTES]);

impl ResetTokenId {
    // There is no `from_bytes` constructor, and its absence is a small
    // guarantee rather than an omission: the only way to obtain an id is
    // [`parse_plaintext`], so every id in the program came from a string a
    // client presented, in the one spelling the encoder produces.

    /// The id's raw bytes, as the column stores them.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ResetTokenId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ResetTokenId({})", base64url::encode(&self.0))
    }
}

// ---------------------------------------------------------------------------
// PlaintextReset
// ---------------------------------------------------------------------------

/// The one and only copy of a reset token's plaintext.
///
/// Handed back by [`PasswordResets::issue`](super::PasswordResets::issue) and
/// never again: the row holds a digest of the secret half, and a digest does
/// not run backwards. Put it in the mail and drop it.
///
/// The same three properties as
/// [`PlaintextToken`](crate::tokens::PlaintextToken), for the same reasons: no
/// `Clone`, a `Debug` that prints nothing, and a `Drop` that zeroizes. The
/// zeroize is best-effort and does not reach a copy the caller made -- a
/// formatted mail body, a log line, a `String` of its own.
#[non_exhaustive]
pub struct PlaintextReset(String);

impl PlaintextReset {
    /// Wrap a freshly minted plaintext.
    pub(crate) fn new(plaintext: String) -> Self {
        Self(plaintext)
    }

    /// The plaintext, for the one mail that carries it to its owner.
    ///
    /// Named `expose` rather than `as_str` because every call site should read
    /// as a decision.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlaintextReset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PlaintextReset([redacted])")
    }
}

impl Drop for PlaintextReset {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

// ---------------------------------------------------------------------------
// IssuedPasswordReset
// ---------------------------------------------------------------------------

/// What [`PasswordResets::issue`](super::PasswordResets::issue) returns.
///
/// The plaintext to mail, plus the two facts a mail template needs: who it is
/// for, and when it stops working. Nothing else about the row is here, because
/// nothing else about the row is anybody's business -- there is no accessor
/// for the digest for the same reason there is no accessor for the secret.
#[derive(Debug)]
#[non_exhaustive]
pub struct IssuedPasswordReset {
    subject: String,
    expires_at: DateTime<Utc>,
    plaintext: PlaintextReset,
}

impl IssuedPasswordReset {
    /// Pair a freshly written row with its plaintext.
    pub(crate) fn new(
        subject: String,
        expires_at: DateTime<Utc>,
        plaintext: PlaintextReset,
    ) -> Self {
        Self {
            subject,
            expires_at,
            plaintext,
        }
    }

    /// Whoever the token resets, in the application's own spelling.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// When the token stops working.
    #[must_use]
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// The plaintext, to be mailed once.
    #[must_use]
    pub fn plaintext(&self) -> &PlaintextReset {
        &self.plaintext
    }

    /// Split into the subject, the deadline, and the plaintext.
    #[must_use]
    pub fn into_parts(self) -> (String, DateTime<Utc>, PlaintextReset) {
        (self.subject, self.expires_at, self.plaintext)
    }
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

/// Assemble the plaintext a caller sees: prefix, public id, separator, secret.
pub(crate) fn format_plaintext(id: &[u8; ID_BYTES], secret: &[u8; SECRET_BYTES]) -> String {
    format!(
        "{RESET_TOKEN_PREFIX}{}{SEPARATOR}{}",
        base64url::encode(id),
        base64url::encode(secret)
    )
}

/// Split a presented plaintext back into its public id and its secret half.
///
/// Returns `None` for anything that is not exactly the shape
/// [`format_plaintext`] writes. A caller holding `None` was handed something
/// this crate never minted and can reject it without asking the database,
/// which is the point: a malformed string should not cost a query.
///
/// The strictness is inherited from [`base64url::decode`], which refuses
/// padding, refuses a length no encoder could produce, and refuses
/// non-canonical trailing bits. That last one matters more here than it looks:
/// a lax decoder would give one stored row a family of distinct spellings, and
/// "this token was already spent" is a comparison against exactly one of them.
pub(crate) fn parse_plaintext(presented: &str) -> Option<(ResetTokenId, [u8; SECRET_BYTES])> {
    let (id_text, secret_text) = presented
        .strip_prefix(RESET_TOKEN_PREFIX)?
        .split_once(SEPARATOR)?;

    let id: [u8; ID_BYTES] = base64url::decode(id_text)?.try_into().ok()?;
    let secret: [u8; SECRET_BYTES] = base64url::decode(secret_text)?.try_into().ok()?;

    Some((ResetTokenId(id), secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plaintext_carries_the_scanner_prefix_and_both_halves() {
        let plaintext = format_plaintext(&[0xab; ID_BYTES], &[0xcd; SECRET_BYTES]);
        assert!(plaintext.starts_with(RESET_TOKEN_PREFIX));
        // prefix + 22 symbols + separator + 43 symbols
        assert_eq!(plaintext.len(), RESET_TOKEN_PREFIX.len() + 22 + 1 + 43);
    }

    #[test]
    fn a_minted_plaintext_parses_back_to_what_went_in() {
        let id = [7u8; ID_BYTES];
        let secret = [9u8; SECRET_BYTES];
        let (parsed_id, parsed_secret) =
            parse_plaintext(&format_plaintext(&id, &secret)).expect("round trip");
        assert_eq!(parsed_id.as_bytes(), &id[..]);
        assert_eq!(parsed_secret, secret);
    }

    #[test]
    fn the_whole_plaintext_is_safe_in_a_url_without_escaping() {
        // The reason this token is base64url and not something prettier: it
        // goes in a link. Every character has to be RFC 3986 unreserved, or a
        // mail client that percent-encodes on the way out and a router that
        // decodes on the way in stop agreeing on what the token was.
        let plaintext = format_plaintext(&[0xffu8; ID_BYTES], &[0x00u8; SECRET_BYTES]);
        assert!(
            plaintext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')),
            "not URL-safe: {plaintext}"
        );
    }

    #[test]
    fn a_string_this_crate_never_minted_costs_no_query() {
        // Each of these fails in `parse_plaintext`, before the store has a
        // reason to touch the database.
        for hostile in [
            "",
            "arcpwr_",
            "arcpat_AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "arcpwr_AAAAAAAAAAAAAAAAAAAAAA",
            "arcpwr_AAAAAAAAAAAAAAAAAAAAAA.short",
            "arcpwr_short.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "arcpwr_AAAAAAAAAAAAAAAAAAAA==.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(
                parse_plaintext(hostile).is_none(),
                "accepted {hostile:?} as a token"
            );
        }
    }

    #[test]
    fn a_non_canonical_spelling_of_a_real_id_is_refused() {
        // 16 bytes encode to 22 symbols whose last symbol carries only four
        // meaningful bits. `AQ` and `AR` differ in bits the encoder always
        // writes as zero, so exactly one of them is a spelling of that id --
        // and the other must not be a second key to the same row.
        let good = format_plaintext(&[0u8; ID_BYTES], &[0u8; SECRET_BYTES]);
        assert!(parse_plaintext(&good).is_some());

        // Flip the final symbol of the id half from `A` (all six bits zero) to
        // `B` (the lowest bit set), which is a bit the encoder never sets.
        let id_end = RESET_TOKEN_PREFIX.len() + 22;
        assert_eq!(&good[id_end - 1..id_end], "A");
        let smuggled = format!("{}B{}", &good[..id_end - 1], &good[id_end..]);
        assert_ne!(smuggled, good, "the test did not actually change anything");
        assert!(parse_plaintext(&smuggled).is_none());
    }

    #[test]
    fn a_redacted_debug_does_not_contain_the_secret() {
        let plaintext = PlaintextReset::new("arcpwr_dead.beef".to_owned());
        assert!(!format!("{plaintext:?}").contains("beef"));
    }

    #[test]
    fn the_id_debug_is_not_the_plaintext() {
        // The id is not a secret, so it prints -- but it prints as an id, not
        // as something that could be pasted back in as a token.
        let plaintext = format_plaintext(&[0u8; ID_BYTES], &[0u8; SECRET_BYTES]);
        let (id, _) = parse_plaintext(&plaintext).expect("round trip");
        let rendered = format!("{id:?}");
        assert!(rendered.starts_with("ResetTokenId("));
        assert!(!rendered.contains(RESET_TOKEN_PREFIX));
    }
}
