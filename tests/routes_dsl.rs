//! Integration tests for the `routes!` DSL and the `#[middleware]` attribute.
//!
//! The macro crate's own tests inspect generated tokens; these compile the
//! generated code against the real framework and exercise it, which is the
//! only way to prove the expansion type-checks and that the router it builds
//! actually matches the declared paths.

#![cfg(all(feature = "macros", feature = "dx"))]

use arcature::routing::{Request, Response};
use arcature::{Next, Result, middleware, routes};

async fn home() -> &'static str {
    "home"
}

async fn login() -> &'static str {
    "login"
}

async fn store() -> &'static str {
    "store"
}

async fn panel() -> &'static str {
    "panel"
}

#[middleware]
pub async fn require_auth(request: Request, next: Next) -> Result<Response> {
    Ok(next.run(request).await)
}

routes! {
    pub app {
        get "/" => home { name: home, page: "Home" }

        group "/auth" {
            get  "/login" => login { name: auth.login }
            post "/login" => store { name: auth.store }
        }

        group "/admin" {
            middleware: [RequireAuth];
            get "/panel" => panel { name: admin.panel }
        }
    }
}

#[test]
fn the_router_function_builds() {
    let routes = app_routes();
    let names: Vec<_> = routes.named().map(|(name, path)| (name.clone(), path.clone())).collect();
    assert!(names.contains(&("home".to_string(), "/".to_string())));
    assert!(names.contains(&("auth.login".to_string(), "/auth/login".to_string())));
    assert!(names.contains(&("admin.panel".to_string(), "/admin/panel".to_string())));
}

#[test]
fn url_for_resolves_a_declared_name() {
    let routes = app_routes();
    assert_eq!(routes.url_for("auth.login", &[]).unwrap(), "/auth/login");
}

#[test]
fn the_metadata_const_describes_every_route() {
    assert_eq!(APP_ROUTES.len(), 4);

    let home = APP_ROUTES.iter().find(|r| r.name == "home").expect("home route");
    assert_eq!(home.method, arcature::RouteMethod::Get);
    assert_eq!(home.path, "/");
    assert_eq!(home.handler, "home");
    assert_eq!(home.pages, &["Home"]);

    let store = APP_ROUTES
        .iter()
        .find(|r| r.name == "auth.store")
        .expect("auth.store route");
    assert_eq!(store.method, arcature::RouteMethod::Post);
    assert_eq!(store.path, "/auth/login");
}

#[test]
fn the_helper_module_builds_urls() {
    assert_eq!(app_route::home(), "/");
    assert_eq!(app_route::auth::login(), "/auth/login");
    assert_eq!(app_route::admin::panel(), "/admin/panel");
}

// --- A stateful declaration -------------------------------------------------

#[derive(Clone)]
struct AppState;

async fn health() -> &'static str {
    "ok"
}

routes! {
    pub api {
        state: AppState;
        get "/health" => health { name: api.health }
    }
}

#[test]
fn a_stateful_declaration_builds_a_stateful_router() {
    let routes: arcature::Routes<AppState> = api_routes();
    assert_eq!(routes.url_for("api.health", &[]).unwrap(), "/health");
}

// --- A resource declaration -------------------------------------------------

struct LinksController;

impl LinksController {
    async fn index() -> &'static str {
        "index"
    }

    async fn show() -> &'static str {
        "show"
    }

    async fn destroy() -> &'static str {
        "destroy"
    }
}

routes! {
    pub web {
        resource "/links" => LinksController {
            name: links,
            only: [index, show, destroy]
        }
    }
}

#[test]
fn a_resource_expands_to_its_selected_actions() {
    let names: Vec<_> = WEB_ROUTES.iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["links.index", "links.show", "links.destroy"]);

    let destroy = WEB_ROUTES.iter().find(|r| r.name == "links.destroy").unwrap();
    assert_eq!(destroy.method, arcature::RouteMethod::Delete);
    assert_eq!(destroy.handler, "LinksController::destroy");
}

#[test]
fn a_resource_helper_takes_its_path_parameter() {
    assert_eq!(web_route::links::index(), "/links");
    assert_eq!(web_route::links::show(42), "/links/42");
}
