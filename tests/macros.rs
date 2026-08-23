//! Tests for the arcature-macros proc-macro crate.
//!
//! Gated on `dx` as well as `macros`, because that is what the code these
//! macros emit is written against: `#[controller]`, `module!`, `#[provider]`,
//! `#[listener]` and `#[command]` all expand into implementations of
//! contracts that live behind `dx`. With `macros` alone the macros still
//! expand, and every one of those expansions names a trait that is not
//! compiled.
//!
//! That combination is reachable -- `test-kit` enables `macros` and not `dx`,
//! so `cargo test --no-default-features --features test-kit` produced fifty
//! three compile errors from this file. Nothing caught it because no job
//! compiles test targets in that shape: `features` passes `--no-dev-deps`,
//! `powerset` builds without `--all-targets`, and `drivers` does use
//! `--all-targets` but with a feature list holding both. A test target that
//! will not build is worse than one that fails, because the failure is
//! attributed to whatever the reader was doing at the time.

#![cfg(all(feature = "macros", feature = "dx"))]

use arcature::jobs::JobRequest;
use serde::{Deserialize, Serialize};

// --- #[derive(Event)] ---

// `pub`, not private: `#[listener]` expands to a `pub` handler taking this
// type, and a private parameter on a public fn is a leak clippy rejects.
#[derive(Debug, Clone, Serialize, Deserialize, arcature::Event)]
pub struct UserRegistered {
    user_id: u64,
    email: String,
}

#[test]
fn derive_event_generates_dxcomponent_name() {
    use arcature::DxComponent;
    assert_eq!(UserRegistered::NAME, "UserRegistered");
}

// --- #[derive(Job)] ---

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Job)]
struct SendVerificationEmail {
    user_id: u64,
}

#[test]
fn derive_job_generates_dxcomponent_name() {
    use arcature::DxComponent;
    assert_eq!(SendVerificationEmail::NAME, "SendVerificationEmail");
}

#[test]
fn derive_job_generates_job_const_with_defaults() {
    // Default: kind = snake_case name, version = 1, attempts = 3.
    assert_eq!(SendVerificationEmail::JOB.kind(), "send_verification_email");
    assert_eq!(SendVerificationEmail::JOB.version(), 1);
    assert_eq!(SendVerificationEmail::JOB.max_attempts(), 3);
}

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Job)]
#[job(kind = "custom_kind", version = 2, attempts = 5)]
struct CleanupSessions {
    user_id: u64,
}

#[test]
fn derive_job_respects_helper_attributes() {
    assert_eq!(CleanupSessions::JOB.kind(), "custom_kind");
    assert_eq!(CleanupSessions::JOB.version(), 2);
    assert_eq!(CleanupSessions::JOB.max_attempts(), 5);
}

#[test]
fn derive_job_job_const_works_with_job_request() {
    let payload = SendVerificationEmail { user_id: 1 };
    let req = JobRequest::new(&SendVerificationEmail::JOB, &payload).unwrap();
    assert_eq!(req.kind(), "send_verification_email");
    assert_eq!(req.version(), 1);
    assert_eq!(req.effective_max_attempts(), 3);
}

// --- #[request] ---

#[derive(Debug, Clone, Deserialize, Serialize)]
#[arcature::request]
struct StoreLinkRequest {
    #[validate(url)]
    pub url: String,
    #[validate(length(min = 1, max = 120))]
    pub title: String,
}

#[test]
fn request_macro_prepends_validate() {
    // If the macro worked, the struct implements Validate.
    use validator::Validate;
    let req = StoreLinkRequest {
        url: "https://example.com".into(),
        title: "Example".into(),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn request_macro_validation_rejects_invalid_url() {
    use validator::Validate;
    let req = StoreLinkRequest {
        url: "not-a-url".into(),
        title: "Example".into(),
    };
    assert!(req.validate().is_err());
}

// --- #[controller] ---

pub struct HomeController;

#[arcature::controller]
impl HomeController {
    pub async fn index() -> String {
        "hello".to_string()
    }

    pub async fn show(id: u64) -> String {
        format!("show {id}")
    }
}

#[test]
fn controller_macro_emits_the_impl_unchanged() {
    // The methods remain genuine functions the caller can invoke.
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(rt.block_on(HomeController::index()), "hello");
    assert_eq!(rt.block_on(HomeController::show(7)), "show 7");
}

#[test]
fn controller_macro_emits_controller_metadata() {
    use arcature::ControllerMetadata;

    let methods = <HomeController as ControllerMetadata>::METHODS;
    assert_eq!(methods.len(), 2);
    assert_eq!(methods[0].name, "index");
    assert_eq!(methods[0].params, [] as [&str; 0]);
    assert_eq!(methods[1].name, "show");
    assert_eq!(methods[1].params, ["id"]);
}

#[test]
fn controller_macro_infers_no_page_from_a_non_page_return_type() {
    use arcature::ControllerMetadata;

    for method in <HomeController as ControllerMetadata>::METHODS {
        assert_eq!(method.page, None, "{}", method.name);
    }
}

/// A `#[page]` type: `PAGE_CONTRACT` exists only for these, which is what
/// makes the return-type page derivation a firewall rather than a guess.
#[arcature::page("Dashboard")]
pub struct DashboardPage {
    pub title: String,
}

pub struct DashboardController;

#[arcature::controller]
impl DashboardController {
    /// The golden path: the page edge is read off the return type.
    pub async fn index() -> arcature::Page<DashboardPage> {
        arcature::dx::page(DashboardPage {
            title: "Dashboard".to_string(),
        })
    }

    /// The escape hatch: a handler that renders a page without returning
    /// `Page<T>` declares the identity explicitly.
    #[page("Reports")]
    pub async fn reports() -> String {
        "reports".to_string()
    }
}

#[test]
fn controller_macro_derives_the_page_edge_from_the_return_type() {
    use arcature::ControllerMetadata;

    let methods = <DashboardController as ControllerMetadata>::METHODS;
    assert_eq!(methods[0].name, "index");
    assert_eq!(methods[0].page, Some("Dashboard"));
}

#[test]
fn controller_macro_honours_an_explicit_page_attribute() {
    use arcature::ControllerMetadata;

    let methods = <DashboardController as ControllerMetadata>::METHODS;
    assert_eq!(methods[1].name, "reports");
    assert_eq!(methods[1].page, Some("Reports"));
}

// --- module! over a real #[controller] (Blocker 2 regression guard) ---
//
// `module!` emits `<Ctrl as ControllerMetadata>::METHODS` unconditionally.
// Before `#[controller]` generated that impl, *any* module naming a
// controller failed to compile. This test exists so that never regresses:
// if it compiles, the DSL is usable.

arcature::module! {
    pub Dashboard {
        controllers: [DashboardController],
    }
}

#[test]
fn module_macro_aggregates_controller_metadata() {
    let descriptor = dashboard_module();
    assert_eq!(descriptor.name, "Dashboard");
    assert_eq!(descriptor.controllers, ["DashboardController"]);
    assert_eq!(descriptor.controller_methods.len(), 1);

    let methods = descriptor.controller_methods[0];
    assert_eq!(methods.len(), 2);
    assert_eq!(methods[0].page, Some("Dashboard"));
    assert_eq!(methods[1].page, Some("Reports"));
}

// --- #[listener(Event)] ---

#[arcature::listener(UserRegistered)]
pub async fn send_welcome_email(
    event: UserRegistered,
) -> Result<(), arcature::events::DispatchError> {
    let _ = event;
    Ok(())
}

#[test]
fn listener_macro_emits_binding() {
    // The macro generates a `LISTENER_BINDING` static next to the fn.
    assert_eq!(LISTENER_BINDING.event, "UserRegistered");
    assert_eq!(LISTENER_BINDING.listener, "send_welcome_email");
}

#[test]
fn listener_macro_emits_fn_unchanged() {
    // If the macro worked, the fn is still callable.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let event = UserRegistered {
        user_id: 1,
        email: "a@b.com".into(),
    };
    let result = rt.block_on(send_welcome_email(event));
    assert!(result.is_ok());
}

// --- #[command] ---
//
// The point of these is that the annotated function is genuinely reachable
// through `CommandRegistry::run`, which dispatches by name. A macro that
// only emitted a name would pass none of them.

/// Application state for the command and cache tests. Real applications
/// have a `Db` here; these tests need only something to resolve from.
#[derive(Debug, Clone)]
pub struct TestState {
    greeting: &'static str,
}

/// A resolvable dependency, standing in for a `#[service]`.
#[derive(Debug, Clone)]
pub struct Greeter {
    greeting: &'static str,
}

impl arcature::Resolve<TestState> for Greeter {
    fn resolve(state: &TestState) -> Self {
        Greeter {
            greeting: state.greeting,
        }
    }
}

#[derive(Debug)]
pub struct CommandFailure;

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the command declined")
    }
}

impl std::error::Error for CommandFailure {}

#[arcature::command("greet:everyone")]
pub async fn greet_everyone(greeter: Greeter) -> Result<(), CommandFailure> {
    assert_eq!(greeter.greeting, "hello");
    Ok(())
}

#[arcature::command("greet:nobody")]
pub async fn greet_nobody() -> Result<(), CommandFailure> {
    Err(CommandFailure)
}

#[test]
fn command_macro_emits_the_binding_descriptor() {
    assert_eq!(GREET_EVERYONE_COMMAND.name, "greet:everyone");
    assert_eq!(GREET_EVERYONE_COMMAND.function, "greet_everyone");
}

#[test]
fn a_command_type_runs_through_the_registry_under_its_declared_name() {
    let registry = arcature::CommandRegistry::<TestState>::new()
        .register_command::<GreetEveryoneCommand>()
        .register_command::<GreetNobodyCommand>();

    assert_eq!(registry.names(), vec!["greet:everyone", "greet:nobody"]);

    let state = TestState { greeting: "hello" };
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert!(rt.block_on(registry.run("greet:everyone", &state)).is_ok());
}

#[test]
fn a_failing_command_surfaces_its_error_through_the_registry() {
    let registry =
        arcature::CommandRegistry::<TestState>::new().register_command::<GreetNobodyCommand>();

    let state = TestState { greeting: "hello" };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(registry.run("greet:nobody", &state))
        .unwrap_err();
    assert!(err.to_string().contains("the command declined"), "{err}");
}

// --- #[provider] ---

#[derive(Debug)]
pub struct SearchInitError;

impl std::fmt::Display for SearchInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("search client could not start")
    }
}

impl std::error::Error for SearchInitError {}

#[arcature::provider(error = SearchInitError, deps = [Greeter])]
pub struct SearchClient {
    endpoint: String,
}

#[arcature::provider]
pub struct ClockProvider {
    offset: i64,
}

#[test]
fn provider_macro_generates_a_usable_provider_impl() {
    use arcature::{DxComponent, Provider};

    assert_eq!(SearchClient::NAME, "SearchClient");
    assert_eq!(<SearchClient as Provider>::DEPS, ["Greeter"]);

    // The associated error type is the declared one: this only compiles
    // because `Provider` is genuinely implemented, not merely documented.
    fn describe<P: Provider>(error: P::Error) -> String {
        error.to_string()
    }
    assert_eq!(
        describe::<SearchClient>(SearchInitError),
        "search client could not start"
    );

    let client = SearchClient {
        endpoint: "http://localhost".to_string(),
    };
    assert_eq!(client.endpoint, "http://localhost");
}

#[test]
fn a_provider_that_declares_no_failure_is_infallible() {
    use arcature::Provider;

    // `Infallible` has no values, so a match on `P::Error` needs no arms --
    // the type system carries "init cannot fail".
    fn unreachable_error(error: <ClockProvider as Provider>::Error) -> ! {
        match error {}
    }
    let _ = unreachable_error;

    assert!(<ClockProvider as Provider>::DEPS.is_empty());
    let clock = ClockProvider { offset: 0 };
    assert_eq!(clock.offset, 0);
}

// --- #[request_cache] ---

use std::sync::atomic::{AtomicUsize, Ordering};

static PROFILE_LOADS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    user_id: u64,
}

#[arcature::request_cache(name = "load_profile", key = "user_id")]
pub async fn load_profile(
    cache: arcature::dx::RequestCache,
    user_id: u64,
) -> Result<Profile, CommandFailure> {
    PROFILE_LOADS.fetch_add(1, Ordering::SeqCst);
    Ok(Profile { user_id })
}

#[test]
fn request_cache_macro_emits_the_descriptor() {
    assert_eq!(LOAD_PROFILE_REQUEST_CACHE.name, "load_profile");
    assert_eq!(LOAD_PROFILE_REQUEST_CACHE.key_fields, ["user_id"]);
}

#[test]
fn a_memoized_resolver_computes_once_per_request_and_key() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    PROFILE_LOADS.store(0, Ordering::SeqCst);

    rt.block_on(async {
        let request = arcature::dx::RequestCache::new();

        let first = load_profile(request.clone(), 7).await.unwrap();
        let second = load_profile(request.clone(), 7).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(PROFILE_LOADS.load(Ordering::SeqCst), 1);

        // A different key value is a different computation.
        load_profile(request.clone(), 8).await.unwrap();
        assert_eq!(PROFILE_LOADS.load(Ordering::SeqCst), 2);

        // A different request shares nothing.
        let other_request = arcature::dx::RequestCache::new();
        load_profile(other_request, 7).await.unwrap();
        assert_eq!(PROFILE_LOADS.load(Ordering::SeqCst), 3);
    });
}

// --- module! { pages: [...] } ---

arcature::module! {
    pub Reporting {
        controllers: [DashboardController],
        pages: [DashboardPage],
    }
}

#[test]
fn module_macro_records_page_identities_from_their_contract_entries() {
    let descriptor = reporting_module();
    assert_eq!(descriptor.pages, ["Dashboard"]);
    // The identity is the same string the controller edge carries, so the
    // UAG can join route -> controller method -> page without a lookup
    // table.
    assert_eq!(descriptor.controller_methods[0][0].page, Some("Dashboard"));
}
