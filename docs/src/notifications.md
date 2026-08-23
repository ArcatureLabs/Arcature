# Notifications

One event, told to one person, over whichever channels apply: an email, a row
in an in-app inbox, a live push to a socket that is open right now.

Not enabled by default. `notifications` is absent from the crate's default
feature list, and so are the three that build on it.

`Notifier` is a value, not a namespace. Nothing in `Application` constructs
one — you build it at startup and put it in application state, the same way
you would a `Mail` or a `Jobs`.

## What a notification is

A notification is a type that knows how to render itself for each channel. The
trait has one method per channel and every one of them defaults to `None`:

| Method | Returns | Channel |
| --- | --- | --- |
| `to_mail(&self, recipient)` | `Option<MailContent>` | `Channel::Mail` |
| `to_database(&self, recipient)` | `Option<DatabaseContent>` | `Channel::Database` |
| `to_broadcast(&self, recipient)` | `Option<BroadcastContent>` | `Channel::Broadcast` |

```rust,ignore
use arcature::notifications::{MailContent, Notification, Recipient};

struct InvoicePaid {
    amount_cents: i64,
}

impl Notification for InvoicePaid {
    fn to_mail(&self, recipient: &Recipient) -> Option<MailContent> {
        // No address, no mail -- and no error, because this notification is
        // genuinely not a mail notification for this person.
        recipient.email_address()?;

        Some(MailContent::new(
            "Your invoice is paid",
            format!("We received {}.{:02}.", self.amount_cents / 100, self.amount_cents % 100),
        ))
    }
}
```

`impl Notification for Silent {}` compiles and reaches nobody. That is what
makes adding a channel later additive: a notification written today keeps
compiling when a fourth method appears, and does not use it.

### There is no `via`

Laravel names the channels in `via()` and renders them in
`toMail`/`toDatabase`/`toBroadcast`. Two places, and nothing keeps them
agreeing: a channel in `via()` with no method behind it throws at runtime, and
a method `via()` forgot is never called.

Here the channel set is derived rather than declared. A notification reaches a
channel exactly when that channel's method returns `Some`, so the list *is*
the methods. The per-recipient decision `via($notifiable)` exists to make is
still available — every method receives the `Recipient` — but it is made in
the same place that produces the content.

`to_database` and `to_broadcast` exist whatever features are on, and so do the
`Channel::Database` and `Channel::Broadcast` variants. Rendering costs nothing
but `serde_json`; it is *delivering* that needs a feature. A method compiled
out by a feature flag would be a notification that silently changes what it
does.

### The three content types

`MailContent::new(subject, text)` takes the plain-text body as a mandatory
argument and `MailContent::html(html)` adds the HTML one. That order is
deliberate: an HTML-only email is unreadable in a text client, in a screen
reader that falls back, and in the preview line every mail app shows, and it
is one of the older signals a spam filter weighs. `html_body()` is `None`
until `.html(..)` is called, and the last call wins. The HTML is used
verbatim — nothing escapes what a caller interpolates into it.

`DatabaseContent` and `BroadcastContent` are the same pair of fields — a
`kind` string and a `serde_json::Value` payload — and deliberately two types.
An inbox row is read on purpose and can afford detail; a live push arrives
unasked, is usually a toast or a badge, and is often smaller. A notification
that wants them identical builds both from the same value, which is one line;
one that wants them different has nowhere to say so if they share a method.

Both have two constructors. `new(kind, value)` cannot fail, because
`serde_json::json!` produces a `Value` infallibly. `serializing(kind, &T)`
takes a `Serialize` value and hands back the `serde_json::Error`. The split
exists because `to_database` returns an `Option` and an `Option` has nowhere
to put an error: a constructor that serialised would turn a `#[serde(..)]`
mistake into a notification that never appears.

The `kind` is the application's own name — `"invoice.paid"`, `"mention"` —
and deliberately not a Rust type path. It is stored in a row and switched on
by a front end, so deriving it from a type name would make `refactor: rename`
a silent protocol change.

## Recipients

```rust,ignore
use arcature::notifications::{Notifiable, Recipient};

struct User {
    id: i64,
    email: String,
}

impl Notifiable for User {
    fn recipient(&self) -> Recipient {
        Recipient::new(format!("user:{}", self.id)).email(&self.email)
    }
}
```

A `Recipient` is a stable key plus whatever a channel needs to reach them. The
key is the same shape the rest of the framework uses for a subject — the
string an API token is issued to — so a notification, a token and an audit
line name the same person the same way. It should be a primary key rather than
an email address, because the inbox stores it alongside every delivered row.

A fresh `Recipient` has **no** email address; `email_address()` returns `None`
until `.email(..)` is called, and a second call replaces the first. A
recipient with no address is ordinary rather than broken — a notification that
only writes to an inbox needs no way to email anybody.

`recipient()` is called once per send, so it may allocate. It must not query a
database.

`Recipient` implements `Notifiable` for itself, so `notifier.send(&recipient,
..)` works without a wrapper type.

## The four features

| Feature | Implies | What it adds |
| --- | --- | --- |
| `notifications` | `mail` | `Notification`, `Recipient`, `Notifiable`, `Notifier`, `Delivery`, `Channel`, `NotificationError`, the three content types, and the mail channel |
| `notifications-db` | `notifications`, `database` | `DatabaseNotifications`, `StoredNotification`, `NotificationId`, `NotificationPool`, `Notifier::with_database` — plus one table and one migration |
| `notifications-broadcast` | `notifications`, `realtime` | `BroadcastChannels`, `PerRecipientChannels`, `BroadcastNotifications`, `Notifier::with_broadcast` |
| `notifications-queue` | `notifications`, `jobs` | `Notifier::queue`, `NotificationQueue`, `QueuedMail`, `MAIL_JOB`, `register_mail_handler` |

None of the four adds a crate to the dependency graph. `notifications` is
`mail` plus the unconditional `thiserror`; `notifications-db` rides the `sqlx`
that `database` already brings, with `serde_json` and `getrandom`
unconditional; `notifications-broadcast` is `realtime`, which is `tokio`,
`futures` and `bytes` — axum is unconditional and no feature turns it on;
`notifications-queue` is `jobs`, which is `database` plus `tokio` and
`tokio-util`, both of which the default feature set already brings.

### Why four and not one

`notifications` implies `mail` rather than splitting a channel-less core into
its own feature. Mail is the channel a notification system is overwhelmingly
used for, and the alternative — a `notifications` that can deliver nothing
plus a `notifications-mail` on top — would be two features and two powerset
dimensions to spare a dependency the same application has almost certainly
already enabled.

The other three earn their separation because each costs something a
mail-only application should not pay:

- **`notifications-db` brings a schema.** A table and a migration are not a
  line in `Cargo.toml`; they are a thing an operator has to run and a thing a
  backup has to hold. An application that only sends mail should not carry
  them.
- **`notifications-broadcast` answers a different question from the inbox.**
  The inbox is what a recipient sees when they arrive; the broadcast is what
  they see without reloading. Wanting one is not wanting the other, so the
  cost of each is opt-in on its own.
- **`notifications-queue` changes where the work happens.** It is the only one
  of the four that moves work rather than adding work: an application enabling
  it takes on running a worker process, and one that has no worker should not
  be offered a method that writes rows nobody drains.

## Wiring the notifier

```rust,ignore
use arcature::jobs::Jobs;
use arcature::mail::Mail;
use arcature::notifications::{
    BroadcastNotifications, DatabaseNotifications, NotificationQueue, Notifier,
    PerRecipientChannels,
};

let channels = PerRecipientChannels::new(64).expect("capacity is non-zero");

let notifier = Notifier::new()
    .with_mail(Mail::new(mailer, "noreply@example.com".parse()?))
    .with_database(DatabaseNotifications::new(pool.clone()))
    .with_broadcast(BroadcastNotifications::new(channels.clone()))
    .with_queue(NotificationQueue::new(Jobs::new(pool.clone())));
```

That example needs all four features; each `with_*` past `with_mail` is gated
on its own. `Notifier::new()` (and `Notifier::default()`) has **nothing**
wired: every channel is absent until it is given a backing. `has_mail()`,
`has_database()`, `has_broadcast()` and `has_queue()` report what is there.

`Notifier` is cheap to clone. Its `Debug` prints one boolean per channel and
nothing from behind them — a `Mailer` holds SMTP credentials and a pool holds
a database URL, and a `Debug` that printed either would put it in the first
log line that formats application state.

## Sending

```rust,ignore
let delivery = notifier.send(&user, &InvoicePaid { amount_cents: 1250 }).await?;
assert!(delivery.reached(Channel::Mail));
```

The order is part of the contract: **inbox, then live push, then mail**. The
durable local record first, then the local push, then the one thing that
leaves the process. The inbox cannot fail for a reason outside the
application, so writing it first means an SMTP server that is down leaves the
notification visible in the application rather than losing it along with the
email. The reverse order would trade a recoverable failure for an
unrecoverable one.

Delivery stops at the first failing channel.

`Delivery` is returned rather than discarded because "reached nobody" is a
real outcome and an invisible one:

| Call | Answers |
| --- | --- |
| `delivery.channels()` | the channels that ran, in the order they were tried |
| `delivery.reached(channel)` | whether that channel ran |
| `delivery.queued()` | the channels handed to the queue instead of run |
| `delivery.is_queued(channel)` | whether that channel was queued rather than run |
| `delivery.is_empty()` | whether nothing ran and nothing was queued |

`channels()` and `queued()` never overlap. A job row is not a delivery, and
folding the two together would make `reached(Channel::Mail)` say yes to a row
in a table.

`Channel::Broadcast` appears in `channels()` only when at least one connection
actually received the push. Nobody connected is not a failure — it is the
ordinary state of a recipient who is not looking at the application — so it
is reported as the channel not being among the ones that ran.

### Nothing is delivered quietly

Asking for a channel the notifier was never given returns
`NotificationError::NotConfigured` instead of skipping it. A forgotten
`.with_mail(..)` at startup fails on the first send rather than becoming
password-reset emails that never arrive.

| Variant | Raised when | Needs |
| --- | --- | --- |
| `NotConfigured { channel }` | the notification rendered content for a channel with no backing | — |
| `NoAddress { key }` | mail content for a recipient with no email address | — |
| `Mail { source }` | the transport refused the message or could not deliver it | — |
| `Database { source }` | the database rejected a statement or was unreachable | `notifications-db` |
| `Decode(String)` | a stored row did not hold what the schema promises | `notifications-db` |
| `Timestamp(String)` | a stored epoch-millisecond value is not a representable time | `notifications-db`, SQLite only |
| `IdCollision { attempts }` | eight random ids were all taken | `notifications-db` |
| `Entropy` | the OS randomness source was unavailable | `notifications-db` |
| `Encode(String)` | a broadcast payload could not be serialized | `notifications-broadcast` |
| `Queue { source }` | the job row could not be written | `notifications-queue` |
| `QueueNotConfigured` | `Notifier::queue` was called with no queue wired | `notifications-queue` |

`NotConfigured` is also what you get for `Channel::Database` or
`Channel::Broadcast` when the *feature* is off entirely, rather than a compile
error. The trait methods exist in every build, so the mistake surfaces on the
first send, naming the channel that has no backing.

`QueueNotConfigured` is feature-gated where `NotConfigured` is not, because a
louder signal exists there: `Notifier::queue` cannot be called without
`notifications-queue`, so the same mistake is already a compile error.

## The mail channel

The mail channel is the one `notifications` itself brings. A `MailContent`
goes through the same `Mail::to(..).send(..)` path a hand-written `Mailable`
uses, so address parsing, the `From` header and the transport's error mapping
stay in one place. See [Mail](mail.md) for the transport.

Two failures are distinguished. A notification that returns `None` from
`to_mail` for a recipient with no address is not an error — it decided mail
does not apply. A notification that returns content anyway for a recipient
with no address is a contradiction, and raises `NoAddress { key }`.

## The database channel: an in-app inbox

`notifications-db` adds a table. Enabling the feature is not enough; the
schema has to be created.

### The table and its migration

`DatabaseNotifications::migrate()` creates `arcature_notifications` and its
two indexes. It is idempotent, records what it applied in
`arcature_notifications_schema_migrations`, and is safe to run from every
replica at once: PostgreSQL takes `pg_advisory_lock(71420006)`, MySQL takes
`GET_LOCK('arcature_notifications_migrate', 10)`, and SQLite takes no lock
because it serialises writers itself and every statement is `IF NOT EXISTS`.
Call it at startup, or run the bundled SQL alongside the application's own
migrations.

One row per notification delivered to this channel:

| Column | PostgreSQL | SQLite | MySQL 8 |
| --- | --- | --- | --- |
| `id` | `BYTEA` primary key | `BLOB` primary key | `BINARY(16)` primary key |
| `notifiable_key` | `TEXT NOT NULL` | `TEXT NOT NULL` | `VARCHAR(191) NOT NULL` |
| `kind` | `TEXT NOT NULL` | `TEXT NOT NULL` | `VARCHAR(191) NOT NULL` |
| `data` | `JSONB NOT NULL` | `TEXT NOT NULL` | `JSON NOT NULL` |
| `read_at` | `TIMESTAMPTZ`, nullable | `INTEGER` epoch ms, nullable | `DATETIME(6) NULL` |
| `created_at` | `TIMESTAMPTZ NOT NULL DEFAULT now()` | `INTEGER NOT NULL`, computed default | `DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)` |

Indexes: `arcature_notifications_inbox_idx` on `(notifiable_key, created_at
DESC)` for the listing, and `arcature_notifications_unread_idx` on
`(notifiable_key, read_at)` for the badge.

Three things about that schema are decisions rather than accidents:

- **`notifiable_key` is not a foreign key.** A notification is a record of
  something that was said, and it should outlive a soft-delete or an account
  merge rather than vanish with it. The cost is that nothing cascades: an
  account deletion has to call `delete_all_for` itself.
- **`read_at` is nullable, and null is the whole meaning of unread.** A
  boolean would answer "has this been read" and nothing else; a timestamp
  answers "when", which is what an inbox grouping by day and a support
  engineer reading a complaint both need.
- **There is no expiry column.** Unlike an API token, a notification is not a
  credential and nothing gets safer by dropping it on a schedule.

SQLite stores both timestamps as epoch milliseconds because it has no
timestamp type: text timestamps compare correctly only while every writer
agrees on the format down to the digit, and integers always do. Sub-millisecond
precision is dropped there.

### Reading and writing the inbox

```rust,ignore
let inbox = DatabaseNotifications::new(pool);
inbox.migrate().await?;

let row = inbox
    .store("user:42", &DatabaseContent::new("invoice.paid", serde_json::json!({ "amount": 4200 })))
    .await?;

assert_eq!(inbox.unread_count("user:42").await?, 1);
assert!(inbox.mark_read("user:42", row.id()).await?);
```

| Call | Returns |
| --- | --- |
| `store(key, &content)` | the written `StoredNotification` |
| `inbox(key, limit)` | that recipient's notifications, newest first, at most `limit` |
| `unread(key, limit)` | the unread ones only, same order and bound |
| `unread_count(key)` | `u64`, a `COUNT` rather than the length of a listing |
| `mark_read(key, id)` | `bool` — whether the statement changed a row |
| `mark_all_read(key)` | `u64` rows affected |
| `delete(key, id)` | `bool` — whether it existed |
| `delete_all_for(key)` | `u64` rows affected |
| `prune_read_before(cutoff)` | `u64` rows affected, across all recipients |
| `pool()` | the `NotificationPool` underneath |

`NotificationPool` is the application's own pool — the same `Pool` the
`database` feature exposes. The inbox opens no connection of its own.

There is no unbounded listing. `limit` is mandatory on both readers, because
an inbox read is a page render and a method that could return every
notification a long-lived account ever received is a memory spike waiting for
the one account that has them.

`unread_count` is the badge. It is a `COUNT` because the number next to a bell
is asked for on far more page loads than the inbox is opened, and it should
not cost the rows.

`mark_read` returning `false` does not say which of three things happened: no
such notification, somebody else's notification, or one already read. That is
deliberate — a handler that could distinguish "not yours" from "does not
exist" is an oracle for which ids exist, and none of the three calls for a
different response. A notification that was already read keeps its original
read time, because the statement carries `read_at IS NULL`.

`StoredNotification` exposes `id()`, `notifiable_key()`, `kind()`, `data()`,
`read_at()`, `is_read()` and `created_at()`. The payload is a
`serde_json::Value` rather than a typed struct: the rows one query returns
were written by different notifications with different shapes, and a list that
could hold only one shape would not be an inbox. Match on `kind()` first, then
deserialize.

### The inbox cannot be read across recipients

Every method takes the recipient key, including the ones that already have an
id, and the key is in the `WHERE` clause rather than checked in Rust
afterwards. There is no statement in the store a handler can reach with an id
alone. Passing somebody else's notification id returns `false`, not a
deletion.

This is the difference between an ownership check a handler can forget and one
it cannot reach around. An inbox is exactly the endpoint that grows an
insecure-direct-object-reference bug.

`prune_read_before` is the single exception, and it is scoped by `read_at`
instead: it can only reach rows a recipient has already seen.

### Ids are random

A `NotificationId` is 16 bytes from the OS randomness source, with no
fallback — an id drawn from a clock is guessable, and `Entropy` is reported
rather than worked around. `store` draws a fresh id and retries on a clash up
to eight times before returning `IdCollision`; eight collisions on a 128-bit
id is not chance, it is a randomness source that is not random.

Random rather than sequential because the id appears in the URL a "mark as
read" button posts to. Guessing one still gets nobody anywhere, since every
statement is recipient-scoped too, but it makes the two defences independent
rather than one defence written twice.

`NotificationId::from_hex(text)` parses the 32-character spelling that arrives
from a route parameter, returning `None` for anything that is not exactly 32
hex digits. `to_hex()` writes it back in lowercase; uppercase input parses to
the same id.

### Retention

Nothing expires on its own. An unread notification is still worth reading a
month later, so how long an inbox keeps history is an application decision,
made by calling `prune_read_before(cutoff)` on whatever schedule suits.

That sweep only ever touches notifications that were *read*. An inbox that
quietly empties itself of things nobody has seen is worse than one that grows.

## The broadcast channel

`notifications-broadcast` pushes to whoever is connected right now, over the
`realtime` WebSocket and SSE machinery.

`realtime` offers one thing: a `Broadcast`, a bounded fanout where every
subscriber receives every message. That is right for "the build status
changed" and wrong for something addressed to a person — publishing
notifications onto one shared `Broadcast` would hand every connected user
every other user's notifications.

So the broadcast channel is not a channel. It is a `BroadcastChannels`
resolver: given a recipient key, hand back the `Broadcast` that recipient's
connections are subscribed to, or `None`.

```rust,ignore
pub trait BroadcastChannels: Send + Sync + fmt::Debug {
    fn channel_for(&self, notifiable_key: &str) -> Option<Broadcast>;
}
```

Targeting is then which channel the bytes go into, not a filter applied
afterwards. There is no code path that puts one recipient's payload into
another's channel, so there is no rule for a handler to remember.

If you write your own resolver — grouping by tenant, team or document — the
contract is that everything subscribed to the returned channel is entitled to
see this recipient's notifications. A resolver that maps two people onto one
channel to save an allocation has turned a targeted notification into a leak,
and nothing downstream can detect it.

### `PerRecipientChannels`

The built-in resolver: one `Broadcast` per recipient key, created when the
first connection subscribes.

```rust,ignore
let channels = PerRecipientChannels::new(64).expect("capacity is non-zero");

// A websocket handler subscribes the connection it has accepted.
let subscription = channels.subscribe("user:1");
assert_eq!(channels.connections("user:1"), 1);
```

`new(capacity)` returns `Option<Self>` and gives `None` for a capacity of
zero — a channel that can hold nothing drops every message. There is no
default capacity; the argument is mandatory. It is per recipient, and it
bounds how far one connection may fall behind before it starts missing
messages. It does not need to be large: a notification the recipient missed is
still in the inbox if `notifications-db` is on, and a connection thousands of
notifications behind has a problem a bigger buffer postpones rather than
solves.

Dropping the `Subscription` releases the connection. The map entry is
reclaimed lazily, on a later call to `subscribe`, which keeps the drop path
free of a lock. `connections(key)`, `len()` and `is_empty()` report the shape;
`len()` counts entries including unswept ones, so it is a metric rather than a
count of who is online.

`channel_for` deliberately does not create. A resolver that created a channel
per push would grow the map once per notification sent to someone offline —
which is most of them — and none of those channels would have a subscriber to
sweep it away.

`Debug` on `PerRecipientChannels` prints the capacity and the entry count,
never the keys: the keys are recipient identifiers, and printing them would
put a list of everyone currently online into a log line.

### What reaches the browser

`BroadcastNotifications::push(key, &content)` publishes the JSON object
`{"kind": <kind>, "data": <data>}` and returns how many connections received
it. A subscriber gets those bytes verbatim. Nothing filters them on the way
out, so a field the recipient should not learn does not belong in `data` even
if the page would not display it.

`Ok(0)` means the recipient has no live connection. The only error is
`Encode`, for a payload that could not be serialized. A recipient with no
channel, a channel with no subscribers, and a channel whose last subscriber
dropped between the lookup and the send all report zero.

### The inbox and the push are complements

The push is what a recipient sees without reloading; the inbox is what they
see when they arrive. A recipient who was offline missed the push and lost
nothing, provided the inbox was written too.

An application enabling `notifications-broadcast` alone is choosing
best-effort delivery.

## Queueing the mail channel

`Notifier::send` talks to the SMTP server while the request is still open.
`Notifier::queue`, behind `notifications-queue`, writes a job row instead and
lets a worker do the talking.

```rust,ignore
let delivery = notifier.queue(&user, &InvoicePaid { amount_cents: 1250 }).await?;
assert!(delivery.is_queued(Channel::Mail));
assert!(!delivery.reached(Channel::Mail));
```

Wiring a queue changes nothing about `send`, which still sends inline. The two
are separate methods so that a handler asking to defer is saying so, rather
than finding out from whether startup happened to call `with_queue`.

### Only mail is queued

The inbox row and the live push still run inline, in the same order `send`
runs them. Both for reasons about correctness rather than taste:

- The **inbox** is a write to the same database the job row goes into.
  Deferring it would buy nothing and cost the guarantee that matters: a
  recipient who opens the application immediately after the event would find
  an empty inbox, because the row they are looking for is sitting in a queue
  behind it.
- The **live push** reaches the connections held by *this* process. A worker
  is a different process and holds none of them, so queueing a push is not
  deferring it — it is dropping it.

A queued send is an inline send with one thing moved: the part that leaves the
machine.

### What the latency claim is

The request stops waiting on SMTP. A connection, a TLS handshake, and a server
that may itself be waiting on a DNS lookup become one `INSERT` into a table
the request is already connected to.

The *variation* goes with it. How long an SMTP conversation takes depends on
the address at the other end — whether the domain resolves, whether the server
greylists, whether the recipient exists — and a handler that answers a
registration form at a speed that depends on those things is telling anyone
with a stopwatch which addresses are already taken. The enqueue costs the same
for an address that will bounce as for one that will not.

It does **not** make the handler constant-time. The inbox write and the live
push still happen inline, and password hashing — the usual reason a
registration handler is timed — is somewhere else entirely. This removes one
oracle, not the category.

### An email can arrive twice

[Jobs](jobs.md) is at-least-once. A worker that hands a message to the SMTP
server and then dies before marking the row complete leaves a job another
worker will claim, and the message is sent again.

That is not a bug that can be fixed here. Handing bytes to a remote server and
recording that you did are two operations in two systems, and no amount of
care makes them one. Anything whose *second* delivery is harmful — a one-time
code consumed on send, an email that charges a card — should not rely on the
send being the only record that it happened.

### The job, and the worker that runs it

`MAIL_JOB` is the shared identity: kind `"arcature.notifications.mail"`,
version 1, three attempts. It is public because the two halves live in
different processes — the web process enqueues against it, the worker
registers a handler for it — and a disagreement of one character would leave
jobs sitting in the table with nobody to run them. The kind is namespaced
under `arcature.` because the table is shared; an application's own job called
`mail` would otherwise collide.

```rust,ignore
// In the worker process.
use arcature::jobs::Registry;
use arcature::notifications::register_mail_handler;

let mut registry = Registry::new();
register_mail_handler(&mut registry, mail)?;
```

The `Mail` given to the worker need not be the one the web process was built
with, and usually is not — once its mail goes through the queue, the web
process may have no SMTP credentials at all. Registering twice returns
`RegisterError::AlreadyRegistered`, because two registrations mean two
transports for one job and which one wins is an accident of call order.

Retry classification is the one decision the handler makes. A message the
transport could not *build* — a malformed address, a body that is not valid
MIME — is permanent; nothing about waiting fixes an address that will not
parse. Everything else is retryable, because SMTP reply codes are advisory and
a 5xx from a misconfigured relay is not the recipient's fault. Retrying a
genuinely permanent failure costs two extra attempts; treating a temporary one
as permanent costs the email.

### What is stored in the row

`QueuedMail` holds the *rendered* email — `to`, `subject`, `text` and an
optional `html` — not the notification.

Laravel serializes the notification object and re-renders it in the worker.
That needs every notification to be serializable, plus a registry mapping a
stored type name back to a Rust type, and it means the content is produced by
whatever version of the code the *worker* is running, which during a deploy is
not the version that decided to send it.

Here the render happens in the request, where it is a few string allocations,
and what is stored is the result. Nothing new is required of `Notification`, a
notification holding borrowed data queues as well as any other, and the email
that arrives says what the code that sent it meant.

The cost is that the payload carries the body rather than a reference to it,
so a large email is a large row. `MAIL_JOB` inherits the queue's default
payload limit, and an oversized payload is refused at enqueue rather than
truncated.

`QueuedMail::request()` hands back the `JobRequest`, which is what an
application needs to enqueue the mail in the *same* transaction as whatever
caused the notification, via `Jobs::enqueue_tx`. Enqueueing outside that
transaction is a job that runs for a change that then rolled back.

`NoAddress` is checked at `queue` time rather than in the worker. An address
that does not exist is not going to appear by the time the job runs, and
failing now puts the error in the request that caused it instead of in a dead
job row. `QueueNotConfigured` is likewise loud rather than falling back to an
inline send — the fallback would take exactly the latency the caller asked to
avoid, and only under load, which is when it is least affordable and hardest
to see.

## `arc make:notification`

```
arc make:notification InvoicePaid
```

Writes `app/notifications/invoice_paid.rs` and registers the module in the
sibling `mod.rs`.

The generated file implements **all three** channels, because every method on
`Notification` defaults to `None`, which makes a channel nobody considered
indistinguishable from a channel that was considered and declined. Deleting
the two that do not apply is how the decision gets recorded.

The `kind` string is a named constant — `const KIND: &str = "invoice.paid"`,
the file stem with underscores turned into dots — with the reason beside it.
It is not derived from the Rust type, so renaming the type stays free and
changing the protocol stays a migration.

It is one of the few `make:` kinds whose output does **not** compile in a
fresh application. `arc new` does not enable `notifications`, and the
generator does not edit `Cargo.toml`: a generator that reaches into the
manifest is a generator that can break a build it was never pointed at. The
artifact's notes name the feature instead, and add that `to_database` and
`to_broadcast` render whatever the features are but need `notifications-db`
and `notifications-broadcast` to deliver.

## Limits

**What needs a migration.** Only `notifications-db`. It adds
`arcature_notifications` and `arcature_notifications_schema_migrations`.
Enabling the feature does not create them — call
`DatabaseNotifications::migrate()` at startup or run the bundled SQL yourself.
An inbox whose table is missing fails on the first notification, which is the
same outage discovered later. Nothing else here touches the schema: the mail
channel has no storage, the broadcast channel has none, and
`notifications-queue` writes into the tables [Jobs](jobs.md) already owns
through `jobs.migrate()`.

**What needs a worker process.** Only `notifications-queue`. `Notifier::queue`
writes a row and returns; unless something runs a `Worker` whose registry had
`register_mail_handler` called on it, the rows accumulate and none of the
emails are sent. Nothing warns.

`arc queue work` is **not** that something, and reaching for it is the
mistake this paragraph exists to prevent. It builds a worker with an empty
registry — it sweeps expired leases, marks jobs it has no handler for as
dead, and prints a note saying so. Pointed at a queue of notification mail it
will discard the rows rather than send them. Real dispatch is the
application's own in-process worker, through `ApplicationBuilder::jobs`.

Registering the handler is a second, separate step: enabling the feature and
running a worker are not enough on their own, because a worker with no
registration for `arcature.notifications.mail` leaves the rows exactly where
they are — or, with `arc queue work`, does something worse than leave them.

**The broadcast is per process, and there is no switch.** `Broadcast` wraps a
`tokio::sync::broadcast`, a channel between tasks inside one process. A push
from instance A reaches only the connections held by instance A. Nothing
errors and nothing warns — subscribers on instance B never see it. This bites
notifications harder than the rest of `realtime`, because a notification is
exactly the kind of thing an application sends from a background worker, and a
worker holds none of the web process's sockets: a push from a queue worker
reaches nobody at all. Until a cross-process bridge exists, an application
running more than one instance should treat the push as an optimisation over
the inbox and enable `notifications-db` alongside it. The same limit, and the
three honest ways to live with it, are set out in
[Deployment](deployment.md).

**A failed send does not report what already succeeded.** Delivery stops at
the first failing channel and the whole call returns `Err`, so a mail
transport failure after the inbox row was written gives you
`NotificationError::Mail` and no `Delivery`. The row is still there. The
channel order exists so that this is the recoverable direction, but the caller
cannot learn from the error which earlier channels ran.

**No deduplication and no delivery log.** Sending the same notification twice
writes two inbox rows and sends two emails. `Delivery` is returned to the
caller and stored nowhere.

**No preference storage.** There is no opt-out table and no per-channel
subscription model. The mechanism is returning `None` from a channel method
for that recipient; where the preference is kept is the application's
decision.

**No templating.** `MailContent` takes strings. The HTML body is used
verbatim, and nothing escapes what a caller interpolates into it — an email
body is a good place to land a phishing link. Render it through a template
engine that escapes.

**Three channels, no extension point.** Mail, database and broadcast are the
methods on the trait; `Channel` is `#[non_exhaustive]` and a fourth would be
added here rather than by an application. There is no SMS, push-notification
or chat channel.

**Nothing wires it for you.** There is no `Application::notifications`. Build
the `Notifier` and put it in state.
