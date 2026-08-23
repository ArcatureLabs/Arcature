# Views

Server-rendered HTML from templates that are Rust code by the time the binary
exists.

`#[derive(Template)]` reads the `.html` file when the crate compiles and emits
the `write!` calls that produce the page. What ships is a function. There is
no template text in the process, no loader, and no evaluator.

## Turning it on

The feature is `views`, and it is off in the framework's default set:

```toml
arcature = { version = "0.1", features = ["views"] }
```

`views = ["dep:askama"]`. It pulls nothing else.

A generated application already has it on -- `arc new` writes `app/views/`,
`templates/layout.html` and `templates/welcome.html`, and lists `"views"` in
the app's `Cargo.toml`. If every screen in the application is an
[Inertia](inertia.md) page, remove the feature and those two directories
together.

| Where | State |
| --- | --- |
| framework `default` | off |
| framework `fullstack` | on |
| generated application | on |

Askama is pinned at `0.16.0` with `default-features = false` and only two of
its features enabled: `derive` and `std`. The omissions have consequences you
can see from application code.

| Askama feature | Consequence of it being off |
| --- | --- |
| `config` | TOML parsing is off, so an `askama.toml` is a compile error rather than a config file. Arcature ships none, so the template directory, the syntax and the escaper table are the defaults. |
| `urlencode` | There is no `urlencode` or `urlencode_strict` filter. |
| `serde_json` | There is no `json` or `json_pretty` filter. |
| `code-in-doc` | `#[template(in_doc = true)]` is unavailable. |

`config` and `urlencode` are two of the four askama defaults, so this is a
narrowing, not an unchanged baseline. `config` would add `basic-toml`, `glob`,
`serde` and `serde_derive` to the build-time graph to read a file the framework
does not ship; `urlencode` would add `percent-encoding` for a filter an HTML
template does not need, since escaping is the autoescaper's job.

`askama_axum` is deliberately absent as well. It was folded into askama and
then dropped; the `IntoResponse` impl lives in `src/view/response.rs`, where
it can answer a render failure the way the rest of the framework answers one.

`views` does not imply `observe`. That default matters and it is covered under
[Render failures](#render-failures).

## Why the templates are compiled

This is the decision the feature exists to make, so it is worth arguing rather
than asserting.

A runtime template engine -- minijinja, tera, handlebars -- is two programs
shipped as a library: a parser that turns template text into a tree, and an
evaluator that walks the tree against a context and produces output. Both of
them run inside the request path, because that is when the template is
rendered.

That arrangement is what server-side template injection is. SSTI is not a
parsing bug; it is the engine doing exactly its job on input that reached it
from the wrong direction. If any string a request controls is handed to the
parser -- a template chosen by name from a query parameter, a page fragment
stored in a database and rendered as a template, a subject line assembled with
`format!` and then passed through the engine -- then the evaluator will
evaluate it. An expression language with attribute access and method calls is
one hop from the host process, which is why SSTI is the shortest route there
is from a form field to remote code execution.

The usual answer is discipline: never render user input as a template, audit
the places templates are loaded from, keep the sandbox on. That is a defence,
and a defence is a thing that can be forgotten in one commit.

Askama makes the class of bug unreachable instead. The parser runs in the
proc-macro at build time. The output is Rust. At runtime there is no parser to
reach, no evaluator to abuse, and no template text in the binary to be
substituted -- only the statements the compiler emitted. There is nothing to
forget, because there is nothing there.

The same property has a second effect, which is smaller but is felt daily. A
runtime engine binds names when it renders, so a name the template uses and
the data does not supply is discovered when a page is served. A compiled
template resolves names against the struct's fields when the crate compiles: a
`{{ subtitle }}` with no `subtitle` field is a build failure, not a blank space
on a page nobody looked at.

| | Runtime engine | Compiled templates |
| --- | --- | --- |
| Parser in the request path | yes | no |
| Expression evaluator in the request path | yes | no |
| SSTI | defended against | structurally absent |
| Unknown name in a template | render time | compile time |
| Template must be on disk at runtime | yes | no |
| Editing a template | reload | rebuild |

The last row is the price, and it is a real one. It is paid in full under
[Costs](#costs).

Localization is the one place the framework accepted a runtime parser anyway,
and `src/i18n/mod.rs` states the boundary: a Fluent catalog is a file a
developer wrote and a request never names, supplies or selects. That is a
different input from a template rendering attacker-supplied values.

## Writing a view

A view is a struct whose fields are the values its template names.

```rust,ignore
use arcature::view::Template;

/// `templates/welcome.html`.
#[derive(Template)]
#[template(path = "welcome.html", askama = arcature::askama)]
pub struct WelcomeView {
    pub title: String,
    pub message: String,
}
```

`path` resolves against `templates/` in the crate root -- askama's default
directory, and with the `config` feature off there is no `askama.toml` that
could move it.

`askama = arcature::askama` is not decoration. `#[derive(Template)]` writes
code that says `askama::`, which does not resolve in a crate that depends only
on Arcature. Pointing the derive at the re-export means the application
compiles against the askama the framework pins and cannot drift to a second
version of it. An application that would rather write a bare
`#[derive(Template)]` can add `askama` to its own `Cargo.toml`; the price is a
version number to keep in step by hand.

For a template short enough to read in place, `source` and `ext` replace
`path`:

```rust
use arcature::view::{Template, view};

#[derive(Template)]
#[template(
    source = "<h1>{{ title }}</h1>",
    ext = "html",
    askama = arcature::askama
)]
struct Welcome {
    title: String,
}

let html = view(Welcome { title: "Hello".into() }).render().unwrap();
assert_eq!(html, "<h1>Hello</h1>");
```

The `#[template(..)]` keys this chapter relies on:

| Key | Meaning |
| --- | --- |
| `path` | template file, resolved under `templates/` |
| `source` | template text written inline; requires `ext` |
| `ext` | the extension `source` should be treated as having |
| `escape` | override the escaper the extension would select |
| `askama` | the path to the askama crate the derive should name |

### The template

The scaffold ships a base and one page that extends it. `templates/layout.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{% block title %}Acme{% endblock %}</title>
  </head>
  <body>
    {% block content %}{% endblock %}
  </body>
</html>
```

`templates/welcome.html`:

```html
{% extends "layout.html" %}

{% block title %}{{ title }}{% endblock %}

{% block content %}
<main>
  <h1>{{ title }}</h1>
  <p>{{ message }}</p>
</main>
{% endblock %}
```

Template syntax, inheritance and filters are askama's, and `arcature::view`
puts nothing in front of them. It is a seam, not a wrapper. The askama crate
is re-exported as `arcature::askama`, and `Template` -- both the trait and the
derive macro, which share one name -- comes from `arcature::view::Template` or
from the prelude.

## arc make:view

```sh
arc make:view admin/receipt
```

writes two files:

| Path | Contents | Registered in a `mod.rs` |
| --- | --- | --- |
| `app/views/admin/receipt_view.rs` | `pub struct ReceiptView` with `#[template(path = "admin/receipt.html", askama = arcature::askama)]` | yes -- `pub mod receipt_view;` |
| `templates/admin/receipt.html` | a template extending `layout.html` | no |

The struct is the file stem plus `View`; the template keeps the base name,
because `path` names a template and nothing in askama makes it a type's name.
The generated struct has two fields, `title: String` and `message: String`, and
the generated template uses both.

Only the Rust half is declared to rustc. There is no `templates/admin/mod.rs`,
because `templates/` is read by the askama derive rather than walked by the
compiler, and a `mod.rs` there would be a Rust file in a directory that has no
Rust in it.

Two files rather than one is the point of this generator. Askama reads the
template when the crate compiles, so a view struct whose `path` names a file
that is not there is not a scaffold with a gap in it -- it is a compile error,
and the project stops building until somebody writes the half the generator
declined to. The pair is the artifact.

## Rendering a view as a response

`view(template)` wraps a template value; `View::new(template)` is the same
thing with a name that is easier to use in generic code.

```rust,ignore
use arcature::prelude::*;

use crate::app::views::WelcomeView;
use crate::bootstrap::AppState;

pub struct HomeController;

#[controller]
impl HomeController {
    /// `GET /welcome`
    pub async fn welcome(State(state): State<AppState>) -> Result<Response> {
        Ok(view(WelcomeView {
            title: state.app_name.clone(),
            message: "Rendered on the server.".to_string(),
        })
        .into_response())
    }
}
```

The return type is `Response`, not `Page<..>`: there is no client component
behind a view, and the HTML is finished when it leaves the server.

A fresh `View` carries three things a compiled template does not know:

| Property | Default | Set with |
| --- | --- | --- |
| status | `200 OK` | `.status(StatusCode)` |
| content type | `text/html; charset=utf-8` | `.content_type(HeaderValue)` |
| `Content-Language` | **absent** | `.in_locale(&Locale)` (feature `i18n`) |

HTML is the default rather than a guess from the template's extension because
askama 0.16 does not keep the extension on the compiled type -- `Template` has
no `MIME_TYPE` to read. A view over a `.txt` or `.xml` template has to say so:

```rust
use arcature::axum::http::HeaderValue;
use arcature::prelude::*;
use arcature::view::{Template, view};

#[derive(Template)]
#[template(
    source = "User-agent: *\nDisallow: {{ path }}\n",
    ext = "txt",
    askama = arcature::askama
)]
struct Robots {
    path: String,
}

let response = view(Robots { path: "/admin".into() })
    .content_type(HeaderValue::from_static("text/plain; charset=utf-8"))
    .into_response();

assert_eq!(response.headers()["content-type"], "text/plain; charset=utf-8");
```

The rest of the surface is small: `.render()` produces a `String`,
`.template()` borrows the wrapped value, and `.into_template()` gives it back.
`View<T>` is `Debug + Clone` and `#[non_exhaustive]`.

`arcature::view` exports `View`, `ViewError`, `view` and `Template`; the crate
root re-exports `View`, `ViewError` and `view`; the prelude carries `Template`,
`View` and `view` under the `views` feature, which is why the controller above
imports nothing else.

### Render failures

`ViewError` has one variant, `Render { source: askama::Error }`. That is the
whole runtime failure surface: askama resolved the parse and the names at build
time, so what is left is a value whose `Display` impl returned `Err`, or a
writer that refused the bytes.

When `IntoResponse` hits one, the response is a plain `500` with the
framework's ordinary internal error body. It says nothing, and it is worth
being precise about what "nothing" excludes, because the obvious
implementation leaks all three:

* the template's own text, which is application source, and on an error page
  is often the half somebody was mid-edit;
* the template's path, which is a map of the source tree and of the filesystem
  the process runs on;
* the value that would not format -- whatever the failing `Display` had
  already written before it gave up, plausibly a session token or a database
  row.

The conversion `From<ViewError> for Error` produces `Error::Other("view
rendering failed")`, which answers status `500` with code `internal_error`.
There is no development-mode variant that shows more, because there is no
build in which a template's contents are a reasonable thing to send to a
browser. A `Content-Language` a handler declared does not survive onto the
failure response either: the body is the framework's error document, not the
page that failed.

The askama message goes to `tracing::error!` -- and this is where the feature
graph bites. `tracing` arrives with `observe`, and `views` does not imply it.
**In a build with `views` and without `observe`, the askama message is
discarded with nothing recorded anywhere.** The client still gets its
uninformative `500`; the operator gets silence. If you enable `views`, enable
`observe`.

### Declaring a language

`in_locale` exists only under the `i18n` feature and sends `Content-Language`:

```rust,ignore
let response = view(Greeting { locale: locale.clone() })
    .in_locale(&locale)
    .into_response();
```

The framework does not infer this header, in either direction. A compiled
template carries no language -- askama resolved it to `write!` calls -- and the
locale `LocaleLayer` negotiated is what the request *asked* for, which is not
the same claim as what the bytes in the response are actually in. A handler
that renders a French template says so; one that renders a template it did not
translate says nothing, which is better than an untrue header.

Translation itself stays in the template: give the struct a `Locale` field and
call it. There is no filter and no `{{ t("key") }}` syntax, because adding one
would mean a lookup the compiler cannot check -- the opposite of the reason
this module exists.

## Mail bodies from the same templates

With `views` on, the [Mail](mail.md) builder grows two terminators that take
compiled templates instead of strings.

```rust
use arcature::mail::Email;
use arcature::view::Template;

#[derive(Template)]
#[template(
    source = "Hello {{ name }}, your invoice is ready.",
    ext = "txt",
    askama = arcature::askama
)]
struct InvoiceText {
    name: String,
}

#[derive(Template)]
#[template(
    source = "<p>Hello {{ name }}, your invoice is ready.</p>",
    ext = "html",
    askama = arcature::askama
)]
struct InvoiceHtml {
    name: String,
}

let message = Email::builder()
    .from("Billing <billing@example.com>".parse().unwrap())
    .to("ada@example.com".parse().unwrap())
    .subject("Your invoice")
    .templated(
        &InvoiceText { name: "Ada".into() },
        &InvoiceHtml { name: "Ada".into() },
    )
    .unwrap();

let raw = String::from_utf8(message.formatted()).unwrap();
assert!(raw.contains("multipart/alternative"));
```

| Terminator | Produces |
| --- | --- |
| `templated(plain, html)` | `multipart/alternative` from both templates |
| `templated_with_attachments(plain, html, attachments)` | the same, inside `multipart/mixed` |

Plain first, matching `Email::alternative`.

Both halves are taken in one call on purpose. A `multipart/alternative` mail
carries the same message twice, and the two copies drifting apart is the
ordinary way mail templating goes wrong: the HTML half gets the new wording,
the plain half keeps the old, and only the readers on the text client ever see
it. Taking the pair together makes a change to a message a change to a pair.

They are two templates rather than one because escaping is chosen by
extension. The `.html` template escapes its values; the `.txt` one does not.
Rendering a text body through an HTML template would send `&#38;` to somebody
reading plain text.

Both halves render before the message is assembled, so a template that cannot
render stops before a `Message` exists. The error is `MailViewError`:

| Variant | Cause | Becomes |
| --- | --- | --- |
| `Render { source: ViewError }` | either template failed to render | the same generic `500`, through `From<ViewError> for Error` |
| `Build { source: EmailError }` | lettre could not assemble the message | `Error::Mail(..)` |

The render path deliberately goes through the view conversion, so a template's
text cannot reach a response body by way of the mail subsystem either.

### These terminators do not fit `Mailable`

`Mailable::build` is declared `fn build(&self, email: Email) -> Result<Message,
EmailError>`. `templated` returns `Result<Message, MailViewError>`, and there
is no `From<MailViewError> for EmailError` -- `EmailError` is not
`#[non_exhaustive]`, so it cannot grow a variant without breaking every
downstream match.

The consequence is concrete: **a `Mailable` implementation cannot call
`templated` and use `?`.** This does not compile, and no import fixes it.

`arc make:mail` writes a plain-text `Mailable`, and its comment gives a
related but different reason -- that a `format!` into an HTML body escapes
nothing, so the HTML half should come from a template. The `templated`
incompatibility above is not mentioned there. To send a templated mail, build the `Message` outside the trait and
hand it to the mailer:

```rust,ignore
let message = Email::builder()
    .from(from_mailbox)
    .to(to_mailbox)
    .subject("Your invoice")
    .templated(&InvoiceText { name }, &InvoiceHtml { name })?;

mail.mailer().send(&message).await?;
```

`Mail::mailer()` borrows the transport and `Mailer::send(&Message)` takes a
finished message, so this path keeps the configured transport and the capture
mailers used in tests. What it gives up is `Mail::to(..).send(..)` setting
`From` and `To` for you.

## How escaping works, and on what basis

Escaping is selected by the template's extension, at compile time, from a fixed
table. It is not a runtime decision and not a per-value one.

| Extension | Escaper | Effect |
| --- | --- | --- |
| `askama`, `html`, `htm`, `j2`, `jinja`, `jinja2`, `rinja`, `svg`, `xml` | `Html` | five characters replaced with entities |
| `md`, `none`, `txt`, `yml`, and no extension | `Text` | nothing is escaped |
| anything else | none exists | **compile error** |

That last row is worth reading twice. An extension in neither list is not a
silent fall-through to "no escaping" -- the derive fails with `no escaper
defined for extension '...'`. A `.rss` or `.csv` template does not build until
you say what it is, with `escape = "html"` or `escape = "none"`.

The HTML escaper replaces exactly five characters:

| Character | Output |
| --- | --- |
| `"` | `&#34;` |
| `&` | `&#38;` |
| `'` | `&#39;` |
| `<` | `&#60;` |
| `>` | `&#62;` |

So `{{ value }}` in an `.html` template with `<script>alert(1)</script>` in it
produces text on the page and not markup. There is a test in `src/view/mod.rs`
asserting exactly that, and a matching one asserting that a `.txt` template
leaves `a < b` alone, so neither half is taken on trust.

`{{ value|safe }}` opts out. Write it for markup you produced yourself, never
for a value that arrived on a request.

### Escaping is not context-aware

This is the part that a five-character replacement table cannot do, and it is
the same limitation every non-contextual autoescaper has.

The escaper does not know where in the document a value lands. It escapes the
same five characters whether the value is body text, an attribute value, a URL,
or the inside of a `<script>` block. Three consequences follow, and all three
are the template author's to handle:

* **URLs.** `<a href="{{ url }}">` with `url` set to `javascript:alert(1)`
  contains none of the five characters. It is emitted unchanged and it runs.
  Check the scheme in Rust before the value reaches the template.
* **Script blocks.** Inside `<script>`, HTML entities are not decoded the way
  they are in markup, and the five-character table is not a JavaScript string
  escape. Do not interpolate request data into a `<script>` body. Askama's
  `json` filter would be the tool for handing data to JavaScript, and this
  build does not have it -- `serde_json` is one of the askama features
  Arcature leaves off. Encode the value in Rust, or use an Inertia page, which
  is what the prop channel is for.
* **Unquoted attributes.** `<div class={{ value }}>` is injectable through a
  space, since space, `/`, `=` and backtick are all left alone. Quote every
  attribute.

Nothing here is specific to Arcature and nothing here is a defect in askama.
It is the boundary of what "the template escapes its values" means, and a
chapter that did not say so would be lying by omission.

## Costs

Three, and none of them is hypothetical.

### Editing a template means rebuilding

There is no reload. The template is in the binary as emitted code, so a change
to `templates/welcome.html` reaches a running process only after the crate is
compiled again.

`cargo build` does notice. Askama emits a
`const _: &[u8] = include_bytes!("<template path>")` for every template file it
reads, which makes rustc track the file as a dependency of the crate: touching
the `.html` alone is enough to make the next `cargo build` recompile.

**`arc dev` does not notice.** The supervisor's watcher -- see
[The dev loop](dev-loop.md) -- classifies a change, and only three kinds of
file mean anything to it:

| Change | Action |
| --- | --- |
| any `*.rs` | rebuild, then restart |
| `Cargo.toml`, `Cargo.lock` | rebuild, then restart |
| `.env`, `.env.*` | restart, no compile |
| everything else, templates included | nothing |

That filter is deliberate for the frontend -- a `.tsx` or `.css` edit is Vite's
business and costs no Rust rebuild -- and an askama template falls on the same
side of it. Saving a template during `arc dev` produces no rebuild and no
visible change on refresh. Touch any `.rs` file, or run `cargo build`, to pick
it up. (The comment in the generated `app/views/mod.rs` says `arc dev` already
rebuilds on save. It does not.)

### A Dockerfile has to COPY templates before cargo build

`templates/` is a source directory, in the same sense `src/` and `app/` are. It
is read by the compiler, not by the process.

This is the failure mode worth knowing before you meet it: the local build
succeeds, because the templates are on the developer's disk, and the image
build fails on a template it cannot find. The error names a path that plainly
exists, which is what makes it confusing rather than obvious.

The generated `Dockerfile` -- described in full under
[Deployment](deployment.md) -- copies it alongside the rest of the sources:

```dockerfile
COPY src ./src
COPY app ./app
COPY bootstrap ./bootstrap
COPY config ./config
COPY database ./database
COPY routes ./routes
# Askama reads the templates at *build* time and compiles them into the
# binary, so this is a source directory like the ones above, not runtime data.
COPY templates ./templates
RUN cargo build --release --locked
```

The runtime stage copies the binary, `public/` and `storage/`, and no
templates. Nothing at runtime reads them back, and a template shipped into a
production image is dead weight at best.

### Views and Inertia are both on, and that is fine

A generated application serves both. Inertia renders the application -- the
screens behind sign-in, where the client is already loaded and a JSON page
object is the cheap answer. Views render the pages that have to work with no
JavaScript at all: an unsubscribe confirmation, an emailed receipt, an RSS
feed, a marketing page, a fallback error page.

They do not interact. A view is a plain `Response` and never carries a page
object; an Inertia page never goes through `View`. The scaffold demonstrates
the split in one controller: `GET /` returns `Page<HomePage>` and `GET /welcome`
returns `Response` from `view(WelcomeView { .. })`.

The one shared cost is the two directory trees, `app/pages/` plus
`resources/js/pages/` for one and `app/views/` plus `templates/` for the other.
If an application never serves HTML from the server, delete `app/views/` and
`templates/` and drop `"views"` from its feature list; if it never serves an
SPA, the same applies to `inertia`. Keeping both is a choice, not a default you
are stuck with.

## What views do not do

Collected, because a chapter that lists only what works is useless to somebody
deciding whether to depend on this.

* **No runtime template loading.** There is no `render("name", context)` taking
  a template chosen at runtime, and there cannot be one. That is the feature.
* **No hot reload**, and no rebuild from `arc dev` on a template edit.
* **No `askama.toml`.** The `config` feature is off, so the template directory,
  the syntax and the escaper table are the defaults.
* **No `urlencode` filter, and no `json` filter.** Those askama features are
  off too.
* **No content-type inference.** A `.txt` or `.xml` view answers
  `text/html; charset=utf-8` until you call `.content_type(..)`.
* **No `Content-Language` unless you declare one**, and no inference from the
  negotiated locale.
* **No translation filter.** No `{{ t("key") }}`; put a `Locale` on the struct.
* **No context-aware escaping.** Five characters, everywhere, regardless of
  where the value lands.
* **No render-failure detail anywhere without `observe`.** The message is
  dropped, not logged.
* **No `Mailable` integration.** `templated` returns an error type
  `Mailable::build` cannot return.
