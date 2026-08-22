//! The token value types: what a token is, what it may do, and what the
//! caller gets back exactly once.

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use zeroize::Zeroize;

/// How many bytes of the public half of a token.
pub(crate) const ID_BYTES: usize = 16;

/// How many bytes of the secret half of a token.
///
/// 256 bits of uniform randomness. This number is the entire reason the
/// stored digest is a fast hash rather than a slow one; see the comment at
/// the hashing site in [`super::store`].
pub(crate) const SECRET_BYTES: usize = 32;

/// The fixed opening of every token this crate mints.
///
/// A leaked credential is found by whoever greps for it first. A distinctive
/// literal prefix is what lets a secret scanner -- a pre-commit hook, a CI
/// step, a log pipeline -- recognise an Arcature token in a paste, a diff, or
/// a bug report without knowing anything else about the application. It is
/// not a security control by itself; it is what makes one possible.
///
/// ```
/// use arcature::tokens::TOKEN_PREFIX;
///
/// // A scanner rule is one literal.
/// assert_eq!(TOKEN_PREFIX, "arcpat_");
/// ```
pub const TOKEN_PREFIX: &str = "arcpat_";

/// The lowercase hex alphabet.
const HEX: [u8; 16] = *b"0123456789abcdef";

/// Encode bytes as lowercase hex.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Decode lowercase-or-uppercase hex into `out`, which must be exactly half
/// the length of `text`. Returns `false` if it is not, or if any character is
/// not a hex digit.
pub(crate) fn hex_decode(text: &str, out: &mut [u8]) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != out.len() * 2 {
        return false;
    }
    // The length check above makes the remainder empty by construction.
    let (pairs, _) = bytes.as_chunks::<2>();
    for (slot, pair) in out.iter_mut().zip(pairs) {
        let (Some(high), Some(low)) = (nibble(pair[0]), nibble(pair[1])) else {
            return false;
        };
        *slot = (high << 4) | low;
    }
    true
}

/// One hex digit as a nibble, or `None`.
fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ApiTokenId
// ---------------------------------------------------------------------------

/// The public half of a token: the 16 bytes a row is looked up by.
///
/// This is **not** a secret. It travels in the `Authorization` header in the
/// clear next to the secret half, it is what an index seeks on, and it is
/// safe to log, to show in a token-management UI, and to accept from a
/// revocation request. Splitting a token into a public locator and a secret
/// proof is what lets the secret be compared in constant time against exactly
/// one stored digest rather than searched for.
///
/// ```
/// use arcature::tokens::ApiTokenId;
///
/// let id = ApiTokenId::from_hex("0123456789abcdef0123456789abcdef").unwrap();
/// assert_eq!(id.to_hex(), "0123456789abcdef0123456789abcdef");
/// assert_eq!(id.as_bytes().len(), 16);
///
/// // Anything that is not exactly 32 hex digits is not an id.
/// assert!(ApiTokenId::from_hex("abc").is_none());
/// assert!(ApiTokenId::from_hex("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ApiTokenId([u8; ID_BYTES]);

impl ApiTokenId {
    /// Build an id from its 16 raw bytes.
    pub(crate) fn from_bytes(bytes: [u8; ID_BYTES]) -> Self {
        Self(bytes)
    }

    /// Parse an id from its 32-character hex spelling, or `None`.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let mut bytes = [0u8; ID_BYTES];
        hex_decode(text, &mut bytes).then_some(Self(bytes))
    }

    /// The id as 32 lowercase hex characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }

    /// The id's raw bytes, as the column stores them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for ApiTokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ApiTokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApiTokenId({})", self.to_hex())
    }
}

// ---------------------------------------------------------------------------
// Abilities
// ---------------------------------------------------------------------------

/// What a token is allowed to do.
///
/// A list of opaque strings the application chooses -- `"posts:read"`,
/// `"billing:write"`, whatever vocabulary it already has -- plus one reserved
/// spelling, [`Abilities::ALL`] (`"*"`), which matches everything.
///
/// Matching is exact, never prefix or glob. `"posts:*"` is a perfectly legal
/// ability string and it grants exactly the ability spelled `"posts:*"`; it
/// does not grant `"posts:read"`. Inventing a wildcard grammar here would
/// mean every application's authorization decisions depend on this crate's
/// pattern matcher agreeing with the application's intuition, and that is not
/// a bet worth taking in an authorization path.
///
/// ```
/// use arcature::tokens::Abilities;
///
/// let scoped = Abilities::of(["posts:read", "posts:write"]);
/// assert!(scoped.contains("posts:read"));
/// assert!(!scoped.contains("billing:write"));
/// assert!(!scoped.is_all());
///
/// // The one wildcard, and the only one.
/// let root = Abilities::all();
/// assert!(root.contains("anything at all"));
/// assert!(root.is_all());
///
/// // No prefix matching: `posts:*` grants `posts:*` and nothing else.
/// let literal = Abilities::of(["posts:*"]);
/// assert!(literal.contains("posts:*"));
/// assert!(!literal.contains("posts:read"));
///
/// // A token can be minted with no abilities at all; it can then do nothing.
/// assert!(!Abilities::none().contains("posts:read"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Abilities {
    entries: Vec<String>,
}

impl Abilities {
    /// The reserved ability that matches every other one.
    pub const ALL: &'static str = "*";

    /// A token that may do anything.
    #[must_use]
    pub fn all() -> Self {
        Self {
            entries: vec![Self::ALL.to_owned()],
        }
    }

    /// A token that may do nothing.
    #[must_use]
    pub fn none() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// A token scoped to the given abilities.
    #[must_use]
    pub fn of<I, S>(abilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            entries: abilities.into_iter().map(Into::into).collect(),
        }
    }

    /// Add one ability.
    #[must_use]
    pub fn with(mut self, ability: impl Into<String>) -> Self {
        self.entries.push(ability.into());
        self
    }

    /// Whether this set grants `ability`.
    #[must_use]
    pub fn contains(&self, ability: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry == Self::ALL || entry == ability)
    }

    /// Whether this set is the unrestricted one.
    #[must_use]
    pub fn is_all(&self) -> bool {
        self.entries.iter().any(|entry| entry == Self::ALL)
    }

    /// The abilities as stored.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// PlaintextToken
// ---------------------------------------------------------------------------

/// The one and only copy of a token's plaintext.
///
/// Handed back by [`ApiTokens::issue`](super::ApiTokens::issue) and never
/// again: the database holds a digest of the secret half, and a digest does
/// not run backwards. If the caller loses this value the only remedy is to
/// issue another token.
///
/// Three properties are deliberate:
///
/// * **No `Clone`.** A credential that is trivially copied is a credential
///   with an unknown number of copies.
/// * **`Debug` prints nothing.** The overwhelmingly common way a secret
///   reaches a log file is a struct that derived `Debug` three types up.
/// * **The bytes are zeroized on drop**, so the plaintext does not linger in
///   freed heap for a core dump or a later allocation to find. This is a
///   best-effort measure, not a guarantee: anything the caller copies the
///   string into -- a response body, a format argument, a `String` of its own
///   -- is outside this type's reach.
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::tokens::{Abilities, ApiTokens, NewApiToken};
/// use std::time::Duration;
///
/// # async fn example(tokens: ApiTokens) -> Result<(), Box<dyn std::error::Error>> {
/// let issued = tokens
///     .issue(&NewApiToken::expiring_in("user:42", "CI deploy key", Duration::from_secs(86_400))
///         .abilities(Abilities::of(["deploy:write"])))
///     .await?;
///
/// // Show it once. There is no second chance.
/// println!("{}", issued.plaintext().expose());
///
/// // And it is not in the Debug output.
/// assert_eq!(format!("{:?}", issued.plaintext()), "PlaintextToken([redacted])");
/// # Ok(())
/// # }
/// ```
#[non_exhaustive]
pub struct PlaintextToken(String);

impl PlaintextToken {
    /// Wrap a freshly minted plaintext.
    pub(crate) fn new(plaintext: String) -> Self {
        Self(plaintext)
    }

    /// The plaintext, for the one response that carries it to its owner.
    ///
    /// Named `expose` rather than `as_str` because every call site should
    /// read as a decision.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlaintextToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaintextToken([redacted])")
    }
}

impl Drop for PlaintextToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Assemble the plaintext a caller sees: prefix, public id, separator, secret.
///
/// The separator is `_`, which is not in the hex alphabet, so the split is
/// unambiguous without a length assumption.
pub(crate) fn format_plaintext(id: &[u8; ID_BYTES], secret: &[u8; SECRET_BYTES]) -> String {
    format!("{TOKEN_PREFIX}{}_{}", hex_encode(id), hex_encode(secret))
}

/// Split a presented plaintext back into its public id and its secret half.
///
/// Returns `None` for anything that is not exactly the shape
/// [`format_plaintext`] writes: the prefix, sixteen bytes of hex, the
/// separator, thirty-two bytes of hex, and nothing after. A caller holding
/// `None` was handed something this crate never minted and can reject it
/// without asking the database, which is the point -- a malformed string
/// should not cost a query.
///
/// This is deliberately strict about length. `hex_decode` refuses input whose
/// length does not match the buffer exactly, so a truncated or padded id
/// fails here rather than silently decoding to a different token.
pub(crate) fn parse_plaintext(presented: &str) -> Option<(ApiTokenId, [u8; SECRET_BYTES])> {
    let (id_hex, secret_hex) = presented.strip_prefix(TOKEN_PREFIX)?.split_once('_')?;

    let mut id = [0u8; ID_BYTES];
    let mut secret = [0u8; SECRET_BYTES];
    if !hex_decode(id_hex, &mut id) || !hex_decode(secret_hex, &mut secret) {
        return None;
    }

    Some((ApiTokenId(id), secret))
}

// ---------------------------------------------------------------------------
// ApiToken
// ---------------------------------------------------------------------------

/// A token as the database holds it -- everything except the secret.
///
/// There is no accessor for the secret half because there is nothing to
/// accessor: the row holds a digest, and this type never sees even that.
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::tokens::{ApiTokenId, ApiTokens};
///
/// # async fn example(tokens: ApiTokens, id: ApiTokenId) -> Result<(), Box<dyn std::error::Error>> {
/// if let Some(token) = tokens.find(id).await? {
///     assert_eq!(token.id(), id);
///     if token.can("posts:write") {
///         println!("{} may publish", token.name());
///     }
///     println!("issued {}, expires {}", token.created_at(), token.expires_at());
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ApiToken {
    id: ApiTokenId,
    tokenable_id: String,
    name: String,
    abilities: Abilities,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

impl ApiToken {
    /// Build a token from a row. Crate-internal: the only honest source of an
    /// `ApiToken` is the table.
    pub(crate) fn from_row(
        id: ApiTokenId,
        tokenable_id: String,
        name: String,
        abilities: Abilities,
        expires_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            tokenable_id,
            name,
            abilities,
            expires_at,
            created_at,
        }
    }

    /// The public half of the token.
    #[must_use]
    pub fn id(&self) -> ApiTokenId {
        self.id
    }

    /// Whoever the token acts for, in the application's own spelling.
    #[must_use]
    pub fn tokenable_id(&self) -> &str {
        &self.tokenable_id
    }

    /// The human label the token was minted with.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the token may do.
    #[must_use]
    pub fn abilities(&self) -> &Abilities {
        &self.abilities
    }

    /// Whether the token grants `ability`. See [`Abilities`] for the exact
    /// matching rule -- it is exact, never a glob.
    #[must_use]
    pub fn can(&self, ability: &str) -> bool {
        self.abilities.contains(ability)
    }

    /// When the token stops working. Always in the future for a token that
    /// came back from a query: expiry is in the `WHERE` clause, not a check
    /// the caller makes afterwards.
    #[must_use]
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// When the token was minted.
    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

// ---------------------------------------------------------------------------
// NewApiToken
// ---------------------------------------------------------------------------

/// A request to mint a token.
///
/// Every token has a deadline: there is no constructor that omits one, and
/// the column has no null state to hold one. A credential that outlives the
/// reason it was minted is the ordinary way a leak stays useful, so "forever"
/// has to be typed out as a date somebody chose.
///
/// ```
/// use arcature::tokens::{Abilities, NewApiToken};
/// use chrono::{Duration, Utc};
/// use std::time::Duration as StdDuration;
///
/// // Either spell the deadline...
/// let explicit = NewApiToken::new("user:42", "laptop", Utc::now() + Duration::days(30));
/// assert_eq!(explicit.tokenable_id(), "user:42");
/// assert_eq!(explicit.name(), "laptop");
///
/// // ...or the time-to-live.
/// let ttl = NewApiToken::expiring_in("user:42", "CI", StdDuration::from_secs(3600))
///     .abilities(Abilities::of(["deploy:write"]))
///     .ability("deploy:read");
/// // `abilities` is the builder's setter, so reading them back is
/// // `abilities_ref`. A record gives; a builder takes.
/// assert!(ttl.abilities_ref().contains("deploy:read"));
/// assert!(ttl.expires_at() > Utc::now());
///
/// // A token with no abilities named can do nothing, which is the default.
/// assert!(!explicit.abilities_ref().contains("posts:read"));
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NewApiToken {
    tokenable_id: String,
    name: String,
    abilities: Abilities,
    expires_at: DateTime<Utc>,
}

impl NewApiToken {
    /// A token for `tokenable_id`, labelled `name`, dying at `expires_at`.
    ///
    /// `tokenable_id` is whatever the application calls the subject -- a user
    /// id, a tenant id, a service name. It is stored as text so this crate
    /// never has an opinion about the shape of an application's primary key.
    #[must_use]
    pub fn new(
        tokenable_id: impl Into<String>,
        name: impl Into<String>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            tokenable_id: tokenable_id.into(),
            name: name.into(),
            abilities: Abilities::none(),
            expires_at,
        }
    }

    /// A token dying `ttl` from now.
    ///
    /// A `ttl` too large for the calendar saturates at the furthest instant
    /// `chrono` can represent rather than wrapping into the past, because
    /// wrapping would silently mint a token that is already dead.
    #[must_use]
    pub fn expiring_in(
        tokenable_id: impl Into<String>,
        name: impl Into<String>,
        ttl: std::time::Duration,
    ) -> Self {
        let delta = Duration::from_std(ttl).unwrap_or(Duration::MAX);
        let expires_at = Utc::now()
            .checked_add_signed(delta)
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        Self::new(tokenable_id, name, expires_at)
    }

    /// Replace the ability set.
    #[must_use]
    pub fn abilities(mut self, abilities: Abilities) -> Self {
        self.abilities = abilities;
        self
    }

    /// Add one ability to the set.
    #[must_use]
    pub fn ability(mut self, ability: impl Into<String>) -> Self {
        self.abilities = std::mem::take(&mut self.abilities).with(ability);
        self
    }

    /// The subject this token will act for.
    #[must_use]
    pub fn tokenable_id(&self) -> &str {
        &self.tokenable_id
    }

    /// The human label.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ability set as configured so far.
    #[must_use]
    pub fn abilities_ref(&self) -> &Abilities {
        &self.abilities
    }

    /// The deadline.
    #[must_use]
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }
}

// ---------------------------------------------------------------------------
// IssuedApiToken
// ---------------------------------------------------------------------------

/// What [`ApiTokens::issue`](super::ApiTokens::issue) returns: the stored
/// record, and the plaintext that will never be available again.
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::tokens::{ApiTokens, NewApiToken};
/// use std::time::Duration;
///
/// # async fn example(tokens: ApiTokens) -> Result<(), Box<dyn std::error::Error>> {
/// let issued = tokens
///     .issue(&NewApiToken::expiring_in("user:42", "laptop", Duration::from_secs(86_400)))
///     .await?;
///
/// let (record, plaintext) = issued.into_parts();
/// println!("token {} for {}", record.id(), record.tokenable_id());
/// println!("show this once: {}", plaintext.expose());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct IssuedApiToken {
    token: ApiToken,
    plaintext: PlaintextToken,
}

impl IssuedApiToken {
    /// Pair a freshly written row with its plaintext.
    pub(crate) fn new(token: ApiToken, plaintext: PlaintextToken) -> Self {
        Self { token, plaintext }
    }

    /// The stored record.
    #[must_use]
    pub fn token(&self) -> &ApiToken {
        &self.token
    }

    /// The plaintext, to be shown once.
    #[must_use]
    pub fn plaintext(&self) -> &PlaintextToken {
        &self.plaintext
    }

    /// Split into the record and the plaintext.
    #[must_use]
    pub fn into_parts(self) -> (ApiToken, PlaintextToken) {
        (self.token, self.plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0x00u8, 0x0f, 0xf0, 0xff, 0x7a];
        let text = hex_encode(&bytes);
        assert_eq!(text, "000ff0ff7a");
        let mut back = [0u8; 5];
        assert!(hex_decode(&text, &mut back));
        assert_eq!(back, bytes);
    }

    #[test]
    fn hex_decode_rejects_the_wrong_length() {
        let mut out = [0u8; 4];
        assert!(!hex_decode("0011", &mut out));
        assert!(!hex_decode("001122334455", &mut out));
    }

    #[test]
    fn hex_decode_rejects_a_non_hex_character() {
        let mut out = [0u8; 2];
        assert!(!hex_decode("00zz", &mut out));
    }

    #[test]
    fn the_plaintext_carries_the_scanner_prefix_and_both_halves() {
        let plaintext = format_plaintext(&[0xab; ID_BYTES], &[0xcd; SECRET_BYTES]);
        assert!(plaintext.starts_with(TOKEN_PREFIX));
        // prefix + 32 hex + separator + 64 hex
        assert_eq!(plaintext.len(), TOKEN_PREFIX.len() + 32 + 1 + 64);
        assert!(plaintext.contains("_abababababababababababababababab_"));
    }

    #[test]
    fn the_wildcard_ability_is_the_only_one_that_is_not_literal() {
        let abilities = Abilities::of(["a:b", "*x"]);
        assert!(abilities.contains("a:b"));
        assert!(abilities.contains("*x"));
        // `*x` is not the wildcard; only a bare `*` is.
        assert!(!abilities.contains("anything"));
        assert!(!abilities.is_all());
    }

    #[test]
    fn a_redacted_debug_does_not_contain_the_secret() {
        let token = PlaintextToken::new("arcpat_dead_beef".to_owned());
        assert!(!format!("{token:?}").contains("beef"));
    }

    #[test]
    fn an_absurd_ttl_saturates_into_the_future_rather_than_wrapping() {
        let request = NewApiToken::expiring_in("u", "n", std::time::Duration::from_secs(u64::MAX));
        assert!(request.expires_at() > Utc::now());
    }

    #[test]
    fn an_id_round_trips_through_hex() {
        let id = ApiTokenId::from_bytes([7u8; ID_BYTES]);
        assert_eq!(ApiTokenId::from_hex(&id.to_hex()), Some(id));
    }
}
