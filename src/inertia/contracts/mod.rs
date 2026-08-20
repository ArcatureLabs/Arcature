//! The Client Exposure Firewall: explicit, deterministic contracts for
//! Inertia page props.
//!
//! Contracts describe only pages and browser props. They are not a route
//! registry -- Axum remains the sole owner of route selection.
//!
//! The firewall's rule is one sentence: **`serde::Serialize` does not mean
//! browser-safe.** Only a type that explicitly implements [`ClientData`] may
//! cross the browser boundary through [`Inertia::render_page`]. An internal
//! domain model that merely derives `Serialize` cannot accidentally reach
//! the browser, because `render_page` requires `P: ClientData`.
//!
//! The same metadata graph drives the cross-stack linker, `arc exposure`,
//! and the generated TypeScript contract -- there is one source of truth,
//! not two.
//!
//! Each responsibility lives in its own file:
//!
//! * [`contract_type`] -- [`ContractType`], the JSON-compatible browser type.
//! * [`prop_schema`] -- [`PropSchema`], one prop's type and requiredness.
//! * [`props_schema`] -- [`PropsSchema`], the browser prop object schema.
//! * [`client_data`] -- [`ClientData`] and [`PageProps`], the exposure traits.
//! * [`page_contract`] -- [`PageContract`] and [`PageContractEntry`], page
//!   identity bound to a prop type.
//! * [`page_schema`] -- [`PageSchema`], one registered page's schema.
//! * [`registry`] -- [`PageContracts`], the explicit registry.
//! * [`artifact`] -- [`ContractArtifact`], the machine-readable output.
//! * [`error`] -- [`ContractError`], registration failures.
//!
//! [`Inertia::render_page`]: crate::inertia::Inertia::render_page

pub mod artifact;
pub mod client_data;
pub mod contract_type;
pub mod error;
pub mod page_contract;
pub mod page_schema;
pub mod prop_schema;
pub mod props_schema;
pub mod registry;

pub use artifact::ContractArtifact;
pub use client_data::{ClientData, PageProps};
pub use contract_type::ContractType;
pub use error::ContractError;
pub use page_contract::{PageContract, PageContractEntry};
pub use page_schema::PageSchema;
pub use prop_schema::PropSchema;
pub use props_schema::PropsSchema;
pub use registry::PageContracts;
