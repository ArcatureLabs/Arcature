//! Tests for the realtime broadcast channel and origin policy.

#![cfg(feature = "realtime")]

use arcature::realtime::{Broadcast, ChannelError, ChannelPayload, OriginPolicy, VerifiedOrigin};

#[test]
fn broadcast_new_rejects_zero_capacity() {
    assert!(Broadcast::new(0).is_none());
}

#[test]
fn broadcast_new_accepts_positive_capacity() {
    let bc = Broadcast::new(16).unwrap();
    assert_eq!(bc.capacity(), 16);
    assert_eq!(bc.subscriber_count(), 0);
}

#[test]
fn broadcast_publish_with_no_subscribers_returns_error() {
    let bc = Broadcast::new(16).unwrap();
    let result = bc.publish(ChannelPayload::from_static(b"hello"));
    // tokio broadcast returns Err when there are no subscribers.
    assert!(matches!(result, Err(ChannelError::Closed)));
}

#[test]
fn broadcast_subscribe_and_receive() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bc = Broadcast::new(16).unwrap();
    let mut sub = bc.subscribe();
    assert_eq!(bc.subscriber_count(), 1);

    bc.publish(ChannelPayload::from_static(b"hello")).unwrap();

    rt.block_on(async {
        let payload = sub.recv().await.unwrap();
        assert_eq!(payload.as_bytes(), b"hello");
    });
}

#[test]
fn broadcast_subscription_drop_decrements_count() {
    let bc = Broadcast::new(16).unwrap();
    {
        let _sub = bc.subscribe();
        assert_eq!(bc.subscriber_count(), 1);
    }
    assert_eq!(bc.subscriber_count(), 0);
}

#[test]
fn broadcast_closed_channel_returns_closed_error() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bc = Broadcast::new(16).unwrap();
    let mut sub = bc.subscribe();
    drop(bc);

    rt.block_on(async {
        let result = sub.recv().await;
        assert!(matches!(result, Err(ChannelError::Closed)));
    });
}

// --- OriginPolicy ---

#[test]
fn origin_policy_deny_all() {
    let policy = OriginPolicy::deny_all();
    let result = policy.authorize(None);
    assert_eq!(result, arcature::realtime::OriginDecision::Denied);
}

#[test]
fn origin_policy_allow_exact() {
    let policy = OriginPolicy::allow_exact(VerifiedOrigin::from_trusted("https://example.com"));
    let header = axum::http::HeaderValue::from_static("https://example.com");
    assert_eq!(
        policy.authorize(Some(&header)),
        arcature::realtime::OriginDecision::Allowed
    );

    let wrong = axum::http::HeaderValue::from_static("https://evil.com");
    assert_eq!(
        policy.authorize(Some(&wrong)),
        arcature::realtime::OriginDecision::Denied
    );
}

#[test]
fn origin_policy_allow_set() {
    let policy = OriginPolicy::allow_set(vec![
        VerifiedOrigin::from_trusted("https://a.com"),
        VerifiedOrigin::from_trusted("https://b.com"),
    ]);
    let header_a = axum::http::HeaderValue::from_static("https://a.com");
    let header_b = axum::http::HeaderValue::from_static("https://b.com");
    let header_c = axum::http::HeaderValue::from_static("https://c.com");
    assert_eq!(
        policy.authorize(Some(&header_a)),
        arcature::realtime::OriginDecision::Allowed
    );
    assert_eq!(
        policy.authorize(Some(&header_b)),
        arcature::realtime::OriginDecision::Allowed
    );
    assert_eq!(
        policy.authorize(Some(&header_c)),
        arcature::realtime::OriginDecision::Denied
    );
}

#[test]
fn verified_origin_rejects_non_ascii() {
    let header = axum::http::HeaderValue::from_bytes(b"not-ascii-\xe2").unwrap();
    assert!(VerifiedOrigin::from_header(&header).is_none());
}

#[test]
fn verified_origin_accepts_ascii() {
    let header = axum::http::HeaderValue::from_static("https://example.com");
    assert!(VerifiedOrigin::from_header(&header).is_some());
}
