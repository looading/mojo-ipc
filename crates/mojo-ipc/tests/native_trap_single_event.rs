//! Opt-in native semantics, not a timing or allocation benchmark.
use mojo_ipc::system::{
    message::SendableMessage,
    trap::{ArmResult, HandleSignals, SingleEventArmResult, Trap, TrapError, TriggerCondition},
};
use mojo_ipc::{MessageEndpoint, Role, Runtime};
use std::{sync::mpsc, time::Duration};

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY (or MOJO_NATIVE_LIB) pointing to the pinned native SDK"]
fn stack_arm_preserves_blocking_readable_rearm_disconnect_and_cancellation() {
    let library = std::env::var_os("MOJO_RUNTIME_LIBRARY")
        .or_else(|| std::env::var_os("MOJO_NATIVE_LIB"))
        .expect("set MOJO_RUNTIME_LIBRARY to the trusted pinned native image");
    // SAFETY: this test is explicitly opted into the authentic native SDK.
    let _runtime =
        unsafe { Runtime::load(std::path::PathBuf::from(library), Role::Broker) }.unwrap();
    let trap = Trap::new().unwrap();
    assert!(matches!(
        trap.arm_single_event(),
        SingleEventArmResult::NoTriggers
    ));
    assert!(matches!(trap.arm(), ArmResult::NoTriggers));

    let (sender, receiver) = MessageEndpoint::create_pipe().unwrap();
    let (events, notifications) = mpsc::channel();
    let trigger = trap.add_trigger(
        &receiver,
        HandleSignals::READABLE,
        TriggerCondition::TriggerWhenSatisfied,
        move |event| {
            let _ = events.send(event.result());
        },
    );
    sender
        .write(SendableMessage::new_with_data_parts(&[b"queued"], vec![]).unwrap())
        .unwrap();
    match trap.arm_single_event() {
        SingleEventArmResult::BlockingEvent(event) => {
            assert_eq!(event.result(), Ok(()));
            assert!(event.signals_state().satisfied().is_readable());
        }
        _ => panic!("queued data must block single-event arm"),
    }
    // A blocked arm returns data for the caller; it does not invoke the handler.
    assert!(notifications.try_recv().is_err());
    let blocked = trap
        .arm()
        .expect_blocking("legacy arm must still return its one-event Vec");
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].result(), Ok(()));
    assert!(blocked[0].signals_state().satisfied().is_readable());
    drop(receiver.read().unwrap());
    assert!(matches!(
        trap.arm_single_event(),
        SingleEventArmResult::Success
    ));

    sender
        .write(SendableMessage::new_with_data_parts(&[b"after arm"], vec![]).unwrap())
        .unwrap();
    assert_eq!(
        notifications.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(())
    );
    drop(receiver.read().unwrap());
    assert!(matches!(
        trap.arm_single_event(),
        SingleEventArmResult::Success
    ));
    drop(sender);
    assert_eq!(
        notifications.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(TrapError::FailedPrecondition)
    );
    trap.remove_trigger(trigger).unwrap();
    assert_eq!(
        notifications.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(TrapError::Cancelled)
    );
    assert!(notifications.try_recv().is_err());
    assert!(matches!(
        trap.arm_single_event(),
        SingleEventArmResult::NoTriggers
    ));
    assert!(matches!(trap.arm(), ArmResult::NoTriggers));

    // Dropping a trap with a live, armed trigger still performs one final
    // cancellation; the stack result never owns or frees the trigger context.
    let (_peer, watched) = MessageEndpoint::create_pipe().unwrap();
    let (cancelled, cancellation) = mpsc::channel();
    let trap = Trap::new().unwrap();
    trap.add_trigger(
        &watched,
        HandleSignals::READABLE,
        TriggerCondition::TriggerWhenSatisfied,
        move |event| {
            let _ = cancelled.send(event.result());
        },
    );
    assert!(matches!(
        trap.arm_single_event(),
        SingleEventArmResult::Success
    ));
    drop(trap);
    assert_eq!(
        cancellation.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(TrapError::Cancelled)
    );
    assert!(cancellation.try_recv().is_err());
}
