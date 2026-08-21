//! `RouteTable` -- the named-route map, detached from the router.
//!
//! [`Routes`](super::Routes) is generic over the application state and owns an
//! `axum::Router`, so it cannot be held by a layer that runs for every request
//! or handed to a subsystem that has no business knowing the state type. The
//! name-to-template map is the only part of it URL generation needs, and that
//! part is plain data.
//!
//! [`RouteTable`] is that part: state-free, cheap to clone (one `Arc`), and
//! ordered. It is what the redirect response mapper resolves
//! `redirect().route("users.show", id)` against, and what `arc routes` prints.
//!
//! # Why ordered
//!
//! `Routes` stores its names in a `HashMap`, so
//! [`Routes::named`](super::Routes::named) yields a different order on every
//! run. Anything that renders the table -- the `arc routes` output, the UAG
//! artifact, a generated `routes.ts` -- has to be byte-identical across runs
//! or it is not diffable in CI. Sorting here means no caller has to remember
//! to sort.

use std::collections::BTreeMap;
use std::collections::btree_map;
use std::sync::Arc;

use crate::error::{Error, Result};

/// A state-free, ordered snapshot of an application's named routes.
///
/// Built with [`Routes::table`](super::Routes::table). Cloning shares the map
/// rather than copying it, so installing the same table on a layer, in an
/// extension, and in a CLI command costs three pointer bumps.
///
/// ```
/// use arcature::routing::{Route, Routes};
///
/// let routes: Routes = Routes::new([
///     Route::get("/users/{id}", || async { "ok" }).name("users.show"),
/// ]);
/// let table = routes.table();
///
/// assert_eq!(table.url_for("users.show", &["7"]).unwrap(), "/users/7");
/// assert!(table.url_for("users.edit", &[]).is_err());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteTable {
    names: Arc<BTreeMap<String, String>>,
}

impl RouteTable {
    /// An empty table -- what an application with no named routes has.
    #[must_use]
    pub fn empty() -> Self {
        RouteTable::default()
    }

    /// Render a named route's path, filling `{param}` segments from `params`
    /// in declaration order.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if no route carries `name`, and
    /// [`Error::BadRequest`] if the template has more parameters than were
    /// supplied. Both are caller errors, not request errors -- a redirect to
    /// a route that does not exist is a bug in the application, and the
    /// mapper turns it into a `500` rather than telling the browser it sent a
    /// bad request.
    pub fn url_for(&self, name: &str, params: &[&str]) -> Result<String> {
        let template = self
            .names
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("route `{name}` is not defined")))?;
        super::render_path(template, params)
    }

    /// The raw path template registered under `name`, `{param}` segments and
    /// all.
    #[must_use]
    pub fn template(&self, name: &str) -> Option<&str> {
        self.names.get(name).map(String::as_str)
    }

    /// Whether a route is registered under `name`.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    /// Iterate `(name, template)` pairs in name order.
    pub fn iter(&self) -> btree_map::Iter<'_, String, String> {
        self.names.iter()
    }

    /// The number of named routes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the application has no named routes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl<'a> IntoIterator for &'a RouteTable {
    type Item = (&'a String, &'a String);
    type IntoIter = btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

// A later name wins, matching `Routes`: `.name(..)` called twice on the same
// route overwrites, and building a table from an iterator should not disagree
// with the collection it came from.
impl<N, T> FromIterator<(N, T)> for RouteTable
where
    N: Into<String>,
    T: Into<String>,
{
    fn from_iter<I: IntoIterator<Item = (N, T)>>(iter: I) -> Self {
        RouteTable {
            names: Arc::new(
                iter.into_iter()
                    .map(|(n, t)| (n.into(), t.into()))
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RouteTable;

    fn table() -> RouteTable {
        [
            ("users.show", "/users/{id}"),
            ("users.index", "/users"),
            ("posts.comment", "/posts/{post}/comments/{comment}"),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn a_name_renders_its_template_with_the_parameters_filled() {
        assert_eq!(table().url_for("users.show", &["7"]).unwrap(), "/users/7");
        assert_eq!(
            table().url_for("posts.comment", &["3", "18"]).unwrap(),
            "/posts/3/comments/18"
        );
    }

    #[test]
    fn a_route_with_no_parameters_needs_none() {
        assert_eq!(table().url_for("users.index", &[]).unwrap(), "/users");
    }

    #[test]
    fn an_unknown_name_is_an_error_rather_than_an_empty_path() {
        let error = table().url_for("users.edit", &[]).unwrap_err().to_string();
        assert!(error.contains("users.edit"), "{error}");
    }

    #[test]
    fn too_few_parameters_is_an_error_rather_than_a_literal_brace_in_the_url() {
        let error = table().url_for("users.show", &[]).unwrap_err().to_string();
        assert!(error.contains("id"), "{error}");
    }

    #[test]
    fn iteration_is_in_name_order_so_rendered_output_is_diffable() {
        let table = table();
        let names: Vec<&str> = table.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["posts.comment", "users.index", "users.show"]);
    }

    #[test]
    fn a_clone_shares_the_map_rather_than_copying_it() {
        let original = table();
        let clone = original.clone();
        assert!(std::sync::Arc::ptr_eq(&original.names, &clone.names));
        assert_eq!(original, clone);
    }

    #[test]
    fn an_empty_table_reports_itself_empty_and_resolves_nothing() {
        let empty = RouteTable::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert!(!empty.contains("home"));
        assert!(empty.url_for("home", &[]).is_err());
    }

    #[test]
    fn a_repeated_name_keeps_the_last_template() {
        let table: RouteTable = [("home", "/old"), ("home", "/")].into_iter().collect();
        assert_eq!(table.template("home"), Some("/"));
    }
}
