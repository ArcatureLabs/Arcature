# Changelog

All notable changes to Arcature are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Arcature is in `0.x`, where SemVer shifts one field left and Cargo follows it:
a **minor** bump (`0.1` -> `0.2`) is the breaking bump, and a **patch** bump
(`0.1.0` -> `0.1.1`) is backwards compatible. `arcature = "0.1"` therefore
accepts patches and refuses `0.2`, which is the protection a caret requirement
gives at `1.x` and above. No extra pinning is needed.

**The public API is not frozen.** `0.x` says so, and it is meant literally:
any release before `1.0` may remove or reshape a public item. Before `1.0`
can be tagged, `test_kit`, `uag` and `oauth` have to be exercised by real
applications rather than only by their own tests, and the "Not yet
implemented" list below has to be empty or deliberately closed out.

**`0.1.0` restarts the version number.** The `arcature` name on crates.io
already carries an earlier line, `2026.0.0` through `2026.2.1`, published from
a predecessor repository that spread the framework over thirteen crates and
was then abandoned. This repository is a rewrite down to the crate graph
rather than an upgrade of it: no item, no feature name and no crate boundary
survives unchanged, so there is no migration path from `2026.x` and none is
offered. Those four versions are yanked when `0.1.0` publishes -- a yank
removes a version from new resolution and leaves every existing lockfile
resolving exactly as before, so nothing that already depends on `2026.x`
breaks. `arcature-macros` has no prior release, and `0.1.0` below is
therefore its first.

## [Unreleased]

### Added

- **An application encrypter, behind the new off-by-default `crypt`
  feature.** `crypt::Encrypter` seals bytes with XChaCha20-Poly1305 into a
  versioned, URL-safe token and refuses to return a single byte of one that
  has been altered -- a tag mismatch is an error, never a partial plaintext.
  The X variant is deliberate: its 192-bit nonce makes a randomly generated
  nonce per message safe, where AES-GCM's 96-bit nonce has a birthday cliff
  that costs both confidentiality and integrity. The key is a labelled
  32-byte subkey of `APP_KEY` rather than `APP_KEY` itself, so the encrypter
  and every future consumer are domain-separated by construction. The
  dependency is pure Rust: no C, no assembly, nothing from OpenSSL.
- **Signed, expiring URLs, behind the new off-by-default `signed-urls`
  feature.** `crypt::UrlSigner` mints a link that carries its own proof of
  origin and, optionally, its own deadline, so "let this one person fetch
  this one thing" needs no account, no database row and no lookup. The MAC
  is HMAC-SHA256 over the path and **every** query parameter including
  `expires`, which is what stops a recipient from typing a bigger number
  into their own link. Parameters are sorted and percent-decoded before the
  MAC is taken, so a mail client that reorders a query does not break the
  link while an edit to one still fails. The presented signature is compared
  with `subtle::ConstantTimeEq` and never with `==`: an early-returning
  comparison is a timing oracle that turns 2^256 guesses into a few
  thousand. Expiry is read from an injectable `Clock`, so an application can
  test "this dies in an hour" without waiting one. A separate feature from
  `crypt` on purpose -- an application that only signs links should not pull
  an AEAD into its dependency graph.
- **CI runs the suite against all three SQL dialects.** A new `Database`
  matrix job builds `jobs,test-kit` once per driver and points
  `ARCATURE_TEST_DB_URL` at a live PostgreSQL 17, MySQL 8, or SQLite file.
  Until now the only database CI ever started was PostgreSQL, so the
  MySQL and SQLite statement text was proven to compile and never proven
  to parse. The job is required by the `CI success` gate.
- **The database tests run instead of being skipped by default.** The two
  tests that need a live server carried `#[ignore]`, which meant no
  ordinary `cargo test` anywhere -- laptop or CI -- ever ran them. They now
  ask `TestDatabase::optional()` for a database and return early when none
  is configured, so a machine with no server stays green, while
  `ARCATURE_REQUIRE_TEST_DB=1` turns that skip into a failure. CI sets it
  on every leg that starts a database, so a leg whose service failed to
  come up can no longer skip its way to a pass. `just db-test` runs the
  same thing locally, one build per driver.
- **The PostgreSQL claim is proven to hand each job to one worker.**
  `src/jobs/dialect/postgres.rs` had no tests at all, so `FOR UPDATE SKIP
  LOCKED` and `RETURNING` were only ever checked by reading them. Eight
  claimers now race over forty jobs against a live server, in batches of
  five and again one at a time, and every job must come back owned by
  exactly one worker with `attempts` at 1. A shared fixture,
  `src/jobs/test_support.rs`, migrates and empties the table and hands out
  an exclusive lock, since the claim is blind to `kind` and two tests
  running at once would take each other's jobs.
- **The MySQL pick-then-mark claim is proven exclusive.** MySQL 8 has no
  `RETURNING`, so its claim reads a set of rows and then marks them one at
  a time -- a window the other two dialects do not have, and one that only
  opens under contention. `src/jobs/dialect/mysql.rs` now runs the same
  eight-claimer race as PostgreSQL against a live server, plus a direct
  test that `CLAIM_MARK`'s `AND status = 'pending'` refuses a row another
  claimer already took. Drop `FOR UPDATE SKIP LOCKED` from the pick and
  these fail with two workers holding one job.
- **The SQLite `BEGIN IMMEDIATE` claim is proven exclusive, and proven to
  wait.** SQLite has neither `SKIP LOCKED` nor `RETURNING`: it excludes
  claimers instead of letting them past each other, which is only correct
  if the write lock is taken before the pick rather than at the first
  write. `src/jobs/dialect/sqlite.rs` now runs the same race as the other
  two dialects, with a batch of one so all eight connections contend for
  every row. Because the fixture treats a claim error as a failure rather
  than an empty batch, a `SESSION_SETUP` that stopped reaching the
  connection now fails the suite with `SQLITE_BUSY` instead of quietly
  losing throughput.
- **The fencing token is proven to reject a zombie worker.** Every
  completion mutation in `src/jobs/complete.rs` fences on `id = ? AND
  status = 'running' AND claim_token = ?`, and whether that clause matches
  nothing is a question only the server can answer. The new tests build a
  zombie the way production does -- let the lease expire, sweep, let a
  second worker reclaim -- and then require the first worker's success,
  retry, death and heartbeat all to be refused while the second worker's
  are accepted. The claim token is per claim, so the reclaim's token
  differs and the stale writes land on nothing.
- **The job migrations are run against all three dialects.** The existing
  tests split the migration files into statements; nothing ever handed
  those statements to a server, so three grammars were being trusted on
  the strength of one. `src/jobs/migrate.rs` now applies the bundled
  migration for real, checks the history table records it, enqueues into
  the table it created, applies twice more to prove idempotence and that
  the dialect's advisory lock is released, and applies inside a
  transaction the caller then rolls back. Which dialect is exercised is
  the build's choice of driver, so CI's `Database` matrix covers all
  three.
- **`AppConfig` consumes `APP_URL` and `APP_NAME`.** Both were parsed from the
  environment, stored, and read by nothing; the changelog entry below promising
  that a `1.0` cannot ship until they are consumed was the only place either
  had an effect. Two accessors make `url` reachable rather than merely
  present: `AppConfig::base_url` returns it with trailing slashes trimmed, and
  `AppConfig::absolute_url` joins a path onto it with exactly one separator.
  `absolute_url` *joins*, it never substitutes -- a path of `//evil.test`,
  `///evil.test` or `https://evil.test` lands as a path segment under
  `APP_URL`, because the leading slashes are stripped before the join. That
  property is what makes the accessor safe to reach for from the signed-URL
  and mail-link code that will call it, and it is pinned by a test. `name` and
  `base_url` are also now what the process announces at startup: the
  `listening` line carries both, so the application says which application it
  is and at which public address, instead of only the socket it happened to
  bind. Nothing was removed and no signature changed; `AppConfig::url` and
  `AppConfig::name` remain the fields they were. Closes #9.
- **Per-page document metadata: `inertia::Head`.** A page title, meta
  description, canonical URL, and the Open Graph and Twitter card fields,
  rendered as HTML by `Head::to_html`. Every value is HTML-escaped by the
  setter that stores it rather than by the renderer that writes it, because a
  page title is routinely a database row and a renderer that forgets is a
  stored-XSS hole; the consequence to know is that the accessors return
  escaped text. `og:title`, `og:description` and `og:url` fall back to the
  title, description and canonical URL, and `twitter:card` defaults to
  `summary_large_image` or `summary` -- X renders no preview at all without
  that tag. Nothing here executes JavaScript: this is meta tags for scrapers,
  not server-side rendering.
- **`ScriptBody` carries the page head.** `ScriptBody::head` hands a root
  document the `Head` for the page it is about to wrap, so a custom
  `Fn(ScriptBody) -> String` can put real `<title>` and `og:` tags in the
  document it builds instead of one title shared by every route. The head
  travels beside the markup rather than inside it because only the root
  document knows where its own `<head>` element is. `RootDocument::render`
  keeps its signature and every existing closure keeps compiling.
- **A handler can set the page head: `Inertia::with_head`.** Also
  `Inertia::set_head` for `&mut` and branch-by-branch construction, and
  `Inertia::head` to read back what is set. The head is rendered on a first
  visit, where the server writes the document; an Inertia visit is JSON for a
  client-side router that already owns the document, so a head set there is
  inert rather than wrong. `render_page` fills in a title humanised from the
  page contract's component name when the handler set none -- a change in the
  `<title>` such pages emit, on the view that every route in an application
  sharing one title is a defect, not a default.
- **The stock root documents emit the page head.** `default_root_document`
  and `vite_root_document` write the `Head` a handler set into the `<head>`
  they build, so a scaffolded application gets a server-rendered `<title>`,
  meta description and `og:` tags without hand-writing a root document. The
  application title passed to either function is the *fallback* for the page
  title, never a prefix for it -- a page that says what it is says it alone --
  and it is escaped on the way out even though it comes from configuration.
  A page with no head renders exactly the document these functions rendered
  before.
- **A `uploads` feature, off by default.** It turns on axum's
  `multipart/form-data` body parser, and nothing else in the crate reads a
  multipart body without it. The default stays off because an upload
  endpoint is the largest attacker-authored surface a web application has --
  the filename, the declared content type and the byte count are all written
  by the client -- and a build with no upload route should not carry the
  parser for one. `uploads` implies `validation` and `storage-fs`: an upload
  reports RFC 9457 problem details like every other extractor, and it is
  written to a storage disk rather than to a path the request named.
- **A filename sanitizer, `SafeFilename` and `StoragePath::from_filename`
  (feature `uploads`).** A `filename=` parameter is not a name, it is an
  argument to whatever opens it next. `StoragePath::new` validates and
  rejects, so `Ảnh chụp màn hình.png` -- an ordinary filename -- failed a
  check that was never aimed at it, and a sanitizer that rejects real names
  teaches applications to bypass the sanitizer. The new path discards
  directory components before anything else runs, normalizes to NFC, and
  then splits the input two ways: characters a human plausibly typed that
  are only dangerous downstream (`:`, `*`, `?`, `<`, `>`, `|`, `"`, and the
  inner dot of `shell.php.jpg`) are repaired to `_`, while characters whose
  presence *is* the attack (NUL and the other C0/C1 controls, DEL, the bidi
  overrides that make `invoice<U+202E>gpj.exe` render as a `.jpg`, the
  zero-width joiners, the byte-order mark, the blank-rendering separators and
  the whole U+E0000-U+E007F tag block) are fatal. Variation selectors are
  deliberately kept: they occur beside an emoji in names people really have.
  Windows device names are refused with any extension and with trailing dots
  or spaces, so `CON.txt` and `CON.foo.txt. ` are both rejected. Extensions
  are checked against an `AllowedExtensions` whitelist -- never a blacklist
  -- of lowercase ASCII alphanumerics; `AllowedExtensions::images()`
  deliberately omits `svg`, which is a scriptable XML document. The result is
  always exactly one path segment. It is still only metadata: the object
  itself is meant to be stored under a name derived from its own bytes.
- **Content-addressed object names, `ContentAddress` and `ContentHasher`
  (feature `uploads`).** Path traversal is a family of bugs, not one bug --
  `../`, `..\`, `%2e%2e%2f`, `....//`, `..%c0%af`, a NUL that truncates the
  name in the C library underneath, an NTFS alternate data stream, a symlink
  a previous upload planted -- and a sanitizer answers them one at a time,
  which means it is only ever as complete as the last person who thought
  about it. Naming an object after `SHA-256(bytes)` plus its whitelisted
  extension answers all of them at once: no byte of the request reaches the
  path, so there is nothing left for a payload to be in. The key fans out as
  `ab/cd/<digest>.<ext>` so a filesystem disk gets 65,536 leaf directories
  rather than one with a million entries; identical bytes deduplicate and
  re-uploads become idempotent. `ContentHasher` is fed a chunk at a time, so
  the digest never requires the object in memory. This does not replace the
  filename sanitizer -- the sanitizer makes the *metadata* safe to display,
  content addressing makes the *path* safe to resolve -- and an unguessable
  key is not authorization: authorize the download.
- **Bounded multipart bodies, `MultipartLimits` and `BoundedMultipart`
  (feature `uploads`).** A body-size cap is three quarters of an answer. It
  says nothing about part *count* -- fifty thousand parts of two bytes each
  fit inside a 1 MiB body, and the cost of a part is not its length but a
  header parse, an allocation, a filename sanitization and a storage round
  trip -- and it says nothing at all about a client that sends a byte a
  minute, because a request that never finishes never exceeds anything. The
  new `MultipartLimits` carries all four bounds: a total, a per-part cap, a
  part count (default 32) and a per-read timeout (default 30s) enforced with
  `tokio::time::timeout`. The count is checked *before* the part is parsed,
  and a chunk that crosses a byte cap is counted and then refused rather than
  returned. This is the inner bound, not a replacement for the outer one:
  stage 12 of the pipeline (`tower-http`'s `RequestBodyLimitLayer`) still
  applies to the body before a byte reaches the parser, nothing here can
  raise it, and `MultipartLimits` is a `tower::Layer` so one upload route can
  be made stricter than the rest of the application without loosening
  anything. A route with no layer gets the conservative defaults; there is no
  way to end up unbounded. Failures report RFC 9457 problem details with a
  fixed per-category `detail`, never axum's parser message, which can quote
  header bytes the client wrote. `BoundedField::declared_content_type` is
  named for what it is -- a claim by the client about bytes the client also
  chose.
- **Streaming uploads, `Disk::begin_upload` and `UploadWriter` (feature
  `uploads`).** Reading a whole upload into a `Vec<u8>` before storing it
  makes the request's peak memory a function of what the client chose to
  send, and multiplies it by the number of clients: a hundred concurrent
  8 MiB uploads is 800 MiB of heap that no size cap prevents, because every
  one of them is within the cap. `UploadWriter` writes each chunk through
  `Disk::writer` as it arrives and hashes it on the way past, so an upload of
  any size costs one chunk of memory and the size limits become a policy
  rather than the only thing between a request and the heap. Because the
  content-addressed key is not known until the last byte has been seen, the
  bytes go to a unique transient key under `_staging` first -- process id,
  nanoseconds and a counter, never anything from the request -- and
  `finish()` closes the object *before* moving it onto `ab/cd/<digest>.<ext>`,
  so a failed upload cannot leave a truncated object under a digest that does
  not describe it. If the destination already exists the staging copy is
  deleted rather than renamed over it: the key is the digest, so the bytes
  are already there, and a rename over a file another reader has open fails
  on Windows. `abort()` is the counterpart for a rejection after the first
  byte. Neither is optional -- dropping an `UploadWriter` leaves the staging
  object behind, and no `Drop` can fix it because both fixes are `async`.
- **Magic-byte content sniffing, `storage::sniff` and
  `UploadWriter::finish_verified` (feature `uploads`).** A multipart part
  arrives with two claims about its type and the client wrote both of them:
  `Content-Type: image/png` is a string in a header the uploader chose, and
  `filename="avatar.png"` is a string in the same header chosen by the same
  uploader. Neither is evidence. The only statement about an upload the
  client did not author is the one its first few hundred bytes make, so
  `sniff` reads those -- at most `SNIFF_BYTES` (512) of them, the one part of
  an upload ever held in memory -- and `verify` holds them and the accepted
  extension to a symmetric agreement. An extension with a known signature
  (`png`, `jpg`, `pdf`, ...) *must* sniff to it, and bytes that sniff to
  nothing are refused rather than waved through, because "unrecognized" is
  precisely what `shell.php` renamed `avatar.png` looks like. An extension
  with no signature (`txt`, `csv`) must sniff to nothing in turn, so a `.txt`
  whose first bytes are a zip header is refused too. There is no "unknown"
  state an upload can land in and be accepted by default.
  `UploadWriter::finish_verified` is the call an upload handler wants: it
  removes the staging object *before* returning the rejection, so a refused
  upload costs no disk. `UploadError` keeps the two failures apart because
  they mean opposite things -- a rejected file is a 4xx and a backend failure
  is a 5xx, and collapsing them is how an upload endpoint reports a bad file
  as an outage. Nothing here decodes: the check is a byte-prefix comparison
  against `infer`'s table and never an image parse, because a decoder is an
  interpreter for attacker-controlled input and the densest source of memory
  CVEs in any web stack. This is a statement about format, not about safety.
- **Downloads are attachments, `http::download::Attachment` (feature
  `uploads`).** Accepting an upload safely and serving it safely are two
  different jobs, and doing the first perfectly buys nothing if the second
  hands the file to a browser as a document: a stored file served inline is
  content the attacker wrote, on the application's own origin, with the
  application's cookies attached. `Attachment` closes that by construction
  rather than by asking a handler to remember, sending
  `Content-Disposition: attachment`, `X-Content-Type-Options: nosniff` and
  `Content-Security-Policy: default-src 'none'; sandbox` on every response,
  and refusing to emit a scriptable media type (`text/html`,
  `image/svg+xml`, the XML family) even when the bytes really are one -- the
  honest `Content-Type` for an HTML file handed back as a download is
  "bytes", not "document". The type is sniffed from the object's own leading
  bytes and never read from the request; `with_content_type` takes a
  `SniffedType` rather than a string, so there is no way to put a
  client-authored media type on a response. `from_disk` reads only the first
  512 bytes to decide the type and streams the rest, so serving a large
  object costs one buffer. The suggested filename takes a `SafeFilename`
  rather than a `&str` for the same reason -- a string parameter there is a
  header-injection hole waiting for the one caller who forgets -- and is
  emitted twice per RFC 6266, quoted ASCII plus an RFC 5987
  `filename*=UTF-8''`, because `báo-cáo.pdf` is not representable in the
  first form and silently mangling it is worse than sending both.
- **An uploaded-file extractor, `validation::upload::UploadedFile` (feature
  `uploads`).** An upload endpoint has to get five separate things right --
  bounds, a whitelisted extension, a sanitized filename, bytes that match
  that extension, and a rejection that does not quote the request back --
  and the failure mode of forgetting any one of them is a stored file that
  is not what it looks like. `UploadedFile` makes all five a condition of
  extraction, so a handler that compiles has them, and it reports every
  refusal as an RFC 9457 problem through `validation_problem` under a fixed
  `file` key -- never the part's own name, never the filename, and never the
  sniffed type, which would turn the endpoint into a free file-type oracle.
  `UploadPolicy` is the per-route `tower::Layer` carrying the whitelist and,
  optionally, the required part name; a route that forgot the layer gets the
  image whitelist rather than "anything goes", exactly as a route that forgot
  `MultipartLimits` still runs bounded. `from_multipart_rejection` joins its
  siblings in `validation::rejection`. This extractor buffers the part, and
  says so: an extractor completes before the handler runs, so it has nowhere
  to put the bytes but memory, bounded by `MultipartLimits::field_bytes`. It
  is the convenience for avatars; `BoundedMultipart` plus `UploadWriter`
  stays the load-bearing path for anything larger.
- **An end-to-end test that a hostile upload is refused *and stores
  nothing*.** `tests/uploads.rs` drives a real route, a real filesystem
  disk and hand-rolled `multipart/form-data` bodies that no well-behaved
  client would write, and every case asks two questions rather than one:
  the status, and what is on the disk afterwards. The second is the one
  that matters -- a 4xx with the file written anyway is not a rejection,
  it is a rejection notice attached to a successful upload. The cases are
  the ones that show up in a real log: `../../etc/passwd.png` and its
  backslash twin, a NUL in the name, `CON.png` and `LPT1.png`, `.php`,
  `.phtml`, `.exe`, `.sh`, `.jsp`, `avatar.php.png`, a PHP web shell
  renamed to each of the four image extensions with `Content-Type:
  image/png` on the part, a two-megabyte body against a sixty-four
  kilobyte cap, and a thousand-part body. `ảnh đại diện.png` is there for
  the opposite reason: sanitizing a name must not mean ASCII-folding it,
  and the composed and decomposed spellings of it must land on the same
  object. A unit test alongside them proves the 413 arrives *early*: a
  body offering a gigabyte is refused after a few hundred kilobytes were
  pulled, because a rejection issued after the body is buffered is an
  out-of-memory waiting for a slightly larger upload.

- **A TCP request now carries its peer address.** The serve path installs a
  connect-info make-service of its own, so every request accepted on a TCP
  listener arrives with `ConnectInfo<SocketAddr>` in its extensions and the
  `ConnectInfo` extractor works in a handler. Axum's
  `into_make_service_with_connect_info` was not available here: what
  Arcature serves is the composed pipeline service rather than a `Router`,
  and `ServiceExt` has no connect-info counterpart. The IPC serve path is
  unchanged and deliberately has no `ConnectInfo` -- a Unix domain socket
  or a Windows named pipe has no peer address to report.


- **The client IP behind a proxy is resolved once, on the way in.** A TCP
  request now also carries a `ClientIp` extension, produced by the same
  per-connection service that installs `ConnectInfo`. If the immediate peer
  is a trusted proxy, `X-Forwarded-For` is walked from the right, skipping
  further trusted hops, and the first untrusted entry wins; otherwise the
  peer address is used and the header is ignored entirely. The trusted list
  defaults to empty (`TrustedProxies::none()`), so out of the box no
  forwarded header is believed -- believing one unconditionally is how
  rate limits and bans get bypassed. Configure it with
  `ApplicationBuilder::trusted_proxies`, or call
  `ServeTarget::serve_with_trusted_proxies` directly. New in
  `arcature::http`: `ClientIp`, `TrustedProxies`, `ProxyNet`,
  `ProxyNetError` and `X_FORWARDED_FOR`; `TrustedProxies` and `ProxyNet`
  parse from CIDR text such as `"10.0.0.0/8, 127.0.0.1"`. The IPC serve
  path resolves nothing, having no peer address to start from.

- **The access log records the client address.** `AccessLogLayer` adds a
  `client_ip` field to the request span and the access line, taken from
  the `ClientIp` extension; it is empty when nothing resolved an address,
  the way `request_id` already is. The address is a structured field and
  never part of the rendered message: redaction decides per field name, so
  an address interpolated into the message would sit past the only
  checkpoint there is. It goes through `redact::apply` on the way out, so
  adding an address term to the deny-list would withhold it everywhere at
  once. Nothing is added to `DENY_LIST` here -- a log that redacts the
  field by default would be the same no-op the limiter was.
- **A database-backed session store, behind the new `session-store-db`
  feature.** Until now the only ready-made store was `tower-sessions`'
  `MemoryStore`, a `HashMap` in one process: every deploy logged every user
  out, and a second replica could not see the first one's sessions.
  `arcature::auth::session_store::DbSessionStore` keeps them in one
  `arcature_sessions` table in the application's own database, with an
  embedded per-dialect migration for PostgreSQL, SQLite and MySQL. Two
  properties are deliberate. The row key is the SHA-256 digest of the session
  id, never the id, because a session id is a bearer credential and a table of
  them is a table of logins that every backup and replica carries; and every
  read carries `expires_at > now()`, evaluated by the database, so an expired
  session stops working the instant it expires whether or not
  `sweep_expired` has run. The feature is off by default, so an existing build
  is unchanged.
- **Round-trip tests for the session store against a real database.** Every
  property the store claims is a property of its SQL, so a mock would only
  agree with whatever the Rust already believes. The tests save, load, delete,
  overwrite and sweep against a live server, and two of them exist to pin the
  security decisions rather than the happy path: one saves an already-expired
  session, checks the row is still on disk, and checks it does not load --
  which is the difference between expiry enforced by the query and expiry
  enforced by a cleanup task -- and one reads the stored key back to prove it
  is a thirty-two byte digest and not the session id. They are gated on
  `test-kit`, which already owns the "is a test database configured, and is it
  safe to write to" decision, and skip when no database is configured --
  `ARCATURE_REQUIRE_TEST_DB=1` turns that skip into a failure, and `just
  db-test` now builds `session-store-db` alongside `jobs` so CI's `Database`
  matrix runs them on all three dialects. SQLite needs no server, so one of
  the three is always runnable on a laptop.

- **A compiled view layer, behind the new `views` feature.** `arcature::view`
  renders [askama](https://crates.io/crates/askama) templates:
  `view(template).render()` gives a `String`, `ViewError` names the one
  failure a compiled template still has, and the certified `askama` is
  re-exported at `arcature::askama` so an application points
  `#[template(askama = arcature::askama)]` at the framework's copy rather
  than resolving a second version of its own. The engine being compiled
  rather than interpreted is the whole of the choice. minijinja, tera and
  handlebars each carry a parser and an expression evaluator that run
  *inside the request path*, and that is where server-side template
  injection lives -- the shortest route there is from a form field to remote
  code execution. Askama emits `write!` calls at build time, so at runtime
  there is no parser for hostile input to reach and the class of bug is
  absent rather than mitigated. The price, paid knowingly, is that editing a
  template needs a rebuild. Autoescaping is picked from the template's
  extension and is proven by a test that renders a `<script>` through an
  `.html` template and gets entities back. Off by default: a generated
  application renders through Inertia, and a build that never serves a
  server-rendered page has no reason to carry a template compiler.

- **A view is now a response: `View<T>` implements `IntoResponse`, and a
  render failure tells the client nothing.** A handler returns
  `view(Page { .. })` and gets a `200 OK` of `text/html; charset=utf-8`;
  `.status()` and `.content_type()` change either half, which is how a view
  serves a `404` page or an `.xml` feed. HTML is the default rather than a
  guess from the template's extension because askama 0.16 does not keep the
  extension on the compiled type -- there is no `MIME_TYPE` to read. The
  failure path is the part worth stating. A template that fails to render --
  a value whose `Display` returns `Err` -- produces the framework's ordinary
  internal error, and the askama message goes to `tracing`. Three things
  deliberately do not reach the client: the template's own text, which is
  application source; the template's path, which maps the source tree and
  the filesystem the process runs on; and whatever the failing `Display` had
  already written, which could be a token or a database row. Unlike
  `Error::into_response` and the `ErrorMapping` layer, this does not offer a
  chattier development mode, because there is no build in which a template's
  contents are a reasonable thing to send to a browser. A test renders a
  deliberately unformattable template and asserts the body contains neither
  the template text, nor the words `template` or `askama`, nor a source
  path.

- **Mail bodies can be rendered from a pair of compiled templates:
  `Email::templated` and `Email::templated_with_attachments`, behind the
  `views` feature.** A `multipart/alternative` mail carries the same message
  twice, and the two copies drifting apart is the ordinary way mail
  templating goes wrong -- the HTML half gets the new wording, the plain half
  keeps the old, and only the readers on a text client ever see it. These
  terminators take both templates in one call, so changing a message means
  changing a pair. They stay two templates rather than one because askama
  picks its escaper from the extension: the `.html` half escapes its values
  and the `.txt` half does not, and rendering a text body through an HTML
  template would post `&#38;` to someone reading plain text. Argument order
  matches the existing `Email::alternative`, plain first. Failures land in a
  new `#[non_exhaustive]` `MailViewError` rather than a new `EmailError`
  variant, because `EmailError` is not `#[non_exhaustive]` and growing it
  would break every downstream match; its conversion into the framework error
  routes a render failure through `ViewError`, so the template text does not
  reach a response body by way of the mail path either.

- **`arc new` scaffolds a `templates/` directory and one worked view.** A
  generated project now ships `templates/layout.html`, a
  `templates/welcome.html` that extends it, an `app/views/` module holding
  the `WelcomeView` struct whose fields the template names, and a
  `GET /welcome` route that renders it -- the server-rendered counterpart to
  the Inertia page already on `/`. The generated manifest turns the
  framework's `views` feature on, which is the one place the scaffold departs
  from the framework default: the framework cannot know whether a given
  application serves HTML, and this one does. Delete the two directories and
  the feature if every screen is an Inertia page. The view struct carries
  `askama = arcature::askama`, so the generated project depends on askama
  only through Arcature and cannot drift to a second version of it. A test
  generates all nine stack-and-driver combinations and asserts the templates,
  the module, the route and the feature are all present, because they live in
  four different files and any one of them missing is a project that does not
  compile. The generated `Dockerfile` copies `templates/` into the Rust
  stage, which is easy to miss: askama reads the files during `cargo build`,
  so they are source like `app/` and `routes/` rather than runtime data, and
  omitting the line would leave a tree that compiles on a laptop and cannot
  build an image at all. That line has its own assertion.
- **The OAuth authorization code flow is proven end to end against a
  provider.** `tests/oauth.rs` had twenty-nine tests and not one of them
  completed a flow: every one asserted a property of a value in isolation,
  so `authorize` and `exchange` had never been shown to agree with each
  other, let alone with a server. `tests/oauth_round_trip.rs` stands up a
  mock authorization server on a loopback port and drives the real
  `OauthClient` from `authorize()` through the redirect, the callback and
  `exchange()` to a bearer-authenticated userinfo call. The provider
  recomputes the PKCE challenge from the verifier it is handed, so the
  suite proves the challenge sent to `/authorize` really is the SHA-256 of
  the verifier sent to `/token` and that a mismatched verifier is refused;
  a callback carrying another flow's `state` never reaches the token
  endpoint; an `{"error": "invalid_grant"}` body surfaces as
  `OauthError::Provider` rather than a success; and a refreshed token set
  carries the new access token. The mock is written against the `axum` and
  `tokio` already in the tree rather than a mocking crate, and so is the
  test's own SHA-256, which is pinned against the FIPS 180-4 and RFC 7636
  vectors -- a dependency not taken is a dependency nobody has to watch for
  advisories.
- **CI runs the OAuth suite, which it never did before.** `oauth` is not a
  default feature, so the `Test` job compiled none of `src/oauth/` and ran
  none of its tests; the `Full features, per driver` job named the feature
  but only `cargo check`ed it. Twenty-nine property tests and the new round
  trip were therefore invisible on the pull-request path. A new `OAuth
  round-trip` job builds `--no-default-features --features oauth` and runs
  both test binaries, and the `CI success` gate requires it. It needs no
  secret and no network -- the authorization server is an axum router on a
  loopback port inside the test process -- so it behaves identically on a
  pull request from a fork.

- **The OTLP exporter is proven against a collector rather than against a
  builder.** `src/observe/otel.rs` had two unit tests and both of them read
  a field back off the builder that had just been handed it; the module's
  own test file said why, in as many words -- an export test "would need a
  live collector, which is an integration concern". `tests/observe_otlp.rs`
  is that collector: a `TraceService` gRPC server on a kernel-assigned
  loopback port, decoding the protobuf the exporter actually wrote, sharing
  nothing with the code under test but a socket. Seven tests pin what a
  backend needs before it can draw anything -- a span arrives at all, a
  parent and its child arrive under one trace id, the child names its
  parent's span id, a three-level nesting arrives as a chain rather than a
  fan hung off the root, `service.name` travels as a resource attribute, a
  recorded field arrives as an attribute, and a collector that never answers
  fails the export instead of panicking the process. A trace whose spans are
  all roots is worse than no trace: it looks like working telemetry right up
  until somebody needs it. The collector's two dev-dependencies (`tonic`'s
  server half, `opentelemetry-proto`) add no package the lock did not
  already carry through `opentelemetry-otlp`'s `grpc-tonic`.

- **What `/metrics` serves is now checked against the exposition format,
  not against `contains`.** Every existing test of `src/observe/metrics.rs`
  asserts that a substring is present, which cannot see the rules that make
  a document scrapeable: one `# TYPE` per name, a name's samples
  contiguous, a series appearing once, buckets cumulative and ending at
  `+Inf`. Break any of those and every one of those tests still passes
  while Prometheus rejects the scrape -- silently, because nobody reads a
  scrape target's error page. `tests/observe_prometheus.rs` writes the
  format's rules out as a parser and runs the rendered registry, the
  layer's own output and the `/metrics` response through it, checking the
  content type on the way. The parser is itself tested against sixteen
  documents that each break one rule and six that exercise what the format
  allows, because a validator nothing fails is a validator that proves
  nothing. Hand-written rather than a Prometheus client crate, for the same
  reason as everywhere else here: a dependency taken to check forty lines
  of text is a dependency somebody watches for advisories forever.

- **Redaction is now tested on the wire, in all three channels at once --
  and two of the three do not hold.** `src/observe/mod.rs` promises that
  credentials never reach "a log line, a metric label, or a span
  attribute". `tests/observe_redaction.rs` drives one request carrying a
  password, an `Authorization: Bearer` header, a session cookie and a PKCE
  verifier through a router wearing request ids, access logging, metrics
  and trace context, with a JSON sink, a metrics registry and a live OTLP
  collector capturing simultaneously, and searches every byte of all three
  outputs for each secret's value. The framework's own layers pass: nothing
  the request carried appears anywhere, and the access log records the path
  without the query string. The deny-list also holds for anything an
  application records, in the JSON log. It does **not** hold for the other
  two destinations: a field named `password` recorded on a `tracing` span is
  written as `[redacted]` to the log and exported to the collector in full,
  and a metric label named `session_id` is rendered in full, because
  `Telemetry::tracing_layer` and `Metrics` never consult the deny-list.
  Rather than work around that, the suite asserts it, in two tests named so
  nobody mistakes them for a working defence -- and it asserts its own
  channels are non-empty before searching them, so an absence can never
  pass by capturing nothing.

- **CI runs the telemetry suite, which it never did before.** `otel` is not
  a default feature, so the `Test` job compiled none of
  `src/observe/otel.rs`; `Each feature alone` built the feature without
  running a test, and `Full features, per driver` only `cargo check`ed it.
  The OTLP round trip and the end-to-end redaction proof were therefore
  invisible on the pull-request path. A new `Telemetry round-trip` job
  builds `--no-default-features --features otel` and runs both binaries,
  and the `CI success` gate requires it. Like the OAuth job it needs no
  secret and no network -- the collector is a gRPC server on a loopback
  port inside the test process -- so it behaves identically on a pull
  request from a fork. `observe_prometheus` is deliberately not in it:
  `observe` is a default feature, so that binary already runs in `Test`.
- **Fluent translation catalogs, behind the new off-by-default `i18n`
  feature.** `i18n::Catalogs` holds one `i18n::Catalog` per locale, parsed
  from the `.ftl` files an application keeps in its repository, and formats a
  message in a named locale with `i18n::TranslationArgs`. The engine is
  Mozilla Fluent rather than a `HashMap<String, String>` because the map is
  wrong the moment a language has more than two plural forms: Polish has
  four, Arabic six, Japanese one, and the rule is a CLDR table rather than an
  `if n == 1` that calling code could write. Fluent puts the selection in the
  catalog, where the translator can see it, and brings gender agreement and
  locale-aware number formatting with it. Locales are `i18n::LocaleId`, a
  validated canonical BCP-47 tag with one fallible constructor, so a raw
  request string cannot be passed where a locale is expected; the registry is
  an in-memory map built at startup and nothing in the module opens a file,
  so a hostile tag has no path to traverse. A key the chosen locale has not
  translated falls back to the default catalog per key, which is what makes a
  half-translated locale ship a readable page instead of a `500`. `.ftl`
  parsing is a runtime parser, which `views` chose askama specifically to
  avoid -- `src/i18n/mod.rs` states the difference (a catalog is
  developer-authored and never named by a request, and Fluent interpolates
  arguments without re-parsing them) and the rule that keeps it true. The
  dependency subtree adds `self_cell`, which contains `unsafe`; the
  `cargo geiger` baseline is recorded over the default feature set and `i18n`
  is not in it, so the baseline file is unchanged and that is a measurement
  gap rather than an absence of cost.

- **The generated route helper is type-checked against a parameterised
  route.** `routes.ts` emits a conditional rest-argument tuple, so that
  `route("home")` takes no second argument and `route("links.show", {...})`
  requires one. The unit tests in `src/uag/codegen/routes_ts.rs` pinned the
  *text* of that machinery; nothing pinned its *behaviour*, because the
  scaffold ships exactly one route and it has no parameters. The type could
  have collapsed to one that accepts anything and every test would still
  have passed. `tests/uag_typescript.rs` declares four route shapes -- none,
  one, two, and a wildcard -- and runs `tsc` over usage that must compile
  and, the half that carries the weight, seven snippets that must not: an
  omitted parameter object, a misspelt key, a wrong value type, one of two
  parameters, parameters passed to a parameterless route, a wildcard written
  with its star, and a route name outside the union. Each is checked in its
  own file so no failure masks another, and each is pinned to the diagnostic
  code `tsc` actually raises, so "rejected" cannot quietly become "rejected
  for an unrelated reason". The generator turned out to be correct; what was
  missing was the proof. The suite needs a TypeScript compiler and reports
  itself skipped without one -- `docs/decisions/0001-no-npm-package.md`
  means this repo installs nothing on a contributor's behalf -- so point
  `ARCATURE_TSC` at a `tsc` to run it locally.

- **CI type-checks the generated TypeScript.** Unlike the OAuth and telemetry
  jobs above, this gap was not that the tests never ran -- `uag` is in the
  default set, since `cli` pulls it, so the `Test` job already built and ran
  `tests/uag_typescript.rs` and reported it passing. It passed by skipping:
  no TypeScript compiler on the runner, three of the six tests return early,
  and a skip and a pass are the same line in the summary. A new `Generated
  TypeScript` job installs a pinned `typescript` and points `ARCATURE_TSC` at
  it, which turns a missing compiler from a quiet skip into a failed
  assertion, and the `CI success` gate requires it. No `actions/setup-node`:
  the runner image already ships Node, and `typescript` declares no
  dependencies, so the job adds exactly one package and no new pinned action.

- **The request's locale is negotiated, and it is negotiated against a
  whitelist.** `i18n::LocaleLayer` picks one registered locale per request
  and puts it in the request's extensions; `i18n::Locale` is the extractor
  that reads it, and carries `translate` and `message` so a handler asks for
  a locale rather than for a catalog and a tag. The order is an explicit
  `?lang=` (only when the application names the parameter), then a session
  entry (only when it names the key, and only under `auth`), then
  `Accept-Language`, then the default -- `i18n::LocaleSource` says which one
  answered. **A proposed tag is matched, never resolved.** It goes through
  `LocaleId::parse` and then a lookup in `Catalogs`, and a tag that fails
  either is discarded and the next candidate tried, so `../../etc/passwd`, a
  NUL, a CRLF and a 64 KiB string all yield the default and none of them ever
  becomes a `LocaleId`; nothing in `i18n` opens a file, so there is no path
  for one to traverse in the first place. `fr-CA` falls back to a registered
  `fr` and `pt` to a registered `pt-BR`, matched on the language subtag of an
  already-validated identifier rather than on a prefix of the raw bytes. The
  `Accept-Language` parse is bounded at 512 bytes and 16 candidates, honours
  `q=0` as a refusal, and is stable so equal weights keep the client's order.
  Responses get `Content-Language` and gain `Accept-Language` in `Vary` --
  merged into whatever was already there, because without it a shared cache
  serves the French page to the next English reader. A handler that asks for
  a `Locale` on a route without the layer gets a `500` that names neither the
  layer nor the catalog, rather than a language nobody negotiated.

- **The active locale reaches Inertia props and askama views.** With `i18n`
  on and `LocaleLayer` installed, every Inertia page gains a `locale` prop --
  `{ id, source, available }`, an object rather than a bare string because a
  language switcher needs the list to switch between -- and `Inertia::locale`
  hands a handler the same value for translating something itself. Three
  things it deliberately does not do: it never overwrites a `locale` prop the
  application already shares, it sends nothing on a partial reload that did
  not name `locale` in `X-Inertia-Partial-Data`, and it invents nothing when
  no layer negotiated one. On the view side, `View::in_locale` declares the
  language a view was rendered in and sends `Content-Language`; the framework
  does not infer it, because a compiled template carries no language and the
  locale the request *asked* for is not a claim about the bytes in the
  response. Translation itself stays in the template -- give the template
  struct a `Locale` field and call it -- rather than arriving as a `t("key")`
  filter, which would be exactly the runtime lookup askama was chosen to
  avoid. All of it is gated on `i18n`: an application that has not enabled
  the feature cannot reach any of these names, and its pages and views render
  byte for byte as before.


### Changed

- **A page rendered through a `PageContract` now titles itself.** Where every
  such page previously emitted the one application title passed to
  `default_root_document` or `vite_root_document`, `Inertia::render_page` now
  fills in a title humanised from the contract's component name when the
  handler set no head of its own. An application that wants the old document
  sets the head it wants explicitly, and one that never used a `PageContract`,
  or that renders through `Inertia::render`, or that builds its own root
  document from a `Fn(ScriptBody) -> String` closure, is unaffected -- such a
  closure ignores a head it was never written to read.

  This is listed separately because it changes bytes an existing 0.1.0
  application already serves, which nothing else in this release does. It is
  kept because one title shared by every route is a defect in a search result
  rather than a default worth preserving, and because a patch release is the
  cheapest moment to correct it. No signature changed and no build breaks; the
  visible difference is the `<title>` element and the `og:title` that falls
  back to it.
- **`arc new` scaffolds a persistent session store.** The generated
  `bootstrap/app.rs` wired `tower-sessions`' `MemoryStore` and carried a
  comment admitting it had to be replaced before running more than one
  instance -- which is a defect handed to the user with instructions, not a
  default. It now builds `DbSessionStore` from the same `DATABASE_URL` the
  rest of the application uses, so a deploy is a deploy rather than a mass
  logout and a second replica is a replica. `arcature_sessions` is created by
  `--migrate` alongside the application's own migrations, not on boot, for the
  reason the generated `Mode` documentation already gives: a schema change
  made as a side effect of starting is one every replica races to make. The
  `session-store-db` feature joins the generated `Cargo.toml`'s list and the
  `tower-sessions-memory-store` dependency leaves it. This is generator
  output rather than Arcature's own API: an application already generated
  keeps compiling and keeps its old behaviour until its author changes it.

### Deprecated

- **The auth extractors have a module that names them.** `Auth<U>`,
  `OptionalAuth<U>`, `Current<U>`, `OptionalCurrent<U>`, `AuthManager<U>`,
  `LoginBuilder`, `AuthError` and `UserLoader<S>` now live in
  `arcature::auth::extract`. `dx` abbreviates "developer experience", which
  names a goal rather than a thing, so `auth::dx` told a reader nothing about
  what was inside it -- four unrelated concerns in one ~900-line file.
  `arcature::auth::extract` says what it holds. The crate-root and
  `arcature::auth` re-exports are unchanged, and `arcature::auth::dx` still
  resolves.
- **The handler-facing session API has a module that names it.** `Session`
  and `SessionError` now live in `arcature::auth::session_api`, next to the
  `arcature::auth::session` module that configures the cookie and the
  middleware layer. The two halves were previously a file apart for no
  reason other than which one had ended up in `dx`. The re-exports are
  unchanged and `arcature::auth::dx::Session` still resolves.
- **The flash messages have a module that names them.** `Flash`,
  `FlashMessage`, `FlashLevel` and `FlashError` now live in
  `arcature::auth::flash`, together with the two session keys the extractor
  and the redirect mapper have to agree on. The re-exports are unchanged and
  `arcature::auth::dx::Flash` still resolves.
- **Authorization has a module that names it.** The `Policy<M>` trait,
  `AuthzError` and the `Auth::authorize` seam now live in
  `arcature::auth::policy` -- the last of the four concerns to leave
  `auth::dx`, which is now nothing but re-exports. Authentication ("who is
  this?") and authorization ("may they do this?") are separate steps by
  design, and they are now separate files. The re-exports are unchanged and
  `arcature::auth::dx::Policy` still resolves.
- **`arcature::auth::dx` is deprecated and scheduled for removal in `0.2.0`.**
  It is now nothing but re-exports of the four modules above, so every path
  that compiled at `0.1.0` still compiles and the fix is to delete the `dx`
  segment. Most of the names warn when used; `Auth`, `OptionalAuth`,
  `SessionError`, `UserLoader` and `Policy` do not, because rustc ignores
  `#[deprecated]` on a re-export and the alias form that would carry the
  attribute cannot be used as a tuple-struct constructor or pattern -- so
  aliasing them would have broken `Auth(user)` to deliver a warning, which is
  the wrong trade. The module documentation is the notice for those five.

### Fixed

- **The examples in the documentation are compiled again.** 36 of the
  crate's 49 doctests were tagged ```` ```ignore ````. That tag does not
  mean "this example is illustrative"; it means rustdoc never compiles it,
  so the example is free to drift away from the API it documents -- and
  several had drifted far enough to be wrong. They are being un-ignored a
  cluster at a time, and the ones that turned out not to compile are
  corrected rather than re-tagged:
  - **`src/dx/` -- the eight extension-point contracts.** All eight now
    compile. Two were wrong: `Inject<T>` is a newtype with no `Deref`, so
    the `Inject` example's `svc.recent_for(..)` never could have resolved
    and now reaches through `.0`; and the `Resolve` example opened with an
    `impl<S> Resolve<S> for Db` that no reader could have written, because
    both halves are foreign and the orphan rule reserves it for Arcature --
    the prose now says so and the example shows the `#[service]`-generated
    impl, which a reader *can* write.
  - **The crate-root quick start and the application builder.** The first
    example a reader meets did not compile: it called `.run()` on the
    builder, where `run` is on `Application`, and gave `main` the framework
    `Result` where `run` returns `EngineResult`. The builder module's own
    example claimed `pub fn app() -> Application<AppState>` for a function
    that returns an `ApplicationBuilder<AppState>`. Both are corrected, and
    the `layer`, `security_headers` and `cors` examples are now `no_run`
    programs rather than floating expressions.
  - **`auth` -- `AuthUser`, `Auth::authorize`, `AuthManager::login`,
    `Policy`.** The `Policy` example called
    `auth.authorize::<LinkPolicy>("view", &link)`, which cannot compile:
    `authorize` takes the resource type *and* the policy type, and Rust has
    no partial turbofish. It also passed a `&Bound<Link>` where a `&Link` was
    wanted, and swallowed the result with `?` in a handler returning the
    framework `Result` -- but `AuthzError` has no `From` into `Error`, so
    that `?` never compiled either. The corrected example maps it through
    `forbidden(..)` and says in a comment that the choice is the handler's.
  - **`routing` -- the module example and the `Middleware` contract.** The
    `Middleware` example was written as `async fn handle(..) -> Result<Response>`.
    The trait method is not async: it returns
    `Pin<Box<dyn Future<Output = Result<Response>> + Send>>`, because the
    trait is object-safe by hand. The example also returned
    `next.run(request).await` directly, but `Next::run` yields a `Response`,
    not a `Result<Response>` -- a continuation cannot fail. Both are fixed,
    with the reason for each stated above the block, and the missing
    `#[derive(Clone)]` that `Middleware: Clone` requires is now there.
  - **`config` and `database`.** The `database` module example wrote
    `index(db: Db)` and then called `inertia!(..)`, a macro that expands to
    `Inertia::render(&inertia, ..)` and therefore needs an `Inertia` in the
    handler. It also assumed a `User` model that the example never declared.
    The corrected version declares one with `#[model]` and renders through
    `Inertia::render` rather than the `inertia!()` shorthand, which does not
    compile from any call site (see the `inertia` bullet). `Model`'s example
    filtered on `UserColumn::Role` over a struct that has no `role` field.
  - **`inertia` -- the module example, the root document and the props
    schema.** Four of the six now compile. The two that do not are the
    `inertia!()` examples, and they stay `ignore` because the macro they
    show does not compile from *any* call site: its expansion names
    `inertia` without taking it as a macro argument, so `macro_rules`
    mixed-site hygiene resolves that identifier at the definition site,
    finds the `arcature::inertia` module, and every call fails with
    `error[E0423]: expected value, found module 'inertia'`. Both blocks now
    say so inline, and the macro has a "Known limitation" section pointing
    at `Inertia::render`, which is the call the expansion was reaching for.
    Of the rest: the module example dropped its `?`, and the
    `ScriptBody::nonce_attribute` example was a floating `format!` with no
    `ScriptBody` in reach -- a downstream crate cannot construct one, so it
    is now shown the way it is actually met, inside a root-document
    function.
  - **`mail`, `cache` and `storage` -- the three resource facades.** All
    four examples were fragments referring to bindings that were never
    introduced. `Mailable` is now a whole impl; the `Mail` example builds
    on `Mailer::capture_ok`, the transport that accepts everything and
    sends nothing, so the documented send actually runs during
    `cargo test --doc`. `Cache::remember` and the `Storage` builder need a
    Valkey server and a filesystem root, so they are `no_run`: compiled
    against the real signatures, not executed. The storage example also
    dropped its second disk, which called `StorageConfig::s3` -- a method
    that does not exist unless the non-default `storage-s3` feature is on.
  - **`jobs` and `events`.** The `jobs` module example used a `pool` that
    appeared from nowhere and left the one interesting line -- the handler
    registration -- commented out; it is a `no_run` function over a
    `JobPool` parameter now, with the handler registered and the closure's
    `Result<(), JobError>` spelled out, because that bound is the one a
    reader gets wrong. `JobModel`'s example is a runnable `const` with its
    accessors asserted. The `events` example ran `.await` at the top level
    of a block that had no runtime, and imported `Event` from
    `arcature::events`, which is the *trait*: `#[derive(Event)]` needs the
    derive macro of the same name from the crate root. Both are fixed and
    the dispatch now runs.
  - **`validation` -- the `Validated<T>` extractor.** The example was
    wrong in four places at once: it forgot the `#[derive(Deserialize)]`
    that `#[request]` deliberately does not add, put `required` on two
    `String` fields (`validator`'s `required` is for `Option<T>`; a
    `String` serde could not fill is already a deserialization failure),
    returned a bare `Redirect` type that is not the framework's
    `RedirectResponse`, and finished with `redirect!(route::links::index())`
    -- a macro this crate does not define, over a module the example never
    declared. It compiles now, and `redirect()` is shown as what it is, a
    builder.
  - **`oauth` -- the PKCE flow.** Behind a non-default feature, so
    `cargo test --doc` alone never sees it; verified with
    `cargo test --doc --features oauth`. The example wrote
    `oauth::GITHUB` from inside the `oauth` module itself, called a
    `session` that was never introduced, and returned a `redirect(..)`
    from a snippet with no function around it. It is `no_run` -- the token
    exchange would reach GitHub -- and now shows both halves of the flow
    as one compiled function, which is the point of the example: the state
    and the verifier that leave step one are the same two values step two
    reads back.
  - **`arcature-macros` -- the 25 that stay ignored, and why.** Every
    example in the proc-macro crate's module docs shows code naming
    `::arcature::` items, and `arcature-macros` must not depend on
    `arcature` -- that dependency is the cycle. There is nothing in that
    crate for those blocks to compile against, so all 25 keep `ignore` and
    each now carries the reason on its first line rather than leaving a
    reader to guess whether the tag was laziness. `lib.rs` states it once
    at length and points at `arcature`, where the compiled examples for
    the items these macros generate impls for actually live.

- **`arcature-macros` ships its licence text.** The crate declares
  `license = "Apache-2.0"` but the published `0.1.0` tarball contained no
  `LICENSE` file, because Cargo only picks one up from the package
  directory and the licence lived at the workspace root. Apache-2.0 4(a)
  requires the text to travel with the distributed work, so the omission
  was a licensing defect rather than an inconvenience. `macros/LICENSE` is
  now a copy of the root file and appears in `cargo package --list`.
- **CI's test database name now clears the harness's own safety gate.**
  `src/test_kit/database.rs` refuses any database whose name does not start
  with `arcature_test_`, and it reads the URL from `ARCATURE_TEST_DB_URL`.
  CI provisioned `arcature_test` -- one underscore short of the prefix --
  and exported it only as `DATABASE_URL`, so the database service looked
  wired up while every test that asked for it would have been refused
  twice over. The service is now `arcature_test_ci`
  (`arcature_test_release` in the release workflow) and both variables are
  set.

- **`KeySource::Ip` actually keys on the client.** It read
  `ConnectInfo<SocketAddr>`, which nothing installed, so every request
  fell into the shared `UNIDENTIFIED_KEY` bucket and IP rate limiting was
  a no-op -- one global quota shared by every caller, which is a security
  defect and not a missing nicety. It now reads the `ClientIp` extension,
  falls back to `ConnectInfo`, and only then to `UNIDENTIFIED_KEY`. **This
  changes behaviour by design:** an application using `KeySource::Ip` over
  TCP goes from one bucket for the whole world to one bucket per client
  address, so callers that were previously sharing an allowance now each
  get their own -- and a limiter tuned around the collapsed behaviour will
  admit more traffic than before. Behind a reverse proxy, set
  `ApplicationBuilder::trusted_proxies`; without it the peer address is
  the proxy and every client behind it still shares a bucket. A forwarded
  header from an untrusted peer is ignored, so a caller cannot mint a
  fresh bucket per request.

- **The redaction deny-list now catches a header's own spelling.**
  `observe::is_sensitive` lowercased a field name and searched it for a
  needle, and every multi-word needle is written with `_`. So `x-api-key`,
  `x-session-id`, `x-private-key`, `x-pin-code` and `x-cache-value` matched
  nothing and were written to the log in full -- an application that records
  a header map field-by-field under each header's own name got no redaction
  for precisely the headers worth redacting. `authorization` and `set-cookie`
  escaped this only by accident, because `auth` and `cookie` carry no
  separator. `-` and `.` are now folded to `_` before the test, which covers
  the header spelling and the OpenTelemetry one
  (`http.request.header.authorization`) at the same time. **This can only
  redact more:** no needle contains `-` or `.`, so every name redacted before
  is still redacted, and some names that were logged are now withheld. A
  camelCase spelling of a multi-word needle (`privateKey`, `sessionId`) is
  still missed and is pinned as such in `tests/observe_redaction.rs`.


- **CI reports every failing test suite, not the first one.** The `Test` job
  ran `cargo test` without `--no-fail-fast`, so cargo stopped at the first
  test target that failed and never built the remaining binaries or ran a
  single doctest. A run with two unrelated breakages reported one of them;
  the second appeared only after the first was fixed and the job re-run,
  which turns one review cycle into as many as there are broken suites. The
  failure it hid best is the accidental one -- a change that breaks a suite
  alphabetically later than a suite already red looks free until the day the
  first one goes green.

### Security

- **Tampering with a token or a signed URL is proven to fail, one byte at a
  time.** Two adversarial suites now stand behind the `crypt` and
  `signed-urls` features, and the two central tests are exhaustive rather
  than illustrative: every single-bit flip anywhere in an encrypted token --
  all 568 of them, across nonce, ciphertext and tag -- must come back as an
  authentication failure, and every byte position of a signed URL must be
  rejected under both substitution and deletion, which covers the origin,
  the path, each parameter name and value, the expiry, the separators and
  the signature without anyone remembering to add a case. Around them:
  a failed tag check yields no plaintext at all rather than a prefix, the
  same plaintext never seals to the same token twice, an expiry edited
  forward is refused, an expired link is refused against an injected clock
  rather than a sleep, a reordered query still verifies while values swapped
  between their keys do not, and a second `signature` parameter is refused
  rather than one of the two being picked.

- **`SECURITY.md` says that the largest memory-safety surface here is not
  Rust.** `cargo geiger` counts `unsafe` in Rust, so the baseline it produces
  is silent about C and assembly reached through FFI -- and the biggest such
  body in this tree is AWS-LC, vendored by `aws-lc-sys`, which `aws-lc-rs`
  builds. The manifest selects it deliberately at two sites (`sqlx` with
  `tls-rustls-aws-lc-rs`, `lettre` with `aws-lc-rs`); both crates also offer
  `ring`. Version `0.44.0` carries 414 C files, 270 headers and 941 assembly
  files across all targets, and on `x86_64-pc-windows-msvc` compiles to 254
  objects in a 16 MB static library that is linked into every binary reaching
  a database over TLS, sending mail, or making an outbound HTTPS request --
  which is the default feature set. The new section states plainly that
  "Arcature is pure Rust" is therefore false, gives the reasoning for
  choosing AWS-LC over `ring` so a future reader can disagree with an
  argument instead of guessing there was one, and records the consequence: a
  CVE in AWS-LC is an Arcature security release.

- **The dependency tree's `unsafe` is counted and recorded.**
  `#![forbid(unsafe_code)]` covered the crate and said nothing about the 359
  crates under it, which is where the `unsafe` actually is.
  `unsafe-baseline.<host-target>.txt` now records a `cargo-geiger` reading,
  one line per crate: on `x86_64-pc-windows-msvc`, 153 of the 360 crates in
  the default `--all-targets` graph contain `unsafe`, 49 forbid it, and the
  build reaches 342 `unsafe` functions, 31094 expressions, 823 impls, 76
  traits and 992 methods. `just geiger` recomputes the reading and diffs it
  against the baseline for the host it runs on; `just geiger-accept` records
  a new one. The name carries the target because the reading is a property
  of the target and not of the tree -- the platform crates near the leaves
  differ by operating system, so a Windows reading and a Linux reading
  disagree by dozens of crates and neither is wrong. A host with no baseline
  yet records one and fails once, which is a distinct outcome from counts
  having moved. The diff is a review prompt rather than a gate, so a pull
  request that changes a dependency has to account for what moved.
  `SECURITY.md` explains how to read the file and `CONTRIBUTING.md` explains
  when to re-record it.

- **The `unsafe` count is now checked on a clock, not on somebody
  remembering.** A baseline nobody recomputes is a number that was true
  once. The new `Geiger` workflow runs `just geiger` on every push to `main`
  that touches `Cargo.lock`, the baselines, the `justfile` or the workflow
  itself -- so a dependency bump gets its answer in the same push that
  caused it -- and weekly as a backstop for the changes a path trigger
  cannot see, such as a `cargo-geiger` release that counts differently. It
  is deliberately not a job in `ci.yml`: cargo-geiger rebuilds the whole
  graph through its own compiler wrapper, which is a ten-minute cold build
  bought on every pull request in exchange for a signal that can only change
  when the lockfile does. A run that finds a difference uploads the fresh
  report as an artifact, so accepting it is reading a diff rather than
  reproducing a cold build locally. It writes nothing back: a moved count
  becomes a red run and a human commit, never a silent re-baseline.

- **A scaffolded project's release build is pinned by a test.** The
  generated `Cargo.toml` already took `arcature` with
  `default-features = false` and an explicit list, so `cli`, `templates`
  and `dev-proxy` stay out of a production binary -- but the only guard was
  a test asserting those strings do not appear in the manifest, which would
  pass unchanged if `default-features = false` were dropped and every one of
  them came back on. The new test asserts the line itself, asserts the
  application's own `default = []`, and asserts that `dev-proxy` is
  reachable from the `dev` feature and from nowhere else in the file. It
  also checks the dependency section specifically -- not the whole manifest
  -- for `uag`, `otel`, `oauth`, `api-docs`, `storage-s3` and `test-kit`, so
  the `dev` and `uag` feature definitions cannot mask a real entry. No
  template content changed; what changed is that reverting it now fails.

- **`cargo-deny` bans `native-tls`.** The `[bans]` list already refused
  `openssl` and `openssl-sys`, but `native-tls` is the facade that pulls
  them in: it is Schannel on Windows, Secure Transport on macOS, and
  OpenSSL on Linux -- which is every machine an Arcature application is
  deployed on. A dependency offering a `native-tls`/`rustls` choice often
  defaults to the former, so the OpenSSL entries alone would only fail
  after the C library was already in the graph. `native-tls`,
  `tokio-native-tls` and `hyper-tls` are now denied by name, so the check
  points at the crate that made the choice.

- **The observability documentation no longer overstates what the deny-list
  covers.** `src/observe/mod.rs` said that credentials never reach "a log
  line, a metric label, or a span attribute", and `src/observe/otel.rs` said
  that "the same deny-list applies" to exported spans. The first clause is
  true of the framework's own layers and the second is not true at all:
  `redact::is_sensitive` is consulted by the JSON log layer and by nothing
  else, so a field an application records under a deny-listed name is
  written as `[redacted]` to the log and exported to an OTLP collector in
  full, and a label an application chooses is rendered into the exposition
  in full. A reader following the old wording would reasonably have put a
  password in a span field and expected it to be redacted. The docs now say
  which mechanism covers which destination, name the two that are
  unenforced, and point at `tests/observe_redaction.rs`, which pins both.
  No behaviour changed; closing the gaps means changing what `otel.rs` and
  `metrics.rs` do, which is not a patch-release change.

### Performance

- **A test now holds the frontend out of the Rust build.** A generated
  project has no build script and no `include_str!`, `include_bytes!` or
  `include_dir!` in any `.rs` file, which are the only two ways a `.tsx`,
  `.css` or `.vue` file can become an input to a Cargo rebuild. That was
  already true and is now asserted, across all nine stack and driver
  combinations. `arc dev`'s watcher had the matching test on its side --
  it refuses to run Cargo for a frontend save at all -- but nothing stopped
  a future template from quietly reintroducing the dependency underneath it.
- **A generated application no longer carries its dependencies' debug
  information.** `[profile.dev.package."*"]` in the scaffold adds
  `debug = false`, and a new `[profile.dev.build-override]` does the same for
  build scripts and proc macros. Dependency debuginfo is not a one-time cost:
  generic code from a dependency is monomorphised into the application's own
  crate, so rustc writes it and the linker merges it on every save. Measured
  with `cargo build --timings` on a generated application, the same one-line
  handler change, both runs reporting `Max concurrency: 1 (jobs=4 ncpu=4)`:
  the two units the change dirties fell from 90.0s to 45.3s of *per-unit
  compile time*, and the program database from 71 MB to 29 MB. That is not a
  wall-clock claim. Wall-clock rebuild series taken minutes apart on the
  measuring machine varied by more than the change being measured, so they
  are not offered as evidence for anything; `The dev loop` in the guide has
  the method and the full table. Backtraces are unaffected -- the application's
  own crates keep `line-tables-only` -- and a full step debugger is one
  `CARGO_PROFILE_DEV_PACKAGE_*_DEBUG=2` away.
- **The framework's own debug builds drop the same weight.** The workspace had
  no `[profile.dev]`, so `cargo test` and `cargo clippy` in this repository
  were paying for full debug information nobody reads. It is now
  `line-tables-only`, matching what the scaffold has always given a generated
  application.
- **Proc macros and build scripts compile optimised.** The workspace had no
  `[profile.dev]` of any kind, so `arcature-macros` and the `syn` stack
  beneath it were built at `opt-level = 0` with full debug information.
  Those crates are not shipped code -- rustc loads and *runs* them, once per
  macro invocation, in every crate of every dev build -- so their own
  compiled speed is a tax on all later builds. `[profile.dev.build-override]`
  now sets `opt-level = 2` and `debug = false` for build scripts, proc-macro
  crates and their dependencies. Nothing about the profile of the code being
  shipped changes.
- **Inertia props resolve concurrently.** A page whose props load four things
  from four places used to wait for the sum of the four: resolution was a
  single pass that inspected, awaited and recorded each prop before looking at
  the next. Nothing about the props required that -- it was the shape of the
  loop. Every decision about every prop is now taken up front, while the
  output is still empty, so no resolver can observe another's result; the
  resolvers are then polled together and the results recorded in declaration
  order. Same props, same metadata, same order, in the time of the slowest
  resolver rather than the sum of all of them.

  One behaviour does change, and it is inherent to running them at once rather
  than a choice made here. Previously the first resolver to fail ended
  resolution, and resolvers declared after it never ran. They now all start
  before any error is examined, so a resolver that follows a failing one will
  execute -- its side effects happen, and the request still fails with the
  same error, chosen by declaration order rather than by which failure arrived
  first. Resolvers that only read are unaffected; a resolver that writes
  something on a page that also has a failing prop is the case to look at.

  Not built on `tokio::spawn`: the runtime turns a spawned task's panic into a
  `JoinError`, which would quietly demote a panicking resolver to one failed
  prop. Polling in place leaves a panic unwinding the request as it did
  before. No new dependency either -- the join is twenty lines over
  `std::future::poll_fn` rather than a combinator crate to track advisories
  for.

### Documentation

- **`arcature::dx` says what `dx` means, once.** The module now opens by
  stating that the name covers exactly one thing -- the runtime contract
  layer the macro DSL generates code against, which is also precisely what
  the `dx` Cargo feature switches on -- and that no other module may take the
  name for a second meaning. It was worth writing down because a second
  meaning had already appeared: `auth::dx` read `dx` as "developer
  experience" in general, which names a goal rather than a thing and so
  admitted four unrelated concerns into one file. A module named `dx` that is
  not this one is now documented as a defect.
- **`arcature-macros` has a README.** The crates.io page for `0.1.0` is
  blank, which for a proc-macro crate that nobody should depend on
  directly is the wrong first impression: the page now opens by saying so
  and pointing at `arcature`. It also lists all 23 entry points, the two
  properties the expansions guarantee -- no hidden registry, no panics on
  ordinary mistakes -- and the full `ARC-M001`..`ARC-M014` diagnostic
  table. A README is part of the published tarball, so this reaches
  crates.io with the next release rather than retroactively.
- **`SECURITY.md` has an attack-surface inventory.** The policy said what
  was in scope; it did not say where the scope is. A new section tables
  every place a byte an attacker chose meets something that interprets it
  -- the request line, the four validated extractors, the session and CSRF
  cookies, the Inertia headers, WebSocket frames, storage paths, job
  payloads read back out of the database -- with the parser, the guard, and
  the guard's **default** named in each row. Defaults that are off or unset
  say so: the framework sets no body limit and no request timeout (the
  scaffold sets 2 MiB and 30 s), `SecurityHeaders` is absent until asked
  for, HSTS and CSP are off inside it, and `OriginPolicy::DenyAll` is the
  realtime default. It also records two properties nothing tests --- every
  `std::process::Command::new` is under `src/cli/`, and no SQL in
  `src/jobs/` is built by interpolation --- and two facts a reader should
  not have to discover: the CSRF token is compared with `==` rather than in
  constant time, and `Pages::serve` does not canonicalise, so a symlink out
  of its root is followed. The point of the table is reviewability: a pull
  request either adds a row, changes a guard, or does neither.
- **The README no longer says three CLI commands are unwired.** It claimed
  `arc dev`, `arc typegen` and `arc build` "parse and report that they are
  not wired yet". All three have been implemented for some time --
  `arc dev` alone is ~4,300 lines under `src/cli/commands/dev/`, and the
  three are dispatched at `src/cli/mod.rs:131,149,151`. The command table
  gains a row for each of them plus `arc routes`, and a paragraph now says
  what `arc dev` does and that `routes`/`typegen`/`build` are gated on the
  `uag` feature.
- **The community-health files moved into `.github/`.**
  `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`, `SECURITY.md`
  and `SUPPORT.md` are no longer at the repository root, which leaves
  `README.md`, `CHANGELOG.md` and `LICENSE` there. GitHub reads all five
  from `.github/`, so nothing it surfaces changes, but a direct link to an
  old path now 404s and the paths inside the published tarball changed
  with them. All five also gained an entry point: the README lists them
  with the question each answers, and the guide has a `The project` page
  in its back matter. Before this, nothing anywhere linked to
  `GOVERNANCE.md` or `SUPPORT.md`.
- **The guide is published.** It is at
  <https://arcaturelabs.github.io/Arcature/>, rebuilt from `main` whenever
  `docs/` changes. CI had been building the book on every pull request and
  discarding it, so reading the guide meant cloning the repository and
  installing mdBook. `homepage` in both manifests moves from the
  repository URL -- where `repository` already pointed -- to the guide, so
  the crates.io sidebar links two different things instead of the same
  thing twice.
- **The dev loop has a measured baseline.** `The dev loop` in the guide
  records what one save costs in a generated application, how it was
  measured, and on what machine. The finding the numbers settle is that a
  Rust-only change rebuilds exactly two units out of 489 -- nothing
  spurious, not `arcature`, not `arcature-macros`, not the embedded
  scaffold templates -- and that 95% of the trip is code generation and
  linking rather than type-checking. Issue #8 quoted a figure with no
  method behind it; this is the method, so the next change to the loop can
  be shown to have worked rather than asserted to have.
- **The dev loop page carries the after-numbers, and says the target is
  missed.** `The dev loop` gained a `What was cut` section with the
  before-and-after table for the profile change, and a `What is left`
  section naming the two costs that remain -- linking the executable, and
  monomorphising the application crate -- with the reason each is
  proportional to the size of the program rather than the size of the
  change. Only `cargo build --timings` runs at equal reported concurrency
  are compared; the page says outright that wall-clock series taken minutes
  apart on the measuring machine differ by more than the effect being
  measured and are not offered as evidence. Issue #8's 2.5s target is not
  met -- the measured cost halved, and halving was not enough -- so the
  three candidates that could close the rest (`-Zshare-generics`, a
  Cranelift backend, hot-patching) are listed with what each would cost,
  and the issue stays open.

- **`Application::serve` says that it has no client address.** The raw
  escape hatch installs neither `ConnectInfo` nor `ClientIp`, so
  `KeySource::Ip` falls into the shared bucket there and the access log
  records an empty `client_ip`. Its docs now say so, and say why it is
  not a gap to be closed later: the method accepts any listener whose
  address is only `Debug` -- a Unix socket, a named pipe, an in-memory
  duplex -- and installing the extensions would require
  `Listener<Addr = SocketAddr>`, narrowing the escape hatch to TCP and
  shutting out the listeners it exists for. The entry points that do bind
  TCP are named, along with `ClientIp::resolve` for anyone keeping their
  own listener.

- Deployment guide: a "Running more than one instance" section that
  separates the subsystems with a cross-instance mode from the one
  without. Sessions share through `session-store-db`; rate limiting
  shares through `RateLimit::redis`, failing closed when Redis is
  unreachable. Realtime fan-out does not share and has no switch:
  `Broadcast` wraps a `tokio::sync::broadcast` channel, which reaches
  only the subscribers connected to the publishing process, so two
  instances mean roughly half of every broadcast is missing from a given
  client's view -- with no error and no warning, because the channel
  delivers correctly to everyone it can see. The section lists the three
  ways to live with that and says why the obvious Redis pub/sub bridge is
  not written yet. The `realtime` module docs and the README feature table
  now say the same thing where a reader meets them first.

- ADR 0006, "The generated TypeScript stays derived": why
  `resources/js/generated/` is written by `arc typegen` and never committed,
  and why that makes the type-safe `route()` helper opt-in rather than the
  default. Turning it on means the scaffold's own page imports a directory
  that does not exist in a fresh clone, in the `Dockerfile`'s Node-only asset
  stage, or in any frontend job with no Rust toolchain in reach. The record
  states the price of both ways out -- committing the directory makes `tsc`
  check against a snapshot that goes stale silently, and putting Rust in the
  asset stage undoes the reason the stages were split -- and names the
  condition under which it should be reopened.

- The scaffold's layout now says, at the `href="/"` it would replace, that
  `route('home')` is the typed alternative and that `just check-ts` is the
  command that makes it work -- it runs `arc typegen` before `tsc`, so route
  names are checked against the live graph rather than a stale file. The
  guidance sits at the call site in all three stacks, because that is where
  someone deciding between the two is reading.

- The scaffold's `.gitignore` and `.dockerignore` now each say what their
  `resources/js/generated` exclusion is holding up, and link to ADR 0006. The
  two lines carry different reasons -- a committed copy is a second source of
  truth for every route name, and a copy that exists only on the machine that
  last ran `arc typegen` must not decide what the image's Node-only asset
  stage bundles -- so neither line is safe to delete on the strength of the
  other. A new scaffold test fails if either one goes.



## [0.1.0]

### Added

- **The kernel.** The error model, HTTP routing, the application lifecycle and
  typed configuration. Routes are ordinary Rust values --
  `Routes::new([Route::get("/", index).name("home")])` -- rather than a string
  DSL, and named routes generate URLs through `Routes::url_for`.
- **Native Inertia v3.** The server half of the protocol, implemented directly,
  so a stock `@inertiajs/react`, `@inertiajs/vue3` or `@inertiajs/svelte`
  client talks to an Arcature application with no Arcature-supplied JavaScript
  package. The `Inertia` extractor, the `inertia!()` macro, the prop strategies
  (eager, always, lazy, optional, deferred, merge), partial reloads and asset
  versioning.
- **Database.** One PostgreSQL pool shared by SeaORM and SQLx, the `Db` handle,
  the `Query` facade (`where_eq`, `where_in`, `latest`, `paginate`, `count`,
  and the rest), transactions that span both paths, and migrations. The
  `db-postgres` / `db-sqlite` / `db-mysql` split keeps a SQLite user from
  compiling the Postgres protocol.
- **Auth.** Argon2id hashing with rehash detection, tower-sessions cookie
  sessions, double-submit CSRF, the `Auth<U>` / `OptionalAuth<U>` /
  `AuthManager<U>` extractors, `Session` and `Flash`, and the `Policy`
  authorization seam. Logging in rotates the session id without being asked.
- **Validation.** `Validated<T>` and the `ValidatedJson` / `ValidatedForm` /
  `ValidatedQuery` / `ValidatedPath` extractors over the `validator` crate,
  with every rejection mapped to an RFC 9457 problem response.
- **API problems.** `Problem` and `ProblemKind` (RFC 9457), compiled in
  unconditionally because validation depends on them, served as
  `application/problem+json`.
- **Cache.** One multiplexed Valkey/Redis connection, typed operations, key
  namespacing and the `remember` cache-aside helper. A miss is `Ok(None)`; a
  backend failure is an error and never quietly becomes a miss.
- **Storage.** OpenDAL-backed named disks -- `fs` always, S3 behind
  `storage-s3` -- with `StoragePath` rejecting traversal, absolute paths,
  backslashes and control characters before any I/O runs.
- **Mail.** lettre SMTP over rustls, the `Email` builder, the `Mailable` trait,
  the `Mail` facade, and an in-memory capture transport so a test can assert on
  what would have been sent.
- **Jobs.** A PostgreSQL queue on the application's existing pool, claiming
  with `FOR UPDATE SKIP LOCKED` and fencing each claim with a UUID so a stale
  worker cannot commit over a live one's work. A worker run loop with a
  concurrency semaphore, heartbeats, lease sweeping and graceful shutdown;
  exponential backoff with jitter; a scheduler; an observer seam.
- **Events.** In-process typed dispatch, sequential in registration order,
  erased through `serde_json::Value` rather than through `TypeId` and `Any`.
- **Realtime.** Thin WebSocket and SSE wrappers over axum, a bounded broadcast
  channel, an origin policy, and a connection registry with a cap and a drain.
- **Observability.** Validated `x-request-id` generation and echo, stable span
  names, and one structured access-log line per request. No global subscriber
  is installed on the production path.
- **Static assets.** `public/` served as the router fallback, with
  `Cache-Control` chosen per response: a hashed bundle is immutable for a year,
  anything else revalidates, a 404 carries none. The root document resolves its
  entry through Vite's `manifest.json`.
- **The pre-routing proxy.** An application-owned request policy --
  `Continue`, `Redirect`, `Rewrite`, `ShortCircuit` -- that runs before route
  selection, with CRLF-injection defence on redirect targets, rewritten URIs
  and header values.
- **The one-port dev proxy.** Vite runs in `middlewareMode` over an IPC
  endpoint and binds no TCP port; the Rust process owns the only listener and
  forwards Vite's requests, HMR WebSocket included. One origin in development,
  as in production.
- **The Client Exposure Firewall.** `ClientData`, `PropsSchema`, `PageContract`
  and `Inertia::render_page`. A type that merely derives `Serialize` cannot
  reach the browser as page props; exposure is an explicit opt-in the compiler
  checks.
- **The DX layer** behind the `dx` feature: `ApplicationGraph` with duplicate,
  unknown-import and cycle validation; `ModuleDescriptor`; `Resolve<S>` typed
  injection with no runtime container; `Service`, `Provider`, `Command`,
  `RouteModel`, `Bound<T>`, `DbFromState`.
- **The unified DSL macros:** `module!`, `application!`, `routes!`, `redirect!`,
  `page_macro!`, the attributes `#[model]`, `#[request]`, `#[controller]`,
  `#[service]`, `#[provider]`, `#[policy]`, `#[middleware]`, `#[command]`,
  `#[job_handler]`, `#[route_model]`, `#[request_cache]`, `#[resource]`,
  `#[page]`, `#[listener]`, and the derives `Job`, `Event`, `DxComponent`.
  Every macro reports a mistake as a `compile_error!` carrying an `ARC-M<NNN>`
  code; none panics on ordinary bad syntax.
- **Production pipeline stages,** each off unless asked for: compression,
  security headers, CORS, request id, access log, panic catching, error
  mapping, body limit, timeout, maintenance mode, session, CSRF, Inertia and
  user layers. The order they compose in is fixed in
  `src/application/pipeline.rs` rather than following builder call order.
- **Error mapping.** Every bodiless error a layer produced — the bare `404`,
  `405`, `408` and `413` that axum and `tower-http` emit — gets an RFC 9457
  body, and a `text/plain` 5xx is redacted in release builds, because in
  practice that shape is a stringified internal error carrying a connection
  URL or a build-machine path.
- **Health endpoints.** `/up/live` and `/up/ready` are merged beside the
  application router rather than layered over it, so an orchestrator probing
  every few seconds pays no session load, no maintenance check and no log
  line.
- **Security headers.** `nosniff`, `DENY` framing and a strict referrer policy
  always; HSTS and CSP opt-in. An existing header wins, so the layer is a floor
  and not a ceiling.
- **Zero-config CSRF for Inertia.** `CsrfConfig::inertia()` uses the
  `XSRF-TOKEN` cookie and `x-xsrf-token` header axios hard-codes, so a stock
  Inertia client posts successfully without a line of application JavaScript.
- **The `arc` CLI:** `new`, `version`, `serve`, `migrate`, `schedule`,
  `queue work|drain|stats`, `db seed|fresh|reset`, `key:generate`,
  `storage:link`, `doctor`, and the `make:<kind>` generator family
  (controller, model, migration, request, resource, policy, service, job,
  event, listener, middleware, command, page, test, factory, seeder). Parsed
  with clap's builder API, shipped from the same package behind the `cli`
  feature so a normal application never compiles it. `dev` supervises the one
  TCP port, `routes` prints the route table (`--json` for tooling), `typegen`
  emits the TypeScript, and `build` runs validate, typegen, `cargo build
  --release` and `vite build` in that order, failing at the first stage that
  fails and naming it.
- **The application scaffold.** `arc new` writes a Laravel-shaped tree: `app/`
  (controllers, models, services, requests, policies, resources), `bootstrap/`,
  `config/`, `database/migrations/`, `routes/`, `resources/js` and
  `resources/css`, `public/`, `storage/`, `.env` and a smoke test.
- **CI.** An MSRV-and-stable matrix, `cargo fmt --check`, clippy with warnings
  denied, a PostgreSQL 17 service, the whole feature surface through
  `cargo hack`, and `cargo publish --dry-run`.

### Fixed

- `#[controller]` validated its impl block and re-emitted it unchanged while
  `module!` referred to `ControllerMetadata::METHODS`, so any real `module!`
  failed to compile. That is why the scaffold used no DSL at all.
- `ApplicationBuilder` had no way to install a `tower::Layer`. `InertiaLayer`,
  `SessionLayer` and `CsrfLayer` were all written and none could be attached,
  so a scaffolded application answered `500 inertia adapter error` on its own
  home page.
- A route's middleware wrapped every route registered before it. `Route` held a
  closure folding the whole `axum::Router` and `Routes::new` folds routes into
  one accumulating router, so a public route silently inherited a guard
  declared later in the same array. `Route` now owns a `MethodRouter`, which
  cannot reach past the one path it serves.
- `ARCATURE_VITE_IPC` was never read. The dev proxy could only be switched on
  by a builder call the scaffold does not make, so Vite requests would have
  404'd. The builder resolves the endpoint at construction now, and
  `dev_proxy_endpoint()` is the override its documentation already claimed to
  be.
- `tower-http` was optional while the pipeline's body limit and timeout used it
  unconditionally, so `--no-default-features` failed to build. `observe` was
  missing `dep:uuid` and `pages` was missing `dep:tokio`; both only ever built
  because some other feature happened to pull the dependency in.
- The WebSocket run loop had a hard-coded 20-second heartbeat. It honours
  `WsLimits` now and closes a connection whose pong does not arrive.
- `AccessLogLayer` had been written and never applied.
- `IntoRouteParams` appeared in the signature of a public method while being
  crate-private, so no outside caller could name the bound.
- The `opentelemetry` feature was declared and unused. Removed.

### Security

- A caught panic returns an RFC 9457 `Problem` and the panic payload is
  discarded rather than reported: a panic message routinely carries a file
  path, a SQL fragment, or the value that caused it. `tower-http` still logs it
  for the operator.
- HSTS is opt-in. A development server that sends it pins `localhost` to HTTPS
  for a year, and the pin outlives the header that set it.
- `DispatchError::Deserialize` carries no message, because a serde error may
  echo the payload it failed on.
- `CacheConfig`, `S3Config`, `SmtpConfig` and `SmtpCredentials` implement
  `Debug` by hand and redact their secrets.
- Redirect targets are checked against open redirects: an absolute or
  scheme-relative URL pointing at another host is rejected.
- `StoragePath` rejects `..`, absolute paths, backslashes and control
  characters before any I/O is attempted.
- Job claims are fenced with a per-claim UUID, so a worker that lost its lease
  cannot commit a result over the worker that took it over.

### Not yet implemented

Gaps documented in the source at the point of use, repeated here so nobody has
to find them at runtime.

- `AppConfig` reads `APP_NAME`, `APP_URL`, `APP_ENV` and `APP_PORT`, and the
  framework consumes exactly one of them. `port` becomes the port the process
  listens on, below `ARCATURE_BACKEND_PORT` and `PORT` in precedence. `name`,
  `url` and `env` are carried so that `Application::config` can hand them
  back, and are read by no framework code: no surface builds an absolute URL
  yet, and `env` is forbidden from gating behaviour rather than merely not
  gating it -- a protection an operator can switch off with an environment
  variable is a protection in name only, so redaction and the dev-only UAG
  endpoint key off `cfg!(debug_assertions)` instead. See the type
  documentation.

[Unreleased]: https://github.com/ArcatureLabs/Arcature/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ArcatureLabs/Arcature/releases/tag/v0.1.0
