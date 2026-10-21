// Copyright 2026 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Opt-in native scheduling semantics, not a performance benchmark.
//!
//! These tests use the real pinned runtime, real pipes, and its dispatch
//! sequence. No assertion depends on the historical second posted router
//! task: other application tasks may interleave differently after dispatch
//! coalescing. Native callback assertions are caught and sent to the Rust test
//! thread so a failed assertion never unwinds across the native task ABI.

use super::endpoint_registry::{EndpointInfo, InterfaceId, PRIMARY_INTERFACE_ID};
use super::multiplex_router::MultiplexRouter;
use crate::message::MojomMessage;
use crate::message_header::{MessageHeader, MessageHeaderFlags};
use crate::pending_associated_endpoint_parsing::Registrar;
use sequenced_task_runner::SequencedTaskRunnerHandle;
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, mpsc},
    time::{Duration, Instant},
};
use system::{message_pipe::MessageEndpoint, mojo_types::MojoError};

const METHOD: u32 = 0x4e41_5449;
const LIMIT: Duration = Duration::from_secs(15);
static NATIVE_TESTS: Mutex<()> = Mutex::new(());

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Message(InterfaceId, u64),
    Disconnected(InterfaceId),
    Reentered(u64),
    DroppedLastOwner,
}
type EventSender = mpsc::Sender<Result<Event, String>>;

fn native() -> (MutexGuard<'static, ()>, SequencedTaskRunnerHandle) {
    // Serialize these tests because they intentionally share the process-wide
    // native runtime and dispatch sequence. Real initialization is idempotent.
    let guard = NATIVE_TESTS.lock().unwrap();
    let library = std::env::var_os("MOJO_RUNTIME_LIBRARY")
        .or_else(|| std::env::var_os("MOJO_NATIVE_LIB"))
        .expect("set MOJO_RUNTIME_LIBRARY to the trusted pinned native SDK");
    // SAFETY: --ignored explicitly opts into loading the operator-supplied,
    // authentic pinned SDK image. No replacement API table or fake FFI is used.
    unsafe { raw_ffi::initialize(&PathBuf::from(library), true) }.unwrap();
    let runner = SequencedTaskRunnerHandle::dispatch_runner();
    assert!(!runner.runs_tasks_in_current_sequence());
    (guard, runner)
}

fn panic_message(value: Box<dyn Any + Send>) -> String {
    if let Some(message) = value.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = value.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "non-string panic in native test callback".to_owned()
    }
}

fn capture<T>(sender: &mpsc::Sender<Result<T, String>>, operation: impl FnOnce() -> T) {
    let result = catch_unwind(AssertUnwindSafe(operation)).map_err(panic_message);
    let _ = sender.send(result);
}

fn receive<T>(receiver: &mpsc::Receiver<Result<T, String>>, deadline: Instant) -> T {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .expect("native dispatch stalled or event channel closed")
        .unwrap_or_else(|message| panic!("native callback assertion: {message}"))
}

fn on_sequence<T: Send + 'static>(
    runner: &SequencedTaskRunnerHandle,
    deadline: Instant,
    operation: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (sender, receiver) = mpsc::channel();
    let expected = runner.clone();
    assert!(runner.post_task(move || {
        capture(&sender, || {
            assert!(expected.runs_tasks_in_current_sequence());
            operation()
        });
    }));
    receive(&receiver, deadline)
}

fn message(interface: InterfaceId, id: u64) -> MojomMessage {
    // Independent valid wire fixture for Params { uint64 id; }: a 16-byte
    // version-zero struct. The runtime's actual Mojom message-header builder
    // supplies tracing/time fields and its regular parser reads it on receipt.
    let mut payload = vec![16, 0, 0, 0, 0, 0, 0, 0];
    payload.extend_from_slice(&id.to_le_bytes());
    MojomMessage {
        header: MessageHeader::new(interface, METHOD, MessageHeaderFlags::empty(), id, 0),
        payload,
        handles: vec![],
        raw_message_handle: None,
    }
}

fn validate(message: &MojomMessage, interface: InterfaceId) -> u64 {
    assert_eq!(message.header.interface_id, interface);
    assert_eq!(message.header.name, METHOD);
    assert!(message.header.flags.is_empty());
    assert!(message.handles.is_empty());
    assert_eq!(message.payload.len(), 16);
    assert_eq!(&message.payload[..8], &[16, 0, 0, 0, 0, 0, 0, 0]);
    let id = u64::from_le_bytes(message.payload[8..16].try_into().unwrap());
    assert_eq!(id, message.header.request_id);
    id
}

fn endpoint_info(
    interface: InterfaceId,
    runner: &SequencedTaskRunnerHandle,
    events: &EventSender,
) -> EndpointInfo {
    let incoming = events.clone();
    let expected = runner.clone();
    let disconnected = events.clone();
    let disconnect_runner = runner.clone();
    EndpointInfo {
        incoming_message_handler: Arc::new(move |message, _sender| {
            capture(&incoming, || {
                assert!(expected.runs_tasks_in_current_sequence());
                Event::Message(interface, validate(&message, interface))
            });
        }),
        runner: runner.clone(),
        disconnect_handler: Some(Box::new(move || {
            capture(&disconnected, || {
                assert!(disconnect_runner.runs_tasks_in_current_sequence());
                Event::Disconnected(interface)
            });
        })),
    }
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_primary_fifo_256_messages_then_disconnect() {
    let (_serial, runner) = native();
    let deadline = Instant::now() + LIMIT;
    let (events, received) = mpsc::channel();
    let (peer, endpoint) = MessageEndpoint::create_pipe().unwrap();
    let router = MultiplexRouter::new(
        endpoint,
        false,
        endpoint_info(PRIMARY_INTERFACE_ID, &runner, &events),
    );
    assert!(router.primary_dispatch_eligible_for_testing());
    for id in 0..256 {
        peer.write(message(PRIMARY_INTERFACE_ID, id).into())
            .unwrap();
    }
    // Closing a pipe with unread messages must not make the disconnect
    // callback overtake any of those messages.
    drop(peer);
    for id in 0..256 {
        assert_eq!(receive(&received, deadline), Event::Message(0, id));
    }
    assert_eq!(receive(&received, deadline), Event::Disconnected(0));
    assert!(received.try_recv().is_err());
    drop(router);
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_callback_can_register_drop_and_respond_without_router_lock() {
    let (_serial, runner) = native();
    let deadline = Instant::now() + LIMIT;
    let (events, received) = mpsc::channel();
    let incoming = events.clone();
    let expected = runner.clone();
    let (peer, endpoint) = MessageEndpoint::create_pipe().unwrap();
    let router = MultiplexRouter::new(
        endpoint,
        false,
        EndpointInfo {
            incoming_message_handler: Arc::new(move |message, sender| {
                capture(&incoming, || {
                    assert!(expected.runs_tasks_in_current_sequence());
                    let id = validate(&message, PRIMARY_INTERFACE_ID);
                    // All three operations reacquire router state. This would
                    // deadlock if a fast-dispatch job retained its router lock.
                    let associated = sender.register_new_endpoint(None, None).unwrap();
                    drop(associated);
                    sender.send_message(self::message(PRIMARY_INTERFACE_ID, id));
                    Event::Reentered(id)
                });
            }),
            runner: runner.clone(),
            disconnect_handler: None,
        },
    );
    peer.write(message(PRIMARY_INTERFACE_ID, 1).into()).unwrap();
    assert_eq!(receive(&received, deadline), Event::Reentered(1));
    assert!(!router.primary_dispatch_eligible_for_testing());
    // Registration was temporary, but fast eligibility must stay disabled.
    // The same callback must also work through the old queue/post fallback.
    peer.write(message(PRIMARY_INTERFACE_ID, 2).into()).unwrap();
    assert_eq!(receive(&received, deadline), Event::Reentered(2));
    assert!(!router.primary_dispatch_eligible_for_testing());
    drop((peer, router));
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_unbound_associated_backlog_blocks_later_primary_until_bind() {
    let (_serial, runner) = native();
    let deadline = Instant::now() + LIMIT;
    let associated_id = 0x8000_0001; // peer-owned when this router sets no high bit
    let (events, received) = mpsc::channel();
    let (peer, endpoint) = MessageEndpoint::create_pipe().unwrap();
    peer.write(message(associated_id, 1).into()).unwrap();
    peer.write(message(PRIMARY_INTERFACE_ID, 2).into()).unwrap();

    // The pinned ipcz MojoTrap accepts UNSATISFIED but deliberately does not
    // implement it (mojo/core/ipcz_driver/mojo_trap.cc:228-233). Observe actual
    // fallback queue insertions through a cfg(test)-only notification instead
    // of guessing dispatch turns, sleeping, polling, or fabricating a native
    // signal. The tested reads, routing and binding remain fully genuine.
    let (queued, queue_events) = mpsc::channel();
    let info = endpoint_info(PRIMARY_INTERFACE_ID, &runner, &events);
    let router = on_sequence(&runner, deadline, move || {
        let router = MultiplexRouter::new(endpoint, false, info);
        assert_eq!(
            router.add_associated_interface(Some(associated_id), None),
            Some(associated_id)
        );
        assert!(!router.primary_dispatch_eligible_for_testing());
        router.set_queued_task_observer_for_testing(queued);
        router
    });
    assert_eq!(
        queue_events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap(),
        1
    );
    assert_eq!(
        queue_events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap(),
        2
    );
    let bind_router = router.clone();
    let info = endpoint_info(associated_id, &runner, &events);
    let received = on_sequence(&runner, deadline, move || {
        assert_eq!(bind_router.queued_task_count_for_testing(), 2);
        assert!(
            received.try_recv().is_err(),
            "unbound associated message or later primary escaped its backlog"
        );
        bind_router.bind_interface(associated_id, info);
        received
    });
    assert_eq!(
        receive(&received, deadline),
        Event::Message(associated_id, 1)
    );
    assert_eq!(
        receive(&received, deadline),
        Event::Message(PRIMARY_INTERFACE_ID, 2)
    );
    let drop_router = router.clone();
    on_sequence(&runner, deadline, move || {
        drop_router.notify_dropped(associated_id);
        assert!(!drop_router.primary_dispatch_eligible_for_testing());
    });
    peer.write(message(PRIMARY_INTERFACE_ID, 3).into()).unwrap();
    assert_eq!(
        receive(&received, deadline),
        Event::Message(PRIMARY_INTERFACE_ID, 3)
    );
    drop(peer);
    assert_eq!(
        receive(&received, deadline),
        Event::Disconnected(PRIMARY_INTERFACE_ID)
    );
    drop(router);
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_callback_can_drop_last_watcher_owner_before_returning() {
    let (_serial, runner) = native();
    let deadline = Instant::now() + LIMIT;
    let (events, received) = mpsc::channel();
    let owner = Arc::new(Mutex::new(None::<MultiplexRouter>));
    let callback_owner = owner.clone();
    let expected = runner.clone();
    let (peer, endpoint) = MessageEndpoint::create_pipe().unwrap();
    let peer = Arc::new(peer);
    let callback_peer = peer.clone();
    let router = MultiplexRouter::new(
        endpoint,
        false,
        EndpointInfo {
            incoming_message_handler: Arc::new(move |message, _sender| {
                capture(&events, || {
                    assert!(expected.runs_tasks_in_current_sequence());
                    assert_eq!(validate(&message, PRIMARY_INTERFACE_ID), 1);
                    let last_owner = callback_owner.lock().unwrap().take().unwrap();
                    drop(last_owner);
                    // If either Watcher's temporary Arc or the router's
                    // response sender retained a strong watcher reference,
                    // this write would incorrectly still succeed here.
                    assert_eq!(
                        callback_peer.write(self::message(PRIMARY_INTERFACE_ID, 2).into()),
                        Err(MojoError::FailedPrecondition)
                    );
                    Event::DroppedLastOwner
                });
            }),
            runner: runner.clone(),
            disconnect_handler: None,
        },
    );
    *owner.lock().unwrap() = Some(router);
    peer.write(message(PRIMARY_INTERFACE_ID, 1).into()).unwrap();
    assert_eq!(receive(&received, deadline), Event::DroppedLastOwner);
    assert!(owner.lock().unwrap().is_none());
}
