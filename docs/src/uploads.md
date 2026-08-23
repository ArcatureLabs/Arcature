# Uploads

`multipart/form-data` request bodies: bounded, sanitized, held to their own
bytes, and written to a storage disk under a key derived from those bytes.

Off by default, and that is the security decision rather than a packaging one.
An upload endpoint is the largest attacker-authored surface a web application
has — the filename, the declared content type and the byte count all come
from the client — and a build with no upload route has no business carrying a
multipart parser for one.

## Turning it on

`uploads` is not in `default`. It is in `fullstack`.

```toml
arcature = { version = "0.1", features = ["uploads"] }
```

| It pulls | Because |
| --- | --- |
| `axum/multipart` | the parser (`multer`, via `axum::extract::Multipart`) |
| `validation` | the extractor lives beside the other validated extractors and reports RFC 9457 problem details |
| `storage-fs` | an upload is written to a storage disk, never to a path the request named |
| `dep:tokio` | one thing only: `tokio::time::timeout`. A byte cap cannot express "the client stopped sending", because a request that never finishes never exceeds anything |
| `unicode-normalization` | an attacker-authored filename goes into NFC before the reserved-name and extension checks look at it |
| `sha2` | the content address |
| `infer` | the magic-number table |

`arc make:upload avatar` generates a controller with the whole shape in it.

## The `UploadedFile` extractor

```rust,ignore
use arcature::prelude::*;
use arcature::storage::StorageError;
use arcature::validation::upload::UploadedFile;

/// The disk sub-tree these uploads live under. A constant, and the
/// application's own string -- nothing off the request is ever a prefix.
const PREFIX: &str = "avatars";

pub async fn store(State(state): State<AppState>, upload: UploadedFile) -> Result<Response> {
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| Error::Storage("no storage disk is configured".to_string()))?;

    let address = upload.store_under(&storage.default_disk(), PREFIX).await?;
    let key = address.path_under(PREFIX).map_err(StorageError::from)?;

    Ok((
        StatusCode::CREATED,
        json(serde_json::json!({
            "path": key.as_str(),
            "bytes": address.byte_len(),
            "filename": upload.filename().to_string(),
        })),
    )
        .into_response())
}
```

That is the generated blueprint's shape, not a standalone example: it needs an
`AppState` with a `storage` field and a route to hang off, so it will not
compile on its own.

Five things are true by the time the handler body starts, because extraction
is what makes them true:

1. **Bounds.** Total size, per-part size, part count and a read timeout, from
   the [`MultipartLimits`](#multipartlimits) in the request extensions.
2. **A whitelisted extension**, from the [`UploadPolicy`](#uploadpolicy-and-allowedextensions)
   in the request extensions.
3. **A sanitized filename.** The path, the controls, the bidi overrides and
   the Windows device names are gone, and the result is metadata.
4. **Bytes that match the extension.** `shell.php` renamed `avatar.png` is
   refused here, not discovered later.
5. **A client-safe rejection**, if any of the above failed.

`UploadedFile` implements `FromRequest`, not `FromRequestParts`. It consumes
the body, so it has to be the last handler argument.

| Method | Returns |
| --- | --- |
| `filename()` | `&SafeFilename` — **metadata**: show it, send it in a `Content-Disposition`, never resolve it as a path |
| `content_type()` | `Option<SniffedType>`, derived from the object, never from the part's header |
| `bytes()` | `&Bytes` |
| `byte_len()` | `usize` |
| `into_bytes()` | `Bytes`, consuming the wrapper |
| `store(&disk)` | writes under `ab/cd/<digest>.<ext>` |
| `store_under(&disk, prefix)` | writes under `<prefix>/ab/cd/<digest>.<ext>` |

Both store methods return a `ContentAddress` and fail with `UploadError`,
which has exactly two variants: `UploadError::Storage` (the backend failed —
a 5xx) and `UploadError::Content` (the bytes and the extension disagree — a
4xx). They are kept apart so that a disk that is down and a file that was
refused do not answer with the same status. `From<UploadError> for Error`
preserves the split, which is why the example uses `?` and not a `map_err`.

`store_under` writes under the prefix, so `address.path()` — the address on
its own — will not find the object afterwards. `address.path_under(PREFIX)`
is the key that will.

### What a refusal looks like

Every refusal is an RFC 9457 problem document shaped exactly like any other
field validation failure, reported under the fixed key `file`
(`upload::UPLOAD_FIELD`) and never under the part's own name.

| Code | Status | When |
| --- | --- | --- |
| `missing` | 422 | the body ended without a part that carried a `filename=` the policy would accept |
| `extension` | 422 | the filename could not be sanitized, or its extension is off the whitelist |
| `contents` | 422 | the bytes and the extension disagree |

A body that is not `multipart/form-data` at all is a 415. A bound that was
crossed is a 413 or a 408 — see [`MultipartLimits`](#multipartlimits).

The `code` and the message are both fixed strings chosen in the framework.
Nothing from the request reaches the response body: not the filename, not the
declared content type, and not the sniffed one — reporting the last of those
would let an uploader use the endpoint as a free file-type oracle.

### It buffers, and that is the small-file path

An extractor runs to completion before the handler is called, so it has
nowhere to put the bytes except memory. That buffer is bounded by
`MultipartLimits::field_bytes`, which defaults to 8 MiB; lowering it on an
upload route is how an application keeps the buffer small.

For anything bigger than an avatar, do not use this extractor. Drive
`BoundedMultipart` in the handler and write each chunk through `UploadWriter`,
which never holds more than one chunk regardless of object size. The extractor
is the convenience; that is the load-bearing path.

### It is not authorization

A validated upload is not an authorized upload, exactly as a validated request
is not an authorized request. Whether this user may write to this disk under
this prefix is a separate, explicit decision.

## `UploadPolicy` and `AllowedExtensions`

`UploadPolicy` is a `tower::Layer`. Applying it to a route puts the policy in
that request's extensions, where the extractor reads it back.

```rust,ignore
use arcature::storage::AllowedExtensions;
use arcature::validation::upload::UploadPolicy;

let router = axum::Router::new()
    .route("/attachments", axum::routing::post(store))
    .layer(UploadPolicy::new(AllowedExtensions::documents()).with_field("attachment"));
```

**With no policy layer installed, the default is images and nothing else.**
`UploadPolicy::from_extensions` never returns `None`; it falls back to
`UploadPolicy::default()`, which is `UploadPolicy::new(AllowedExtensions::images())`.
A route that forgot the layer fails closed, because "no policy configured"
must not mean "anything goes". Widening the whitelist is the thing an
application has to do on purpose.

| Built-in | Extensions |
| --- | --- |
| `AllowedExtensions::images()` | `jpg`, `jpeg`, `png`, `gif`, `webp` |
| `AllowedExtensions::documents()` | `pdf`, `txt`, `csv` |
| `AllowedExtensions::new(["JPG", "pdf"])?` | whatever is listed, lowercased; errors on an entry that is not a valid extension |
| `AllowedExtensions::default()` | empty — and an empty whitelist stores nothing at all, which is the right behaviour for a misconfiguration |

`svg` is deliberately absent from `images()`. An SVG is an XML document that
may carry script, so serving one inline is a stored-XSS primitive; an
application that needs SVG adds it knowingly with `.with("svg")?`.

It is a whitelist and never a blacklist. A blacklist of dangerous extensions
is a list of the ones somebody thought of, and the interesting ones are always
the other ones — `.phtml`, `.php7`, `.cgi`, `.jsp`, `.svgz`, `.htaccess`.

The rest of the surface: `contains(&extension)`, `iter()`, `len()`,
`is_empty()`, and `with(extension)` to add one.

An `Extension` is one to sixteen ASCII alphanumerics, lowercased on parse, and
nothing else. That is not tidiness. An extension is the string a web server,
an operating system and a browser each use to decide what a file *is*, and
allowing only `[0-9a-z]{1,16}` leaves them no encoding, no separator and no
homoglyph to disagree about.

`UploadPolicy::with_field("attachment")` requires the file to arrive in the
part of that name. Without it the first part carrying a `filename=` wins,
which is convenient and slightly sloppy: a form with two file inputs becomes
order-dependent.

## Filenames are sanitized, and then they are metadata

`SafeFilename::parse(filename, &allowed)` is the one place a client-authored
`filename=` is disarmed. It handles six different attacks, and a filter that
knows about one of them is a filter that has not been written yet.

| Input | What it wants | What happens |
| --- | --- | --- |
| `../../etc/passwd.png` | escape the storage root | the directory part is discarded before anything else runs; the name is `passwd.png` |
| a filename with a NUL in it | truncate the name in a C API downstream | rejected: control characters are never repaired |
| `CON.png` | open a Windows device instead of a file | rejected, with any extension and with trailing dots or spaces |
| a name with a right-to-left override in it | render so the extension reads `.jpg` | rejected: bidi and other invisible format controls are fatal |
| `shell.php.jpg` | be served by a mis-configured `AddHandler` | the inner extension marker is replaced: `shell_php.jpg` |
| an ordinary Vietnamese filename with diacritics | *nothing* | accepted, NFC-normalized, otherwise unchanged |

That last row is why this exists at all. `StoragePath::new` validates and
rejects; it has no opinion about how to *repair* a name, so a filename with a
space or a diacritic in it fails a check that was never aimed at it, and a
sanitizer that rejects real names teaches applications to bypass the
sanitizer.

The reject-versus-repair split is deliberate. A character is **repaired**
(replaced with `_`) when a human plausibly typed it and it is only dangerous
to a downstream parser: `.` (every remaining one, after the extension is split
off), `/`, `\`, `:`, `*`, `?`, `"`, `<`, `>`, `|`. A character is **rejected**
when its presence is itself the attack: the C0 and C1 controls, DEL, the bidi
overrides and isolates, the zero-width joiners, the word joiner, the
byte-order mark, the blank-rendering separators and the Unicode tag block.
Nobody names a holiday photo with a right-to-left override in it. The
variation selectors are deliberately *not* fatal — they occur beside emoji in
names people really have.

The whole name is fitted inside `MAX_FILENAME_BYTES` (255, the per-component
limit on ext4, XFS, APFS and NTFS alike) by truncating the stem on a character
boundary. The extension is never truncated: half an extension is a different
file type.

`FilenameError` is the coarse reason the parse failed — `Empty`, `TooLong`,
`ControlChar`, `Traversal`, `MissingExtension`, `InvalidExtension`,
`ExtensionNotAllowed`, `EmptyStem`, `ReservedName`. It is coarse on purpose:
the caller reports a fixed string to the client, never the offending input.

The name that comes out is still metadata. `StoragePath::from_filename` exists
for the case where a sanitized name really is the key, and it produces exactly
one path segment — but two users still collide on it, and one overwrites the
other. Where the key does not have to be human-readable, use the content
address and keep the filename for the download header.

## `MultipartLimits`

A multipart body is a stream the client writes and the server reads until the
client stops. Four separate things there are the client's choice, and each is
a separate way to exhaust a process.

| Setting | Default | Const | What it stops |
| --- | --- | --- | --- |
| `fields` | 32 | `DEFAULT_FIELDS` | fifty thousand two-byte parts inside a 1 MiB body — the cost of a part is a header parse and an allocation, not its length |
| `field_bytes` | 8 MiB | `DEFAULT_FIELD_BYTES` | one part that is the entire budget. Deliberately below the total |
| `total_bytes` | 16 MiB | `DEFAULT_TOTAL_BYTES` | one request that fills the disk or the heap |
| `read_timeout` | 30 s | `DEFAULT_READ_TIMEOUT` | a byte a minute, holding a task and a socket open |

**These apply with no layer installed.** `MultipartLimits::from_extensions`
never returns `None`; it falls back to `MultipartLimits::new()`, the values
above. There is no way to end up with *no* bound, and nothing here reads any
value as unlimited — `with_fields(0)` means the body may contain no parts at
all, and that is what it does.

Override them per route:

```rust,ignore
use std::time::Duration;
use arcature::http::multipart::MultipartLimits;

let router = axum::Router::new().route(
    "/avatar",
    axum::routing::post(store).layer(
        MultipartLimits::new()
            .with_total_bytes(2 * 1024 * 1024)
            .with_field_bytes(2 * 1024 * 1024)
            .with_fields(4)
            .with_read_timeout(Duration::from_secs(10)),
    ),
);
```

Readers: `total_bytes()`, `field_bytes()`, `fields()`, `read_timeout()`.

The part count is checked *before* the read, so the part past the cap is
refused rather than parsed and then refused. Byte counts are checked as soon
as the chunk that crosses a cap arrives, so nothing past a cap is ever handed
to the caller.

| `MultipartError` | Status |
| --- | --- |
| `TooManyFields { limit }` | 413 |
| `FieldTooLarge { limit }` | 413 |
| `BodyTooLarge { limit }` | 413 |
| `ReadTimeout { after }` | 408 |
| `Parse { source }` | whatever axum maps the parser error to |

`MultipartError::problem()` builds the RFC 9457 document. Its `detail` is a
fixed per-category string; axum's `MultipartError::body_text` is deliberately
not used, because it can carry the parser's own message, which quotes header
bytes the client wrote.

### The streaming path

`BoundedMultipart::new(multipart, limits)` wraps axum's parser rather than
replacing it. Every read goes through the timeout, every part is counted
before it is handed out, and every chunk is added to both a per-part and a
whole-body total.

```rust,ignore
let mut multipart = BoundedMultipart::new(multipart, limits);
while let Some(mut field) = multipart.next_field().await? {
    while let Some(chunk) = field.chunk().await? {
        upload.write(chunk).await?;
    }
}
```

Parts are handed out one at a time and borrow the parser. That is `multer`'s
requirement, not a choice made here: part *n+1* cannot be read before part *n*
has been consumed.

`BoundedField` offers `name()`, `file_name()` (raw and unsanitized, exactly as
the client wrote it), `declared_content_type()`, `byte_len()`, `chunk()`,
`bytes()` (consuming, bounded by `field_bytes`, and meant for the small text
inputs beside the file) and `text()` (which returns `Ok(None)` rather than an
error when the part is not UTF-8, because that is the caller's schema failing,
not the transfer). `BoundedMultipart` itself reports `limits()`,
`fields_read()` and `bytes_read()`.

## Content-addressed storage

The object key is `SHA-256(bytes)` plus a whitelisted extension, split into
two levels of fan-out:

```text
ab/cd/abcd...9824.png
```

Path traversal is not one bug, it is a family of them, and the family is large
because the input is a string whose structure the attacker chose: `../`,
`..\`, `%2e%2e%2f`, `....//`, a NUL that truncates the name in the C library
underneath, an NTFS alternate data stream, a symlink the previous upload
planted. A sanitizer answers those one at a time and is only ever as complete
as the last person who thought about it. Content addressing answers all of
them at once, structurally: **no byte of the request reaches the path.** There
is nothing left for a traversal payload to be *in*.

Three things fall out of it. The same bytes uploaded twice land on the same
key and occupy the storage once. Re-uploading is idempotent rather than a race
between two writers for one name. And a filesystem disk gets 65,536 leaf
directories instead of one directory with a million entries in it.

`ContentAddress` carries `digest()` (64 lowercase hex characters,
`DIGEST_HEX_LEN`), `byte_len()`, `extension()`, `path()` and
`path_under(prefix)`. `ContentAddress::of(bytes, extension)` addresses a
buffer already in memory; `ContentHasher` (`new`, `update`, `byte_len`,
`finish`) does it a chunk at a time.

The original filename is not part of any of this. It is a label carried
alongside the object, rendered in a `Content-Disposition` header and shown in
a UI, and resolved by nothing. Keep both layers: the sanitizer makes the
metadata safe to display, and content addressing makes the path safe to
resolve. Neither substitutes for the other.

### Staging, because the key is not known until the end

The digest exists only after the last byte has gone past, so bytes go to a
unique transient key under `STAGING_PREFIX` (`_staging`) first and are moved
onto the content-addressed key afterwards. The staging key is process id,
wall-clock nanoseconds and a process-local counter — never anything the
client sent.

```rust,ignore
let mut upload = disk.begin_upload_under("avatars").await?;
upload.write(chunk).await?;
let address = upload.finish_verified(Extension::parse("png")?).await?;
```

`begin_upload()` puts the object at the root; `begin_upload_under(prefix)`
validates the prefix *before* a byte is written, rather than after the whole
body has been streamed to a key that turns out to have nowhere to go.

`UploadWriter::write` sends the chunk to the backend and to the hasher and
then drops it. Nothing accumulates: an upload of any size costs one chunk of
memory, which is what makes the size caps a policy rather than the only thing
standing between a request and the heap. The one exception is the sniff
buffer, at most 512 bytes, readable as `head()`.

`finish(extension)` closes the object — the close is what completes it on an
S3-compatible backend, and it happens before the move — then moves it onto
its content-addressed key. If something is already at the destination the
staging copy is deleted instead. That is not an optimization: the key *is* the
digest, so an object already there has exactly these bytes, and overwriting it
would be a no-op that can still fail (on Windows a rename over a file another
reader has open does).

`finish_verified(extension)` is the call an upload handler wants. It re-checks
the leading bytes against the extension and, on disagreement, removes the
staging object *before* returning the error, so a rejected upload costs no
disk.

`abort()` drops the staging object. **Neither `finish` nor `abort` is
optional.** Dropping an `UploadWriter` without calling one of them leaves the
staging object behind, and on an S3-compatible backend leaves the multipart
upload un-closed. There is no `Drop` that can fix that, because both fixes are
`async`.

### It is not access control

A digest is unguessable in practice, but "unguessable URL" is not
authorization — it leaks through referrers, logs and browser history like any
other URL. Authorize the download.

## The declared `Content-Type` is carried and never believed

A multipart part arrives with two claims about its type, and the client wrote
both of them. `Content-Type: image/png` is a string in a header the uploader
chose. `filename="avatar.png"` is a string in the same header, chosen by the
same uploader. Neither is evidence.

`BoundedField::declared_content_type()` hands the header value back, because
an application may want to log it or compare it. Nothing in Arcature decides
what a file *is* from it. The only statement about an upload that the client
did not author is the one made by its first few hundred bytes.

`sniff(bytes)` compares the leading `SNIFF_BYTES` (512) against a table of
magic numbers and returns `Option<SniffedType>`, which carries both the
canonical `extension()` and the `mime()`. `verify(bytes, &extension)` holds
the two to a symmetric agreement:

* An extension with a known signature — `png`, `jpg`, `pdf`, `docx`, `mp4`,
  and the rest of the table — **must** sniff to that signature. Bytes that
  sniff to nothing are refused, not waved through: "unrecognized" is exactly
  what a PHP script renamed `.jpg` looks like.
* An extension with no signature — `txt`, `csv`, an application's own — must
  sniff to *nothing*. If a `.txt` upload's first bytes are a PE header or a
  zip local-file header, the extension and the content disagree just as
  loudly, in the other direction.

Both directions are refusals, so there is no "unknown" state an upload can
land in and be accepted by default. `expected_signatures(&extension)` exposes
the table entry, or `None` for a format with no magic number. The OOXML
entries accept plain `zip` beside their own type, because a `.docx` *is* a zip
archive and whether the inner content-type part is close enough to the front
of the stream to be seen inside 512 bytes depends on how the producing
application ordered it.

`SniffError` has two variants: `Unrecognized { declared }` and
`Mismatch { declared, sniffed }`.

A `.txt` or `.csv` upload therefore succeeds with `content_type()` of `None`.
That is correct, not a gap: there was no signature to find.

## Serving a stored file back

Accepting an upload safely and serving it safely are two different jobs, and
doing the first perfectly buys nothing if the second hands the file to a
browser as a document. A stored file served inline is content the attacker
wrote, on the application's own origin, with the application's cookies
attached — stored XSS with extra steps.

```rust,ignore
use arcature::http::download::Attachment;

let label = SafeFilename::parse(&stored_name, &AllowedExtensions::documents())
    .map_err(|_| arcature::bad_request("that filename is not storable"))?;

Ok(Attachment::from_disk(&disk, &key)
    .await
    .map_err(|_| arcature::not_found("no such file"))?
    .with_filename(&label)
    .into_response())
```

Every `Attachment` response carries all four of these, by construction:

| Header | Value | Why |
| --- | --- | --- |
| `Content-Disposition` | `attachment`, plus the filename if one was set | the browser saves the file instead of rendering it. Nothing is a document, so nothing has an origin, so nothing has script |
| `X-Content-Type-Options` | `nosniff` | without it a browser is free to disagree with the declared type and render what it thinks it found |
| `Content-Security-Policy` | `default-src 'none'; sandbox` (`DOWNLOAD_CSP`) | the belt to the disposition header's braces, for the browser or plugin that renders it anyway |
| `Content-Type` | the sniffed media type, or `application/octet-stream` (`OCTET_STREAM`) | never taken from the request |

`Content-Length` is set as well when the length is known.

A recognized-but-scriptable media type is downgraded to
`application/octet-stream` rather than forwarded. The list is `text/html`,
`application/xhtml+xml`, `image/svg+xml`, `text/xml`, `application/xml`. The
disposition header should already have made this moot, and `nosniff` and the
sandbox policy should have made it moot twice; this is the third lock, and it
is there because the cost of being wrong is stored XSS on the application's
own origin.

`Attachment::from_disk(&disk, &path)` reads only the leading 512 bytes up
front, to decide the media type, and streams the remainder — so the response
costs one buffer rather than one object. It returns `StorageError` when the
object cannot be stat'd or read, *including* when it does not exist, which a
handler should translate into a 404 rather than passing through.
`Attachment::from_bytes(bytes)` serves something already in memory.
`with_content_type(sniffed)` overrides the type when it was sniffed at upload
time and stored beside the object; it takes a `SniffedType` and there is
deliberately no way to set an arbitrary string, so every media type this
response can carry is one some bytes were recognized as.

`with_filename` takes a `SafeFilename` rather than a `&str` on purpose — a
string parameter would be a header-injection hole waiting for the one caller
who forgot. An ASCII name goes out once, as `filename="report.pdf"`. A name that is
*not* ASCII goes out twice per RFC 6266 — the plain form plus an RFC 5987
`filename*=UTF-8''` — because a name with diacritics is not representable in
the first form and silently mangling it is worse than sending both. The
extended form is only worth sending when the plain one lost something.

## Limits

### axum's own 2 MiB body cap bites first

axum wraps a request body in `http_body_util::Limited` at **2 MiB** unless the
application raises `DefaultBodyLimit`. **Arcature never touches
`DefaultBodyLimit`** — grep the crate and the only mentions are in prose. So
on a default build the effective ceiling on an upload is 2 MiB, not the 16 MiB
`DEFAULT_TOTAL_BYTES` above, and a 5 MiB photograph is refused by axum before
`MultipartLimits` has seen a byte of it.

That failure arrives as `MultipartError::Parse` wrapping axum's
`StreamReadFailed`, and takes the status axum gives it — which is also a
**413**. axum downcasts the inner error to `http_body_util::LengthLimitError`
and answers `PAYLOAD_TOO_LARGE`, so a client cannot tell which of the two caps
refused it from the status alone. Only the `detail` string differs. An
application that accepts files larger than 2 MiB has to raise the axum limit
itself.

Stage 12 of the request pipeline is a separate, third cap:
`tower-http`'s `RequestBodyLimitLayer`, applied only when the application
calls `Application::body_limit(bytes)`. It defaults to absent in the
framework, and the generated application's `bootstrap/app.rs` sets it to
2 MiB. Nothing in `MultipartLimits` can raise either outer wall, and nothing
needs to: a body over one of them is refused without being buffered.

`MultipartLimits` is the inner bound that knows the body has *parts*. Carrying
a total there as well is not redundant — an application that never configured
stage 12 still gets one, and one that did gets to make a single upload route
stricter than the rest of the application without loosening anything.

### `read_timeout` is per read, not per request

The timeout wraps each individual read: each `next_field()` and each
`chunk()`. A large upload over a slow link is many reads that each return
promptly, and it is not what this refuses. What it refuses is the connection
that goes quiet with the request half-sent.

The consequence runs the other way too: a client that sends one byte just
inside the timeout, forever, is not stopped by the clock. It is stopped by
`total_bytes`, which is why both bounds exist. There is no whole-request
deadline here. `Application::timeout(duration)` at stage 13 of the pipeline is
the place for one.

### No image decoding, deliberately

Nothing in the upload path decodes anything. No image is parsed, no dimensions
are read, no pixel is produced, and there is no image crate in the dependency
graph. The type check is a comparison of at most 512 leading bytes against a
table of magic numbers, and that is all it is.

A decoder is an interpreter for attacker-controlled input, and decoders are
the densest source of memory-safety CVEs in any web stack. Running one in the
request path trades a type-confusion bug for a heap-corruption bug. A prefix
comparison has no state for hostile bytes to drive.

So if an application needs an image's dimensions, a thumbnail, a re-encode or
a strip of EXIF, that work belongs in a queue worker with its own memory bound
and its own timeout — see [Jobs](jobs.md) — and not in the handler. Put the
bytes on a disk, put the key in the payload, and let something that is allowed
to crash do the decoding.

### What else it does not do

* **It is not a virus scanner, and not a claim of safety.** A file can be
  genuinely, verifiably a PNG and still be malicious. What sniffing answers is
  narrower and worth having on its own: the bytes and the extension agree.
* **It does not clean up after a process that died mid-upload.** Staging
  objects live under one prefix (`_staging`) precisely so an operator can see
  at a glance whether anything was left behind, but nothing sweeps it.
* **It does not delete stored objects, ever.** Content addressing means two
  references can share one object, so nothing here can know when the last one
  went away. Reference counting and garbage collection are the application's.
* **It does not record the upload anywhere.** No table, no row, no metadata
  store. The handler gets an address and a filename and decides what to
  persist.
* **It does not rate-limit.** Stage 15 of the pipeline does, and an upload
  route usually wants a tighter limit than the rest of the application.
