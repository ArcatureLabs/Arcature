//! [`ContractType`]: the JSON-compatible browser type used by a page
//! contract.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::prop_schema::PropSchema;
use super::props_schema::PropsSchema;

/// A JSON-compatible browser type used by a page contract.
///
/// The variant set is deliberately small: it covers exactly what JSON can
/// carry, so the generated TypeScript contract is a faithful, total mapping
/// with no escape hatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContractType {
    /// A JSON boolean.
    Boolean,
    /// A JSON number.
    Number,
    /// A UTF-8 string.
    String,
    /// A list of values of one type.
    Array {
        /// The element type.
        item: Box<Self>,
    },
    /// A nested object.
    Object {
        /// The nested object's props, ordered by name.
        fields: BTreeMap<String, PropSchema>,
    },
    /// A value that may be null.
    Nullable {
        /// The type when the value is present.
        item: Box<Self>,
    },
}

impl ContractType {
    /// A boolean browser type.
    #[must_use]
    pub const fn boolean() -> Self {
        Self::Boolean
    }

    /// A number browser type.
    #[must_use]
    pub const fn number() -> Self {
        Self::Number
    }

    /// A string browser type.
    #[must_use]
    pub const fn string() -> Self {
        Self::String
    }

    /// A homogeneous list browser type.
    #[must_use]
    pub fn array(item: Self) -> Self {
        Self::Array {
            item: Box::new(item),
        }
    }

    /// A nullable browser type.
    #[must_use]
    pub fn nullable(item: Self) -> Self {
        Self::Nullable {
            item: Box::new(item),
        }
    }

    /// A nested object browser type.
    #[must_use]
    pub fn object(fields: PropsSchema) -> Self {
        Self::Object {
            fields: fields.into_fields(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_nests_its_element_type() {
        let ty = ContractType::array(ContractType::string());
        assert_eq!(
            ty,
            ContractType::Array {
                item: Box::new(ContractType::String)
            }
        );
    }

    #[test]
    fn object_carries_the_nested_props() {
        let ty = ContractType::object(
            PropsSchema::new().required("name", ContractType::string()),
        );
        let ContractType::Object { fields } = ty else {
            panic!("expected an object type");
        };
        assert!(fields.contains_key("name"));
    }

    #[test]
    fn serializes_with_a_kind_tag() {
        let json = serde_json::to_string(&ContractType::number()).unwrap();
        assert_eq!(json, r#"{"kind":"number"}"#);
    }
}
