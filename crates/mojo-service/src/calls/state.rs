use super::{
    CallError,
    timer::{DeadlineKey, Scheduler},
};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Condvar, Mutex, Weak},
    task::{Context, Poll, Waker},
    time::Instant,
};

pub(super) trait Pending: Send + Sync {
    fn fail(&self, error: CallError) -> bool;
}

#[derive(Default)]
pub(super) struct ClientCalls(Mutex<BTreeMap<DeadlineKey, Weak<dyn Pending>>>);

impl ClientCalls {
    pub fn insert(&self, key: DeadlineKey, call: Weak<dyn Pending>) {
        self.0.lock().unwrap().insert(key, call);
    }
    pub fn remove(&self, key: DeadlineKey) {
        self.0.lock().unwrap().remove(&key);
    }
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    pub fn disconnect(&self) {
        let pending = std::mem::take(&mut *self.0.lock().unwrap());
        for call in pending.into_values() {
            if let Some(call) = call.upgrade() {
                call.fail(CallError::Disconnected);
            }
        }
    }
}

struct ResultState<R> {
    claimed: bool,
    complete: bool,
    result: Option<Result<R, CallError>>,
    waker: Option<Waker>,
}

pub(super) struct CallState<R> {
    result: Mutex<ResultState<R>>,
    changed: Condvar,
    scheduler: Weak<Scheduler>,
    clients: Weak<ClientCalls>,
    key: DeadlineKey,
}

impl<R: Send + 'static> CallState<R> {
    pub fn new(key: DeadlineKey, scheduler: Weak<Scheduler>, clients: Weak<ClientCalls>) -> Self {
        Self {
            result: Mutex::new(ResultState {
                claimed: false,
                complete: false,
                result: None,
                waker: None,
            }),
            changed: Condvar::new(),
            scheduler,
            clients,
            key,
        }
    }

    fn finish(&self, result: Result<R, CallError>) -> bool {
        let mut state = self.result.lock().unwrap();
        if state.claimed {
            drop(state);
            drop(result);
            return false;
        }
        // Even if the timer thread was delayed, a late response cannot win.
        let (result, discarded) = if result.is_ok() && Instant::now() >= self.key.at {
            (Err(CallError::TimedOut), Some(result))
        } else {
            (result, None)
        };
        state.claimed = true;
        state.result = Some(result);
        drop(state);
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.remove(self.key);
        }
        if let Some(clients) = self.clients.upgrade() {
            clients.remove(self.key);
        }
        // Publish readiness after local bookkeeping is removed. Cancelled
        // transport slots still remain in the original bindings until reply.
        let mut state = self.result.lock().unwrap();
        state.complete = true;
        let waker = state.waker.take();
        drop(state);
        drop(discarded);
        self.changed.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
        true
    }

    fn expire_if_due(&self) {
        if Instant::now() >= self.key.at {
            self.fail(CallError::TimedOut);
        }
    }
}

impl<R: Send + 'static> Pending for CallState<R> {
    fn fail(&self, error: CallError) -> bool {
        self.finish(Err(error))
    }
}

/// One local RPC result. Await on any executor, or wait on a blocking host
/// thread. Drop/explicit cancel ends local waiting only, not remote execution.
/// The result may be consumed once; polling/waiting after consumption is invalid.
pub struct Call<R: Send + 'static> {
    pub(super) state: Arc<CallState<R>>,
}

impl<R: Send + 'static> Call<R> {
    pub fn cancel(&self) -> bool {
        self.state.fail(CallError::Cancelled)
    }
    pub fn cancellation_handle(&self) -> CallCancellation {
        let state: Arc<dyn Pending> = self.state.clone();
        CallCancellation {
            state: Arc::downgrade(&state),
        }
    }
    pub fn is_finished(&self) -> bool {
        self.state.result.lock().unwrap().complete
    }

    /// Reject native dispatch/timer-thread waits, which can deadlock responses.
    /// A rejected consuming wait cancels its local call as the handle is dropped.
    pub fn wait(self) -> Result<R, CallError> {
        if mojo_ipc::SequencedTaskRunnerHandle::get_current_default().is_some()
            || self
                .state
                .scheduler
                .upgrade()
                .is_some_and(|s| s.on_timer_thread())
        {
            return Err(CallError::WouldBlock);
        }
        let mut state = self.state.result.lock().unwrap();
        loop {
            if state.complete {
                let result = state.result.take();
                drop(state);
                return result.expect("Call result already consumed");
            }
            let remaining = self.state.key.at.saturating_duration_since(Instant::now());
            if state.claimed {
                state = self.state.changed.wait(state).unwrap();
            } else if remaining.is_zero() {
                drop(state);
                self.state.fail(CallError::TimedOut);
                state = self.state.result.lock().unwrap();
            } else {
                state = self.state.changed.wait_timeout(state, remaining).unwrap().0;
            }
        }
    }
}

impl<R: Send + 'static> Future for Call<R> {
    type Output = Result<R, CallError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.state.expire_if_due();
        // Waker clone/drop/wake may execute arbitrary user code. Keep all of it
        // outside the result, scheduler and client registry locks.
        let new_waker = cx.waker().clone();
        let mut state = self.state.result.lock().unwrap();
        if state.complete {
            let result = state.result.take();
            drop(state);
            drop(new_waker);
            Poll::Ready(result.expect("Call polled after completion"))
        } else {
            let old_waker = state.waker.replace(new_waker);
            drop(state);
            drop(old_waker);
            Poll::Pending
        }
    }
}

impl<R: Send + 'static> Drop for Call<R> {
    fn drop(&mut self) {
        self.state.fail(CallError::Cancelled);
    }
}

/// Weak, cloneable cross-thread cancellation; it does not retain the call/result.
#[derive(Clone)]
pub struct CallCancellation {
    state: Weak<dyn Pending>,
}
impl CallCancellation {
    /// True only if this cancellation won the completion race.
    pub fn cancel(&self) -> bool {
        self.state
            .upgrade()
            .is_some_and(|state| state.fail(CallError::Cancelled))
    }
}

/// Move into exactly one generated response callback. Only a weak reference is
/// retained in the bindings; late replies are decoded normally then discarded.
/// Dropping an unanswered bridge reports Disconnected to the local call.
pub struct CallResponder<R: Send + 'static> {
    pub(super) state: Weak<CallState<R>>,
    pub(super) answered: bool,
}

impl<R: Send + 'static> CallResponder<R> {
    pub fn respond(mut self, value: R) -> bool {
        self.answered = true;
        if let Some(state) = self.state.upgrade() {
            state.finish(Ok(value))
        } else {
            false
        }
    }
    pub fn into_callback(self) -> impl FnOnce(R) + Send + 'static {
        move |value| {
            self.respond(value);
        }
    }
}

impl<R: Send + 'static> Drop for CallResponder<R> {
    fn drop(&mut self) {
        if !self.answered {
            if let Some(state) = self.state.upgrade() {
                state.fail(CallError::Disconnected);
            }
        }
    }
}
