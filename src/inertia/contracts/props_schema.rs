//! [`PropsSchema`]: a browser prop object schema, built declaratively.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::client_data::ClientData;
use super::contract_type::ContractType;
use super::prop_schema::PropSchema;

/// A browser prop object schema.
///
/// Built with a chain of `required`/`optional`/`nested*` calls, normally
/// inside a `#[page]` or `#[resource]` macro expansion:
///
/// ```
/// use arcature::inertia::{ClientData, ContractType, PropsSchema};
///
/// #[derive(serde::Serialize)]
/// struct TagResource {
///     label: String,
/// }
///
/// // Nesting demands `ClientData`, not merely `Serialize`: that bound is the
/// // compile-time edge of the Client Exposure Firewall, and `#[resource]`
/// // writes this impl for you.
/// impl ClientData for TagResource {
///     fn exposure_schema() -> PropsSchema {
///         PropsSchema::new().required("label", ContractType::string())
///     }
/// }
///
/// let schema = PropsSchema::new()
///     .required("title", ContractType::string())
///     .optional("description", ContractType::string())
///     .nested_array::<TagResource>("tags");
///
/// // Ordered by name, not by declaration order.
/// let names: Vec<&str> = schema.fields().keys().map(String::as_str).collect();
/// assert_eq!(names, ["description", "tags", "title"]);
/// ```
///
/// Props are stored in a `BTreeMap`, so the schema and every artifact
/// derived from it are deterministic regardless of declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropsSchema {
    fields: BTreeMap<String, PropSchema>,
}

impl PropsSchema {
    /// Start an empty props schema.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    /// Add a required browser prop.
    #[must_use]
    pub fn required(mut self, name: impl Into<String>, ty: ContractType) -> Self {
        self.fields.insert(name.into(), PropSchema::required(ty));
        self
    }

    /// Add an optional browser prop.
    #[must_use]
    pub fn optional(mut self, name: impl Into<String>, ty: ContractType) -> Self {
        self.fields.insert(name.into(), PropSchema::optional(ty));
        self
    }

    /// Add a required nested object whose Rust type is itself [`ClientData`].
    ///
    /// The `T: ClientData` bound is checked at the call site, so a nested
    /// value that is only `Serialize` (an internal domain model) cannot be
    /// placed in the browser schema -- the program does not compile. This is
    /// the compile-time edge of the Client Exposure Firewall.
    #[must_use]
    pub fn nested<T: ClientData>(mut self, name: impl Into<String>) -> Self {
        self.fields.insert(
            name.into(),
            PropSchema::required(ContractType::object(T::exposure_schema())),
        );
        self
    }

    /// Add an optional nested [`ClientData`] object (serialized as
    /// `T | null`).
    #[must_use]
    pub fn nested_optional<T: ClientData>(mut self, name: impl Into<String>) -> Self {
        self.fields.insert(
            name.into(),
            PropSchema::optional(ContractType::nullable(ContractType::object(
                T::exposure_schema(),
            ))),
        );
        self
    }

    /// Add a required array of nested [`ClientData`] objects.
    #[must_use]
    pub fn nested_array<T: ClientData>(mut self, name: impl Into<String>) -> Self {
        self.fields.insert(
            name.into(),
            PropSchema::required(ContractType::array(ContractType::object(
                T::exposure_schema(),
            ))),
        );
        self
    }

    /// The props, ordered by name.
    #[must_use]
    pub fn fields(&self) -> &BTreeMap<String, PropSchema> {
        &self.fields
    }

    /// Consume the schema and return its props. Used by
    /// [`ContractType::object`] to nest one schema inside another.
    pub(super) fn into_fields(self) -> BTreeMap<String, PropSchema> {
        self.fields
    }
}

impl Default for PropsSchema {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Tag;

    impl ClientData for Tag {
        fn exposure_schema() -> PropsSchema {
            PropsSchema::new().required("label", ContractType::string())
        }
    }

    #[test]
    fn required_and_optional_props_are_recorded() {
        let schema = PropsSchema::new()
            .required("title", ContractType::string())
            .optional("description", ContractType::string());
        assert!(schema.fields()["title"].is_required());
        assert!(!schema.fields()["description"].is_required());
    }

    #[test]
    fn field_order_is_deterministic_regardless_of_declaration_order() {
        let one = PropsSchema::new()
            .required("b", ContractType::number())
            .required("a", ContractType::number());
        let two = PropsSchema::new()
            .required("a", ContractType::number())
            .required("b", ContractType::number());
        assert_eq!(one, two);
        assert_eq!(one.fields().keys().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn nested_embeds_the_client_data_schema() {
        let schema = PropsSchema::new().nested::<Tag>("tag");
        let ContractType::Object { fields } = schema.fields()["tag"].ty() else {
            panic!("expected a nested object");
        };
        assert!(fields.contains_key("label"));
    }

    #[test]
    fn nested_array_wraps_the_nested_object_in_an_array() {
        let schema = PropsSchema::new().nested_array::<Tag>("tags");
        assert!(matches!(
            schema.fields()["tags"].ty(),
            ContractType::Array { .. }
        ));
    }

    #[test]
    fn nested_optional_is_optional_and_nullable() {
        let schema = PropsSchema::new().nested_optional::<Tag>("tag");
        assert!(!schema.fields()["tag"].is_required());
        assert!(matches!(
            schema.fields()["tag"].ty(),
            ContractType::Nullable { .. }
        ));
    }
}
