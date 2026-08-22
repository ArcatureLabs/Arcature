//! Who a notification is for.

/// The addressing information a notification needs about one recipient.
///
/// A recipient is a stable key plus whatever a channel needs to reach them.
/// The key is the same shape the rest of the framework uses for a subject --
/// `"user:42"`, the string an API token is issued to -- so that a
/// notification, a token and an audit line all name the same person the same
/// way.
///
/// # Example
///
/// ```
/// use arcature::notifications::Recipient;
///
/// let ada = Recipient::new("user:42").email("ada@example.com");
///
/// assert_eq!(ada.key(), "user:42");
/// assert_eq!(ada.email_address(), Some("ada@example.com"));
///
/// // A recipient with no address is ordinary, not broken: a notification
/// // that only writes to an in-app inbox needs no way to email anyone.
/// let anon = Recipient::new("user:43");
/// assert_eq!(anon.email_address(), None);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Recipient {
    key: String,
    email: Option<String>,
}

impl Recipient {
    /// A recipient identified by `key`, reachable on no channel yet.
    ///
    /// The key should be stable across renames and address changes -- a
    /// primary key, not an email address -- because later channels store it
    /// alongside delivered notifications.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            email: None,
        }
    }

    /// Give the recipient an email address, enabling the mail channel.
    #[must_use]
    pub fn email(mut self, address: impl Into<String>) -> Self {
        self.email = Some(address.into());
        self
    }

    /// The recipient's stable key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The recipient's email address, if they have one.
    #[must_use]
    pub fn email_address(&self) -> Option<&str> {
        self.email.as_deref()
    }
}

/// Anything that can name itself as a notification recipient.
///
/// The application implements this for its user type, so that sending reads
/// `notifier.send(&user, &invoice_paid)` rather than making every call site
/// assemble a [`Recipient`] by hand -- and so that the mapping from a user to
/// an address lives in one place instead of being re-derived wherever a
/// notification is sent.
///
/// # Example
///
/// ```
/// use arcature::notifications::{Notifiable, Recipient};
///
/// struct User {
///     id: i64,
///     email: String,
/// }
///
/// impl Notifiable for User {
///     fn recipient(&self) -> Recipient {
///         Recipient::new(format!("user:{}", self.id)).email(&self.email)
///     }
/// }
///
/// let ada = User { id: 42, email: "ada@example.com".into() };
/// assert_eq!(ada.recipient().key(), "user:42");
/// ```
pub trait Notifiable {
    /// Describe this value as a notification recipient.
    ///
    /// Called once per send, so it may allocate; it should not query a
    /// database.
    fn recipient(&self) -> Recipient;
}

impl Notifiable for Recipient {
    fn recipient(&self) -> Recipient {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recipient_starts_with_no_channels() {
        assert_eq!(Recipient::new("user:1").email_address(), None);
    }

    #[test]
    fn the_last_address_wins() {
        // The builder is `mut self`, so a second call replaces rather than
        // appending. Stated in a test because the alternative -- silently
        // keeping the first -- would be a plausible reading.
        let recipient = Recipient::new("user:1")
            .email("old@example.com")
            .email("new@example.com");
        assert_eq!(recipient.email_address(), Some("new@example.com"));
    }

    #[test]
    fn a_recipient_is_notifiable_as_itself() {
        let recipient = Recipient::new("user:1").email("a@example.com");
        assert_eq!(recipient.recipient(), recipient);
    }
}
