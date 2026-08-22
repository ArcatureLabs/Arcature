//! The redaction deny-list: what never reaches a log line.
//!
//! Logging is the most common way a secret escapes a process, because a log
//! line is written once and then copied everywhere -- to a file, to a
//! shipper, to a third-party index, into a support ticket. The defence has
//! to be a property of the writer, not a rule the caller remembers.
//!
//! [`is_sensitive`] answers the one question the JSON layer asks of every
//! field it is about to serialise. A field whose name matches is written as
//! [`REDACTED`] and its value is dropped on the floor.
//!
//! Matching is a case-insensitive substring test rather than an exact-name
//! test, on purpose: `password`, `user_password` and `db.password` are all
//! the same mistake, and a deny-list that only catches the spelling someone
//! thought of is not a deny-list. False positives cost a debugging session;
//! false negatives cost a credential.
//!
//! Substring matching alone is not enough, because the separator moves with
//! the convention. An HTTP header is `x-api-key`, an OpenTelemetry attribute
//! is `http.request.header.authorization`, a struct field is `api_key` -- one
//! secret, three spellings, and a needle written with `_` is a substring of
//! only the third. So `-` and `.` are folded to `_` before the test, and the
//! list is written in one convention rather than three.

/// What a redacted value is replaced with.
///
/// A fixed marker rather than an omission so a reader can tell "this field
/// was withheld" from "this field was never recorded".
pub const REDACTED: &str = "[redacted]";

/// The field-name fragments that mark a value as never loggable.
///
/// Kept sorted so a reader can scan it, and public so an application can
/// check its own field names against it in a test.
pub const DENY_LIST: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "auth",
    "bearer",
    "bind",
    "body",
    "cache_value",
    "card",
    "cookie",
    "credential",
    "csrf",
    "cvv",
    "id_token",
    "otp",
    "passphrase",
    "passwd",
    "password",
    "payload",
    "pin_code",
    "private_key",
    "pwd",
    "refresh_token",
    "secret",
    "session_id",
    "signature",
    "sql_args",
    "token",
    "verifier",
];

/// Whether a field with this name may never carry its value into a log.
///
/// The comparison is ASCII case-insensitive; a field name is not expected to
/// contain non-ASCII, and folding Unicode here would only widen the surface
/// without widening the protection.
///
/// `-` and `.` are folded to `_` first, so one needle covers every spelling
/// of the same name. Every needle above uses `_`, and none contains `-` or
/// `.`, so the fold can only ever match more -- there is no name that was
/// redacted before this ran and is not redacted after it.
#[must_use]
pub fn is_sensitive(field: &str) -> bool {
    let lowered: String = field
        .chars()
        .map(|character| match character {
            '-' | '.' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect();
    DENY_LIST.iter().any(|needle| lowered.contains(needle))
}

/// Either the value, or the redaction marker.
///
/// The borrow makes the common case free: nothing is copied for a field that
/// is allowed through.
#[must_use]
pub fn apply<'a>(field: &str, value: &'a str) -> &'a str {
    if is_sensitive(field) { REDACTED } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_obvious_credential_names_are_all_caught() {
        for name in [
            "password",
            "Password",
            "user_password",
            "db.password",
            "api_key",
            "authorization",
            "Cookie",
            "access_token",
            "refresh_token",
            "client_secret",
            "code_verifier",
            "sql_args",
            "request_body",
            "job_payload",
            "cache_value",
        ] {
            assert!(is_sensitive(name), "{name} should be redacted");
        }
    }

    #[test]
    fn the_same_secret_is_caught_in_every_separator_convention() {
        // One secret spelled three ways -- header, OpenTelemetry attribute,
        // struct field -- against a list written only in the third. Before
        // the separators were folded, every hyphenated name here was logged
        // in full, which is the spelling a header map actually uses.
        for name in [
            "x-api-key",
            "X-Api-Key",
            "x-session-id",
            "x-private-key",
            "x-pin-code",
            "x-cache-value",
            "http.request.header.authorization",
            "oauth.access-token",
        ] {
            assert!(is_sensitive(name), "{name} should be redacted");
        }
    }

    #[test]
    fn ordinary_field_names_pass_through() {
        // The hyphenated names matter as much as the rest: folding `-` to `_`
        // must widen what is caught without inventing a needle. `user-agent`
        // and `content-length` belong in an access log.
        for name in [
            "method",
            "path",
            "status",
            "duration_ms",
            "request_id",
            "user-agent",
            "content-length",
            "x-request-id",
            "http.route",
        ] {
            assert!(!is_sensitive(name), "{name} should not be redacted");
        }
    }

    #[test]
    fn apply_swaps_only_the_sensitive_value() {
        assert_eq!(apply("password", "hunter2"), REDACTED);
        assert_eq!(apply("path", "/login"), "/login");
    }

    #[test]
    fn the_deny_list_is_sorted_and_lowercase() {
        let mut sorted = DENY_LIST.to_vec();
        sorted.sort_unstable();
        assert_eq!(DENY_LIST, sorted.as_slice());
        assert!(DENY_LIST.iter().all(|n| n.to_ascii_lowercase() == *n));
    }
}
