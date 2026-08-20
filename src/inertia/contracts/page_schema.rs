//! [`PageSchema`]: one registered page's browser prop schema.

use serde::{Deserialize, Serialize};

use super::props_schema::PropsSchema;

/// Schema for one registered page.
///
/// Produced by [`PageContracts`](super::PageContracts) when a page contract
/// is registered, and carried into the [`ContractArtifact`](super::ContractArtifact).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSchema {
    props: PropsSchema,
}

impl PageSchema {
    /// Build a page schema from the page prop type's exposure schema.
    #[must_use]
    pub fn new(props: PropsSchema) -> Self {
        Self { props }
    }

    /// The page's browser prop schema.
    #[must_use]
    pub fn props(&self) -> &PropsSchema {
        &self.props
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::ContractType;

    #[test]
    fn carries_the_props_it_was_built_from() {
        let props = PropsSchema::new().required("name", ContractType::string());
        let page = PageSchema::new(props.clone());
        assert_eq!(page.props(), &props);
    }
}
