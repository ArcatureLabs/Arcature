//! [`ClientData`] and [`PageProps`]: the browser-exposure traits that are
//! the Client Exposure Firewall.

use super::props_schema::PropsSchema;

/// A type explicitly certified as safe to send to the browser.
///
/// `Serialize` alone does **not** make a type browser-safe. Only types that
/// implement `ClientData` may cross the browser boundary through
/// [`Inertia::render_page`](crate::inertia::Inertia::render_page). This is a
/// manual, opt-in declaration: an internal domain model that merely derives
/// `Serialize` cannot accidentally reach the browser, because `render_page`
/// requires `P: ClientData`.
///
/// The `#[page]` and `#[resource]` macros implement this trait from a
/// struct's named fields. The exposure schema they build is the same
/// metadata graph the cross-stack linker, `arc exposure`, and the generated
/// TypeScript contract consume -- one source of truth. Nested exposed values
/// must themselves satisfy this contract; use
/// [`PropsSchema::nested`](super::PropsSchema::nested) (and `nested_array`,
/// `nested_optional`) so the `T: ClientData` bound is checked at the call
/// site.
///
/// # Security note
///
/// Field-name linting for secret-bearing names (`password`, `token`, ...) is
/// a defence-in-depth diagnostic performed by `arc exposure`; it is **not**
/// the security boundary. The boundary is the explicit `impl ClientData for
/// T` opt-in plus the `render_page` type bound.
pub trait ClientData: serde::Serialize {
    /// The deterministic, side-effect-free browser exposure schema for this
    /// type.
    fn exposure_schema() -> PropsSchema;
}

/// Typed metadata for props intentionally associated with an Inertia page.
///
/// Metadata stays explicit: a runtime serialized value never infers it.
pub trait PageProps {
    /// The deterministic browser prop schema for this Rust type.
    fn props_schema() -> PropsSchema;
}

/// Every [`ClientData`] type satisfies the [`PageProps`] metadata seam, so
/// the cross-stack linker and `arc exposure` consume one metadata graph
/// rather than two competing systems.
impl<T: ClientData> PageProps for T {
    fn props_schema() -> PropsSchema {
        <T as ClientData>::exposure_schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::ContractType;

    #[derive(serde::Serialize)]
    struct Home;

    impl ClientData for Home {
        fn exposure_schema() -> PropsSchema {
            PropsSchema::new().required("name", ContractType::string())
        }
    }

    #[test]
    fn client_data_satisfies_page_props() {
        fn takes_page_props<P: PageProps>() -> PropsSchema {
            P::props_schema()
        }
        assert_eq!(takes_page_props::<Home>(), Home::exposure_schema());
    }
}
