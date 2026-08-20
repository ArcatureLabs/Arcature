//! URL path arithmetic for the `routes!` expansion.
//!
//! One responsibility: string-level operations on route paths and dotted
//! route names -- joining a group prefix onto a path, pulling `{param}`
//! segments out of a path, and deriving a resource's singular parameter name
//! from its dotted route name.

/// Joins a group prefix and a route path.
///
/// An empty prefix leaves the path untouched; a `"/"` path contributes
/// nothing beyond the prefix (so `group "/auth" { get "/" }` is `/auth`, not
/// `/auth/`).
pub fn join(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else if path == "/" {
        prefix.to_string()
    } else {
        format!("{prefix}{path}")
    }
}

/// Returns the last segment of a dotted route name (`api.links` -> `links`).
pub fn last_segment(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_string()
}

/// Strips one trailing `s` to form a path-parameter name (`links` -> `link`).
///
/// Deliberately naive: a resource whose parameter needs a real inflection
/// declares the route explicitly rather than relying on pluralization rules.
pub fn singularize(name: &str) -> String {
    if name.ends_with('s') && name.len() > 1 {
        name[..name.len() - 1].to_string()
    } else {
        name.to_string()
    }
}

/// Extracts the `{...}` parameter names from a path, in order.
pub fn params(path: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            continue;
        }
        let mut param = String::new();
        for c in chars.by_ref() {
            if c == '}' {
                break;
            }
            param.push(c);
        }
        if !param.is_empty() {
            found.push(param);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{join, last_segment, params, singularize};

    #[test]
    fn empty_prefix_leaves_the_path_alone() {
        assert_eq!(join("", "/login"), "/login");
    }

    #[test]
    fn root_path_under_a_prefix_is_the_prefix() {
        assert_eq!(join("/auth", "/"), "/auth");
    }

    #[test]
    fn prefix_is_prepended() {
        assert_eq!(join("/auth", "/login"), "/auth/login");
    }

    #[test]
    fn last_segment_of_a_dotted_name() {
        assert_eq!(last_segment("api.v1.links"), "links");
        assert_eq!(last_segment("links"), "links");
    }

    #[test]
    fn singularize_strips_one_trailing_s() {
        assert_eq!(singularize("links"), "link");
        assert_eq!(singularize("s"), "s");
        assert_eq!(singularize("media"), "media");
    }

    #[test]
    fn params_are_extracted_in_order() {
        assert_eq!(
            params("/teams/{team}/links/{link}/edit"),
            vec!["team".to_string(), "link".to_string()]
        );
    }

    #[test]
    fn a_path_without_params_yields_none() {
        assert!(params("/links").is_empty());
    }
}
