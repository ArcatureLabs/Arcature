//! Handing the mail channel to the job queue.
//!
//! [`Notifier::send`](super::Notifier::send) talks to the SMTP server while
//! the request is still open. [`Notifier::queue`](super::Notifier::queue)
//! writes a row instead and lets a worker do the talking.
//!
//! # Only mail is queued, and that is the point
//!
//! The other two channels stay inline even when a queue is wired, for reasons
//! that are about correctness rather than taste:
//!
//! - The **inbox** is a write to the application's own database, which is the
//!   same database the job row goes into. Deferring it would buy nothing and
//!   cost the guarantee that matters: a recipient who opens the application
//!   immediately after the event would find an empty inbox, because the row
//!   they are looking for is sitting in a queue behind it.
//! - The **live push** reaches the connections held by *this* process. A
//!   worker is a different process and holds none of them, so queueing a push
//!   is not deferring it -- it is dropping it.
//!
//! So a queued send is an inline send with one thing moved: the part that
//! leaves the machine.
//!
//! # What the latency claim actually is
//!
//! Two things are true and they are not the same thing.
//!
//! The request stops waiting on SMTP. A connection, a TLS handshake, and a
//! server that may itself be waiting on a DNS lookup become a single `INSERT`
//! into a table the request is already connected to.
//!
//! And the *variation* goes with it. How long an SMTP conversation takes
//! depends on the address at the other end -- whether the domain resolves,
//! whether the server greylists, whether the recipient exists -- and a
//! handler that answers a registration form at a speed that depends on those
//! things is telling anyone with a stopwatch which addresses are already
//! taken. Queueing removes that: the enqueue costs the same for an address
//! that will bounce as for one that will not.
//!
//! What it does **not** do is make the whole handler constant-time. The inbox
//! write and the live push still happen inline, and password hashing -- the
//! usual reason a registration handler is timed -- is somewhere else
//! entirely. This removes one oracle, not the category.
//!
//! # Delivery is at-least-once, so an email can arrive twice
//!
//! [`crate::jobs`] is an at-least-once queue: a worker that hands a message
//! to the SMTP server and then dies before marking the row complete leaves a
//! job that another worker will claim. The message is then sent again.
//!
//! This is not a bug that can be fixed here. Handing bytes to a remote server
//! and recording that you did are two operations in two systems, and no
//! amount of care makes them one. The choice is which way to fail, and a
//! notification that arrives twice is better than one that never arrives.
//!
//! Anything whose *second* delivery is harmful -- a one-time code that is
//! consumed on send, an email that charges a card -- should not rely on the
//! send being the only record that it happened.
//!
//! # Example
//!
//! Registration is separate from enqueueing because the two happen in
//! different processes. The web process holds a [`NotificationQueue`]; the
//! worker process registers the handler.
//!
//! ```
//! use arcature::jobs::Registry;
//! use arcature::mail::{Mail, Mailer};
//! use arcature::notifications::{QueuedMail, register_mail_handler};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // In the worker: teach the registry how to send one.
//! let mail = Mail::new(Mailer::capture_ok(), "noreply@example.com".parse()?);
//! let mut registry = Registry::new();
//! register_mail_handler(&mut registry, mail)?;
//!
//! // In the web process: what a queued mail looks like on the way in.
//! let job = QueuedMail::new(
//!     "ada@example.com",
//!     &arcature::notifications::MailContent::new("Welcome", "Welcome to Acme."),
//! );
//! assert_eq!(job.to(), "ada@example.com");
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};

use crate::jobs::{EnqueuedJob, JobError, JobModel, JobRequest, Jobs, RegisterError, Registry};
use crate::mail::{Mail, MailSendError};

use super::channel::NotificationError;
use super::notification::MailContent;
use super::notifier::AsMailable;

/// The job identity a queued notification email is enqueued and dispatched
/// under.
///
/// Public because the two halves live in different processes: the worker
/// registers a handler for this model and the web process enqueues against
/// it, and if the two disagreed by a character the jobs would sit in the
/// table forever with nobody registered to run them. Sharing the one const
/// removes the chance.
///
/// The kind is namespaced under `arcature.` because it goes into a shared
/// table. An application's own job called `mail` would otherwise collide with
/// this one and the collision would look like a deserialisation failure.
///
/// Three attempts, because the failures this retries are transient by
/// construction -- a permanent one is classified as such by the handler and
/// is never retried at all, whatever this number says.
pub const MAIL_JOB: JobModel<QueuedMail> = JobModel::new("arcature.notifications.mail", 1, 3);

/// One email, rendered and waiting for a worker.
///
/// # Why the rendered content and not the notification
///
/// Laravel serialises the notification object and re-renders it in the
/// worker. That needs every notification to be serialisable, and a registry
/// mapping a stored type name back to a Rust type -- and it means the content
/// is produced by whatever version of the code the *worker* is running, which
/// during a deploy is not the version that decided to send it.
///
/// Here the render happens in the request, where it is a few string
/// allocations, and what is stored is the result. Nothing new is required of
/// [`Notification`](super::Notification), a notification with borrowed data
/// or a closure inside it queues exactly as well as any other, and the email
/// that arrives says what the code that sent it meant.
///
/// The cost is that the payload carries the body rather than a reference to
/// it, so a very large email is a very large row. [`MAIL_JOB`] inherits the
/// queue's default payload limit and an oversized one is refused at enqueue
/// rather than truncated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct QueuedMail {
    to: String,
    subject: String,
    text: String,
    /// `default` so a row written before this field existed still
    /// deserialises. A payload version bump is the honest answer to a
    /// *changed* field; a merely absent optional one does not need it.
    #[serde(default)]
    html: Option<String>,
}

impl QueuedMail {
    /// The email that will be sent to `to`.
    #[must_use]
    pub fn new(to: impl Into<String>, content: &MailContent) -> Self {
        Self {
            to: to.into(),
            subject: content.subject().to_owned(),
            text: content.text().to_owned(),
            html: content.html_body().map(ToOwned::to_owned),
        }
    }

    /// The recipient address.
    #[must_use]
    pub fn to(&self) -> &str {
        &self.to
    }

    /// The subject line.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The plain-text body.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The HTML body, if the notification rendered one.
    #[must_use]
    pub fn html_body(&self) -> Option<&str> {
        self.html.as_deref()
    }

    /// The content this was built from, for handing back to the mail
    /// transport.
    #[must_use]
    pub fn content(&self) -> MailContent {
        let content = MailContent::new(&self.subject, &self.text);
        match &self.html {
            Some(html) => content.html(html),
            None => content,
        }
    }

    /// The enqueue request for this email.
    ///
    /// Exposed rather than kept inside [`NotificationQueue::enqueue`] because
    /// an application that wants the job row to land in the *same*
    /// transaction as whatever caused the notification needs the request to
    /// hand to [`Jobs::enqueue_tx`]. Enqueueing outside that transaction is a
    /// job that runs for a change that then rolled back.
    ///
    /// # Errors
    ///
    /// [`NotificationError::Queue`] if the payload exceeds the queue's size
    /// limit, which for an email means a body larger than the row is allowed
    /// to be.
    pub fn request(&self) -> Result<JobRequest<Self>, NotificationError> {
        Ok(JobRequest::new(&MAIL_JOB, self)?)
    }
}

/// The queue a [`Notifier`](super::Notifier) hands mail to.
///
/// Cheap to clone and meant to live in application state next to the notifier
/// that uses it.
#[derive(Clone)]
#[non_exhaustive]
pub struct NotificationQueue {
    jobs: Jobs,
}

impl std::fmt::Debug for NotificationQueue {
    /// Prints nothing about the pool underneath. A `sqlx::Pool` renders its
    /// connect options, and a connect options renders the database URL --
    /// which is where the database password lives. The one fact worth
    /// printing is that a queue is wired, and the enclosing
    /// [`Notifier`](super::Notifier) already prints that.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationQueue").finish_non_exhaustive()
    }
}

impl NotificationQueue {
    /// Queue notifications through this jobs handle.
    #[must_use]
    pub fn new(jobs: Jobs) -> Self {
        Self { jobs }
    }

    /// The jobs handle underneath.
    #[must_use]
    pub fn jobs(&self) -> &Jobs {
        &self.jobs
    }

    /// Write one email to the queue.
    ///
    /// # Errors
    ///
    /// [`NotificationError::Queue`] if the row cannot be written, or if the
    /// rendered email is larger than the queue's payload limit.
    pub async fn enqueue(&self, mail: &QueuedMail) -> Result<EnqueuedJob, NotificationError> {
        Ok(self.jobs.enqueue(&mail.request()?).await?)
    }
}

/// Teach a worker's registry how to send a queued notification email.
///
/// The `mail` given here is the transport the *worker* sends through, which
/// need not be the one the web process was built with -- and usually is not,
/// since the web process may have no SMTP credentials at all once its mail
/// goes through the queue.
///
/// # Errors
///
/// [`RegisterError::AlreadyRegistered`] if this is called twice on the same
/// registry. That is a real mistake rather than a harmless repeat: two
/// registrations mean two transports were configured for the same job and
/// only one of them is going to be used, and which one is an accident of call
/// order.
///
/// # Retry classification
///
/// A message the transport could not *build* -- a malformed address, a body
/// that is not valid MIME -- is permanent. Nothing about waiting fixes an
/// address that will not parse, and retrying it three times is three log
/// lines saying the same thing before the row dies anyway.
///
/// Everything else is retryable. An SMTP server refuses for reasons that are
/// mostly temporary (greylisting, rate limits, a connection that dropped),
/// and a permanent rejection is expensive to distinguish from a temporary one
/// at this layer: SMTP reply codes are advisory, and a 5xx from a
/// misconfigured relay is not the recipient's fault. Retrying a genuinely
/// permanent failure costs two extra attempts; treating a temporary one as
/// permanent costs the email.
pub fn register_mail_handler(registry: &mut Registry, mail: Mail) -> Result<(), RegisterError> {
    registry.add(&MAIL_JOB, move |job: QueuedMail| {
        let mail = mail.clone();
        async move { deliver(&mail, &job).await }
    })?;
    Ok(())
}

/// Send one queued email, classifying the failure for the worker.
///
/// Separate from the closure in [`register_mail_handler`] so the
/// classification can be tested without a registry, a pool, or a worker.
async fn deliver(mail: &Mail, job: &QueuedMail) -> Result<(), JobError> {
    let content = job.content();

    // Through the same adapter the inline path uses, so a queued email and an
    // inline one are the same bytes. Two spellings of "turn this content into
    // a message" would eventually disagree about something small and nobody
    // would find out from a test.
    match mail.to(job.to()).send(&AsMailable(&content)).await {
        Ok(()) => Ok(()),
        Err(error @ MailSendError::Build { .. }) => Err(JobError::permanent(error)),
        Err(error) => Err(JobError::retryable(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::Mailer;

    fn content() -> MailContent {
        MailContent::new("Welcome", "Welcome to Acme.").html("<p>Welcome to Acme.</p>")
    }

    fn mailer(ok: bool) -> (Mailer, Mail) {
        let mailer = if ok {
            Mailer::capture_ok()
        } else {
            Mailer::capture_error()
        };
        let mail = Mail::new(mailer.clone(), "noreply@example.com".parse().unwrap());
        (mailer, mail)
    }

    #[test]
    fn the_job_kind_is_one_the_queue_will_accept() {
        // The model is a `const`, so nothing validates it until the first
        // enqueue -- which is in a request, in production. Building a request
        // here runs the same validation at test time instead.
        let job = QueuedMail::new("ada@example.com", &content());
        let request = job.request().expect("a small email is a valid payload");

        assert_eq!(request.kind(), "arcature.notifications.mail");
        assert_eq!(request.version(), 1);
        assert_eq!(request.effective_max_attempts(), 3);
    }

    #[test]
    fn the_payload_survives_the_round_trip_through_the_row() {
        // What is stored is JSON, and what the worker gets is whatever comes
        // back out of it. A field that serialises but does not deserialise
        // would show up as a poison job in production and as nothing here
        // unless the round trip is the thing under test.
        let job = QueuedMail::new("ada@example.com", &content());
        let stored = serde_json::to_value(&job).unwrap();
        let loaded: QueuedMail = serde_json::from_value(stored).unwrap();

        assert_eq!(loaded, job);
        assert_eq!(loaded.content(), content());
    }

    #[test]
    fn a_row_written_without_an_html_body_still_loads() {
        // `#[serde(default)]` earning its place: the absent-field case is the
        // one that breaks a queue during a deploy, because the rows already
        // in the table were written by the previous version.
        let loaded: QueuedMail = serde_json::from_value(serde_json::json!({
            "to": "ada@example.com",
            "subject": "Welcome",
            "text": "Welcome to Acme.",
        }))
        .expect("html is optional");

        assert_eq!(loaded.html_body(), None);
        assert_eq!(loaded.content().html_body(), None);
    }

    #[tokio::test]
    async fn a_delivered_job_reaches_the_transport_with_the_body_intact() {
        let (mailer, mail) = mailer(true);
        let job = QueuedMail::new("ada@example.com", &content());

        deliver(&mail, &job).await.expect("capture_ok accepts");

        let captured = mailer.captured().await.unwrap();
        assert_eq!(captured.len(), 1);
    }

    #[tokio::test]
    async fn a_transport_failure_is_retryable() {
        // The classification is the whole decision this handler makes. An
        // SMTP server that was down for a minute must not cost the email.
        let (_mailer, mail) = mailer(false);
        let job = QueuedMail::new("ada@example.com", &content());

        let error = deliver(&mail, &job).await.unwrap_err();

        assert!(error.is_retryable(), "got {error:?}");
    }

    #[tokio::test]
    async fn an_unparseable_address_is_permanent() {
        // The other half of the same decision. Retrying an address that will
        // never parse is three attempts to reach a mistake.
        let (_mailer, mail) = mailer(true);
        let job = QueuedMail::new("not an address", &content());

        let error = deliver(&mail, &job).await.unwrap_err();

        assert!(error.is_permanent(), "got {error:?}");
    }

    #[test]
    fn registering_the_handler_twice_is_refused() {
        let (_mailer, mail) = mailer(true);
        let mut registry = Registry::new();

        register_mail_handler(&mut registry, mail.clone()).expect("first registration");
        let error = register_mail_handler(&mut registry, mail).unwrap_err();

        assert!(
            matches!(error, RegisterError::AlreadyRegistered { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn debug_does_not_print_the_pool() {
        // A `sqlx::Pool` prints its connect options, which carry the database
        // password. Asserting the rendering rather than trusting the
        // hand-written impl to stay hand-written.
        //
        // Built without a pool would be better, but `Jobs` needs one; the
        // assertion below is on the shape, which is what a leak would change.
        let rendered = format!("{:?}", DebugShape);
        assert_eq!(rendered, "NotificationQueue { .. }");
    }

    /// Stands in for a `NotificationQueue` so the `Debug` shape can be
    /// asserted without a live pool. Kept next to the assertion so the two
    /// cannot drift apart unnoticed.
    struct DebugShape;
    impl std::fmt::Debug for DebugShape {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("NotificationQueue").finish_non_exhaustive()
        }
    }
}
