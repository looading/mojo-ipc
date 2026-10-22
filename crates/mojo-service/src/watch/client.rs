use crate::{
    ServiceChange, ServiceInfo,
    control::{ServiceWatch, WatchStatus},
};
use mojo_ipc::{PendingRemote, Remote, SequencedTaskRunnerHandle};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Condvar, Mutex, Weak},
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchError {
    TimedOut,
    Disconnected,
    ResyncRequired,
    Busy,
    WouldBlock,
    InvalidOptions,
}
impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "service watch: {:?}", self)
    }
}
impl std::error::Error for WatchError {}

/// Revisions are local to one subscription, starting at one after its snapshot.
/// Resubscribe resets them; service generations remain manager-wide identities.
#[derive(Debug)]
pub struct ServiceWatchBatch {
    pub revision: u64,
    pub changes: Vec<ServiceChange>,
}
type BatchResult = Result<ServiceWatchBatch, WatchError>;

#[derive(Default)]
struct State {
    done: bool,
    result: Option<BatchResult>,
    waker: Option<Waker>,
}
#[derive(Default)]
struct ReplySlot {
    state: Mutex<State>,
    changed: Condvar,
}

impl ReplySlot {
    fn finish(&self, result: BatchResult) {
        let mut state = self.state.lock().unwrap();
        if state.done {
            drop(state);
            drop(result);
            return;
        }
        state.done = true;
        state.result = Some(result);
        let waker = state.waker.take();
        drop(state);
        self.changed.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll(&self, cx: &mut Context<'_>) -> Poll<BatchResult> {
        let waker = cx.waker().clone();
        let mut state = self.state.lock().unwrap();
        if state.done {
            let result = state.result.take();
            drop(state);
            drop(waker);
            Poll::Ready(result.expect("watch batch already consumed"))
        } else {
            let old = state.waker.replace(waker);
            drop(state);
            drop(old);
            Poll::Pending
        }
    }

    fn wait(&self, deadline: Instant) -> Option<BatchResult> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.done {
                let result = state.result.take();
                drop(state);
                return result;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            state = self.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }

    fn clear_waker(&self) {
        let old = self.state.lock().unwrap().waker.take();
        drop(old);
    }
}

struct ReplyBridge {
    slot: Weak<ReplySlot>,
    answered: bool,
}
impl ReplyBridge {
    fn reply(mut self, status: WatchStatus, revision: u64, changes: Vec<ServiceChange>) {
        self.answered = true;
        let result = match status {
            WatchStatus::Updates => Ok(ServiceWatchBatch { revision, changes }),
            WatchStatus::ResyncRequired => Err(WatchError::ResyncRequired),
            WatchStatus::Closed => Err(WatchError::Disconnected),
            WatchStatus::Busy => Err(WatchError::Busy),
        };
        if let Some(slot) = self.slot.upgrade() {
            slot.finish(result);
        }
    }
}
impl Drop for ReplyBridge {
    fn drop(&mut self) {
        if !self.answered {
            if let Some(slot) = self.slot.upgrade() {
                slot.finish(Err(WatchError::Disconnected));
            }
        }
    }
}

/// Initial authorized snapshot plus bounded, change-driven discovery. One Next
/// RPC/reply is retained across timed-out or dropped waits. Drop closes the pipe;
/// overflow/disconnect is terminal and requires a new directory.watch call.
pub struct ServiceSubscription {
    remote: Option<Remote<dyn ServiceWatch>>,
    snapshot: Vec<ServiceInfo>,
    pending: Option<Arc<ReplySlot>>,
    terminal: Option<WatchError>,
}

impl ServiceSubscription {
    pub(crate) fn bind(
        snapshot: Vec<ServiceInfo>,
        remote: PendingRemote<dyn ServiceWatch>,
        runner: SequencedTaskRunnerHandle,
    ) -> Self {
        Self {
            remote: Some(remote.bind_with_options(Some(runner), None)),
            snapshot,
            pending: None,
            terminal: None,
        }
    }
    pub fn initial_services(&self) -> &[ServiceInfo] {
        &self.snapshot
    }
    pub fn outstanding_requests(&self) -> usize {
        self.remote
            .as_ref()
            .map_or(0, |r| r.pending_response_count())
    }

    fn ensure_pending(&mut self) {
        if self.pending.is_some() || self.terminal.is_some() {
            return;
        }
        let slot = Arc::new(ReplySlot::default());
        let bridge = ReplyBridge {
            slot: Arc::downgrade(&slot),
            answered: false,
        };
        self.remote
            .as_mut()
            .expect("live subscription remote")
            .Next(move |status, revision, changes| {
                bridge.reply(status, revision, changes);
            });
        self.pending = Some(slot);
    }

    fn consume(&mut self, result: BatchResult) -> BatchResult {
        self.pending.take();
        if let Err(error) = result {
            self.terminal = Some(error);
            self.remote.take();
            Err(error)
        } else {
            result
        }
    }

    /// A borrowing Future. Dropping it releases its Waker but keeps the single
    /// in-flight Next RPC/batch in this subscription for a subsequent wait.
    pub fn next(&mut self) -> NextServiceBatch<'_> {
        self.ensure_pending();
        NextServiceBatch {
            subscription: self,
            finished: false,
        }
    }

    /// Host-thread wait. Timeout preserves the existing Next RPC/buffered reply.
    pub fn recv_timeout(&mut self, timeout: Duration) -> BatchResult {
        if SequencedTaskRunnerHandle::get_current_default().is_some() {
            return Err(WatchError::WouldBlock);
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(WatchError::InvalidOptions)?;
        if let Some(error) = self.terminal {
            return Err(error);
        }
        self.ensure_pending();
        match self.pending.as_ref().unwrap().wait(deadline) {
            Some(result) => self.consume(result),
            None => Err(WatchError::TimedOut),
        }
    }
}

impl Drop for ServiceSubscription {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.finish(Err(WatchError::Disconnected));
        }
    }
}

pub struct NextServiceBatch<'a> {
    subscription: &'a mut ServiceSubscription,
    finished: bool,
}
impl Future for NextServiceBatch<'_> {
    type Output = BatchResult;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.finished, "watch Future polled after completion");
        if let Some(error) = this.subscription.terminal {
            this.finished = true;
            return Poll::Ready(Err(error));
        }
        match this.subscription.pending.as_ref().unwrap().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                this.finished = true;
                Poll::Ready(this.subscription.consume(result))
            }
        }
    }
}
impl Drop for NextServiceBatch<'_> {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(pending) = &self.subscription.pending {
                pending.clear_waker();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, task::Wake};
    struct Signal(mpsc::Sender<()>);
    impl Wake for Signal {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn timeout_does_not_consume_future_batch_and_callback_drop_disconnects() {
        let slot = Arc::new(ReplySlot::default());
        assert!(slot.wait(Instant::now()).is_none());
        let bridge = ReplyBridge {
            slot: Arc::downgrade(&slot),
            answered: false,
        };
        bridge.reply(WatchStatus::Updates, 7, Vec::new());
        assert_eq!(slot.wait(Instant::now()).unwrap().unwrap().revision, 7);
        let slot = Arc::new(ReplySlot::default());
        drop(ReplyBridge {
            slot: Arc::downgrade(&slot),
            answered: false,
        });
        assert!(matches!(
            slot.wait(Instant::now()).unwrap(),
            Err(WatchError::Disconnected)
        ));
    }

    struct ReentrantDrop(Weak<ReplySlot>);
    impl Wake for ReentrantDrop {
        fn wake(self: Arc<Self>) {}
    }
    impl Drop for ReentrantDrop {
        fn drop(&mut self) {
            if let Some(slot) = self.0.upgrade() {
                slot.clear_waker();
            }
        }
    }

    #[test]
    fn replaced_and_abandoned_wakers_are_released_outside_the_slot_lock() {
        let slot = Arc::new(ReplySlot::default());
        let waker = Waker::from(Arc::new(ReentrantDrop(Arc::downgrade(&slot))));
        assert!(slot.poll(&mut Context::from_waker(&waker)).is_pending());
        drop(waker);
        slot.clear_waker();
        let (tx, rx) = mpsc::channel();
        let waker = Waker::from(Arc::new(Signal(tx)));
        assert!(slot.poll(&mut Context::from_waker(&waker)).is_pending());
        slot.finish(Err(WatchError::ResyncRequired));
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            slot.poll(&mut Context::from_waker(&waker)),
            Poll::Ready(Err(WatchError::ResyncRequired))
        ));
    }
}
