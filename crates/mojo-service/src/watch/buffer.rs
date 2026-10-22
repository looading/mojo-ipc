use super::MAX_WATCH_EVENTS;
use crate::{ServiceChange, ServiceChangeKind, ServiceInfo, control::WatchStatus};
use mojo_ipc::SequencedTaskRunnerHandle;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

type Reply = Box<dyn FnOnce(WatchStatus, u64, Vec<ServiceChange>) + Send + 'static>;
type Delivery = (Reply, WatchStatus, u64, Vec<ServiceChange>);

pub(crate) fn clone_info(info: &ServiceInfo) -> ServiceInfo {
    ServiceInfo {
        name: info.name.clone(),
        interface_name: info.interface_name.clone(),
        version: info.version,
        generation: info.generation,
        provider: info.provider.clone(),
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<ServiceChange>,
    revision: u64,
    overflow: bool,
    closed: bool,
    scheduled: bool,
    pending: Option<Reply>,
}

impl State {
    fn take_delivery(&mut self) -> Option<Delivery> {
        if self.pending.is_none() {
            return None;
        }
        let status = if self.closed {
            WatchStatus::Closed
        } else if self.overflow {
            WatchStatus::ResyncRequired
        } else if !self.queue.is_empty() {
            WatchStatus::Updates
        } else {
            return None;
        };
        let callback = self.pending.take().unwrap();
        Some((
            callback,
            status,
            self.revision,
            self.queue.drain(..).collect(),
        ))
    }
}

fn deliver(delivery: Option<Delivery>) {
    if let Some((reply, status, revision, changes)) = delivery {
        reply(status, revision, changes);
    }
}

/// Registry owns a strong reference while publishing under its mutex. Only a
/// weak reference enters the dispatch job, so rejection cannot destroy an
/// endpoint/callback under that mutex. Bindings/registry removals close outside it.
pub(crate) struct WatchBuffer {
    capacity: usize,
    state: Mutex<State>,
    runner: Option<SequencedTaskRunnerHandle>,
}

impl WatchBuffer {
    pub fn new(capacity: usize, runner: SequencedTaskRunnerHandle) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            state: Mutex::new(State::default()),
            runner: Some(runner),
        })
    }
    pub fn valid_capacity(&self) -> bool {
        self.capacity > 0 && self.capacity <= MAX_WATCH_EVENTS
    }

    pub fn push(self: &Arc<Self>, kind: ServiceChangeKind, info: &ServiceInfo) {
        let post = {
            let mut state = self.state.lock().unwrap();
            if state.closed {
                return;
            }
            match state.revision.checked_add(1) {
                Some(revision) => state.revision = revision,
                None => {
                    state.overflow = true;
                    state.queue.clear();
                }
            }
            if !state.overflow {
                if state.queue.len() == self.capacity {
                    state.queue.clear();
                    state.overflow = true;
                } else {
                    let revision = state.revision;
                    state.queue.push_back(ServiceChange {
                        kind,
                        service: clone_info(info),
                        revision,
                    });
                }
            }
            if state.pending.is_some() && !state.scheduled {
                state.scheduled = true;
                true
            } else {
                false
            }
        };
        if post {
            if let Some(runner) = &self.runner {
                let weak = Arc::downgrade(self);
                if !runner.post_task(move || {
                    if let Some(buffer) = weak.upgrade() {
                        buffer.flush();
                    }
                }) {
                    self.state.lock().unwrap().scheduled = false;
                }
            }
        }
    }

    pub fn next(&self, reply: Reply) {
        let delivery = {
            let mut state = self.state.lock().unwrap();
            if state.pending.is_some() {
                Some((reply, WatchStatus::Busy, state.revision, Vec::new()))
            } else {
                state.pending = Some(reply);
                state.take_delivery()
            }
        };
        deliver(delivery);
    }

    pub fn flush(&self) {
        let delivery = {
            let mut state = self.state.lock().unwrap();
            state.scheduled = false;
            state.take_delivery()
        };
        deliver(delivery);
    }

    pub fn close(&self) {
        let (delivery, discarded) = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            let discarded = std::mem::take(&mut state.queue);
            (state.take_delivery(), discarded)
        };
        drop(discarded);
        deliver(delivery);
    }

    #[cfg(test)]
    pub fn for_test(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            state: Mutex::new(State::default()),
            runner: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    fn info(generation: u64) -> ServiceInfo {
        ServiceInfo {
            name: "echo".into(),
            interface_name: "demo.Echo".into(),
            version: 1,
            generation,
            provider: "app".into(),
        }
    }

    #[test]
    fn batches_are_ordered_and_push_does_not_invoke_reply_inline() {
        let buffer = WatchBuffer::for_test(4);
        let (tx, rx) = mpsc::channel();
        buffer.next(Box::new(move |status, revision, changes| {
            tx.send((status, revision, changes)).unwrap();
        }));
        buffer.push(ServiceChangeKind::Added, &info(7));
        buffer.push(ServiceChangeKind::Removed, &info(7));
        assert!(rx.try_recv().is_err());
        buffer.flush();
        let (status, revision, changes) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(status, WatchStatus::Updates);
        assert_eq!(revision, 2);
        assert_eq!(
            changes.iter().map(|c| c.revision).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(changes[1].kind, ServiceChangeKind::Removed);
    }

    #[test]
    fn overflow_is_terminal_until_resubscribe_and_does_not_leak_partial_history() {
        let buffer = WatchBuffer::for_test(1);
        for generation in 1..=3 {
            buffer.push(ServiceChangeKind::Added, &info(generation));
        }
        let (tx, rx) = mpsc::channel();
        buffer.next(Box::new(move |status, revision, changes| {
            tx.send((status, revision, changes)).unwrap();
        }));
        let (status, revision, changes) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(status, WatchStatus::ResyncRequired);
        assert_eq!(revision, 3);
        assert!(changes.is_empty());
    }

    #[test]
    fn duplicate_next_preserves_original_waiter_and_close_is_reentrant() {
        let buffer = WatchBuffer::for_test(1);
        let (tx, rx) = mpsc::channel();
        let again = buffer.clone();
        buffer.next(Box::new(move |status, _, _| {
            again.close();
            tx.send(status).unwrap();
        }));
        let (tx, busy) = mpsc::channel();
        buffer.next(Box::new(move |status, _, _| {
            tx.send(status).unwrap();
        }));
        assert_eq!(
            busy.recv_timeout(Duration::from_secs(1)).unwrap(),
            WatchStatus::Busy
        );
        buffer.close();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WatchStatus::Closed
        );
    }
}
