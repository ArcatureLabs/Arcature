//! The value types: what a remember-me cookie looks like on the wire, and
//! what the caller gets back each time one rotates.

use std::fmt;

use chrono::{DateTime, Utc};
use zeroize::Zeroize;

use crate::crypt::base64url;

/// How many bytes of the public half of a remember-me token.
///
/// The public half is called the *series*, and it is the one part of the
/// credential that survives a rotation: the row is looked up by it, and the
/// secret beside it is replaced on every use. Without a stable handle there is
/// nothing to attach a theft report to -- a scheme that replaces the whole
/// credential every time cannot tell a stolen cookie from an unknown one.
pub(crate) const SERIES_BYTES: usize = 16;

/// How many bytes of the secret half of a remember-me token.
///
/// 256 bits of uniform randomness, which is why the stored digest is a fast
/// hash rather than a slow one. The reasoning is spelled out in full at the
/// hashing site in [`crate::tokens`] and is unchanged here.
pub(crate) const SECRET_BYTES: usize = 32;

/// The fixed opening of every remember-me token this crate mints.
///
/// A prefix of its own, distinct from
/// [`TOKEN_PREFIX`](crate::tokens::TOKEN_PREFIX) and
/// [`RESET_TOKEN_PREFIX`](super::super::RESET_TOKEN_PREFIX), for the reason
/// those two are distinct from each other: a secret scanner that finds one of
/// them in a log should be able to say which it found, because the three call
/// for different responses. An API token is a standing grant to revoke; a
/// reset link is a live path to a password change, expiring within the hour; a
/// remember-me cookie is a live session for one device that may have weeks
/// left on it, and finding one means revoking that device *and* looking for
/// what else the same log exposed.
///
/// ```
/// use arcature::auth::flows::REMEMBER_TOKEN_PREFIX;
///
/// // A scanner rule is one literal.
/// assert_eq!(REMEMBER_TOKEN_PREFIX, "arcrmb_");
/// ```
pub const REMEMBER_TOKEN_PREFIX: &str = "arcrmb_";

/// Separates the two halves of the plaintext.
///
/// A `.`, which is outside the base64url alphabet and inside RFC 3986's
/// unreserved set. The first property makes the split unambiguous without a
/// length assumption; the second matters more here than it does for a reset
/// link, because this value goes in a `Set-Cookie` header, and RFC 6265's
/// cookie-value grammar excludes exactly the characters a base64 encoder would
/// otherwise reach for. No quoting, no percent-encoding, no disagreement
/// between the framework that writes the header and the one that reads it
/// back.
const SEPARATOR: char = '.';

// ---------------------------------------------------------------------------
// SeriesId
// ---------------------------------------------------------------------------

/// The public half of a remember-me token: the 16 bytes the row is keyed by.
///
/// Crate-internal, like the password-reset store's token id and unlike
/// [`ApiTokenId`](crate::tokens::ApiTokenId). An API token id is shown in a
/// management screen and accepted from a revocation request, so it has to be
/// a type an application can hold. A series is an
/// implementation detail of the lookup: the application revokes by subject or
/// by handing back the whole cookie, never by naming a series it took apart
/// itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SeriesId([u8; SERIES_BYTES]);

impl SeriesId {
    // There is no `from_bytes` constructor, and its absence is a small
    // guarantee rather than an omission: the only way to obtain a series is
    // [`parse_plaintext`], so every series in the program came from a string a
    // client presented, in the one spelling the encoder produces.

    /// The series' raw bytes, as the column stores them.
    ///
    /// An array reference rather than a slice, because the rotation path needs
    /// both spellings: the bind wants bytes, and [`format_plaintext`] wants a
    /// fixed-width series to put back on the wire beside the new secret. An
    /// array reference coerces to the first and satisfies the second; a slice
    /// would satisfy only the first and force a fallible conversion at the one
    /// call site that cannot fail.
    pub(crate) fn as_bytes(&self) -> &[u8; SERIES_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SeriesId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SeriesId({})", base64url::encode(&self.0))
    }
}

// ---------------------------------------------------------------------------
// PlaintextRememberToken
// ---------------------------------------------------------------------------

/// The one and only copy of a remember-me token's plaintext.
///
/// Handed back when a token is issued and again every time one rotates, and
/// never otherwise: the row holds a digest of the secret half, and a digest
/// does not run backwards. Put it in a cookie and drop it.
///
/// The same three properties as
/// [`PlaintextToken`](crate::tokens::PlaintextToken), for the same reasons: no
/// `Clone`, a `Debug` that prints nothing, and a `Drop` that zeroizes. The
/// zeroize is best-effort and does not reach a copy the caller made -- a
/// formatted header, a log line, a `String` of its own.
#[non_exhaustive]
pub struct PlaintextRememberToken(String);

impl PlaintextRememberToken {
    /// Wrap a freshly minted plaintext.
    pub(crate) fn new(plaintext: String) -> Self {
        Self(plaintext)
    }

    /// The plaintext, for the one cookie that carries it to its owner.
    ///
    /// Named `expose` rather than `as_str` because every call site should read
    /// as a decision.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlaintextRememberToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PlaintextRememberToken([redacted])")
    }
}

impl Drop for PlaintextRememberToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

// ---------------------------------------------------------------------------
// IssuedRememberToken
// ---------------------------------------------------------------------------

/// What [`RememberTokens::issue`](super::RememberTokens::issue) returns.
///
/// The plaintext to set as a cookie, plus the two facts the `Set-Cookie`
/// header needs: who it is for, and when it stops working. The deadline is
/// here because the cookie's `Max-Age` should match the row's `expires_at` --
/// a cookie that outlives its row is a login that fails for no visible reason,
/// and a row that outlives its cookie is a credential nobody can see but the
/// database still honours.
#[derive(Debug)]
#[non_exhaustive]
pub struct IssuedRememberToken {
    subject: String,
    expires_at: DateTime<Utc>,
    plaintext: PlaintextRememberToken,
}

impl IssuedRememberToken {
    /// Pair a freshly written row with its plaintext.
    pub(crate) fn new(
        subject: String,
        expires_at: DateTime<Utc>,
        plaintext: PlaintextRememberToken,
    ) -> Self {
        Self {
            subject,
            expires_at,
            plaintext,
        }
    }

    /// Whoever the token signs in, in the application's own spelling.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// When the token stops working.
    #[must_use]
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// The plaintext, to be set as a cookie once.
    #[must_use]
    pub fn plaintext(&self) -> &PlaintextRememberToken {
        &self.plaintext
    }

    /// Split into the subject, the deadline, and the plaintext.
    #[must_use]
    pub fn into_parts(self) -> (String, DateTime<Utc>, PlaintextRememberToken) {
        (self.subject, self.expires_at, self.plaintext)
    }
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

/// Assemble the plaintext a caller sees: prefix, series, separator, secret.
pub(crate) fn format_plaintext(series: &[u8; SERIES_BYTES], secret: &[u8; SECRET_BYTES]) -> String {
    format!(
        "{REMEMBER_TOKEN_PREFIX}{}{SEPARATOR}{}",
        base64url::encode(series),
        base64url::encode(secret)
    )
}

/// Split a presented plaintext back into its series and its secret half.
///
/// Returns `None` for anything that is not exactly the shape
/// [`format_plaintext`] writes. A caller holding `None` was handed something
/// this crate never minted and can reject it without asking the database,
/// which matters more for this credential than for the others: a remember-me
/// cookie is presented on requests nobody authenticated, by clients that
/// include whatever their cookie jar happens to hold, so the unparseable case
/// is the *common* case rather than the attack.
///
/// The strictness is inherited from [`base64url::decode`], which refuses
/// padding, refuses a length no encoder could produce, and refuses
/// non-canonical trailing bits. That last one matters more here than it looks:
/// a lax decoder would give one stored row a family of distinct spellings of
/// its series, and the theft rule turns on whether a presented secret matches
/// *the* row -- one series with several spellings is several rows' worth of
/// guesses against one victim.
pub(crate) fn parse_plaintext(presented: &str) -> Option<(SeriesId, [u8; SECRET_BYTES])> {
    let (series_text, secret_text) = presented
        .strip_prefix(REMEMBER_TOKEN_PREFIX)?
        .split_once(SEPARATOR)?;

    let series: [u8; SERIES_BYTES] = base64url::decode(series_text)?.try_into().ok()?;
    let secret: [u8; SECRET_BYTES] = base64url::decode(secret_text)?.try_into().ok()?;

    Some((SeriesId(series), secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plaintext_carries_the_scanner_prefix_and_both_halves() {
        let plaintext = format_plaintext(&[0xab; SERIES_BYTES], &[0xcd; SECRET_BYTES]);
        assert!(plaintext.starts_with(REMEMBER_TOKEN_PREFIX));
        // prefix + 22 symbols + separator + 43 symbols
        assert_eq!(plaintext.len(), REMEMBER_TOKEN_PREFIX.len() + 22 + 1 + 43);
    }

    #[test]
    fn a_minted_plaintext_parses_back_to_what_went_in() {
        let series = [7u8; SERIES_BYTES];
        let secret = [9u8; SECRET_BYTES];
        let (parsed_series, parsed_secret) =
            parse_plaintext(&format_plaintext(&series, &secret)).expect("round trip");
        assert_eq!(parsed_series.as_bytes(), &series);
        assert_eq!(parsed_secret, secret);
    }

    #[test]
    fn the_whole_plaintext_is_a_legal_cookie_value_without_quoting() {
        // RFC 6265's `cookie-value` grammar excludes space, comma, semicolon,
        // backslash and double quote. Every character here has to be outside
        // that set, or the header has to be quoted -- and a quoted cookie is
        // where a framework that quotes on the way out and one that does not
        // unquote on the way in stop agreeing on what the token was.
        let plaintext = format_plaintext(&[0xffu8; SERIES_BYTES], &[0x00u8; SECRET_BYTES]);
        assert!(
            plaintext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')),
            "not a bare cookie value: {plaintext}"
        );
    }

    #[test]
    fn a_string_this_crate_never_minted_costs_no_query() {
        // Each of these fails in `parse_plaintext`, before the store has a
        // reason to touch the database. The first two are what an ordinary
        // browser sends: no cookie at all, and a cookie some other part of the
        // application set.
        for hostile in [
            "",
            "session=abc",
            "arcrmb_",
            "arcpwr_AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "arcrmb_AAAAAAAAAAAAAAAAAAAAAA",
            "arcrmb_AAAAAAAAAAAAAAAAAAAAAA.short",
            "arcrmb_short.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "arcrmb_AAAAAAAAAAAAAAAAAAAA==.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(
                parse_plaintext(hostile).is_none(),
                "accepted {hostile:?} as a token"
            );
        }
    }

    #[test]
    fn a_non_canonical_spelling_of_a_real_series_is_refused() {
        // 16 bytes encode to 22 symbols whose last symbol carries only four
        // meaningful bits. `A` and `B` differ in bits the encoder always
        // writes as zero, so exactly one of them is a spelling of that series
        // -- and the other must not be a second key to the same row. Here that
        // is not merely a duplicate lookup: a second spelling would be a
        // second series to present a wrong secret against, and every wrong
        // secret against a live series is a theft report.
        let good = format_plaintext(&[0u8; SERIES_BYTES], &[0u8; SECRET_BYTES]);
        assert!(parse_plaintext(&good).is_some());

        let series_end = REMEMBER_TOKEN_PREFIX.len() + 22;
        assert_eq!(&good[series_end - 1..series_end], "A");
        let smuggled = format!("{}B{}", &good[..series_end - 1], &good[series_end..]);
        assert_ne!(smuggled, good, "the test did not actually change anything");
        assert!(parse_plaintext(&smuggled).is_none());
    }

    #[test]
    fn a_redacted_debug_does_not_contain_the_secret() {
        let plaintext = PlaintextRememberToken::new("arcrmb_dead.beef".to_owned());
        assert!(!format!("{plaintext:?}").contains("beef"));
    }

    #[test]
    fn the_series_debug_is_not_the_plaintext() {
        // The series is not a secret, so it prints -- but it prints as a
        // series, not as something that could be pasted back in as a cookie.
        let plaintext = format_plaintext(&[0u8; SERIES_BYTES], &[0u8; SECRET_BYTES]);
        let (series, _) = parse_plaintext(&plaintext).expect("round trip");
        let rendered = format!("{series:?}");
        assert!(rendered.starts_with("SeriesId("));
        assert!(!rendered.contains(REMEMBER_TOKEN_PREFIX));
    }

    // Gated on the features that define the other two prefixes rather than
    // comparing against string literals copied in here. A literal would agree
    // with itself forever; this fails if somebody changes one of the real
    // constants.
    #[cfg(all(feature = "api-tokens", feature = "auth-reset"))]
    #[test]
    fn the_three_credential_prefixes_are_distinct() {
        // The scanner-rule argument in one assertion. If two of these ever
        // collide, a rule written for one fires on the other and the response
        // playbook is wrong.
        assert_ne!(REMEMBER_TOKEN_PREFIX, crate::tokens::TOKEN_PREFIX);
        assert_ne!(
            REMEMBER_TOKEN_PREFIX,
            super::super::super::RESET_TOKEN_PREFIX
        );
    }
}
