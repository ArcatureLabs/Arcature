//! Converts `validator::ValidationErrors` into a client-safe JSON tree and a
//! [`Problem`](crate::Problem).
//!
//! The validator crate reports validation failures as a nested tree of
//! `ValidationErrors` keyed by field name, with three kinds per key: `Field`
//! (a list of `ValidationError`), `Struct` (nested struct errors), and `List`
//! (collection errors keyed by index). This module walks that tree into a
//! stable JSON object used as the `errors` extension member of a validation
//! [`Problem`](crate::Problem).
//!
//! # Security -- no reflected hostile values
//!
//! Only the validation rule `code` (a static string like `"required"`,
//! `"email"`, `"length"`) and an optional developer-supplied `message` are
//! emitted. The `ValidationError::params` map is deliberately **not** emitted,
//! because it can contain the raw request value that failed (e.g. an
//! out-of-range number), and echoing that back to the client is a reflected-
//! hostile-value risk. Field names come from the Rust struct field names
//! (developer-controlled), never from request input.

use serde_json::{Map, Value};
use validator::{ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::api::{Problem, ProblemKind};

/// Build a validation [`Problem`] from `validator`'s error tree.
///
/// The problem is `ProblemKind::Validation` (422) with an `errors` extension
/// member carrying the structured field error tree.
#[must_use]
pub fn validation_problem(errors: ValidationErrors) -> Problem {
    Problem::of(ProblemKind::Validation)
        .with_detail("Request validation failed")
        .with_extension("errors", flatten_errors(&errors))
}

/// Validate `value` and return `Err(Problem)` on failure.
///
/// Convenience for handlers that have already extracted a value (e.g. via
/// `axum::extract::Query` or `Path`) and want to validate it. For JSON bodies,
/// prefer the [`crate::ValidatedJson`] extractor, which combines extraction and
/// validation and avoids double work.
pub fn validate_or_problem<T>(value: &T) -> Result<(), Problem>
where
    T: validator::Validate,
{
    value.validate().map_err(validation_problem)
}

/// The collection-root sentinel key validator uses for `Vec<T>`/`HashMap`
/// validation results. When the top-level errors map contains only this key,
/// the validated value was a collection, and the real errors live under it.
const COLLECTION_ROOT_KEY: &str = "_tmp_validator";

/// Walk a `ValidationErrors` tree into a JSON object keyed by field name.
fn flatten_errors(errors: &ValidationErrors) -> Value {
    // A pure collection root: validator reports it as a single
    // `_tmp_validator` key holding the list kind. Unwrap it so the top-level
    // value is the index-keyed object, not `{ "_tmp_validator": { ... } }`.
    if errors.0.len() == 1
        && let Some(kind) = errors.0.get(COLLECTION_ROOT_KEY)
    {
        return flatten_kind(kind);
    }
    let mut map = Map::new();
    for (field, kind) in &errors.0 {
        if field == COLLECTION_ROOT_KEY {
            // Mixed root (should not happen in practice); emit under the key.
            map.insert((*field).to_string(), flatten_kind(kind));
            continue;
        }
        map.insert((*field).to_string(), flatten_kind(kind));
    }
    Value::Object(map)
}

/// Walk a single `ValidationErrorsKind` into its JSON representation.
fn flatten_kind(kind: &ValidationErrorsKind) -> Value {
    match kind {
        ValidationErrorsKind::Field(errors) => {
            Value::Array(errors.iter().map(error_object).collect())
        }
        ValidationErrorsKind::Struct(inner) => flatten_errors(inner),
        ValidationErrorsKind::List(indexed) => {
            let mut map = Map::new();
            for (index, inner) in indexed {
                map.insert(index.to_string(), flatten_errors(inner));
            }
            Value::Object(map)
        }
    }
}

/// Build a client-safe JSON object for a single `ValidationError`.
///
/// Emits only `code` and an optional `message`; never `params` (which may
/// carry the raw request value).
fn error_object(error: &ValidationError) -> Value {
    let mut map = Map::new();
    map.insert("code".to_string(), Value::String(error.code.to_string()));
    if let Some(message) = &error.message {
        map.insert("message".to_string(), Value::String(message.to_string()));
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    fn field_error(code: &'static str) -> ValidationError {
        ValidationError::new(code)
    }

    fn field_error_with_message(code: &'static str, message: &'static str) -> ValidationError {
        ValidationError::new(code).with_message(Cow::from(message))
    }

    #[test]
    fn flat_field_errors_serialize_to_array() {
        let mut errors = ValidationErrors::new();
        errors.add("name", field_error("required"));
        errors.add("name", field_error("length"));
        errors.add("email", field_error("email"));

        let value = flatten_errors(&errors);
        assert_eq!(value["name"][0]["code"], "required");
        assert_eq!(value["name"][1]["code"], "length");
        assert_eq!(value["email"][0]["code"], "email");
        assert!(value["name"][0].get("message").is_none());
    }

    #[test]
    fn message_is_emitted_when_present() {
        let mut errors = ValidationErrors::new();
        errors.add("name", field_error_with_message("custom", "name taken"));

        let value = flatten_errors(&errors);
        assert_eq!(value["name"][0]["code"], "custom");
        assert_eq!(value["name"][0]["message"], "name taken");
    }

    #[test]
    fn nested_struct_errors_recurse() {
        let mut inner = ValidationErrors::new();
        inner.add("street", field_error("required"));
        let mut outer = ValidationErrors::new();
        outer.0.insert(
            Cow::Borrowed("address"),
            ValidationErrorsKind::Struct(Box::new(inner)),
        );

        let value = flatten_errors(&outer);
        assert_eq!(value["address"]["street"][0]["code"], "required");
    }

    #[test]
    fn list_errors_are_keyed_by_index() {
        let mut item0 = ValidationErrors::new();
        item0.add("id", field_error("range"));
        let mut indexed: BTreeMap<usize, Box<ValidationErrors>> = BTreeMap::new();
        indexed.insert(0, Box::new(item0));
        let mut list_root = ValidationErrors::new();
        list_root.0.insert(
            Cow::Borrowed(COLLECTION_ROOT_KEY),
            ValidationErrorsKind::List(indexed),
        );

        let value = flatten_errors(&list_root);
        // Collection root is unwrapped: top level is the index-keyed object.
        assert_eq!(value["0"]["id"][0]["code"], "range");
        assert!(value.get(COLLECTION_ROOT_KEY).is_none());
    }

    #[test]
    fn validation_problem_has_errors_extension_and_422() {
        let mut errors = ValidationErrors::new();
        errors.add("name", field_error("required"));

        let problem = validation_problem(errors);
        let json = problem.to_json();
        assert_eq!(json["status"], 422);
        assert_eq!(json["type"], "urn:arcature:problem:validation");
        assert_eq!(json["errors"]["name"][0]["code"], "required");
    }

    #[test]
    fn error_object_omits_params() {
        let mut error = field_error("range");
        error.add_param(Cow::from("min"), &0);
        error.add_param(Cow::from("value"), &9999);

        let value = flatten_errors(&{
            let mut e = ValidationErrors::new();
            e.add("n", error);
            e
        });
        assert!(value["n"][0].get("params").is_none());
        assert_eq!(value["n"][0]["code"], "range");
    }

    #[test]
    fn validate_or_problem_passes_on_valid_and_errors_on_invalid() {
        use validator::Validate;
        #[derive(Validate)]
        struct Ok {
            #[validate(length(min = 1))]
            name: String,
        }
        #[derive(Validate)]
        struct Bad {
            #[validate(length(min = 1))]
            name: String,
        }
        assert!(validate_or_problem(&Ok { name: "x".into() }).is_ok());
        let err = validate_or_problem(&Bad {
            name: String::new(),
        })
        .unwrap_err();
        assert_eq!(err.status().as_u16(), 422);
    }
}
