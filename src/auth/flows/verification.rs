//! Proving that somebody can read the address they signed up with.
//!
//! An email-verification link is a signed URL, so nothing about it is stored:
//! the link carries its own proof of origin and its own deadline, and a
//! deployment holding no rows can still refuse a forged one. What it adds on
//! top of [`UrlSigner`](crate::crypt::UrlSigner) is a *binding* to the address
//! the mail went to, which is the part that is easy to leave out and expensive
//! to leave out.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use secrecy::{ExposeSecret, SecretSlice};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::config::AppConfig;
use crate::crypt::{AppKey, Clock, SignedUrlError, UrlSigner};

/// Domain separator for the address-binding MAC.
///
/// The binding key is derived from [`AppKey`] under its own label, so it is
/// not the key the URL signature uses. Reusing one key for two purposes means
/// a weakness in either is a weakness in both, and there is no reason to
/// accept that when a subkey costs one HMAC at startup.
const BINDING_LABEL: &[u8] = b"arcature/auth-flows/email-binding";

/// The default prefix a verification link is built under.
const DEFAULT_PATH: &str = "/email/verify";

/// Everything outside RFC 3986's unreserved set is escaped in a path segment.
///
/// Deliberately wider than a path segment strictly requires -- `:` and `@` and
/// the sub-delimiters are legal there, but escaping them costs nothing and
/// means the segment cannot be mistaken for structure by anything that reads
/// the URL later. It is the same set [`UrlSigner`] escapes query components
/// with, for the same reason.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The default window a link stays valid for: one hour.
///
/// Long enough to survive a mail queue and somebody finishing their coffee,
/// short enough that a link sitting in an archived mailbox a year later is
/// not a live credential.
const DEFAULT_VALID_FOR: Duration = Duration::from_secs(60 * 60);

/// Why a verification link was not accepted.
///
/// Every variant means the address was **not** verified. There is no
/// partially-accepted outcome, and none of these should be shown to the
/// person clicking: "this link has expired, here is a fresh one" is the whole
/// of what a page needs to say.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmailVerificationError {
    /// The URL was not one this application signed, was edited after signing,
    /// or its deadline has passed. Carries the signer's own reason.
    Link(SignedUrlError),
    /// The signature was genuine, but the address on the account is no longer
    /// the address the link was minted for.
    ///
    /// This is the ordinary outcome of a person changing their address while
    /// an old verification mail is still in the inbox, and it is a refusal
    /// rather than an error in the application.
    AddressChanged,
}

impl fmt::Display for EmailVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link(source) => write!(formatter, "verification link: {source}"),
            Self::AddressChanged => formatter.write_str(
                "the verification link was minted for a different address than the account now \
                 has",
            ),
        }
    }
}

impl std::error::Error for EmailVerificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Link(source) => Some(source),
            Self::AddressChanged => None,
        }
    }
}

impl From<SignedUrlError> for EmailVerificationError {
    fn from(source: SignedUrlError) -> Self {
        Self::Link(source)
    }
}

/// Mints and checks email-verification links.
///
/// # The binding, and why a signed URL alone is not enough
///
/// A link that says only "this user is verified" verifies whatever address
/// the account happens to hold when the link is clicked. That is a real
/// account-takeover path and it needs no forgery at all:
///
/// 1. Register as `attacker@evil.test`. A verification mail arrives; keep the
///    link, do not click it.
/// 2. Change the account's address to `victim@bank.test`.
/// 3. Click the link kept from step 1.
///
/// Every step is legitimate use of the application, the signature is genuine
/// and unexpired, and the account ends up holding a verified address that the
/// attacker never proved they could read -- which is exactly the claim
/// verification exists to make.
///
/// So the link carries a *binding*: a MAC over the user key and the address
/// the mail was sent to. [`confirm`](Self::confirm) recomputes it from the
/// address the account holds **now** and compares in constant time. Step 3
/// then fails with [`AddressChanged`](EmailVerificationError::AddressChanged),
/// and changing the address invalidates every link outstanding against the old
/// one without any row being deleted.
///
/// The binding is a MAC rather than a bare hash on purpose. A plain
/// `sha256(address)` in a URL is a guessable address: the space of real email
/// addresses is small enough to enumerate offline, and links leak -- into
/// access logs, `Referer` headers, browser history, and screenshots. Keyed, it
/// discloses nothing to anybody without the application key.
///
/// # A signed link is replayable until it expires
///
/// Nothing here is stored, so nothing here can be spent. Clicking the same
/// link twice verifies twice, and a link that leaks is usable by whoever holds
/// it until its deadline passes. That is the trade for a stateless mechanism,
/// and it is acceptable *for this one purpose* because the second use asserts
/// what the first already did.
///
/// It is not acceptable for anything whose second use is a fresh grant. A
/// password-reset link must be single-use, and single-use needs a row to
/// spend -- so do not reach for this type to build one.
///
/// # Example
///
/// ```
/// use arcature::auth::flows::EmailVerification;
/// use arcature::config::AppConfig;
/// use arcature::crypt::AppKey;
///
/// let key = AppKey::from_hex(&"4a".repeat(64))?;
/// let config = AppConfig::new().url("https://acme.test");
/// let flow = EmailVerification::new(&key, &config);
///
/// // Mailed to the address on the account.
/// let link = flow.link("user:42", "ada@example.com")?;
/// assert!(link.starts_with("https://acme.test/email/verify/"));
///
/// // The handler receives the binding segment from its own route -- axum's
/// // `Path` extractor hands it over already decoded -- and the address from
/// // the row it just loaded. Taken apart by hand here for the sake of the
/// // example.
/// let binding = link
///     .split('?')
///     .next()
///     .and_then(|path| path.rsplit('/').next())
///     .expect("the link ends in the binding segment");
///
/// flow.confirm(&link, "user:42", binding, "ada@example.com")?;
///
/// // The same link against an address the account no longer has: refused.
/// assert!(flow.confirm(&link, "user:42", binding, "new@example.com").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct EmailVerification {
    signer: UrlSigner,
    binding_key: SecretSlice<u8>,
    path: String,
    valid_for: Duration,
}

impl EmailVerification {
    /// Build a flow signing under `key` and pointing at `config`'s base URL.
    ///
    /// The default link is `/email/verify/{user}/{binding}` and stays valid
    /// for one hour.
    #[must_use]
    pub fn new(key: &AppKey, config: &AppConfig) -> Self {
        Self {
            signer: UrlSigner::new(key, config),
            binding_key: key.subkey(BINDING_LABEL),
            path: DEFAULT_PATH.to_owned(),
            valid_for: DEFAULT_VALID_FOR,
        }
    }

    /// Change the path prefix links are built under.
    ///
    /// The user key and the binding are appended as two further segments, so a
    /// prefix of `/confirm` produces `/confirm/{user}/{binding}` and the route
    /// that receives it takes two path parameters.
    #[must_use]
    pub fn path(mut self, prefix: impl Into<String>) -> Self {
        self.path = prefix.into();
        self
    }

    /// Change how long a freshly minted link stays valid.
    #[must_use]
    pub fn valid_for(mut self, valid_for: Duration) -> Self {
        self.valid_for = valid_for;
        self
    }

    /// Replace the clock the deadline is measured against, for tests.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.signer = self.signer.with_clock(clock);
        self
    }

    /// Mint a link proving whoever reads `email` controls it.
    ///
    /// `user_key` identifies the account to the application: whatever the
    /// handler will look a row up by. It appears in the link, so it should be
    /// an identifier rather than a secret -- a row id or an opaque key, not a
    /// session token.
    ///
    /// # Errors
    ///
    /// Returns [`EmailVerificationError::Link`] if the path cannot be signed:
    /// see [`SignedUrlError`]. In practice that means the configured prefix
    /// carries a `?` or a `#`.
    pub fn link(&self, user_key: &str, email: &str) -> Result<String, EmailVerificationError> {
        let binding = self.binding_of(user_key, email);
        let path = format!(
            "{}/{}/{}",
            self.path.trim_end_matches('/'),
            encode_segment(user_key),
            binding
        );
        Ok(self.signer.sign_temporary(&path, &[], self.valid_for)?)
    }

    /// Check a link against the address the account holds now.
    ///
    /// The three checks run in this order, and the order matters: the
    /// signature first, so a tampered link reports tampering whatever else is
    /// wrong with it; then the deadline; then the binding. The first two are
    /// [`UrlSigner::verify`]'s, and the third is this type's reason to exist.
    ///
    /// `user_key` and `binding` are the two path segments the route matched --
    /// take them from the request's path parameters, decoded, rather than
    /// re-parsing `url`. They are covered by the signature, so a request that
    /// reaches the handler with either of them altered fails the first check.
    ///
    /// `current_email` is the address on the account as it stands, read from
    /// the row `user_key` identified. It must be the same string that was
    /// passed to [`link`](Self::link); an application that normalises
    /// addresses on write should normalise before minting the link too, or
    /// every link it sends will fail this check.
    ///
    /// # Errors
    ///
    /// - [`EmailVerificationError::Link`] -- forged, edited, or expired.
    /// - [`EmailVerificationError::AddressChanged`] -- genuine and unexpired,
    ///   but minted against a different address.
    pub fn confirm(
        &self,
        url: &str,
        user_key: &str,
        binding: &str,
        current_email: &str,
    ) -> Result<(), EmailVerificationError> {
        self.signer.verify(url)?;

        let expected = self.binding_of(user_key, current_email);
        // Constant-time, like every other secret comparison in this crate. The
        // binding is a MAC, and a `==` on a MAC leaks it a byte at a time to
        // anybody who can measure the answer.
        if !bool::from(binding.as_bytes().ct_eq(expected.as_bytes())) {
            return Err(EmailVerificationError::AddressChanged);
        }

        Ok(())
    }

    /// The binding segment for one (user, address) pair.
    fn binding_of(&self, user_key: &str, email: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.binding_key.expose_secret())
            .expect("HMAC-SHA256 accepts a key of any length");
        // Length-prefixed, so no two pairs can produce one input: without it
        // ("ab", "c") and ("a", "bc") are the same bytes, and an application
        // whose user keys are addresses would be minting links that verify
        // each other.
        feed(&mut mac, user_key.as_bytes());
        feed(&mut mac, email.as_bytes());
        crate::crypt::base64url::encode(&mac.finalize().into_bytes())
    }
}

impl fmt::Debug for EmailVerification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No key material, derived or otherwise. `UrlSigner` is not `Debug`
        // for the same reason, so there is nothing to delegate to either.
        formatter
            .debug_struct("EmailVerification")
            .field("path", &self.path)
            .field("valid_for", &self.valid_for)
            .finish_non_exhaustive()
    }
}

/// Absorb one length-prefixed field into a MAC.
fn feed(mac: &mut Hmac<Sha256>, bytes: &[u8]) {
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}

/// Percent-encode one path segment.
///
/// `/` is escaped along with everything else outside the unreserved set: a
/// user key holding a slash would otherwise add a path segment and change
/// which route the link matches.
fn encode_segment(segment: &str) -> String {
    utf8_percent_encode(segment, UNRESERVED).to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{EmailVerification, EmailVerificationError, encode_segment};
    use crate::config::AppConfig;
    use crate::crypt::{AppKey, Clock, SignedUrlError};

    struct Frozen(u64);
    impl Clock for Frozen {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    fn key() -> AppKey {
        AppKey::from_hex(&"4a".repeat(64)).expect("valid key")
    }

    fn flow() -> EmailVerification {
        EmailVerification::new(&key(), &AppConfig::new().url("https://acme.test"))
    }

    /// The binding is the last path segment, which is how a route with two
    /// path parameters would receive it.
    fn binding_of(link: &str) -> &str {
        link.split('?')
            .next()
            .and_then(|path| path.rsplit('/').next())
            .expect("a link has a last segment")
    }

    #[test]
    fn a_fresh_link_confirms_the_address_it_was_minted_for() {
        let flow = flow();
        let link = flow.link("user:42", "ada@example.com").expect("sign");
        assert_eq!(
            flow.confirm(&link, "user:42", binding_of(&link), "ada@example.com"),
            Ok(())
        );
    }

    /// The attack the binding exists to stop: keep the link, change the
    /// address, click. The signature is genuine throughout.
    #[test]
    fn a_link_kept_across_an_address_change_no_longer_confirms() {
        let flow = flow();
        let link = flow.link("user:42", "attacker@evil.test").expect("sign");

        assert_eq!(
            flow.confirm(&link, "user:42", binding_of(&link), "victim@bank.test"),
            Err(EmailVerificationError::AddressChanged),
            "a link minted for one address must not verify another"
        );
    }

    /// One person's link must not verify another person's account, even at
    /// the same address.
    #[test]
    fn a_link_does_not_confirm_a_different_user() {
        let flow = flow();
        let link = flow.link("user:42", "ada@example.com").expect("sign");
        assert_eq!(
            flow.confirm(&link, "user:99", binding_of(&link), "ada@example.com"),
            Err(EmailVerificationError::AddressChanged)
        );
    }

    #[test]
    fn an_edited_link_is_refused_before_the_binding_is_looked_at() {
        let flow = flow();
        let link = flow.link("user:42", "ada@example.com").expect("sign");
        let binding = binding_of(&link).to_owned();

        // Flip the *first* character of the signature body, not the last.
        // A 32-byte MAC is 43 base64url characters, so the final group holds
        // three of them and the last one carries two padding bits the decoder
        // requires to be zero -- only sixteen of the sixty-four alphabet
        // characters are legal in that position. Substituting an arbitrary one
        // is rejected as `Malformed` before the signature is ever compared,
        // which is the decoder doing its job and not what this test is about.
        // The expiry rides inside the signed input, so the signature differs
        // every second: editing the tail failed roughly one run in sixteen,
        // whenever the genuine last character happened to be `A`. The first
        // character sits at the top of a full group, where all sixty-four are
        // legal and any change is a different MAC.
        let marker = "signature=v1.";
        let body = link.find(marker).expect("a signed link") + marker.len();
        let flipped = if link[body..].starts_with('A') { 'B' } else { 'A' };
        let tampered = format!("{}{flipped}{}", &link[..body], &link[body + 1..]);

        assert_eq!(
            flow.confirm(&tampered, "user:42", &binding, "ada@example.com"),
            Err(EmailVerificationError::Link(SignedUrlError::Mismatch)),
            "tampering must be reported as tampering, not as a changed address"
        );
    }

    #[test]
    fn a_link_stops_confirming_after_its_deadline() {
        let minted = EmailVerification::new(&key(), &AppConfig::new().url("https://acme.test"))
            .valid_for(Duration::from_secs(600))
            .with_clock(Arc::new(Frozen(1_000)));
        let link = minted.link("user:42", "ada@example.com").expect("sign");
        let binding = binding_of(&link).to_owned();

        let at_deadline = flow().with_clock(Arc::new(Frozen(1_600)));
        assert_eq!(
            at_deadline.confirm(&link, "user:42", &binding, "ada@example.com"),
            Ok(()),
            "a link is valid up to and including its expiry second"
        );

        let after = flow().with_clock(Arc::new(Frozen(1_601)));
        assert_eq!(
            after.confirm(&link, "user:42", &binding, "ada@example.com"),
            Err(EmailVerificationError::Link(SignedUrlError::Expired))
        );
    }

    /// A link signed under one application key must not verify under another.
    #[test]
    fn a_link_from_another_deployment_is_refused() {
        let ours = flow();
        let theirs = EmailVerification::new(
            &AppKey::from_hex(&"7c".repeat(64)).expect("valid key"),
            &AppConfig::new().url("https://acme.test"),
        );

        let link = theirs.link("user:42", "ada@example.com").expect("sign");
        assert!(
            ours.confirm(&link, "user:42", binding_of(&link), "ada@example.com")
                .is_err()
        );
    }

    /// The binding is keyed, so two deployments minting for the same person at
    /// the same address must not produce the same segment.
    #[test]
    fn the_binding_is_keyed_and_not_a_bare_hash_of_the_address() {
        let ours = flow();
        let theirs = EmailVerification::new(
            &AppKey::from_hex(&"7c".repeat(64)).expect("valid key"),
            &AppConfig::new().url("https://acme.test"),
        );

        let a = ours.link("user:42", "ada@example.com").expect("sign");
        let b = theirs.link("user:42", "ada@example.com").expect("sign");
        assert_ne!(binding_of(&a), binding_of(&b));
    }

    /// Length prefixing: ("ab", "c") and ("a", "bc") must not collide. An
    /// application whose user keys are addresses would otherwise mint links
    /// that verify each other.
    #[test]
    fn the_user_key_and_the_address_cannot_be_slid_across_each_other() {
        let flow = flow();
        let a = flow.link("ab", "c").expect("sign");
        let b = flow.link("a", "bc").expect("sign");
        assert_ne!(binding_of(&a), binding_of(&b));
    }

    /// A user key holding a slash must not add a path segment, which would
    /// change the route the link matches.
    #[test]
    fn a_user_key_cannot_smuggle_a_path_segment() {
        assert_eq!(encode_segment("user/42"), "user%2F42");
        assert_eq!(encode_segment("user:42"), "user%3A42");
        assert_eq!(encode_segment("a.b-c_d~e"), "a.b-c_d~e");

        let flow = flow();
        let link = flow.link("../../admin", "ada@example.com").expect("sign");
        assert!(!link.contains("../"), "{link}");
    }

    #[test]
    fn debug_does_not_print_key_material() {
        let rendered = format!("{:?}", flow());
        assert!(rendered.contains("/email/verify"), "{rendered}");
        assert!(!rendered.to_lowercase().contains("key"), "{rendered}");
    }
}
