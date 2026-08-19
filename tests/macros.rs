//! Tests for the arcature-macros proc-macro crate.

#![cfg(feature = "macros")]

use arcature::jobs::{JobModel, JobRequest};
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

    pub async fn show() -> String {
        "show".to_string()
    }
}

#[test]
fn controller_macro_emits_impl_unchanged() {
    // If the macro worked, the methods are still callable.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(HomeController::index());
    assert_eq!(result, "hello");
}
