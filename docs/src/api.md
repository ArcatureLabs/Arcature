# API

RFC 9457 problem details for errors, and an OpenAPI 3.1 document derived from
the application graph. Two subjects behind two features, sharing one rule:
both are generated from what the code already declares, never written a
second time by hand.

## Turning it on

`arcature::api` compiles unconditionally. `Problem`, `ProblemBuilder`,
`ProblemKind` and `PROBLEM_JSON` exist in a build with no features at all.
The reason is the validation subsystem: it answers a failed `#[validate]`
with a `Problem`, and a feature gate under `Problem` would make `validation`
depend on `api`.

| Feature | In `default` | What it adds |
| --- | --- | --- |
| none | — | `arcature::api` — `Problem`, `ProblemBuilder`, `ProblemKind`, `PROBLEM_JSON` |
| `api` | yes | `Problem` and `ProblemKind` in the prelude, `http::json()`, `Bound<T>` (with `dx` + `database`), `TestResponse::assert_problem` (with `test-kit`). Pulls `http` and `validator`. |
| `api-docs` | no | `api` + `uag`, which is what compiles `arcature::uag::codegen::openapi` |

`api-docs` is in neither `default` nor `fullstack`. An API description is a
map of the attack surface, so it is named explicitly or not at all.

## A problem document

```rust,ignore
use arcature::{Problem, ProblemKind};

Problem::of(ProblemKind::NotFound)
    .with_detail("user 42 does not exist")
    .with_instance("/users/42")
```

```json
{
  "type": "urn:arcature:problem:not-found",
  "title": "Resource not found",
  "status": 404,
  "detail": "user 42 does not exist",
  "instance": "/users/42"
}
```

The `IntoResponse` impl sets the status from the problem, `Content-Type:
application/problem+json`, and `Content-Length`.

| Member | Comes from | Omitted when |
| --- | --- | --- |
| `type` | `kind.type_uri()`, or the URI given to `custom` | never |
| `title` | `kind.title()`, or the status reason phrase for `custom` | never |
| `status` | `kind.status()`, or the status given to `custom` | never |
| `detail` | `with_detail(..)` / `.detail(..)` | not set |
| `instance` | `with_instance(..)` / `.instance(..)` | not set |

Three constructors:

| Constructor | For |
| --- | --- |
| `Problem::of(kind)` | one of the distinguished categories |
| `Problem::builder(kind)` | the same, chained, finished with `.build()` |
| `Problem::custom(type_uri, status)` | a category outside the list |

`Problem::custom` takes the `title` from the status reason phrase —
`StatusCode::PAYMENT_REQUIRED` gives `"Payment Required"` — falling back to
`"Request error"` for a status with no canonical reason. Pass `"about:blank"`
as the type when the status is the whole story.

This is not a `{ "success": false, "message": "..." }` envelope, and it is not
mandatory. A handler returning any `IntoResponse` is free to ignore `Problem`
entirely.

### Extensions

Anything beyond the five standard members is an extension member, serialized
flat alongside them. `with_extension(key, value)` and `.extension(key, value)`
add one; `with_extensions(&value)` and `.extensions(&value)` add every
top-level pair of a value that serializes to a JSON object.

Four things are dropped in silence:

| Dropped | Why |
| --- | --- |
| a key equal to `type`, `title`, `status`, `detail` or `instance` | an extension must never be able to rewrite a standard member. A key that could set `status` to `200` on a `500` is the attack. |
| a value that serializes to JSON `null` | absent and null say the same thing |
| a value whose serialization fails | a response is not the place to discover it |
| a `with_extensions` argument that is not a JSON object | there are no top-level pairs to take |

Extensions live in a `BTreeMap<String, Value>`: one entry per key, last write
wins.

The `detail` member must be short and client-safe. Nothing in `Problem::of`,
`Problem::custom` or the `IntoResponse` impl adds server-side context, so what
leaves is what you put in. Extension members are entirely the application's
responsibility.

If serializing the whole document fails, the body falls back to a fixed
`urn:arcature:problem:internal` string rather than panicking. The status line
is still the problem's own status; only the body is replaced.

`Problem` derives `Debug` and `Clone`, and implements `Serialize` by hand. It
is not `PartialEq` and not `Deserialize`. `ProblemKind` derives `Debug`,
`Clone`, `Copy`, `PartialEq` and `Eq`.

## The kinds

| Variant | Status | `title` | `type` |
| --- | --- | --- | --- |
| `BadRequest` | 400 | Bad request | `urn:arcature:problem:bad-request` |
| `MalformedJson` | 400 | Malformed JSON request body | `urn:arcature:problem:malformed-json` |
| `Authentication` | 401 | Authentication required | `urn:arcature:problem:authentication` |
| `Authorization` | 403 | Access denied | `urn:arcature:problem:authorization` |
| `NotFound` | 404 | Resource not found | `urn:arcature:problem:not-found` |
| `MethodNotAllowed` | 405 | Method not allowed | `urn:arcature:problem:method-not-allowed` |
| `Timeout` | 408 | Request timed out | `urn:arcature:problem:timeout` |
| `Conflict` | 409 | Request conflicts with current state | `urn:arcature:problem:conflict` |
| `PayloadTooLarge` | 413 | Request body too large | `urn:arcature:problem:payload-too-large` |
| `UnsupportedMediaType` | 415 | Unsupported media type | `urn:arcature:problem:unsupported-media-type` |
| `Validation` | 422 | Validation failed | `urn:arcature:problem:validation` |
| `RateLimit` | 429 | Rate limit exceeded | `urn:arcature:problem:rate-limit` |
| `Internal` | 500 | Internal server error | `urn:arcature:problem:internal` |
| `Unavailable` | 503 | Service unavailable | `urn:arcature:problem:unavailable` |

`ProblemKind::ALL` is the same fourteen as a `&'static [ProblemKind]`, kept by
hand so that adding a variant without adding it there fails a test rather than
quietly narrowing what the tests check.

The `type` values are URNs, not URLs. The rejected alternative was an
`https://` URI under a docs domain, which reads better and promises a page
that has to stay alive at that exact address for as long as any client is
running. RFC 9457 permits a `type` that does not dereference, and a client is
required to treat an unknown one as `about:blank`, so the URN costs nothing
and commits to nothing.

The list is closed. An application-specific category is `Problem::custom`,
not a new variant.

### Turning a bare status into a kind

`ProblemKind::for_status(status) -> Option<ProblemKind>` is what gives a
status a body when whatever produced it did not.

| Status | Result |
| --- | --- |
| any status in the table above, except `400` | that row's variant |
| `400` | `BadRequest`, never `MalformedJson` |
| anything else — `402`, `418`, `502`, `504` | `None` |

The mapping is partial on purpose. `400` resolves to the generic kind because
a bare `400` arriving from a layer is not evidence about JSON, and a status
with no distinguished kind gets a generic document rather than being pushed
into a category it does not belong to.

## How a framework error becomes a problem response

There are two paths through the framework, and they are not the same code.

### Errors a layer produced: `ErrorMapping`

Most error responses in a Rust web stack come from something other than the
application. Axum answers an unmatched path with a bare `404`; `tower-http`
answers an oversized body with a bare `413` and an expired deadline with a
bare `408`. Bare is literal: status line, no `Content-Type`, no body. A
`fetch()` caller gets `""` to parse.

`ErrorMapping` is stage 11 of the [pipeline](deployment.md). It is not
installed by default — the slot is `None` until `.error_mapping(..)` is
called — and the application `arc new` generates calls it:

```rust,ignore
use arcature::http::ErrorMapping;

Application::<AppState>::new()
    .catch_panic()
    .error_mapping(ErrorMapping::new())
```

It sits inside the panic catcher and outside the body limit, the timeout, the
session, CSRF and the router, so it sees the responses it exists to dress and
a mapped response is still compressed, still carries the security headers, and
is still logged under its real status.

Precedence, in order:

1. A custom mapper from `ErrorMapping::with(..)`, if it returns `Some`.
2. Redaction, if the response is a `text/plain` 5xx and redaction is on.
3. A problem body, if the response has no `Content-Type` at all.
4. Otherwise the response is passed through untouched.

Anything that is not a 4xx or a 5xx is returned untouched before any of that
runs.

A replacement keeps every header the original carried except `Content-Type`
and `Content-Length`. That matters more than it looks: a `405` carries
`Allow`, a `429` carries `Retry-After`, a `401` carries `WWW-Authenticate`.
Those are the parts a client acts on, and dropping them to deliver a nicer
body would be a bad trade.

`ErrorMapping::with(..)` is handed the status and the headers of the
*request*, never the response body. Reading the body would mean buffering
every error response, and a mapper that needs it is a handler. The request
headers are what content negotiation actually wants — `Accept`,
`X-Requested-With`, `X-Inertia` — so a mapper can answer HTML to a browser
and a problem document to everything else.

### Errors a handler returned: `Error`

A controller returns `Result<Response>`, whose error type is
`arcature::Error`. Its own `IntoResponse` does **not** build a `Problem`.

| Variant | Status | `code` |
| --- | --- | --- |
| `NotFound` | 404 | `not_found` |
| `BadRequest` | 400 | `bad_request` |
| `Unauthorized` | 401 | `unauthorized` |
| `Forbidden` | 403 | `forbidden` |
| `Validation` | 422 | `validation_failed` |
| `Redirect` | 400 | `invalid_redirect` |
| `Io` | 500 | `io_error` |
| `Database` | 500 | `database_error` |
| `Cache` | 500 | `cache_error` |
| `Storage` | 500 | `storage_error` |
| `Mail` | 500 | `mail_error` |
| `Job` | 500 | `job_error` |
| `Serialization` | 500 | `serialization_error` |
| `Config` | 500 | `config_error` |
| `Other` | 500 | `internal_error` |

The body is `application/json` — not `application/problem+json` — with
`type` set to `urn:arcature:problem:` plus the `code` above. Those codes carry
underscores, so `Error::NotFound` produces `urn:arcature:problem:not_found`
while `ProblemKind::NotFound` produces `urn:arcature:problem:not-found`. Two
different strings for the same idea.

Its redaction is keyed on the `APP_ENV` environment variable, read at response
time: `production` or `prod` (case-insensitive) emits `type`, `title` and
`status` and stops; anything else, **including an unset variable**, adds
`detail` from the error's `Display`, which for `Error::Database` is the
underlying driver message. Because the body is `application/json`,
`ErrorMapping` passes it through unchanged.

## Redaction

`ErrorMapping::new()` sets redaction to `!cfg!(debug_assertions)`.

| Build | `redacts()` |
| --- | --- |
| `cargo build`, `cargo test` — `debug-assertions` on | `false` |
| `cargo build --release` — `debug-assertions` off by default | `true` |

`ErrorMapping::redact_errors(bool)` overrides it in either direction. `true`
in a development build is how a test asserts that nothing leaks.

Keying on `debug_assertions` rather than on an environment variable is the
decision. The rejected alternative reads `APP_ENV`, which means a production
binary can be talked into leaking by whoever can set a variable on the host,
with no redeploy and no diff. A compile-time key is decided by the build that
produced the artifact.

What the stage does to a 4xx or 5xx:

| Response leaving the stage | Result |
| --- | --- |
| no `Content-Type` at all | replaced with a problem document |
| `405`, `408` or `413` with `text/plain` | replaced, whether redaction is on or off |
| any other 5xx with `text/plain` | replaced when `redacts()` is true |
| a 4xx with `text/plain` | untouched |
| any status with `text/html`, `application/json`, `application/problem+json`, or anything else | untouched |

`405`, `408` and `413` are the three statuses that, inside this pipeline, come
from a layer rather than a handler, so a `text/plain` body on one of them is a
library's string — `length limit exceeded` — and not a message anyone wrote
for this application's clients. A handler that returns one of those itself has
its body replaced too. That is a smaller loss than leaving an API client with
an unparseable sentence.

A `text/plain` 4xx is left alone because it is a message written for the
client, and deleting it would delete the explanation.

The narrowness is deliberate: a 5xx carrying HTML or JSON is a body somebody
chose, and only the shape nothing chooses on purpose gets replaced. The cost
of that choice is stated under [what this deliberately does not
do](#what-this-deliberately-does-not-do).

Panics are separate. `.catch_panic()` (stage 10, also opt-in) answers with
`Problem::of(ProblemKind::Internal)` and discards the payload entirely — no
`detail`, in any profile. A panic message is written for a developer reading a
backtrace and routinely contains a path, a SQL fragment, or the value that
caused it. The operator still gets all of it from `tower-http`'s log.

## Building an API resource

```rust,ignore
use arcature::resource;

#[resource]
pub struct LinkResource {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
}
```

`#[resource]` takes no arguments, requires named fields, and emits three
things:

1. the struct unchanged, with a `#[derive(Serialize)]` added;
2. `impl inertia::ClientData`, whose `exposure_schema()` is built from the
   named fields — the explicit browser-exposure opt-in;
3. `impl ResourceMetadata`, the same fields as a `&'static [FieldShape]`,
   which `routes!` resolves when a route declares `query: T`.

It generates no `PAGE_CONTRACT`. A resource is a value nested inside page
props, not a page. It needs `macros`, `dx` and `inertia` — `ClientData` lives
in the Inertia module.

A SeaORM entity is not a resource. Convert explicitly with `impl From<Link>
for LinkResource`. The reason is that `Serialize` is not a safety boundary: a
field whose type is not a recognised primitive maps to
`PropsSchema::nested::<T>`, which requires `T: ClientData`, so an internal
domain model nested inside a resource fails to compile. Deriving exposure from
`Serialize` would make every model that can be logged also a model that can be
served.

Returning one:

| Return type | Response | Feature |
| --- | --- | --- |
| `Json<T>` | `T` as JSON, `application/json`, `Content-Length` set | `dx` |
| `Empty` | `204`, empty body | `dx` |
| `Problem` | the document, `application/problem+json` | none |
| `json(value)` | the same as `Json`, as a free function | `api` or `inertia` |

Declaring the shape at the route:

| Key | Means | Constraint |
| --- | --- | --- |
| `action: T` | the request body type; resolves `T: RequestMetadata`, which `#[request]` emits | a non-safe method. A GET is a compile error. |
| `query: T` or `query: Vec<T>` | the response type; resolves the element's `ResourceMetadata` | GET only. A POST is a compile error. |
| `query_string: T` | the typed query string of a query route | requires `query:` on the same route |

`action:` and `query:` on one route is a compile error: a route mutates or it
reads.

```rust,ignore
routes! {
    pub api {
        state: AppState;
        get  "/links"        => LinksController::index { name: links.index, query: Vec<LinkResource> }
        get  "/links/{link}" => LinksController::show  { name: links.show,  query: LinkResource }
        post "/links"        => LinksController::store { name: links.store, action: StoreLinkRequest }
    }
}
```

`Bound<T>` loads a model from the database by a route parameter and answers
with a problem when it cannot:

```rust,ignore
use arcature::{Bound, Json, Result};

async fn show(link: Bound<Link>) -> Result<Json<LinkResource>> {
    let link = link.into_inner();
    // authorize here -- binding did not
    Ok(Json(LinkResource::from(link)))
}
```

| Failure | Kind | Status |
| --- | --- | --- |
| the request has no path parameters | `BadRequest` | 400 |
| no parameter named `T::KEY_PARAM` | `BadRequest` | 400 |
| the value will not parse as `T::Key` | `BadRequest` | 400 |
| `T::load` returned an error | `Internal` | 500 |
| `T::load` returned `None` | `NotFound` | 404 |

Binding is not authorization. `Bound<T>` proves the row exists; whether this
caller may see it is a policy check the handler still owes. That invariant is
permanent — the alternative, an extractor that also authorizes, would make
every route's access rule invisible at the route.

`Bound<T>` needs `dx` + `database` + `api` together. It reads the database
handle through `DbFromState`, not `axum::extract::FromRef`, to avoid
orphan-rule conflicts in application state types.

## The OpenAPI document

`api-docs` turns on `uag`, and the document is generated from the UAG — the
same deterministic artifact behind `arc routes` and `arc typegen`.

There is no `utoipa` and no annotation on the handler. Everything in the
document already exists in the route descriptor: `routes!` baked the request
and response field shapes in, and `#[validate(...)]` rules travel with the
fields. The rejected alternative is attributes above each handler, which is a
second source of truth and is wrong the first time someone renames a field in
one place.

```rust,ignore
use arcature::uag::build;
use arcature::uag::codegen::openapi::{self, OpenApiOptions};

let artifact = build(&app::graph(), &app::page_contracts());
let document = openapi::generate_json(&artifact, &OpenApiOptions {
    title: "Acme API".to_owned(),
    version: "2026.8".to_owned(),
    description: None,
})?;
```

`generate` returns a `serde_json::Value`; `generate_json` returns pretty JSON.
`OpenApiOptions::default()` is title `"Arcature application"`, version
`"0.0.0"`, no description. There is no `generated_at` and no timestamp
anywhere: a timestamp would make every regeneration a diff, which is the one
thing a derived artifact exists to avoid.

Top level:

| Key | Contents |
| --- | --- |
| `openapi` | the const `"3.1.0"` |
| `info` | `title`, `version`, and `description` when set |
| `paths` | one item per path, keyed by lowercase method |
| `components.schemas` | one entry per named `action:` and `query:` type; absent when there are none |

Per operation:

| Key | Source | Absent when |
| --- | --- | --- |
| `operationId` | the route's `name:` verbatim; otherwise the lowercase method followed by the path with every non-alphanumeric replaced by `_` | never |
| `tags` | a single tag, the module name | the route has no module name |
| `parameters` | one `in: path` per path parameter, plus one `in: query` per `query_string:` field | there are none |
| `requestBody` | `application/json`, `required: true`, the `action:` schema | there is no `action:` |
| `responses` | below | see below |

| The route declares | `responses` |
| --- | --- |
| `query: T` | `200`, `application/json`, `$ref` to `T` |
| `query: Vec<T>` | `200`, `application/json`, an array of `$ref` to `T` |
| `page:` / `pages:` and no `query:` | `200`, `text/html`, no schema |
| neither | the key is omitted entirely |

An omitted `responses` is valid OpenAPI 3.1 and is the honest statement.
Claiming a `200` for a handler that redirects would make the document worse
than silence.

Axum and OpenAPI already agree on `{name}`, so only the wildcard marker is
rewritten: `/files/{*rest}` becomes `/files/{rest}`.

Rust types reach JSON Schema through one mapping, shared with the TypeScript
emitters:

| Rust | JSON Schema |
| --- | --- |
| `String`, `str`, `char` | `{"type": "string"}` |
| any integer or float | `{"type": "number"}` |
| `bool` | `{"type": "boolean"}` |
| `Vec<T>` | `{"type": "array", "items": T}` |
| `Option<T>` | `{"anyOf": [T, {"type": "null"}]}`, and the field is left out of `required` |
| anything else | `{}` |

References and lifetimes are stripped and a path is reduced to its last
segment, so `&'a std::string::String` and `String` map the same. Integer width
is not carried, because JSON has one number type and pretending a `u64`
survives JavaScript intact would be a claim the generated types cannot back
up. An unrecognised type becomes the empty schema, which accepts anything —
the honest statement about a type the mapping does not model.

`Option<T>` becomes `anyOf` rather than an omitted key because serde writes an
absent `Option` as `null`. Requiredness is the separate fact recorded in the
object's `required` list.

Validation rules become constraints only where the translation is exact:

| Rule | Becomes |
| --- | --- |
| `email` | `"format": "email"` |
| `url` | `"format": "uri"` |
| `length(min, max)` on a string | `minLength` / `maxLength` |
| `length(min, max)` on a `Vec` | `minItems` / `maxItems` |
| `length(equal = n)` | both bounds set to `n` |
| `range(min, max)` | `minimum` / `maximum` |
| anything else, `regex(...)` and `custom(...)` included | nothing |

A non-numeric argument is skipped rather than coerced. `regex(...)` names a
Rust const, not a pattern the document could carry, and a constraint stated
wrong is worse than one left out, because a generated client enforces it.

Constraints land on the non-null branch: `Option<String>` with
`length(max = 5)` is `anyOf: [{string, maxLength 5}, {null}]`, not a
`maxLength` on the union.

## What this deliberately does not do

**Nothing serves the document.** The `api-docs` comment in `Cargo.toml` names
`/_arcature/openapi.json` and `/_arcature/docs`. Neither route exists. No
source file in the crate is compiled under `cfg(feature = "api-docs")`, so the
feature's entire effect today is to enable `api` and `uag`. Producing the
document means calling `openapi::generate_json` yourself, from a binary or a
test you write.

**No `arc` command emits it.** `arc typegen` writes four files to
`resources/js/generated/` — `routes.ts`, `pages.d.ts`, `forms.ts`,
`index.ts` — and the OpenAPI document is not one of them.

**The document describes success only.** The generator emits exactly one
response, a `200`, and only for a route that declares `query:` or a page. Not
one of the problem documents in this chapter appears in it: not the `422` from
a validated extractor, not the `404` from `Bound<T>`, not the `429` from the
rate limiter. A client generated from it has no error types.

**No `security`, no `servers`, no `securitySchemes`.** A route's `policy:` and
`policies:` are in the artifact and the generator does not read them. The
document does not say which routes need authentication or what they need.

**No summaries, descriptions or examples per operation.** Rust doc comments
are not in the route descriptor, so there is nothing to copy across.

**Path parameters are always strings.** `{"type": "string"}` for every one,
whatever the handler parses it into. The descriptor carries the name, not the
type.

**A request body is always `application/json`.** A route whose `action:` type
arrives as a form submission is still described as JSON.

**Redaction does not cover a JSON body.** Only `text/plain` on a 5xx is
replaced. A `500` whose body is `application/json` or
`application/problem+json` is passed through in every profile. `Bound<T>`
produces one of those: its database-error branch is a `ProblemKind::Internal`
problem whose `detail` is `"database error: "` followed by the driver's
message. `ErrorMapping::with(..)` runs ahead of the redaction check and is
the place to catch it.

**`Error` is not `Problem`.** A handler error takes the second path described
above: `application/json`, an underscored `type` URI, and redaction keyed on
`APP_ENV` at response time rather than on the build. An unset `APP_ENV` is the
non-production branch, so it includes `detail`. `TestResponse::assert_problem`
fails on such a response twice over, on the content type and on the `type`
URI, which is the fastest way to notice which path a route is on. The two
`errors` shapes differ too: `validation_problem` writes an object keyed by
field name holding `[{ "code", "message"? }]`, while `Error::Validation`
writes an array of `{ "field", "message" }` — and writes no `errors` member
at all on the production branch.

**The `type` URIs do not resolve.** They are URNs. There is no page behind
`urn:arcature:problem:not-found` and none is planned.

**`ErrorMapping` is not on unless asked for.** The pipeline slot is `None`
until `.error_mapping(..)` is called. `arc new` calls it; an application that
assembles its own builder and does not gets bodiless `404`s and no redaction
at all.

**No content negotiation, and no mapper shipped.** Everything is a problem
document whatever the request's `Accept` header, so a browser hitting an
unmatched path receives JSON. `ErrorMapping::with(..)` exists precisely to fix
that, and receives the request headers for that purpose, but the framework
ships no HTML error page to install.

**No response envelope.** `Json<T>` writes `T` and nothing around it — no
`data` key, no `meta`, no `links`, no sparse-fieldset or filtering vocabulary.
`paginate(per_page).page(n)` hands back rows and `page_with_count` hands back
rows and a total; shaping those into a response is the resource's job, because
an envelope the framework picked would be one every client then has to unwrap.

**`Problem` cannot be parsed back.** It implements `Serialize` and not
`Deserialize`. A test or a Rust client reads a problem response as
`serde_json::Value`, or through `assert_problem`.
