// Copyright 2026 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Opt-in tests for fused reads with the authentic pinned native runtime.
//! Trap notifications provide bounded waits; no polling or fake FFI is used.

use super::{MojomMessage, ReadableWithHandlesMessage, SendableMessage, WritableMessage};
use crate::message_header::{MessageHeader, MessageHeaderFlags};
use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Mutex, MutexGuard, mpsc},
    time::Duration,
};
use system::{
    message_pipe::MessageEndpoint,
    mojo_types::{MessageHandle, MojoError, UntypedHandle},
    trap::{HandleSignals, SingleEventArmResult, Trap, TriggerCondition},
};

static NATIVE_TESTS: Mutex<()> = Mutex::new(());
const LIMIT: Duration = Duration::from_secs(15);

fn native() -> MutexGuard<'static, ()> {
    // These tests share process-wide Core state, but no test-owned pipe state.
    // A prior failed assertion must not prevent the remaining tests running.
    let guard = NATIVE_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let library = std::env::var_os("MOJO_RUNTIME_LIBRARY")
        .or_else(|| std::env::var_os("MOJO_NATIVE_LIB"))
        .expect("set MOJO_RUNTIME_LIBRARY to the authentic pinned SDK runtime");
    // SAFETY: these ignored tests explicitly load the operator-supplied SDK.
    unsafe { raw_ffi::initialize(&PathBuf::from(library), true) }.unwrap();
    guard
}

fn wait_for(endpoint: &MessageEndpoint, signals: HandleSignals) {
    let trap = Trap::new().unwrap();
    let (sender, receiver) = mpsc::channel();
    trap.add_trigger(
        endpoint,
        signals,
        TriggerCondition::TriggerWhenSatisfied,
        move |event| {
            // Teardown cancellation may race with receiver destruction.
            // Do not panic or assert inside the native callback.
            let _ = sender.send(event.result());
        },
    );
    match trap.arm_single_event() {
        SingleEventArmResult::Success => receiver
            .recv_timeout(LIMIT)
            .expect("native signal did not arrive within the test timeout")
            .expect("requested native signal became impossible"),
        SingleEventArmResult::BlockingEvent(event) => event
            .result()
            .expect("requested native signal was already impossible"),
        SingleEventArmResult::NoTriggers => panic!("native trap lost its live trigger"),
    }
}

fn read(endpoint: &MessageEndpoint) -> ReadableWithHandlesMessage {
    wait_for(endpoint, HandleSignals::READABLE);
    endpoint.read().unwrap()
}

fn transferred(bytes: &[u8], handles: Vec<UntypedHandle>) -> ReadableWithHandlesMessage {
    let (sender, receiver) = MessageEndpoint::create_pipe().unwrap();
    sender
        .write(SendableMessage::new_with_data_parts(&[bytes], handles).unwrap())
        .unwrap();
    read(&receiver)
}

fn attachments(count: usize) -> (Vec<UntypedHandle>, Vec<MessageEndpoint>) {
    (0..count)
        .map(|_| {
            let (attached, peer) = MessageEndpoint::create_pipe().unwrap();
            (attached.into(), peer)
        })
        .unzip()
}

fn assert_closed(peers: Vec<MessageEndpoint>) {
    for peer in peers {
        wait_for(&peer, HandleSignals::PEER_CLOSED);
        assert!(matches!(peer.read(), Err(MojoError::FailedPrecondition)));
    }
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_fused_read_zero_handles_and_payload_boundaries() {
    let _native = native();
    for size in [0, 1, 7, 8, 9, 64, 65_536, 1_048_576] {
        let bytes: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
        let counts: &[usize] = if size == 0 { &[0, 4] } else { &[0] };
        for &count in counts {
            let (handles, peers) = attachments(count);
            let called = Cell::new(0);
            let ((length, handles), owner) = transferred(&bytes, handles)
                .read_data_with(|actual, handles| {
                    called.set(called.get() + 1);
                    assert_eq!(handles.len(), count);
                    assert_eq!(actual, bytes.as_slice());
                    (actual.len(), handles)
                })
                .unwrap();
            assert_eq!(length, size);
            assert_eq!(called.get(), 1);
            // The empty/multiple-handles case exercises both native GetData
            // phases plus the later IGNORE_HANDLES read of an empty buffer.
            assert_eq!(owner.read_bytes().unwrap(), bytes.as_slice());
            drop(owner);
            drop(handles);
            assert_closed(peers);
        }
    }
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_fused_read_transfers_multiple_live_handles() {
    let _native = native();
    let (handles, peers) = attachments(4);
    let called = Cell::new(0);
    let (handles, owner) = transferred(b"four real attachments", handles)
        .read_data_with(|bytes, handles| {
            called.set(called.get() + 1);
            assert_eq!(bytes, b"four real attachments");
            assert_eq!(handles.len(), 4);
            handles
        })
        .unwrap();
    assert_eq!(called.get(), 1);
    assert_eq!(owner.read_bytes().unwrap(), b"four real attachments");
    drop(owner);
    for (handle, peer) in handles.into_iter().zip(peers) {
        // Destroying the received message must not close extracted handles.
        let endpoint = MessageEndpoint::from(handle);
        endpoint
            .write(SendableMessage::new_with_data_parts(&[b"still live"], vec![]).unwrap())
            .unwrap();
        let (handles, received) = read(&peer).read_data().unwrap();
        assert!(handles.is_empty());
        assert_eq!(received.read_bytes().unwrap(), b"still live");
        drop(received);
        drop(endpoint);
        assert_closed(vec![peer]);
    }
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_fused_read_callback_error_and_unwind_release_handles() {
    let _native = native();
    let (handles, peers) = attachments(3);
    let (result, owner) = transferred(b"callback error", handles)
        .read_data_with(|bytes, handles| {
            assert_eq!(bytes, b"callback error");
            assert_eq!(handles.len(), 3);
            drop(handles);
            Err::<(), _>("rejected by the Rust consumer")
        })
        .unwrap();
    assert_eq!(result, Err("rejected by the Rust consumer"));
    assert_eq!(owner.read_bytes().unwrap(), b"callback error");
    drop(owner);
    assert_closed(peers);

    let (handles, peers) = attachments(3);
    let message = transferred(b"callback unwind", handles);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = message.read_data_with::<()>(|bytes, handles| {
            assert_eq!(bytes, b"callback unwind");
            assert_eq!(handles.len(), 3);
            let _owned_handles = handles;
            // This is a direct Rust call on the test thread. It does not
            // unwind through a native task, trap callback, or C ABI frame.
            panic!("intentional callback unwind");
        });
    }));
    assert!(panic.is_err());
    assert_closed(peers);
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_fused_read_native_error_does_not_invoke_callback() {
    let _native = native();
    let handle: MessageHandle = WritableMessage::new().into();
    // Deliberately use the public handle conversion on an uncommitted message
    // to exercise Core's checked FAILED_PRECONDITION result, not fake FFI.
    let message = ReadableWithHandlesMessage::from(handle);
    let called = Cell::new(false);
    let result = message.read_data_with(|_, _| called.set(true));
    assert_eq!(result.err(), Some(MojoError::FailedPrecondition));
    assert!(!called.get());
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_fused_parsed_message_retains_original_bytes_and_handles() {
    let _native = native();
    for count in [0, 4] {
        let (handles, peers) = attachments(count);
        let header = MessageHeader::new(0, 0x5041_5253, MessageHeaderFlags::empty(), 19, 0);
        let payload = vec![16, 0, 0, 0, 0, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1];
        let mut expected_wire = header.serialize();
        expected_wire.extend_from_slice(&payload);
        let parsed =
            MojomMessage::parse_raw_or_report_bad_message(transferred(&expected_wire, handles))
                .expect("valid native message header was rejected");
        assert_eq!(parsed.header, header);
        assert_eq!(parsed.payload, payload);
        assert_eq!(parsed.handles.len(), count);
        assert_eq!(
            parsed
                .raw_message_handle
                .as_ref()
                .unwrap()
                .read_bytes()
                .unwrap(),
            expected_wire.as_slice()
        );
        drop(parsed);
        assert_closed(peers);
    }
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the authentic pinned SDK"]
fn native_fused_rejected_header_keeps_owner_until_error_handling() {
    let _native = native();
    let (handles, peers) = attachments(2);
    let ((parsed, handles), owner) = transferred(b"bad", handles)
        .read_data_with(|bytes, handles| {
            // This is the production header parser's real failure path, with
            // the original owner returned for the caller's error handling.
            // Do not trigger process-wide bad-message punishment from a test
            // whose native error-handler policy is not publicly observable.
            let parsed = MessageHeader::deserialize(bytes).map(|(_, header)| header);
            (parsed, handles)
        })
        .unwrap();
    assert!(parsed.is_err());
    assert_eq!(handles.len(), 2);
    assert_eq!(owner.read_bytes().unwrap(), b"bad");
    drop(handles);
    drop(owner);
    assert_closed(peers);
}
