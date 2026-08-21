# Validation

Validation is the trust boundary. At the point a handler receives a validated
value, it has passed `validator::Validate::validate`, and the handler does not
re-check it.

Validation does not imply authorization. A validated request is a
well-formed one, not a permitted one; authorization is a separate explicit
step, covered in [Authentication](auth.md).

## Declaring a request

```rust,ignore
use arcature::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[arcature::request]
pub struct StoreLinkRequest {
    #[validate(url)]
    pub url: String,
    #[validate(length(min = 1, max = 120))]
    pub title: String,
}
```

Two details that are easy to get wrong.

The rule attribute is `#[validate(...)]`, not `#[rule(...)]`. `#[request]`
prepends `#[derive(::arcature::validator::Validate)]`, and `#[validate]` is
that derive's helper attribute, so the rule vocabulary is the `validator`
crate's: `required`, `email`, `url`, `length`, `range`, `regex`, `contains`,
`custom`, `nested`.

You derive `Deserialize` yourself. The macro deliberately does not add it, to
avoid a duplicate derive when you also want `Serialize` or `Debug`. The
`#[arcature::request]` attribute goes *after* the derives.

Because the macro re-exports `validator` through Arcature, an application
does not need `validator` as a direct dependency.

## What the macro emits

Three things beside the struct:

- `#[derive(Validate)]` and `#[validate(crate = "::arcature::validator")]`.
- `impl arcature::Request`, the marker that makes the type first-class to
  tooling.
- `impl arcature::RequestMetadata`, a `&'static [FieldShape]` describing the
  fields, which `routes!` resolves when a route declares `action: T` so the
  typed input shape lands in the `RouteDescriptor`.

## Using it in a handler

```rust,ignore
use arcature::Validated;

pub async fn store(input: Validated<StoreLinkRequest>) -> Result<Response> {
    let data = input.into_inner();
    Ok(redirect().to("/links").into_response())
}
```

`Validated<T>` extracts a JSON body, deserializes it, and validates it before
the handler body runs. A failure never reaches the handler; it becomes a
response.

Four narrower extractors exist for the other sources:

| Extractor | Source |
| --- | --- |
| `ValidatedJson<T>` | JSON body |
| `ValidatedForm<T>` | form body |
| `ValidatedQuery<T>` | query string |
| `ValidatedPath<T>` | path parameters |

`Validated<T>` delegates to `ValidatedJson<T>`.

For a value you extracted yourself, `validate_or_problem(&value)` validates
it and returns `Err(Problem)` on failure.

## What a failure looks like

A validation failure is an RFC 9457 problem document, `422`, with the field
errors under an `errors` extension:

```json
{
  "type": "urn:arcature:problem:validation",
  "status": 422,
  "detail": "Request validation failed",
  "errors": {
    "url": [{ "code": "url" }],
    "title": [{ "code": "length" }]
  }
}
```

Extractor rejections — malformed JSON, a missing query parameter, a path
segment that will not parse — are mapped to problem documents too, by
`from_json_rejection` and friends, so a client sees one error shape rather
than two.

`validation_problem(errors)` builds the document from a
`validator::ValidationErrors` directly if you need to raise one by hand.
