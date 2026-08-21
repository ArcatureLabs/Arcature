//! [`PageContract`] and [`PageContractEntry`]: a stable page identity bound
//! to its Rust prop type.

use std::marker::PhantomData;

use super::props_schema::PropsSchema;

/// A stable page identity statically associated with its Rust prop type.
///
/// `PageContract<P>` carries no data at runtime -- the `PhantomData` binds
/// the frontend page name to the Rust type whose
/// [`ClientData`](super::ClientData) schema describes its props.
#[derive(Debug, Clone, Copy)]
pub struct PageContract<P> {
    name: &'static str,
    marker: PhantomData<fn() -> P>,
}

impl<P> PageContract<P> {
    /// Declare a stable frontend page identity for `P`.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            marker: PhantomData,
        }
    }

    /// The exact frontend page identity.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }
}

/// The trait form of the `#[page]`-generated `PAGE_CONTRACT` const.
///
/// `PAGE_CONTRACT` is an *inherent* associated const, which is enough for a
/// macro that names the type literally (`<HomePage>::PAGE_CONTRACT.name()`)
/// but not for generic code: `Page<T>` cannot reach an inherent const of an
/// unknown `T`. This trait is that same const, made reachable through a
/// bound.
///
/// The firewall is unchanged and in fact tightened: `PageType` requires
/// [`ClientData`](super::ClientData), so a `Serialize`-only type still
/// cannot be rendered -- and now cannot even be *named* as the payload of a
/// [`Page`](crate::dx::Page).
///
/// The `#[page]` macro emits the impl. Implementing it by hand is allowed
/// and does not weaken anything: `ClientData` is the boundary, and it must
/// be satisfied first.
pub trait PageType: super::ClientData + Sized {
    /// This type's page identity -- the same value as the macro-generated
    /// `PAGE_CONTRACT` inherent const.
    const CONTRACT: PageContract<Self>;
}

/// A non-generic, const-constructible page-contract descriptor.
///
/// Carries the stable page identity (`name`) and a function pointer to the
/// page prop type's `exposure_schema()`. This is the `&'static` aggregation
/// unit the `module!` macro collects into a `&'static [PageContractEntry]`
/// slice, so `application!` can build a
/// [`PageContracts`](super::PageContracts) registry from the graph with no
/// hand-written `.register(T::PAGE_CONTRACT)?` chain.
///
/// The `ClientData` firewall is **not** weakened: the `schema` function
/// pointer is produced by the `#[page]` macro, which emits the `ClientData`
/// impl at the page-definition site. A `Serialize`-only type never gets a
/// `PAGE_CONTRACT_ENTRY` const, so it can never enter the aggregation slice.
///
/// `Copy` and const-constructible so it can live in a `const` slice: the
/// function pointer is a zero-cost address, and the `PropsSchema` is built
/// only when `register_entry` calls `(entry.schema)()` at runtime.
#[derive(Debug, Clone, Copy)]
pub struct PageContractEntry {
    /// The stable frontend page identity (e.g. `"Posts/Show"`, `"NewLink"`).
    pub name: &'static str,
    /// The page's browser-exposure schema, obtained by calling this
    /// function. Produced by the `#[page]` macro from the type's
    /// `ClientData::exposure_schema`.
    pub schema: fn() -> PropsSchema,
}

impl PageContractEntry {
    /// Build a page-contract entry from a name and a schema function
    /// pointer.
    ///
    /// Normally emitted as an associated `PAGE_CONTRACT_ENTRY` const by the
    /// `#[page]` macro, not constructed by hand.
    #[must_use]
    pub const fn new(name: &'static str, schema: fn() -> PropsSchema) -> Self {
        Self { name, schema }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::{ClientData, ContractType};

    #[derive(serde::Serialize)]
    struct Home;

    impl ClientData for Home {
        fn exposure_schema() -> PropsSchema {
            PropsSchema::new().required("name", ContractType::string())
        }
    }

    const HOME: PageContract<Home> = PageContract::new("Home");
    const HOME_ENTRY: PageContractEntry = PageContractEntry::new("Home", Home::exposure_schema);

    #[test]
    fn contract_carries_the_page_identity() {
        assert_eq!(HOME.name(), "Home");
    }

    #[test]
    fn entry_schema_pointer_builds_the_exposure_schema() {
        assert_eq!(HOME_ENTRY.name, "Home");
        assert_eq!((HOME_ENTRY.schema)(), Home::exposure_schema());
    }
}
