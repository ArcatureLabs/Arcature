//! The canonical RESTful resource actions.
//!
//! One responsibility: the seven-action vocabulary a `resource "..." =>
//! Controller` declaration expands to, the `only`/`except` filter over it,
//! and each action's (method, path suffix) mapping.

use super::method::RouteMethodKind;

/// Every canonical resource action, in route-declaration order.
pub const ALL: [&str; 7] = [
    "index", "create", "store", "show", "edit", "update", "destroy",
];

/// Returns true when `name` is one of the canonical resource actions.
pub fn is_valid(name: &str) -> bool {
    ALL.contains(&name)
}

/// Returns the actions to generate, keeping only those listed in `only`
/// (empty = all) and dropping those listed in `except`.
pub fn selected(only: &[String], except: &[String]) -> Vec<&'static str> {
    ALL.into_iter()
        .filter(|a| only.is_empty() || only.iter().any(|o| o == a))
        .filter(|a| !except.iter().any(|e| e == a))
        .collect()
}

/// Returns the HTTP method and path suffix for a resource action, where
/// `param` is the singular path-parameter name (e.g. `link` for `links`).
pub fn route(action: &str, param: &str) -> (RouteMethodKind, String) {
    match action {
        "create" => (RouteMethodKind::Get, "/new".to_string()),
        "store" => (RouteMethodKind::Post, String::new()),
        "show" => (RouteMethodKind::Get, format!("/{{{param}}}")),
        "edit" => (RouteMethodKind::Get, format!("/{{{param}}}/edit")),
        "update" => (RouteMethodKind::Put, format!("/{{{param}}}")),
        "destroy" => (RouteMethodKind::Delete, format!("/{{{param}}}")),
        // "index" and anything unreachable (validated earlier).
        _ => (RouteMethodKind::Get, String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{RouteMethodKind, is_valid, route, selected};

    #[test]
    fn all_seven_actions_are_valid() {
        for action in super::ALL {
            assert!(is_valid(action), "{action} should be valid");
        }
    }

    #[test]
    fn unknown_action_is_rejected() {
        assert!(!is_valid("list"));
    }

    #[test]
    fn empty_only_selects_everything() {
        assert_eq!(selected(&[], &[]).len(), 7);
    }

    #[test]
    fn only_restricts_to_the_listed_actions() {
        let picked = selected(&["index".to_string(), "show".to_string()], &[]);
        assert_eq!(picked, vec!["index", "show"]);
    }

    #[test]
    fn except_drops_the_listed_actions() {
        let picked = selected(&[], &["create".to_string(), "edit".to_string()]);
        assert_eq!(picked, vec!["index", "store", "show", "update", "destroy"]);
    }

    #[test]
    fn action_routes_map_to_rest_conventions() {
        assert_eq!(
            route("index", "link"),
            (RouteMethodKind::Get, String::new())
        );
        assert_eq!(
            route("show", "link"),
            (RouteMethodKind::Get, "/{link}".to_string())
        );
        assert_eq!(
            route("edit", "link"),
            (RouteMethodKind::Get, "/{link}/edit".to_string())
        );
        assert_eq!(
            route("destroy", "link"),
            (RouteMethodKind::Delete, "/{link}".to_string())
        );
    }
}
