//! Tests for the arcature-macros proc-macro crate.

#![cfg(feature = "macros")]

use arcature::jobs::JobRequest;
use serde::{Deserialize, Serialize};

// --- #[derive(Event)] ---

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Event)]
struct UserRegistered {
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
