//! [`PropSchema`]: one browser prop's type and requiredness.

use serde::{Deserialize, Serialize};

use super::contract_type::ContractType;

/// One browser prop's type and requiredness.
///
/// Constructed through [`PropsSchema`](super::PropsSchema)'s builder methods
/// rather than directly, so a prop's requiredness always matches the way it
/// was declared on the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropSchema {
    required: bool,
    #[serde(rename = "type")]
    ty: ContractType,
}

impl PropSchema {
    /// A prop the page component must always receive.
    pub(super) fn required(ty: ContractType) -> Self {
        Self { required: true, ty }
    }

    /// A prop the page component may receive.
    pub(super) fn optional(ty: ContractType) -> Self {
        Self {
            required: false,
            ty,
        }
    }

    /// Whether this prop must be supplied by the page component.
    #[must_use]
    pub fn is_required(&self) -> bool {
        self.required
    }

    /// The prop's browser type.
    #[must_use]
    pub fn ty(&self) -> &ContractType {
        &self.ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_and_optional_differ_only_in_requiredness() {
        let required = PropSchema::required(ContractType::string());
        let optional = PropSchema::optional(ContractType::string());
        assert!(required.is_required());
        assert!(!optional.is_required());
        assert_eq!(required.ty(), optional.ty());
    }

    #[test]
    fn serializes_the_type_under_a_type_key() {
        let json = serde_json::to_value(PropSchema::required(ContractType::boolean())).unwrap();
        assert_eq!(json["required"], true);
        assert_eq!(json["type"]["kind"], "boolean");
    }
}
