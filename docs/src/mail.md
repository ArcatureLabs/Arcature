# Mail

SMTP over lettre. Arcature owns the ergonomics; lettre owns the protocol and
the certified rustls plus aws-lc-rs stack owns TLS.

`Mail` is a value, not a namespace. There is no `Mail::to("a@b.com")` static
constructor — `to` is a method on a facade that already knows the mailer and
the `From` address.

## The transport

```rust,ignore
use std::time::Duration;
use arcature::mail::{Mailer, SmtpConfig, SmtpCredentials, TlsMode};

let config = SmtpConfig::new("smtp.example.com")?
    .port(587)
    .tls_mode(TlsMode::StartTls)
    .credentials(SmtpCredentials::new("user", "secret"))
    .timeout(Duration::from_secs(10));

let mailer = Mailer::smtp(config)?;
```

`SmtpConfig::from_url("smtps://user:pass@host:465")` parses a connection URL
instead.

`Mailer` is `Clone + Send + Sync + 'static`. `Application::mail(config)`
builds one at startup.

Credentials never reach a log. `SmtpCredentials` has a `Debug` that prints
only the type name and deliberately has **no `Display`**; `SmtpConfig`
implements both by hand and never emits the password or the full URL.

## Sending

Implement `Mailable` for the message type. The `build` method receives an
`Email` builder that already has `From` and `To` set:

```rust,ignore
use arcature::mail::{Email, EmailError, Mailable};
use arcature::mail::lettre::message::Message;

pub struct WelcomeEmail {
    pub name: String,
}

impl Mailable for WelcomeEmail {
    fn build(&self, email: Email) -> Result<Message, EmailError> {
        email
            .subject(format!("Welcome, {}", self.name))
            .plain(format!("Welcome, {}", self.name))
    }
}
```

Then send:

```rust,ignore
use arcature::mail::Mail;

let mail = Mail::from_str(mailer, "noreply@example.com")?;
mail.to(user.email).send(&WelcomeEmail { name: user.name }).await?;
```

`Mail::new(mailer, mailbox)` takes an already-parsed `Mailbox` if you have
one; `parse_mailbox("Ada <ada@example.com>")` produces one.

## The message builder

`Email::builder()` starts a message. The chainers — `from`, `reply_to`, `to`,
`cc`, `bcc`, `subject` — return `Self` and do not fail. Only the body
terminators return a `Result`, because that is where the message is actually
assembled:

```rust,ignore
let message = Email::builder()
    .from(parse_mailbox("noreply@example.com")?)
    .to(parse_mailbox("ada@example.com")?)
    .subject("Welcome")
    .html("<h1>Welcome</h1>")?;
```

| Terminator | Produces |
| --- | --- |
| `plain(body)` | text/plain |
| `html(body)` | text/html |
| `alternative(plain, html)` | multipart/alternative |
| `mixed(..)` | multipart/mixed |
| `plain_with_attachments(body, attachments)` | text plus attachments |
| `alternative_with_attachments(plain, html, attachments)` | both, plus attachments |

`EmailAttachment::new(..)` builds an attachment. Its `Debug` redacts the body
bytes — an attachment in a log line is a data leak, not a diagnostic.

`Email::from_builder(builder)` and `email.into_builder()` cross to and from
lettre's own `MessageBuilder` when the wrapper runs out.

## Testing

`Mailer::capture_ok()` records every message in memory and always succeeds.
`Mailer::capture_error()` always fails the send, which is the one a retry path
needs. Neither opens a socket.

```rust,ignore
let mailer = Mailer::capture_ok();
let mail = Mail::from_str(mailer.clone(), "noreply@example.com")?;
mail.to("ada@example.com").send(&WelcomeEmail { name: "Ada".into() }).await?;

let sent = mailer.captured().await.expect("capture mailer");
assert_eq!(sent.len(), 1);
```

`captured()` returns `None` for an SMTP mailer. `is_capture()` and
`is_smtp()` report which kind you hold, so a production guard can refuse to
start with a capture transport.

## What this module does not own

SMTP, TLS, or cryptography. The lettre crate is re-exported as
`arcature::mail::lettre`.
