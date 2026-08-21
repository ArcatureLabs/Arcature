//! Recorders for jobs, mail, and events.
//!
//! Each of these sits on a seam the framework already has rather than
//! introducing a parallel one: [`Jobs`] implements [`crate::jobs::Observer`],
//! [`Mail`] wraps [`crate::mail::Mailer::capture_ok`], and [`Events`] wraps
//! the recording mode already built into [`crate::events::Dispatcher`]. The
//! value they add over using those directly is the failure message: each
//! assertion prints what was recorded, so a miss says what did happen instead
//! of only that the expected thing did not.
//!
//! Every recorder is a value the test passes in. None of them install
//! anything globally, so two tests recording at once cannot see each other.

#[cfg(feature = "events")]
pub use events::Events;
#[cfg(feature = "jobs")]
pub use jobs::{JobOutcome, JobRecord, Jobs};
#[cfg(feature = "mail")]
pub use mail::{Mail, SentMail};

#[cfg(feature = "jobs")]
mod jobs {
    use std::sync::{Arc, Mutex};

    use crate::jobs::{Event, FailReason, Observer};

    /// One job lifecycle event, flattened to what a test asserts on.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct JobRecord {
        /// The job row id, which is how later events are matched to the
        /// `Started` event that named the kind.
        pub job_id: uuid::Uuid,
        /// The job kind, e.g. `SendWelcomeEmail`.
        pub kind: String,
        /// The payload schema version.
        pub version: i16,
        /// Which attempt this was, starting at 1.
        pub attempt: i32,
        /// What happened.
        pub outcome: JobOutcome,
    }

    /// The lifecycle stage a [`JobRecord`] came from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum JobOutcome {
        /// The handler started.
        Started,
        /// The handler returned `Ok`.
        Succeeded,
        /// The handler failed and will run again.
        Retried,
        /// The handler failed for good.
        Failed(FailReason),
    }

    impl std::fmt::Display for JobOutcome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Started => f.write_str("started"),
                Self::Succeeded => f.write_str("succeeded"),
                Self::Retried => f.write_str("retried"),
                Self::Failed(reason) => write!(f, "failed ({reason})"),
            }
        }
    }

    /// A recording [`Observer`] for the job worker.
    ///
    /// # What this records, and what it does not
    ///
    /// It records what the **worker ran**, not what a handler **enqueued**.
    /// [`crate::jobs::Jobs::enqueue`] writes straight to PostgreSQL and has no
    /// interception point, so a recorder cannot honestly claim to see a
    /// dispatch. To assert that a request enqueued a job, query the queue
    /// table inside the test transaction -- see
    /// [`assert_job_enqueued`](super::assert_job_enqueued).
    ///
    /// Attach it with `WorkerBuilder::observer(recorder.clone())`.
    #[derive(Debug, Clone, Default)]
    pub struct Jobs {
        records: Arc<Mutex<Vec<JobRecord>>>,
    }

    impl Jobs {
        /// A fresh recorder.
        #[must_use]
        pub fn recorder() -> Self {
            Self::default()
        }

        /// Everything recorded so far, in order.
        ///
        /// # Panics
        ///
        /// Panics if a previous panic poisoned the record.
        #[must_use]
        pub fn records(&self) -> Vec<JobRecord> {
            self.records
                .lock()
                .expect("job record lock was poisoned by an earlier panic")
                .clone()
        }

        /// Assert the worker ran a job of `kind`.
        ///
        /// # Panics
        ///
        /// Panics when no such job ran, listing what did.
        pub fn assert_job_ran(&self, kind: &str) -> &Self {
            let records = self.records();
            assert!(
                records.iter().any(|record| record.kind == kind),
                "no job of kind `{kind}` ran; {}",
                summarise(&records)
            );
            self
        }

        /// Assert a job of `kind` ran and succeeded.
        ///
        /// # Panics
        ///
        /// Panics when no such job succeeded, listing what did run.
        pub fn assert_job_succeeded(&self, kind: &str) -> &Self {
            let records = self.records();
            assert!(
                records
                    .iter()
                    .any(|r| r.kind == kind && r.outcome == JobOutcome::Succeeded),
                "no job of kind `{kind}` succeeded; {}",
                summarise(&records)
            );
            self
        }

        /// Assert a job of `kind` failed permanently.
        ///
        /// # Panics
        ///
        /// Panics when no such job failed, listing what did run.
        pub fn assert_job_failed(&self, kind: &str) -> &Self {
            let records = self.records();
            assert!(
                records
                    .iter()
                    .any(|r| r.kind == kind && matches!(r.outcome, JobOutcome::Failed(_))),
                "no job of kind `{kind}` failed; {}",
                summarise(&records)
            );
            self
        }
    }

    impl Observer for Jobs {
        fn observe(&self, event: &Event) {
            let Ok(mut records) = self.records.lock() else {
                // A poisoned lock means a test already failed. Losing a
                // record is not worth a second panic from inside the worker.
                return;
            };
            // Only `Started` carries the kind; later events carry the job id,
            // so the kind is recovered from the record this job already left.
            let record = match event {
                Event::Started {
                    job_id,
                    kind,
                    version,
                    attempt,
                } => JobRecord {
                    job_id: *job_id,
                    kind: kind.clone(),
                    version: *version,
                    attempt: *attempt,
                    outcome: JobOutcome::Started,
                },
                Event::Succeeded {
                    job_id, attempt, ..
                } => {
                    let (kind, version) = resolve_kind(&records, *job_id);
                    JobRecord {
                        job_id: *job_id,
                        kind,
                        version,
                        attempt: *attempt,
                        outcome: JobOutcome::Succeeded,
                    }
                }
                Event::Retried {
                    job_id, attempt, ..
                } => {
                    let (kind, version) = resolve_kind(&records, *job_id);
                    JobRecord {
                        job_id: *job_id,
                        kind,
                        version,
                        attempt: *attempt,
                        outcome: JobOutcome::Retried,
                    }
                }
                Event::Failed {
                    job_id,
                    attempt,
                    reason,
                    ..
                } => {
                    let (kind, version) = resolve_kind(&records, *job_id);
                    JobRecord {
                        job_id: *job_id,
                        kind,
                        version,
                        attempt: *attempt,
                        outcome: JobOutcome::Failed(*reason),
                    }
                }
            };
            records.push(record);
        }
    }

    /// Recover the kind and version a job announced when it started.
    fn resolve_kind(records: &[JobRecord], job_id: uuid::Uuid) -> (String, i16) {
        records
            .iter()
            .find(|record| record.job_id == job_id)
            .map_or_else(
                || ("(unknown)".to_owned(), 0),
                |record| (record.kind.clone(), record.version),
            )
    }

    /// Describe what was recorded, for a failure message.
    fn summarise(records: &[JobRecord]) -> String {
        if records.is_empty() {
            return "the worker ran no jobs at all".to_owned();
        }
        let lines: Vec<String> = records
            .iter()
            .map(|r| {
                format!(
                    "  {} v{} attempt {} {}",
                    r.kind, r.version, r.attempt, r.outcome
                )
            })
            .collect();
        format!("what ran:\n{}", lines.join("\n"))
    }
}

#[cfg(feature = "mail")]
mod mail {
    use crate::mail::{Mailer, parse_mailbox};

    /// One captured message: its envelope recipients and its raw text.
    #[derive(Debug, Clone)]
    pub struct SentMail {
        /// The envelope recipients, as written on the wire.
        pub to: Vec<String>,
        /// The full RFC 5322 message, headers and body.
        pub raw: String,
    }

    impl SentMail {
        /// The `Subject` header, if the message has one.
        ///
        /// Reads the raw header line, so an encoded-word subject comes back
        /// encoded. Decoding MIME here would be a second implementation of
        /// something lettre already does on the way in.
        #[must_use]
        pub fn subject(&self) -> Option<&str> {
            self.raw
                .lines()
                .take_while(|line| !line.is_empty())
                .find_map(|line| line.strip_prefix("Subject: "))
        }

        /// Whether the message text contains `needle`.
        #[must_use]
        pub fn contains(&self, needle: &str) -> bool {
            self.raw.contains(needle)
        }
    }

    /// A capture mailer plus the assertions over what it captured.
    #[derive(Clone)]
    pub struct Mail {
        mailer: Mailer,
    }

    impl std::fmt::Debug for Mail {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // `Mailer` is not `Debug`, and what a reader wants here is the
            // fact that this one captures rather than sends.
            f.debug_struct("Mail").field("capture", &true).finish()
        }
    }

    impl Mail {
        /// A mailer that records every message and never sends one.
        #[must_use]
        pub fn fake() -> Self {
            Self {
                mailer: Mailer::capture_ok(),
            }
        }

        /// The mailer to hand to the application under test.
        #[must_use]
        pub fn mailer(&self) -> Mailer {
            self.mailer.clone()
        }

        /// A [`crate::mail::Mail`] sending from `from` through this mailer.
        ///
        /// # Errors
        ///
        /// Returns an error when `from` is not a valid address.
        pub fn sender(
            &self,
            from: &str,
        ) -> Result<crate::mail::Mail, lettre::address::AddressError> {
            Ok(crate::mail::Mail::new(
                self.mailer.clone(),
                parse_mailbox(from)?,
            ))
        }
    }
}

#[cfg(feature = "mail")]
impl mail::Mail {
    /// Everything captured so far, in send order.
    ///
    /// # Panics
    ///
    /// Panics if the mailer is not a capture mailer, which cannot happen for
    /// a value built by [`Mail::fake`].
    pub async fn sent(&self) -> Vec<SentMail> {
        let captured = self
            .mailer()
            .captured()
            .await
            .expect("a fake mailer is always a capture mailer");
        captured
            .into_iter()
            .map(|(envelope, raw)| SentMail {
                to: envelope.to().iter().map(ToString::to_string).collect(),
                raw,
            })
            .collect()
    }

    /// Assert a message was sent to `address`.
    ///
    /// # Panics
    ///
    /// Panics when nothing was sent to that address, listing every recipient
    /// that was written to.
    pub async fn assert_mail_sent(&self, address: &str) -> &Self {
        let sent = self.sent().await;
        let matched = sent
            .iter()
            .any(|message| message.to.iter().any(|to| to == address));
        assert!(
            matched,
            "no mail was sent to `{address}`; {}",
            describe(&sent)
        );
        self
    }

    /// Assert a message to `address` whose text contains `needle`.
    ///
    /// # Panics
    ///
    /// Panics when no such message was sent.
    pub async fn assert_mail_contains(&self, address: &str, needle: &str) -> &Self {
        let sent = self.sent().await;
        let matched = sent
            .iter()
            .any(|message| message.to.iter().any(|to| to == address) && message.contains(needle));
        assert!(
            matched,
            "no mail to `{address}` contains `{needle}`; {}",
            describe(&sent)
        );
        self
    }

    /// Assert nothing was sent.
    ///
    /// # Panics
    ///
    /// Panics when anything was sent, listing it.
    pub async fn assert_no_mail_sent(&self) -> &Self {
        let sent = self.sent().await;
        assert!(sent.is_empty(), "expected no mail; {}", describe(&sent));
        self
    }
}

/// Describe captured mail for a failure message.
#[cfg(feature = "mail")]
fn describe(sent: &[SentMail]) -> String {
    if sent.is_empty() {
        return "nothing was sent at all".to_owned();
    }
    let lines: Vec<String> = sent
        .iter()
        .map(|message| {
            format!(
                "  to [{}] subject {:?}",
                message.to.join(", "),
                message.subject().unwrap_or("(none)")
            )
        })
        .collect();
    format!("what was sent:\n{}", lines.join("\n"))
}

/// Assert a job of `kind` sits in the queue table.
///
/// This is the honest form of "a job was dispatched": enqueueing writes a row,
/// so the assertion reads the row. Run it on the same connection as the test
/// transaction, or the insert will not be visible.
///
/// # Panics
///
/// Panics when no such row exists, listing the kinds that are queued.
#[cfg(feature = "jobs")]
pub async fn assert_job_enqueued(connection: &mut crate::database::Connection, kind: &str) {
    let queued: Vec<String> =
        sqlx::query_scalar::<crate::database::Driver, String>("SELECT kind FROM arcature_jobs")
            .fetch_all(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("could not read `arcature_jobs`: {error}"));
    assert!(
        queued.iter().any(|queued| queued == kind),
        "no job of kind `{kind}` is queued; the queue holds {}",
        if queued.is_empty() {
            "nothing".to_owned()
        } else {
            format!("[{}]", queued.join(", "))
        }
    );
}

#[cfg(feature = "events")]
mod events {
    use crate::events::Dispatcher;

    /// A recording [`Dispatcher`] plus the assertions over what it recorded.
    ///
    /// The recording mode belongs to the dispatcher itself; this adds the
    /// failure message and keeps the recorder a value the test owns.
    #[derive(Clone)]
    pub struct Events {
        dispatcher: Dispatcher,
    }

    impl std::fmt::Debug for Events {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Events")
                .field("dispatched", &self.dispatcher.dispatched_events())
                .finish()
        }
    }

    impl Events {
        /// A dispatcher that records every event dispatched through it.
        #[must_use]
        pub fn fake() -> Self {
            Self {
                dispatcher: Dispatcher::recording(),
            }
        }

        /// The dispatcher to hand to the application under test.
        #[must_use]
        pub fn dispatcher(&self) -> Dispatcher {
            self.dispatcher.clone()
        }

        /// Register a listener, keeping the recording.
        #[must_use]
        pub fn register<E, F, Fut>(self, listener: F) -> Self
        where
            E: crate::events::Event + serde::Serialize + serde::de::DeserializeOwned,
            F: Fn(E) -> Fut + Send + Sync + 'static,
            Fut: std::future::Future<Output = Result<(), crate::events::DispatchError>>
                + Send
                + 'static,
        {
            Self {
                dispatcher: self.dispatcher.register(listener),
            }
        }

        /// The names of the events dispatched so far, in order.
        #[must_use]
        pub fn dispatched(&self) -> Vec<String> {
            self.dispatcher.dispatched_events()
        }

        /// Assert an event of type name `name` was dispatched.
        ///
        /// # Panics
        ///
        /// Panics when it was not, listing what was.
        pub fn assert_dispatched(&self, name: &str) -> &Self {
            let dispatched = self.dispatched();
            assert!(
                dispatched.iter().any(|event| event == name),
                "event `{name}` was not dispatched; {}",
                if dispatched.is_empty() {
                    "nothing was dispatched at all".to_owned()
                } else {
                    format!("what was: [{}]", dispatched.join(", "))
                }
            );
            self
        }

        /// Assert an event of type name `name` was not dispatched.
        ///
        /// # Panics
        ///
        /// Panics when it was.
        pub fn assert_not_dispatched(&self, name: &str) -> &Self {
            let dispatched = self.dispatched();
            assert!(
                !dispatched.iter().any(|event| event == name),
                "event `{name}` was dispatched; the full sequence was [{}]",
                dispatched.join(", ")
            );
            self
        }
    }
}

#[cfg(all(test, feature = "jobs"))]
mod job_recorder_tests {
    use super::{JobOutcome, Jobs};
    use crate::jobs::{Event, FailReason, Observer};
    use std::time::Duration;

    fn started(id: uuid::Uuid, kind: &str) -> Event {
        Event::Started {
            job_id: id,
            kind: kind.to_owned(),
            version: 1,
            attempt: 1,
        }
    }

    #[test]
    fn a_recorder_starts_empty_and_every_assertion_fails_on_it() {
        let recorder = Jobs::recorder();
        assert!(recorder.records().is_empty());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            recorder.assert_job_ran("SendWelcome")
        }));
        assert!(
            outcome.is_err(),
            "an empty recorder must fail every assertion, not pass vacuously"
        );
    }

    #[test]
    fn a_started_event_is_recorded_under_its_kind() {
        let recorder = Jobs::recorder();
        recorder.observe(&started(uuid::Uuid::nil(), "SendWelcome"));
        recorder.assert_job_ran("SendWelcome");
    }

    #[test]
    fn a_later_event_recovers_the_kind_from_the_start_of_the_same_job() {
        let id = uuid::Uuid::from_u128(7);
        let recorder = Jobs::recorder();
        recorder.observe(&started(id, "SendWelcome"));
        recorder.observe(&Event::Succeeded {
            job_id: id,
            attempt: 1,
            duration: Duration::from_millis(3),
        });
        recorder.assert_job_succeeded("SendWelcome");
        assert_eq!(recorder.records()[1].outcome, JobOutcome::Succeeded);
    }

    #[test]
    fn a_job_that_only_started_has_not_succeeded() {
        let recorder = Jobs::recorder();
        recorder.observe(&started(uuid::Uuid::nil(), "SendWelcome"));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            recorder.assert_job_succeeded("SendWelcome")
        }));
        assert!(outcome.is_err(), "starting is not succeeding");
    }

    #[test]
    fn a_permanent_failure_is_recorded_with_its_reason() {
        let id = uuid::Uuid::from_u128(9);
        let recorder = Jobs::recorder();
        recorder.observe(&started(id, "ChargeCard"));
        recorder.observe(&Event::Failed {
            job_id: id,
            attempt: 3,
            duration: Duration::from_millis(1),
            message: "declined".to_owned(),
            reason: FailReason::Exhausted,
        });
        recorder.assert_job_failed("ChargeCard");
        assert_eq!(
            recorder.records()[1].outcome,
            JobOutcome::Failed(FailReason::Exhausted)
        );
    }

    #[test]
    fn a_failure_message_lists_the_jobs_that_did_run() {
        let recorder = Jobs::recorder();
        recorder.observe(&started(uuid::Uuid::nil(), "SendWelcome"));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            recorder.assert_job_ran("ChargeCard")
        }));
        let payload = outcome.expect_err("the assertion must fail");
        let message = payload
            .downcast_ref::<String>()
            .expect("panic payload is a String");
        assert!(message.contains("SendWelcome"), "message: {message}");
    }
}

#[cfg(all(test, feature = "events"))]
mod event_recorder_tests {
    use super::Events;

    #[test]
    fn a_fresh_recorder_reports_nothing_dispatched() {
        let events = Events::fake();
        assert!(events.dispatched().is_empty());
        events.assert_not_dispatched("UserRegistered");
    }

    #[test]
    fn asserting_a_dispatch_on_a_fresh_recorder_fails() {
        let events = Events::fake();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            events.assert_dispatched("UserRegistered")
        }));
        assert!(
            outcome.is_err(),
            "an empty recorder must not satisfy assert_dispatched"
        );
    }
}
