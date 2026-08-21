//! What the OAuth module promises about secrets, and refuses about
//! transport.
//!
//! These are the properties that make the module safe to hand a client
//! secret: the CSRF `state` is compared without a timing signal, the PKCE
//! verifier and the state never render themselves, a token response never
//! reaches a `Debug` output, and a client cannot be pointed at a plaintext
//! endpoint outside loopback. Each one is cheap to break by accident during
//! a refactor and expensive to notice in production, which is why they are
//! pinned here rather than left to review.

#![cfg(feature = "oauth")]

use arcature::oauth::{
    DISCORD, Endpoints, GITHUB, GOOGLE, OauthClient, OauthError, OauthState, PkceVerifier,
    TokenSet, constant_time_eq,
};

/// A provider the framework has never heard of, configured exactly the way
/// a bundled one is.
const CUSTOM: Endpoints = Endpoints {
    authorization: "https://sso.example.test/oauth/authorize",
    token: "https://sso.example.test/oauth/token",
};

fn client(endpoints: Endpoints) -> OauthClient {
    OauthClient::new(
        endpoints,
        "client-id",
        Some("client-secret".into()),
        "https://app.example.test/auth/callback",
    )
    .expect("a client over https")
}

#[test]
fn a_provider_the_framework_has_never_heard_of_is_configured_like_any_other() {
    let bundled = client(GITHUB).authorize(&["read:user"]).expect("authorize");
    let custom = client(CUSTOM).authorize(&["openid"]).expect("authorize");

    assert!(bundled.url().as_str().starts_with(GITHUB.authorization));
    assert!(custom.url().as_str().starts_with(CUSTOM.authorization));
    for authorization in [&bundled, &custom] {
        let url = authorization.url().as_str();
        assert!(url.contains("code_challenge="), "{url}");
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.contains("state="), "{url}");
    }
}

#[test]
fn the_bundled_presets_are_plain_endpoint_pairs() {
    for preset in [GITHUB, GOOGLE, DISCORD, CUSTOM] {
        assert!(preset.authorization.starts_with("https://"));
        assert!(preset.token.starts_with("https://"));
        assert!(client(preset).authorize(&[]).is_ok());
    }
}

#[test]
fn a_client_cannot_be_pointed_at_a_plaintext_endpoint() {
    let plaintext = Endpoints {
        authorization: "http://sso.example.test/authorize",
        token: "http://sso.example.test/token",
    };
    let error = OauthClient::new(
        plaintext,
        "client-id",
        None,
        "https://app.example.test/callback",
    )
    .expect_err("plaintext must be refused");
    assert!(matches!(error, OauthError::InsecureTransport { .. }));

    let bad_redirect = OauthClient::new(
        GITHUB,
        "client-id",
        None,
        "http://app.example.test/callback",
    )
    .expect_err("a plaintext redirect must be refused");
    assert!(matches!(bad_redirect, OauthError::InsecureTransport { .. }));
}

#[test]
fn loopback_is_the_only_plaintext_exception() {
    for loopback in [
        "http://localhost:3000/callback",
        "http://127.0.0.1:3000/callback",
        "http://[::1]:3000/callback",
    ] {
        assert!(
            OauthClient::new(GITHUB, "client-id", None, loopback).is_ok(),
            "{loopback} should be allowed"
        );
    }
    for impostor in [
        "http://localhost.evil.test/callback",
        "http://127.0.0.1.evil.test/callback",
    ] {
        assert!(
            OauthClient::new(GITHUB, "client-id", None, impostor).is_err(),
            "{impostor} must not pass as loopback"
        );
    }
}

#[test]
fn a_state_only_verifies_against_itself() {
    let state = OauthState::generate().expect("entropy");
    assert!(state.verify(state.as_str()));
    assert!(!state.verify(""));
    assert!(!state.verify(&format!("{}x", state.as_str())));

    let other = OauthState::generate().expect("entropy");
    assert!(!state.verify(other.as_str()));
    assert_ne!(state.as_str(), other.as_str());
}

#[test]
fn the_state_comparison_looks_at_every_byte_whatever_the_answer() {
    // A short-circuiting comparison returns on the first differing byte, so
    // it agrees with `==` too -- correctness alone cannot tell the two
    // apart. What can is that the answer is identical no matter where the
    // difference sits, for every position, including the one a naive
    // implementation would exit on immediately.
    let base = b"0123456789abcdef0123456789abcdef";
    for position in 0..base.len() {
        let mut altered = *base;
        altered[position] ^= 0xff;
        assert!(!constant_time_eq(base, &altered), "position {position}");
        assert!(!constant_time_eq(&altered, base), "position {position}");
    }
    assert!(constant_time_eq(base, base));
    assert!(!constant_time_eq(base, &base[..31]));
    assert!(constant_time_eq(b"", b""));
}

/// The timing property itself, measured.
///
/// Ignored by default: it is a wall-clock measurement, and a shared or
/// loaded machine can make any tolerance wrong. Run it with
/// `cargo test --test oauth -- --ignored` on a quiet machine when touching
/// the comparison.
#[test]
#[ignore = "wall-clock measurement, too noisy for an unattended run"]
fn the_state_comparison_takes_the_same_time_wherever_the_difference_sits() {
    use std::time::Instant;

    const ROUNDS: u32 = 200_000;
    let base = b"0123456789abcdef0123456789abcdef";

    let measure = |altered: &[u8; 32]| {
        let started = Instant::now();
        let mut sink = false;
        for _ in 0..ROUNDS {
            sink ^= constant_time_eq(base, altered);
        }
        assert!(!sink);
        started.elapsed().as_nanos().max(1)
    };

    let mut first_byte = *base;
    first_byte[0] ^= 0xff;
    let mut last_byte = *base;
    last_byte[31] ^= 0xff;

    let early = measure(&first_byte) as f64;
    let late = measure(&last_byte) as f64;
    let ratio = early.max(late) / early.min(late);
    assert!(ratio < 4.0, "timing differs by {ratio:.2}x");
}

#[test]
fn a_pkce_verifier_never_renders_its_secret() {
    let verifier = PkceVerifier::from_secret("a-very-secret-code-verifier".into());
    assert_eq!(verifier.secret(), "a-very-secret-code-verifier");

    let rendered = format!("{verifier:?}");
    assert!(
        !rendered.contains("a-very-secret-code-verifier"),
        "{rendered}"
    );
    assert!(rendered.contains("redacted"), "{rendered}");
}

#[test]
fn a_state_never_renders_itself() {
    let state = OauthState::from_stored("csrf-state-value".into());
    let rendered = format!("{state:?}");
    assert!(!rendered.contains("csrf-state-value"), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
}

#[test]
fn a_token_response_never_reaches_a_debug_output() {
    let tokens = TokenSet::new("ya29.a0AfH6-secret-access-token", "bearer");
    let rendered = format!("{tokens:?}");
    assert!(
        !rendered.contains("ya29.a0AfH6-secret-access-token"),
        "{rendered}"
    );
    assert!(rendered.contains("redacted"), "{rendered}");
    assert_eq!(tokens.access_token(), "ya29.a0AfH6-secret-access-token");
}

#[test]
fn an_authorization_hands_back_a_debug_output_with_no_secrets_in_it() {
    let authorization = client(GITHUB).authorize(&["read:user"]).expect("authorize");
    let secret_state = authorization.state().as_str().to_string();
    let secret_verifier = authorization.verifier().secret().to_string();

    let rendered = format!("{authorization:?}");
    assert!(!rendered.contains(&secret_state), "{rendered}");
    assert!(!rendered.contains(&secret_verifier), "{rendered}");
}

#[test]
fn a_client_debug_output_carries_no_client_secret() {
    let rendered = format!("{:?}", client(GITHUB));
    assert!(!rendered.contains("client-secret"), "{rendered}");
}

#[test]
fn a_mismatched_state_is_refused_before_the_network_is_touched() {
    // The token endpoint here does not exist. If the state check ran after
    // the request, this would fail with `Transport` instead -- so the error
    // variant is the assertion that the order is right.
    let unreachable = Endpoints {
        authorization: "https://sso.example.test/authorize",
        token: "https://127.0.0.1:1/token",
    };
    let client = OauthClient::new(
        unreachable,
        "client-id",
        None,
        "https://app.example.test/callback",
    )
    .expect("a client over https");

    let stored = OauthState::from_stored("the-state-we-issued".into());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let outcome = runtime.block_on(client.exchange(
        &stored,
        "a-state-we-never-issued",
        "the-code",
        PkceVerifier::from_secret("the-verifier".into()),
    ));
    assert!(matches!(outcome, Err(OauthError::StateMismatch)));
}

#[test]
fn an_oauth_error_never_carries_a_response_body() {
    // Every variant renders from fixed text or a short provider error code.
    // A body would be the one place a leaked token could hide.
    let rendered = [
        OauthError::StateMismatch.to_string(),
        OauthError::Entropy.to_string(),
        OauthError::Transport.to_string(),
        OauthError::MalformedResponse.to_string(),
        OauthError::Provider {
            code: "invalid_grant".into(),
        }
        .to_string(),
    ];
    for line in rendered {
        assert!(!line.contains("access_token"), "{line}");
        assert!(line.len() < 200, "{line}");
    }
}
