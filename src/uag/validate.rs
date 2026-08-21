//! Cross-stack validation over the assembled artifact.
//!
//! These are the mistakes that compile on both sides and only fail in the
//! browser: a route renamed in Rust while the page component kept the old
//! file name, a `page:` pointing at a contract nobody registered, the same
//! path and method declared twice in two modules, a route guarded by a
//! policy no module declares.
//!
//! Nothing here prints. The functions return typed diagnostics and the CLI
//! decides whether that becomes a table, JSON, or a non-zero exit code --
//! validation that formats its own output cannot be reused by a caller that
//! wants a different format.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::schema::UagArtifact;

/// The frontend component extensions a page identity may resolve to, in the
/// order they are tried.
pub const PAGE_EXTENSIONS: &[&str] = &["tsx", "vue", "svelte"];

/// One cross-stack problem found in the artifact.
///
/// Every variant names the thing at fault in the application's own
/// vocabulary -- a route name, a page identity, a module name -- and never
/// a machine-specific path, so a diagnostic reads the same on CI as it does
/// locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UagDiagnostic {
    /// Two or more routes claim the same method and path. The router picks
    /// one and the others are dead.
    DuplicateRoute {
        /// The uppercase HTTP method.
        method: String,
        /// The path pattern.
        path: String,
        /// The competing handlers, in artifact order.
        handlers: Vec<String>,
    },
    /// A route declares a page that no contract registers, so the props it
    /// renders with are unchecked.
    UnregisteredPage {
        /// The route name, or `"METHOD path"` when the route is unnamed.
        route: String,
        /// The page identity the route declared.
        page: String,
    },
    /// A registered page has no component file on disk under any known
    /// extension. Inertia resolves components by name at runtime, so this
    /// is a blank screen rather than a build error.
    MissingPageComponent {
        /// The page identity.
        page: String,
        /// The relative file names that were looked for, in order.
        searched: Vec<String>,
    },
    /// A route names a policy that no module declares. The route reads as
    /// guarded and is not: `Auth::authorize::<_, ThatPolicy>` would not
    /// compile, so nothing is enforcing it.
    UndeclaredPolicy {
        /// The route name, or `"METHOD path"` when the route is unnamed.
        route: String,
        /// The policy type name the route declared.
        policy: String,
    },
    /// A module exports a name it does not declare as a controller, a
    /// service, or a policy, so importers reference something that is not
    /// there.
    UndeclaredExport {
        /// The exporting module.
        module: String,
        /// The exported name.
        export: String,
    },
}

impl std::fmt::Display for UagDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateRoute {
                method,
                path,
                handlers,
            } => write!(
                f,
                "duplicate route `{method} {path}` declared by {}",
                handlers.join(", ")
            ),
            Self::UnregisteredPage { route, page } => write!(
                f,
                "route `{route}` renders page `{page}`, which no contract registers"
            ),
            Self::MissingPageComponent { page, searched } => write!(
                f,
                "page `{page}` has no component file (looked for {})",
                searched.join(", ")
            ),
            Self::UndeclaredPolicy { route, policy } => write!(
                f,
                "route `{route}` is guarded by policy `{policy}`, which no module declares"
            ),
            Self::UndeclaredExport { module, export } => write!(
                f,
                "module `{module}` exports `{export}`, which it does not declare"
            ),
        }
    }
}

impl std::error::Error for UagDiagnostic {}

/// What validation is allowed to look at outside the artifact.
///
/// The only such thing is the page component directory. It is optional
/// because `arc typegen` running in CI may have no checkout of the frontend
/// -- and a check that cannot see the files must be skipped, not guessed.
#[derive(Debug, Clone, Default)]
pub struct ValidateOptions {
    /// The directory page identities resolve against (typically
    /// `resources/js/pages`). `None` skips the component-file check.
    pub pages_dir: Option<PathBuf>,
}

impl ValidateOptions {
    /// Validate without touching the filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve page identities against `dir`.
    #[must_use]
    pub fn with_pages_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.pages_dir = Some(dir.into());
        self
    }
}

/// Run every cross-stack check over the artifact.
///
/// # Errors
///
/// Returns every diagnostic found, in a stable order (duplicate routes,
/// then unregistered pages, then missing components, then undeclared
/// policies, then undeclared exports). Validation does not stop at the
/// first problem: a developer fixing a rename wants the whole list, not one
/// line at a time.
pub fn validate(
    artifact: &UagArtifact,
    options: &ValidateOptions,
) -> Result<(), Vec<UagDiagnostic>> {
    let mut diagnostics = Vec::new();
    duplicate_routes(artifact, &mut diagnostics);
    unregistered_pages(artifact, &mut diagnostics);
    if let Some(dir) = &options.pages_dir {
        missing_page_components(artifact, dir, &mut diagnostics);
    }
    undeclared_policies(artifact, &mut diagnostics);
    undeclared_exports(artifact, &mut diagnostics);

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

/// Groups routes by method+path; any group of two or more is a conflict.
fn duplicate_routes(artifact: &UagArtifact, out: &mut Vec<UagDiagnostic>) {
    let mut by_key: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
    for route in artifact.routes() {
        by_key
            .entry((route.method.as_str(), route.path.as_str()))
            .or_default()
            .push(route.handler.clone());
    }
    for ((method, path), handlers) in by_key {
        if handlers.len() > 1 {
            out.push(UagDiagnostic::DuplicateRoute {
                method: method.to_owned(),
                path: path.to_owned(),
                handlers,
            });
        }
    }
}

/// Every page a route claims to render must have a registered contract.
fn unregistered_pages(artifact: &UagArtifact, out: &mut Vec<UagDiagnostic>) {
    for route in artifact.routes() {
        for page in &route.pages {
            if !artifact.pages().contains_key(page) {
                out.push(UagDiagnostic::UnregisteredPage {
                    route: route_label(route),
                    page: page.clone(),
                });
            }
        }
    }
}

/// Every registered page must have a component file under one of
/// [`PAGE_EXTENSIONS`].
fn missing_page_components(artifact: &UagArtifact, dir: &Path, out: &mut Vec<UagDiagnostic>) {
    for page in artifact.pages().keys() {
        let searched: Vec<String> = PAGE_EXTENSIONS
            .iter()
            .map(|ext| format!("{page}.{ext}"))
            .collect();
        if !searched.iter().any(|name| dir.join(name).is_file()) {
            out.push(UagDiagnostic::MissingPageComponent {
                page: page.clone(),
                searched,
            });
        }
    }
}

/// Every policy a route names must be declared by some module.
///
/// The check is deliberately application-wide rather than per-module: a
/// route lives in one module and may legitimately be guarded by a policy
/// another module exports, so narrowing it to the declaring module would
/// reject a correct application.
///
/// What is *not* checked here: whether a mutating route names a policy at
/// all. That rule has real exceptions -- login, logout, register, and a
/// public webhook are all unguarded POSTs by design -- so enforcing it would
/// mean an allowlist, and an allowlist that everyone edits is a rule nobody
/// reads.
fn undeclared_policies(artifact: &UagArtifact, out: &mut Vec<UagDiagnostic>) {
    let declared: BTreeSet<&String> = artifact
        .modules()
        .values()
        .flat_map(|module| module.policies.iter())
        .collect();

    for route in artifact.routes() {
        for policy in &route.policies {
            if !declared.contains(policy) {
                out.push(UagDiagnostic::UndeclaredPolicy {
                    route: route_label(route),
                    policy: policy.clone(),
                });
            }
        }
    }
}

/// An export must name something the module declares. The module's own
/// controllers, services, and policies are the whole set of things it can
/// hand to an importer.
fn undeclared_exports(artifact: &UagArtifact, out: &mut Vec<UagDiagnostic>) {
    for (name, module) in artifact.modules() {
        for export in &module.exports {
            let declared = module.controllers.contains_key(export)
                || module.services.contains(export)
                || module.policies.contains(export);
            if !declared {
                out.push(UagDiagnostic::UndeclaredExport {
                    module: name.clone(),
                    export: export.clone(),
                });
            }
        }
    }
}

/// How a route is named in a diagnostic: its route name when it has one,
/// and `"METHOD path"` otherwise.
fn route_label(route: &super::schema::UagRoute) -> String {
    if route.name.is_empty() {
        format!("{} {}", route.method, route.path)
    } else {
        route.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::inertia::contracts::{ContractType, PageSchema, PropsSchema};
    use crate::uag::schema::{UagModule, UagRoute};

    fn route(name: &str, method: &str, path: &str, handler: &str, pages: &[&str]) -> UagRoute {
        UagRoute {
            module: "Links".to_owned(),
            method: method.to_owned(),
            path: path.to_owned(),
            name: name.to_owned(),
            handler: handler.to_owned(),
            params: Vec::new(),
            pages: pages.iter().map(|p| (*p).to_owned()).collect(),
            action: None,
            query: None,
            query_string: None,
            policies: BTreeSet::new(),
        }
    }

    fn page_map(names: &[&str]) -> BTreeMap<String, PageSchema> {
        names
            .iter()
            .map(|n| {
                (
                    (*n).to_owned(),
                    PageSchema::new(PropsSchema::new().required("id", ContractType::number())),
                )
            })
            .collect()
    }

    fn artifact(routes: Vec<UagRoute>, pages: &[&str]) -> UagArtifact {
        UagArtifact::new(BTreeMap::new(), routes, page_map(pages))
    }

    #[test]
    fn a_clean_artifact_produces_no_diagnostics() {
        let art = artifact(
            vec![route(
                "links.show",
                "GET",
                "/links/{link}",
                "C::show",
                &["Show"],
            )],
            &["Show"],
        );
        assert_eq!(validate(&art, &ValidateOptions::new()), Ok(()));
    }

    #[test]
    fn the_same_path_and_method_twice_is_reported_once_with_both_handlers() {
        let art = artifact(
            vec![
                route("a", "GET", "/links", "A::index", &[]),
                route("b", "GET", "/links", "B::index", &[]),
            ],
            &[],
        );
        let found = validate(&art, &ValidateOptions::new()).unwrap_err();
        assert_eq!(
            found,
            vec![UagDiagnostic::DuplicateRoute {
                method: "GET".to_owned(),
                path: "/links".to_owned(),
                handlers: vec!["A::index".to_owned(), "B::index".to_owned()],
            }]
        );
    }

    #[test]
    fn the_same_path_under_two_methods_is_not_a_duplicate() {
        let art = artifact(
            vec![
                route("a", "GET", "/links", "A::index", &[]),
                route("b", "POST", "/links", "A::store", &[]),
            ],
            &[],
        );
        assert_eq!(validate(&art, &ValidateOptions::new()), Ok(()));
    }

    #[test]
    fn a_page_without_a_contract_is_reported_against_its_route() {
        let art = artifact(
            vec![route("links.show", "GET", "/l", "C::show", &["Ghost"])],
            &["Show"],
        );
        let found = validate(&art, &ValidateOptions::new()).unwrap_err();
        assert_eq!(
            found,
            vec![UagDiagnostic::UnregisteredPage {
                route: "links.show".to_owned(),
                page: "Ghost".to_owned(),
            }]
        );
    }

    #[test]
    fn an_unnamed_route_is_identified_by_method_and_path() {
        let art = artifact(vec![route("", "GET", "/l", "C::show", &["Ghost"])], &[]);
        let found = validate(&art, &ValidateOptions::new()).unwrap_err();
        assert!(matches!(
            &found[0],
            UagDiagnostic::UnregisteredPage { route, .. } if route == "GET /l"
        ));
    }

    #[test]
    fn the_component_check_is_skipped_when_no_pages_directory_is_given() {
        let art = artifact(Vec::new(), &["Nowhere"]);
        assert_eq!(validate(&art, &ValidateOptions::new()), Ok(()));
    }

    #[test]
    fn a_page_with_no_component_file_on_disk_is_reported() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let art = artifact(Vec::new(), &["Missing"]);
        let found = validate(&art, &ValidateOptions::new().with_pages_dir(dir.path())).unwrap_err();
        assert_eq!(
            found,
            vec![UagDiagnostic::MissingPageComponent {
                page: "Missing".to_owned(),
                searched: vec![
                    "Missing.tsx".to_owned(),
                    "Missing.vue".to_owned(),
                    "Missing.svelte".to_owned(),
                ],
            }]
        );
    }

    #[test]
    fn any_supported_extension_satisfies_the_component_check() {
        for ext in PAGE_EXTENSIONS {
            let dir = tempfile::tempdir().expect("a temp dir");
            std::fs::write(dir.path().join(format!("Home.{ext}")), "").expect("write component");
            let art = artifact(Vec::new(), &["Home"]);
            assert_eq!(
                validate(&art, &ValidateOptions::new().with_pages_dir(dir.path())),
                Ok(()),
                "{ext} should satisfy the check"
            );
        }
    }

    #[test]
    fn a_nested_page_identity_resolves_to_a_nested_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::create_dir_all(dir.path().join("links")).expect("create nested dir");
        std::fs::write(dir.path().join("links/Show.tsx"), "").expect("write component");
        let art = artifact(Vec::new(), &["links/Show"]);
        assert_eq!(
            validate(&art, &ValidateOptions::new().with_pages_dir(dir.path())),
            Ok(())
        );
    }

    #[test]
    fn an_export_that_names_nothing_the_module_declares_is_reported() {
        let module = UagModule {
            exports: BTreeSet::from(["LinksService".to_owned(), "Ghost".to_owned()]),
            services: BTreeSet::from(["LinksService".to_owned()]),
            ..UagModule::default()
        };
        let art = UagArtifact::new(
            BTreeMap::from([("Links".to_owned(), module)]),
            Vec::new(),
            BTreeMap::new(),
        );
        let found = validate(&art, &ValidateOptions::new()).unwrap_err();
        assert_eq!(
            found,
            vec![UagDiagnostic::UndeclaredExport {
                module: "Links".to_owned(),
                export: "Ghost".to_owned(),
            }]
        );
    }

    /// A route with `policies` attached, guarded by the named policies.
    fn guarded(name: &str, method: &str, path: &str, policies: &[&str]) -> UagRoute {
        UagRoute {
            policies: policies.iter().map(|p| (*p).to_owned()).collect(),
            ..route(name, method, path, "C::act", &[])
        }
    }

    /// An artifact whose single module declares `policies`.
    fn with_policies(routes: Vec<UagRoute>, policies: &[&str]) -> UagArtifact {
        let module = UagModule {
            policies: policies.iter().map(|p| (*p).to_owned()).collect(),
            ..UagModule::default()
        };
        UagArtifact::new(
            BTreeMap::from([("Links".to_owned(), module)]),
            routes,
            BTreeMap::new(),
        )
    }

    #[test]
    fn a_route_guarded_by_a_declared_policy_is_clean() {
        let art = with_policies(
            vec![guarded(
                "links.update",
                "PUT",
                "/links/{link}",
                &["LinkPolicy"],
            )],
            &["LinkPolicy"],
        );
        assert_eq!(validate(&art, &ValidateOptions::new()), Ok(()));
    }

    #[test]
    fn a_policy_no_module_declares_is_reported_against_the_route() {
        let art = with_policies(
            vec![guarded(
                "links.update",
                "PUT",
                "/links/{link}",
                &["GhostPolicy"],
            )],
            &["LinkPolicy"],
        );
        let found = validate(&art, &ValidateOptions::new()).unwrap_err();
        assert_eq!(
            found,
            vec![UagDiagnostic::UndeclaredPolicy {
                route: "links.update".to_owned(),
                policy: "GhostPolicy".to_owned(),
            }]
        );
    }

    #[test]
    fn a_policy_declared_by_another_module_still_counts_as_declared() {
        let owner = UagModule {
            policies: BTreeSet::from(["LinkPolicy".to_owned()]),
            ..UagModule::default()
        };
        let art = UagArtifact::new(
            BTreeMap::from([
                ("Admin".to_owned(), UagModule::default()),
                ("Links".to_owned(), owner),
            ]),
            vec![guarded(
                "admin.links.update",
                "PUT",
                "/admin/links/{l}",
                &["LinkPolicy"],
            )],
            BTreeMap::new(),
        );
        assert_eq!(validate(&art, &ValidateOptions::new()), Ok(()));
    }

    #[test]
    fn an_unnamed_route_with_an_undeclared_policy_is_reported_by_method_and_path() {
        let art = with_policies(
            vec![guarded("", "DELETE", "/links/{link}", &["GhostPolicy"])],
            &[],
        );
        let found = validate(&art, &ValidateOptions::new()).unwrap_err();
        assert_eq!(
            found,
            vec![UagDiagnostic::UndeclaredPolicy {
                route: "DELETE /links/{link}".to_owned(),
                policy: "GhostPolicy".to_owned(),
            }]
        );
    }

    #[test]
    fn an_unguarded_mutation_is_not_a_diagnostic() {
        let art = with_policies(vec![guarded("auth.login", "POST", "/login", &[])], &[]);
        assert_eq!(validate(&art, &ValidateOptions::new()), Ok(()));
    }

    #[test]
    fn a_diagnostic_message_names_the_application_not_the_machine() {
        let message = UagDiagnostic::MissingPageComponent {
            page: "links/Show".to_owned(),
            searched: vec!["links/Show.tsx".to_owned()],
        }
        .to_string();
        assert_eq!(
            message,
            "page `links/Show` has no component file (looked for links/Show.tsx)"
        );
    }
}
