//! What a notification looks like once it is a row: its id, and the record an
//! inbox read hands back.

use std::fmt;

use chrono::{DateTime, Utc};

/// How many bytes of notification id.
pub(crate) const ID_BYTES: usize = 16;

/// The lowercase hex alphabet.
const HEX: [u8; 16] = *b"0123456789abcdef";

/// Encode bytes as lowercase hex.
///
/// A private copy, as in [`crate::auth::csrf`], [`crate::oauth::pkce`],
/// [`crate::observe::trace_context`], and [`crate::tokens`]. Each of those
/// lives behind a different feature gate, so a shared helper would either
/// force one of them on or need a gate of its own that is true when any of
/// them is -- a condition that has to be edited every time a feature is
/// added. Ten lines duplicated is cheaper than that.
fn hex_encode(bytes: &[u8]) -> String {
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
fn hex_decode(text: &str, out: &mut [u8]) -> bool {
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
// NotificationId
// ---------------------------------------------------------------------------

/// The identity of one stored notification: 16 random bytes.
///
/// Random rather than sequential, and that is the whole point of the choice.
/// A notification id appears in the URL a "mark as read" or "dismiss" button
/// posts to, so a sequential id would let anyone holding one guess the ones
/// on either side of it. Guessing them still gets nobody anywhere -- every
/// statement the store issues is scoped by the recipient as well -- but an id
/// that cannot be guessed means the two defences are independent rather than
/// one defence written twice.
///
/// ```
/// use arcature::notifications::NotificationId;
///
/// let id = NotificationId::from_hex("0123456789abcdef0123456789abcdef").unwrap();
/// assert_eq!(id.to_hex(), "0123456789abcdef0123456789abcdef");
/// assert_eq!(id.as_bytes().len(), 16);
///
/// // Anything that is not exactly 32 hex digits is not an id.
/// assert!(NotificationId::from_hex("abc").is_none());
/// assert!(NotificationId::from_hex("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct NotificationId([u8; ID_BYTES]);

impl NotificationId {
    /// Build an id from its 16 raw bytes.
    pub(crate) fn from_bytes(bytes: [u8; ID_BYTES]) -> Self {
        Self(bytes)
    }

    /// Parse an id from its 32-character hex spelling, or `None`.
    ///
    /// This is the one a handler calls: an id arrives from a route parameter
    /// as text, and everything after this point holds bytes that were
    /// definitely 32 hex digits.
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

impl fmt::Display for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NotificationId({})", self.to_hex())
    }
}

// ---------------------------------------------------------------------------
// StoredNotification
// ---------------------------------------------------------------------------

/// One row of somebody's inbox.
///
/// The payload is a [`serde_json::Value`] rather than a typed struct because
/// an inbox is heterogeneous by nature: the rows a single query returns were
/// written by different notifications with different shapes, and a list that
/// could only hold one shape would not be an inbox. Naming the shape is what
/// [`kind`](Self::kind) is for, and deserialising into a typed struct is the
/// application's call, once it has matched on the kind.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoredNotification {
    /// This row's id.
    pub(crate) id: NotificationId,
    /// Whose inbox it is in.
    pub(crate) notifiable_key: String,
    /// The application's own name for what this notification is.
    pub(crate) kind: String,
    /// The payload, as the notification rendered it.
    pub(crate) data: serde_json::Value,
    /// When it was read, or `None` while it is unread.
    pub(crate) read_at: Option<DateTime<Utc>>,
    /// When it was stored.
    pub(crate) created_at: DateTime<Utc>,
}

impl StoredNotification {
    /// This row's id.
    #[must_use]
    pub fn id(&self) -> NotificationId {
        self.id
    }

    /// The recipient key this notification belongs to -- the same string
    /// [`Recipient::key`](super::Recipient::key) returns.
    #[must_use]
    pub fn notifiable_key(&self) -> &str {
        &self.notifiable_key
    }

    /// The application's own name for what this notification is, as
    /// [`DatabaseContent`](super::DatabaseContent) set it.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The payload.
    #[must_use]
    pub fn data(&self) -> &serde_json::Value {
        &self.data
    }

    /// When this notification was read, or `None` while it is unread.
    ///
    /// A timestamp rather than a flag, because the question an inbox is asked
    /// is usually "since when": what arrived after the last visit, how long a
    /// notice sat unread before anyone acted on it.
    #[must_use]
    pub fn read_at(&self) -> Option<DateTime<Utc>> {
        self.read_at
    }

    /// Whether this notification has been read.
    #[must_use]
    pub fn is_read(&self) -> bool {
        self.read_at.is_some()
    }

    /// When this notification was stored.
    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_round_trips_through_hex() {
        let id = NotificationId::from_bytes([0xab; ID_BYTES]);
        let text = id.to_hex();
        assert_eq!(text.len(), ID_BYTES * 2);
        assert_eq!(NotificationId::from_hex(&text), Some(id));
    }

    #[test]
    fn an_id_is_not_parsed_from_anything_that_is_not_one() {
        // Every one of these can arrive from a route parameter.
        for text in [
            "",
            "abc",
            "0123456789abcdef0123456789abcde",   // one short
            "0123456789abcdef0123456789abcdeff", // one long
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            "0123456789abcdef0123456789abcde ",
        ] {
            assert!(
                NotificationId::from_hex(text).is_none(),
                "{text:?} parsed as an id"
            );
        }
    }

    #[test]
    fn uppercase_hex_parses_to_the_same_id() {
        let lower = NotificationId::from_hex("0123456789abcdef0123456789abcdef");
        let upper = NotificationId::from_hex("0123456789ABCDEF0123456789ABCDEF");
        assert_eq!(lower, upper);
        // But it is spelled back out in one canonical case.
        assert_eq!(upper.unwrap().to_hex(), "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn a_notification_is_unread_until_it_has_a_read_time() {
        let mut row = StoredNotification {
            id: NotificationId::from_bytes([0; ID_BYTES]),
            notifiable_key: "user:42".to_owned(),
            kind: "invoice.paid".to_owned(),
            data: serde_json::json!({}),
            read_at: None,
            created_at: Utc::now(),
        };
        assert!(!row.is_read());
        row.read_at = Some(Utc::now());
        assert!(row.is_read());
    }
}
