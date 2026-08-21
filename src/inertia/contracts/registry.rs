//! [`PageContracts`]: the explicit registry of typed page contracts.

use std::collections::BTreeMap;

use super::artifact::ContractArtifact;
use super::client_data::ClientData;
use super::error::ContractError;
use super::page_contract::{PageContract, PageContractEntry};
use super::page_schema::PageSchema;

/// An explicit set of typed page contracts.
///
/// Two registration paths, one behaviour: [`register`](Self::register) takes
/// a typed [`PageContract<P>`] and [`register_entry`](Self::register_entry)
/// takes a non-generic [`PageContractEntry`] from a `module!` slice. Both
/// enforce the Client Exposure Firewall and both reject duplicate and
/// case-conflicting identities.
#[derive(Debug, Default)]
pub struct PageContracts {
    pages: BTreeMap<String, PageSchema>,
}

impl PageContracts {
    /// Start an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one page contract.
    ///
    /// The `P: ClientData` bound is the Client Exposure Firewall: only types
    /// explicitly certified as browser-safe enter the contract artifact that
    /// `arc typegen`, `arc build`, and the cross-stack linker consume. A
    /// plain `Serialize` type cannot be registered here.
    ///
    /// # Errors
    ///
    /// Returns [`ContractError`] when the identity is already registered or
    /// conflicts by ASCII case with an existing one.
    pub fn register<P: ClientData>(self, page: PageContract<P>) -> Result<Self, ContractError> {
        self.insert(page.name(), P::exposure_schema)
    }

    /// Register a page contract from a non-generic [`PageContractEntry`].
    ///
    /// This is the aggregation path used by the `application!` macro's
    /// generated page-contracts function: it iterates each module's
    /// `&'static [PageContractEntry]` slice and registers every entry,
    /// replacing a hand-written registration chain.
    ///
    /// The `ClientData` bound is still enforced -- the `schema` function
    /// pointer is produced by the `#[page]` macro, which requires
    /// `ClientData` at the page-definition site. A `Serialize`-only type
    /// never gets a `PAGE_CONTRACT_ENTRY` const and can never enter the
    /// slice.
    ///
    /// # Errors
    ///
    /// Returns [`ContractError`] when the identity is already registered or
    /// conflicts by ASCII case with an existing one.
    pub fn register_entry(self, entry: &PageContractEntry) -> Result<Self, ContractError> {
        self.insert(entry.name, entry.schema)
    }

    /// Produce the artifact consumed by `arc typegen` and `arc build`.
    #[must_use]
    pub fn artifact(&self) -> ContractArtifact {
        ContractArtifact::new(self.pages.clone())
    }

    /// Insert a page identity, rejecting duplicates and case conflicts. The
    /// schema function is called only once the identity is accepted.
    fn insert(
        mut self,
        name: &str,
        schema: fn() -> super::props_schema::PropsSchema,
    ) -> Result<Self, ContractError> {
        if self.pages.contains_key(name) {
            return Err(ContractError::DuplicatePage(name.to_owned()));
        }
        if let Some(existing) = self
            .pages
            .keys()
            .find(|existing| existing.eq_ignore_ascii_case(name))
        {
            return Err(ContractError::CaseConflict {
                existing: existing.clone(),
                attempted: name.to_owned(),
            });
        }
        self.pages
            .insert(name.to_owned(), PageSchema::new(schema()));
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::{ContractType, PropsSchema};

    #[derive(serde::Serialize)]
    struct Home;

    impl ClientData for Home {
        fn exposure_schema() -> PropsSchema {
            PropsSchema::new().required("name", ContractType::string())
        }
    }

    #[derive(serde::Serialize)]
    struct Dashboard;

    impl ClientData for Dashboard {
        fn exposure_schema() -> PropsSchema {
            PropsSchema::new()
        }
    }

    const HOME: PageContract<Home> = PageContract::new("Home");
    const HOME_LOWER: PageContract<Dashboard> = PageContract::new("home");
    const DASH: PageContract<Dashboard> = PageContract::new("Dashboard");
    const HOME_ENTRY: PageContractEntry = PageContractEntry::new("Home", Home::exposure_schema);
    const DASH_ENTRY: PageContractEntry =
        PageContractEntry::new("Dashboard", Dashboard::exposure_schema);

    #[test]
    fn artifact_is_deterministic() {
        let registry = PageContracts::new().register(HOME).unwrap();
        assert_eq!(
            registry.artifact().to_json().unwrap(),
            registry.artifact().to_json().unwrap()
        );
    }

    #[test]
    fn duplicate_identities_are_rejected() {
        let error = PageContracts::new()
            .register(HOME)
            .and_then(|registry| registry.register(HOME))
            .unwrap_err();
        assert!(matches!(error, ContractError::DuplicatePage(_)));
    }

    #[test]
    fn case_conflicts_are_rejected() {
        let error = PageContracts::new()
            .register(HOME)
            .and_then(|registry| registry.register(HOME_LOWER))
            .unwrap_err();
        assert!(matches!(error, ContractError::CaseConflict { .. }));
    }

    #[test]
    fn register_entry_builds_the_same_registry_as_register() {
        let typed = PageContracts::new()
            .register(HOME)
            .unwrap()
            .register(DASH)
            .unwrap();
        let entries = PageContracts::new()
            .register_entry(&HOME_ENTRY)
            .unwrap()
            .register_entry(&DASH_ENTRY)
            .unwrap();
        assert_eq!(
            typed.artifact().to_json().unwrap(),
            entries.artifact().to_json().unwrap()
        );
    }

    #[test]
    fn register_entry_rejects_duplicates() {
        let error = PageContracts::new()
            .register_entry(&HOME_ENTRY)
            .and_then(|registry| registry.register_entry(&HOME_ENTRY))
            .unwrap_err();
        assert!(matches!(error, ContractError::DuplicatePage(_)));
    }

    #[test]
    fn register_entry_rejects_case_conflicts() {
        const HOME_LOWER_ENTRY: PageContractEntry =
            PageContractEntry::new("home", Home::exposure_schema);
        let error = PageContracts::new()
            .register_entry(&HOME_ENTRY)
            .and_then(|registry| registry.register_entry(&HOME_LOWER_ENTRY))
            .unwrap_err();
        assert!(matches!(error, ContractError::CaseConflict { .. }));
    }
}
