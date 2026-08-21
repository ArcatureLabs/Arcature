//! [`ContractArtifact`]: the machine-readable page-contract registry that
//! `arc typegen` and `arc build` consume.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::page_schema::PageSchema;

/// A machine-readable representation of an Inertia page-contract registry.
///
/// Produced by [`PageContracts::artifact`](super::PageContracts::artifact).
/// Pages are stored in a `BTreeMap`, so the JSON is byte-for-byte
/// deterministic across runs -- two builds of an unchanged application
/// produce identical bytes, so a diff means the contracts really changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractArtifact {
    format: String,
    pages: BTreeMap<String, PageSchema>,
}

impl ContractArtifact {
    /// The stable artifact format identifier.
    pub const FORMAT: &'static str = "arcature.page-contract.v1";

    /// Build an artifact from the registry's pages.
    #[must_use]
    pub fn new(pages: BTreeMap<String, PageSchema>) -> Self {
        Self {
            format: Self::FORMAT.to_owned(),
            pages,
        }
    }

    /// The artifact format identifier this artifact was written with.
    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }

    /// The registered pages, ordered by stable page identity.
    #[must_use]
    pub fn pages(&self) -> &BTreeMap<String, PageSchema> {
        &self.pages
    }

    /// Serialize this artifact deterministically as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns the `serde_json` error if serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::{ContractType, PropsSchema};

    fn artifact() -> ContractArtifact {
        let mut pages = BTreeMap::new();
        pages.insert(
            "Home".to_owned(),
            PageSchema::new(PropsSchema::new().required("name", ContractType::string())),
        );
        ContractArtifact::new(pages)
    }

    #[test]
    fn carries_the_stable_format_identifier() {
        assert_eq!(artifact().format(), ContractArtifact::FORMAT);
    }

    #[test]
    fn json_is_deterministic() {
        assert_eq!(artifact().to_json().unwrap(), artifact().to_json().unwrap());
    }

    #[test]
    fn round_trips_through_json() {
        let json = artifact().to_json().unwrap();
        let parsed: ContractArtifact = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed, artifact());
    }
}
