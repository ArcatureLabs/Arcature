//! Custom keywords for the `routes!` DSL.
//!
//! One responsibility: declare the identifiers the `routes!` parser treats as
//! keywords. They live in two groups so a route option named `get` could
//! never be confused with the `get` method keyword: [`option`] holds the
//! declaration/option keywords, [`method`] holds the HTTP method keywords.

/// Declaration-level and route-option keywords.
pub mod option {
    syn::custom_keyword!(state);
    syn::custom_keyword!(group);
    syn::custom_keyword!(resource);
    syn::custom_keyword!(name);
    syn::custom_keyword!(page);
    syn::custom_keyword!(pages);
    syn::custom_keyword!(action);
    syn::custom_keyword!(query);
    syn::custom_keyword!(query_string);
    syn::custom_keyword!(policy);
    syn::custom_keyword!(policies);
    syn::custom_keyword!(only);
    syn::custom_keyword!(except);
    syn::custom_keyword!(bind);
    syn::custom_keyword!(middleware);
}

/// HTTP method keywords that open a route entry.
pub mod method {
    syn::custom_keyword!(get);
    syn::custom_keyword!(post);
    syn::custom_keyword!(put);
    syn::custom_keyword!(patch);
    syn::custom_keyword!(delete);
    syn::custom_keyword!(head);
    syn::custom_keyword!(options);
}
