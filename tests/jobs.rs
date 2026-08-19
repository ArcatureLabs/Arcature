//! Tests for the jobs subsystem (model, retry policy, worker config, registry).

use std::time::Duration;

use arcature::jobs::{
    JobModel, JobRequest, JobStatus, RegisterError, RetryPolicy, RetryPolicyError,
    WorkerConfig, WorkerConfigError,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SendWelcome {
    email: String,
}

const SEND_WELCOME: JobModel<SendWelcome> = JobModel::new("send_welcome", 1, 3);

#[test]
fn job_model_new_floors_max_attempts_to_one() {
    let model: JobModel<SendWelcome> = JobModel::new("kind", 1, 0);
    assert_eq!(model.max_attempts(), 1);
}

#[test]
fn job_model_default_payload_limit_is_64k() {
    assert_eq!(SEND_WELCOME.max_payload_bytes(), 65_536);
}

#[test]
fn job_model_with_max_payload_bytes_overrides() {
    let model = SEND_WELCOME.with_max_payload_bytes(1024);
    assert_eq!(model.max_payload_bytes(), 1024);
}

#[test]
fn job_request_new_serializes_payload() {
    let req = JobRequest::new(
        &SEND_WELCOME,
        &SendWelcome {
            email: "a@b.com".into(),
        },
    )
    .unwrap();
    assert_eq!(req.kind(), "send_welcome");
    assert_eq!(req.version(), 1);
    assert_eq!(req.effective_max_attempts(), 3);
    assert!(req.payload().is_object());
}

#[test]
fn job_request_max_attempts_override_floors_to_one() {
    let req = JobRequest::new(
        &SEND_WELCOME,
        &SendWelcome {
            email: "a@b.com".into(),
        },
    )
    .unwrap()
    .max_attempts(0);
    assert_eq!(req.effective_max_attempts(), 1);
}

#[test]
fn job_status_display() {
    assert_eq!(JobStatus::Pending.to_string(), "pending");
    assert_eq!(JobStatus::Running.to_string(), "running");
    assert_eq!(JobStatus::Succeeded.to_string(), "succeeded");
    assert_eq!(JobStatus::Dead.to_string(), "dead");
    assert_eq!(JobStatus::Cancelled.to_string(), "cancelled");
}

#[test]
fn job_status_from_db() {
    assert_eq!(JobStatus::from_db("pending"), Some(JobStatus::Pending));
    assert_eq!(JobStatus::from_db("running"), Some(JobStatus::Running));
    assert_eq!(JobStatus::from_db("unknown"), None);
}

// --- RetryPolicy ---

#[test]
fn retry_policy_default_is_exponential() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.base(), Duration::from_secs(1));
    assert_eq!(policy.cap(), Duration::from_secs(3600));
}

#[test]
fn retry_policy_fixed_delay() {
    let policy = RetryPolicy::fixed(Duration::from_secs(5));
    assert_eq!(policy.delay_for(0), Duration::from_secs(5));
    assert_eq!(policy.delay_for(1), Duration::from_secs(5));
    assert_eq!(policy.delay_for(10), Duration::from_secs(5));
}

#[test]
fn retry_policy_exponential_backoff() {
    let policy = RetryPolicy::exponential(Duration::from_secs(1), 2.0, Duration::from_secs(3600));
    assert_eq!(policy.delay_for(0), Duration::from_secs(1));
    assert_eq!(policy.delay_for(1), Duration::from_secs(1));
    assert_eq!(policy.delay_for(2), Duration::from_secs(2));
    assert_eq!(policy.delay_for(3), Duration::from_secs(4));
    assert_eq!(policy.delay_for(4), Duration::from_secs(8));
}

#[test]
fn retry_policy_caps_at_cap() {
    let policy = RetryPolicy::exponential(Duration::from_secs(1), 2.0, Duration::from_secs(10));
    assert_eq!(policy.delay_for(100), Duration::from_secs(10));
}

#[test]
fn retry_policy_validate_rejects_nan() {
    let policy = RetryPolicy::exponential(Duration::from_secs(1), f64::NAN, Duration::from_secs(10));
    assert!(matches!(
        policy.validate(),
        Err(RetryPolicyError::MultiplierNotFinite { .. })
    ));
}

#[test]
fn retry_policy_validate_rejects_negative() {
    let policy = RetryPolicy::exponential(Duration::from_secs(1), -1.0, Duration::from_secs(10));
    assert!(matches!(
        policy.validate(),
        Err(RetryPolicyError::MultiplierNegative { .. })
    ));
}

// --- WorkerConfig ---

#[test]
fn worker_config_default_heartbeat_is_lease_div_3() {
    let config = WorkerConfig::default();
    assert_eq!(config.get_lease(), Duration::from_secs(300));
    assert_eq!(config.get_heartbeat_interval(), Duration::from_secs(100));
}

#[test]
fn worker_config_validate_rejects_timeout_exceeding_lease() {
    let config = WorkerConfig::default()
        .lease(Duration::from_secs(10))
        .job_timeout(Duration::from_secs(20));
    assert!(matches!(
        config.validate(),
        Err(WorkerConfigError::JobTimeoutExceedsLease { .. })
    ));
}

#[test]
fn worker_config_validate_rejects_heartbeat_at_or_above_lease() {
    // job_timeout must be <= lease first, so set a valid job_timeout too.
    let config = WorkerConfig::default()
        .lease(Duration::from_secs(10))
        .job_timeout(Duration::from_secs(5))
        .heartbeat_interval(Duration::from_secs(10));
    assert!(matches!(
        config.validate(),
        Err(WorkerConfigError::HeartbeatIntervalNotBelowLease { .. })
    ));
}

#[test]
fn worker_config_validate_accepts_valid_config() {
    let config = WorkerConfig::default()
        .lease(Duration::from_secs(300))
        .job_timeout(Duration::from_secs(60))
        .heartbeat_interval(Duration::from_secs(30));
    assert!(config.validate().is_ok());
}

// --- Registry ---

#[test]
fn registry_add_and_get() {
    use arcature::jobs::{JobError, Registry};

    let model: JobModel<SendWelcome> = JobModel::new("send_welcome", 1, 3);
    let mut registry = Registry::new();
    assert!(registry.is_empty());

    registry
        .add(&model, |job: SendWelcome| async move {
            assert_eq!(job.email, "a@b.com");
            Ok::<(), JobError>(())
        })
        .unwrap();

    assert_eq!(registry.len(), 1);
    assert!(registry.handles("send_welcome", 1));
    assert!(!registry.handles("send_welcome", 2));
}

#[test]
fn registry_rejects_duplicate() {
    use arcature::jobs::Registry;

    let model: JobModel<SendWelcome> = JobModel::new("send_welcome", 1, 3);
    let mut registry = Registry::new();
    registry
        .add(&model, |_job: SendWelcome| async { Ok(()) })
        .unwrap();

    let result = registry.add(&model, |_job: SendWelcome| async { Ok(()) });
    assert!(matches!(result, Err(RegisterError::AlreadyRegistered { .. })));
}
