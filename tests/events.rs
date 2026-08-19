//! Tests for the events Dispatcher (in-process typed dispatch).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use arcature::events::{Dispatcher, DispatchError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Event)]
struct UserRegistered {
    user_id: u64,
    email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Event)]
struct OrderShipped {
    order_id: u64,
}

#[test]
fn dispatch_with_no_listeners_is_a_noop() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dispatcher = Dispatcher::new();
    rt.block_on(async {
        let result = dispatcher
            .dispatch(&UserRegistered {
                user_id: 1,
                email: "a@b.com".into(),
            })
            .await;
        assert!(result.is_ok());
    });
}

#[test]
fn dispatch_runs_all_listeners_in_registration_order() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc1 = call_count.clone();
    let cc2 = call_count.clone();

    let dispatcher = Dispatcher::new()
        .register(move |event: UserRegistered| {
            let cc = cc1.clone();
            async move {
                assert_eq!(event.user_id, 1);
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .register(move |_event: UserRegistered| {
            let cc = cc2.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

    rt.block_on(async {
        let result = dispatcher
            .dispatch(&UserRegistered {
                user_id: 1,
                email: "a@b.com".into(),
            })
            .await;
        assert!(result.is_ok());
    });
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn dispatch_returns_first_error_but_all_listeners_run() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran2 = ran.clone();

    let dispatcher = Dispatcher::new()
        .register(move |_event: UserRegistered| async move {
            Err(DispatchError::Listener("first".into()))
        })
        .register(move |_event: UserRegistered| {
            let ran2 = ran2.clone();
            async move {
                ran2.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

    rt.block_on(async {
        let result = dispatcher
            .dispatch(&UserRegistered {
                user_id: 1,
                email: "a@b.com".into(),
            })
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            DispatchError::Listener(msg) => assert_eq!(msg, "first"),
            _ => panic!("expected Listener error"),
        }
    });
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

#[test]
fn recording_dispatcher_tracks_dispatched_events() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dispatcher = Dispatcher::recording();
    rt.block_on(async {
        dispatcher
            .dispatch(&UserRegistered {
                user_id: 1,
                email: "a@b.com".into(),
            })
            .await
            .unwrap();
        dispatcher
            .dispatch(&OrderShipped { order_id: 1 })
            .await
            .unwrap();
    });
    assert!(dispatcher.was_dispatched("UserRegistered"));
    assert!(dispatcher.was_dispatched("OrderShipped"));
    assert!(!dispatcher.was_dispatched("Unknown"));
    assert_eq!(dispatcher.dispatched_events().len(), 2);
}

#[test]
fn dispatcher_listener_count() {
    let dispatcher = Dispatcher::new()
        .register(|_e: UserRegistered| async { Ok(()) })
        .register(|_e: UserRegistered| async { Ok(()) })
        .register(|_e: OrderShipped| async { Ok(()) });
    assert_eq!(dispatcher.listener_count("UserRegistered"), 2);
    assert_eq!(dispatcher.listener_count("OrderShipped"), 1);
    assert_eq!(dispatcher.listener_count("Unknown"), 0);
}

#[test]
fn dispatcher_debug_does_not_leak_closures() {
    let dispatcher = Dispatcher::new().register(|_e: UserRegistered| async { Ok(()) });
    let debug = format!("{dispatcher:?}");
    assert!(!debug.contains("0x"));
    assert!(!debug.contains("closure"));
}
