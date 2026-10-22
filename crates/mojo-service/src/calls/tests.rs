use super::{Call, CallContext, CallError, CallResponder, state::ClientCalls};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    task::{Context, Poll, Wake, Waker},
    thread,
    time::Duration,
};

fn pair<R: Send + 'static>(
    context: &CallContext,
    calls: &Arc<ClientCalls>,
    timeout: Duration,
) -> (Call<R>, CallResponder<R>) {
    context.start(calls, timeout).unwrap()
}

struct Signal(mpsc::Sender<()>);
impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
}

#[test]
fn response_completion_and_response_drop_remove_deadlines() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (call, responder) = pair(&context, &calls, Duration::from_secs(10));
    assert!(responder.respond(42));
    assert_eq!(call.wait(), Ok(42));
    let (call, responder) = pair::<()>(&context, &calls, Duration::from_secs(10));
    drop(responder);
    assert_eq!(call.wait(), Err(CallError::Disconnected));
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
}

#[test]
fn timer_wakes_an_executor_without_an_external_poll_loop() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (mut call, _responder) = pair::<()>(&context, &calls, Duration::from_millis(100));
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(Signal(tx)));
    let first = Pin::new(&mut call).poll(&mut Context::from_waker(&waker));
    if first.is_pending() {
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(
            Pin::new(&mut call).poll(&mut Context::from_waker(&waker)),
            Poll::Ready(Err(CallError::TimedOut))
        );
    } else {
        assert_eq!(first, Poll::Ready(Err(CallError::TimedOut)));
    }
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
}

#[test]
fn cancellation_handles_are_weak_and_completion_is_single_winner() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    for value in 0..32 {
        let (call, responder) = pair(&context, &calls, Duration::from_secs(10));
        let cancel = call.cancellation_handle();
        let second = cancel.clone();
        let (responded, cancelled) = thread::scope(|scope| {
            let response = scope.spawn(move || responder.respond(value));
            let cancellation = scope.spawn(move || cancel.cancel());
            (response.join().unwrap(), cancellation.join().unwrap())
        });
        assert_ne!(responded, cancelled);
        assert_eq!(
            call.wait(),
            if responded {
                Ok(value)
            } else {
                Err(CallError::Cancelled)
            }
        );
        assert!(!second.cancel());
    }
    let (call, responder) = pair::<()>(&context, &calls, Duration::from_secs(10));
    let weak = call.cancellation_handle();
    drop(call);
    assert!(!weak.cancel());
    assert!(!responder.respond(()));
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
}

#[test]
fn shutdown_resolves_all_calls_and_rejects_new_deadlines() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let mut pending = Vec::new();
    let mut responders = Vec::new();
    for _ in 0..16 {
        let (call, responder) = pair::<()>(&context, &calls, Duration::from_secs(60));
        pending.push(call);
        responders.push(responder);
    }
    context.shutdown();
    assert!(context.is_shutdown());
    for call in pending {
        assert_eq!(call.wait(), Err(CallError::ContextClosed));
    }
    assert!(matches!(
        context.start::<()>(&calls, Duration::from_secs(1)),
        Err(CallError::ContextClosed)
    ));
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
    drop(responders);
}

#[test]
fn completed_or_dropped_calls_do_not_accumulate_scheduler_records() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    for _ in 0..1000 {
        let (call, responder) = pair(&context, &calls, Duration::from_secs(3600));
        drop(call);
        assert!(!responder.respond(42));
    }
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
    assert!(matches!(
        context.start::<()>(&calls, Duration::ZERO),
        Err(CallError::TimedOut)
    ));
    assert!(matches!(
        context.start::<()>(&calls, Duration::MAX),
        Err(CallError::InvalidOptions)
    ));
}

struct ReentrantWake {
    context: CallContext,
    cancel: super::CallCancellation,
    signalled: mpsc::Sender<()>,
}
impl Wake for ReentrantWake {
    fn wake(self: Arc<Self>) {
        assert!(!self.cancel.cancel());
        assert_eq!(self.context.pending_deadlines(), 0);
        self.context.shutdown();
        self.signalled.send(()).unwrap();
    }
}

#[test]
fn wake_can_reenter_cancellation_and_context_shutdown() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (mut call, _responder) = pair::<()>(&context, &calls, Duration::from_secs(10));
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(ReentrantWake {
        context: context.clone(),
        cancel: call.cancellation_handle(),
        signalled: tx,
    }));
    assert!(
        Pin::new(&mut call)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert!(call.cancel());
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(call.wait(), Err(CallError::Cancelled));
}

struct CancelOnDrop(super::CallCancellation);
impl Wake for CancelOnDrop {
    fn wake(self: Arc<Self>) {}
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[test]
fn replacing_a_waker_drops_it_outside_the_result_lock() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (mut call, _responder) = pair::<()>(&context, &calls, Duration::from_secs(10));
    let old = Waker::from(Arc::new(CancelOnDrop(call.cancellation_handle())));
    assert!(
        Pin::new(&mut call)
            .poll(&mut Context::from_waker(&old))
            .is_pending()
    );
    drop(old);
    let (tx, rx) = mpsc::channel();
    let new = Waker::from(Arc::new(Signal(tx)));
    assert!(
        Pin::new(&mut call)
            .poll(&mut Context::from_waker(&new))
            .is_pending()
    );
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(call.wait(), Err(CallError::Cancelled));
}

struct ReentrantPayload {
    context: CallContext,
    drops: Arc<AtomicUsize>,
}
impl Drop for ReentrantPayload {
    fn drop(&mut self) {
        assert_eq!(self.context.pending_deadlines(), 0);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn late_payload_is_dropped_without_retaining_future_state() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (call, responder) = pair(&context, &calls, Duration::from_secs(10));
    let drops = Arc::new(AtomicUsize::new(0));
    assert!(call.cancel());
    assert!(!responder.respond(ReentrantPayload {
        context: context.clone(),
        drops: drops.clone()
    }));
    assert!(matches!(call.wait(), Err(CallError::Cancelled)));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn client_drop_tracking_disconnects_waiters_without_native_dispatch() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (call, responder) = pair::<()>(&context, &calls, Duration::from_secs(10));
    calls.disconnect();
    assert_eq!(call.wait(), Err(CallError::Disconnected));
    assert!(!responder.respond(()));
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
}

#[test]
fn last_context_owner_drop_closes_calls_without_a_join() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (call, responder) = pair::<()>(&context, &calls, Duration::from_secs(60));
    let clone = context.clone();
    drop(context);
    assert!(!call.is_finished());
    drop(clone);
    assert_eq!(call.wait(), Err(CallError::ContextClosed));
    assert_eq!(calls.len(), 0);
    assert!(!responder.respond(()));
}

struct WaitFromTimer {
    other: std::sync::Mutex<Option<Call<()>>>,
    result: mpsc::Sender<Result<(), CallError>>,
}
impl Wake for WaitFromTimer {
    fn wake(self: Arc<Self>) {
        let call = self.other.lock().unwrap().take().unwrap();
        self.result.send(call.wait()).unwrap();
    }
}

#[test]
fn blocking_wait_is_rejected_on_the_deadline_worker() {
    let context = CallContext::new().unwrap();
    let calls = Arc::new(ClientCalls::default());
    let (other, _other_reply) = pair::<()>(&context, &calls, Duration::from_secs(10));
    let (mut trigger, _trigger_reply) = pair::<()>(&context, &calls, Duration::from_millis(500));
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(WaitFromTimer {
        other: std::sync::Mutex::new(Some(other)),
        result: tx,
    }));
    assert!(
        Pin::new(&mut trigger)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        Err(CallError::WouldBlock)
    );
    assert_eq!(trigger.wait(), Err(CallError::TimedOut));
    assert_eq!((context.pending_deadlines(), calls.len()), (0, 0));
}
