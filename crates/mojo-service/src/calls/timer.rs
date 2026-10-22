use super::{
    CallError,
    state::{Call, CallResponder, CallState, ClientCalls, Pending},
};
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Condvar, Mutex, OnceLock, Weak},
    thread::{self, JoinHandle, ThreadId},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DeadlineKey {
    pub at: Instant,
    id: u64,
}

struct Timers {
    closed: bool,
    next_id: u64,
    pending: BTreeMap<DeadlineKey, Weak<dyn Pending>>,
}

pub(super) struct Scheduler {
    timers: Mutex<Timers>,
    changed: Condvar,
    thread_id: OnceLock<ThreadId>,
}

impl Scheduler {
    pub fn remove(&self, key: DeadlineKey) {
        self.timers.lock().unwrap().pending.remove(&key);
        self.changed.notify_one();
    }

    pub fn on_timer_thread(&self) -> bool {
        self.thread_id
            .get()
            .is_some_and(|id| *id == thread::current().id())
    }

    fn close(&self) {
        let pending = {
            let mut timers = self.timers.lock().unwrap();
            timers.closed = true;
            std::mem::take(&mut timers.pending)
        };
        self.changed.notify_all();
        for call in pending.into_values() {
            if let Some(call) = call.upgrade() {
                call.fail(CallError::ContextClosed);
            }
        }
    }

    fn run(&self) {
        let _ = self.thread_id.set(thread::current().id());
        loop {
            let call = {
                let mut timers = self.timers.lock().unwrap();
                loop {
                    if timers.closed {
                        return;
                    }
                    match timers.pending.first_key_value() {
                        Some((key, _)) if key.at <= Instant::now() => {
                            break timers.pending.pop_first().unwrap().1;
                        }
                        Some((key, _)) => {
                            let remaining = key.at.saturating_duration_since(Instant::now());
                            timers = self.changed.wait_timeout(timers, remaining).unwrap().0;
                        }
                        None => timers = self.changed.wait(timers).unwrap(),
                    }
                }
            };
            if let Some(call) = call.upgrade() {
                call.fail(CallError::TimedOut);
            }
        }
    }
}

struct Driver {
    scheduler: Arc<Scheduler>,
    _thread: JoinHandle<()>,
}
impl Drop for Driver {
    fn drop(&mut self) {
        // Signal and detach rather than join from a timer/dispatch callback.
        // The worker owns only Scheduler, not Driver, and exits on closed.
        self.scheduler.close();
    }
}

/// Shared deadline worker for multiple clients, independent of async executors.
/// One timer thread is created per context, not per request/connection. Explicit
/// shutdown resolves pending calls and signals worker exit; it does not join.
/// Last-owner drop also signals shutdown. Clients retain a clone while bound.
#[derive(Clone)]
pub struct CallContext {
    driver: Arc<Driver>,
}

impl CallContext {
    pub fn new() -> io::Result<Self> {
        let scheduler = Arc::new(Scheduler {
            timers: Mutex::new(Timers {
                closed: false,
                next_id: 1,
                pending: BTreeMap::new(),
            }),
            changed: Condvar::new(),
            thread_id: OnceLock::new(),
        });
        let worker = scheduler.clone();
        let thread = thread::Builder::new()
            .name("mojo-call-timer".into())
            .spawn(move || worker.run())?;
        Ok(Self {
            driver: Arc::new(Driver {
                scheduler,
                _thread: thread,
            }),
        })
    }

    pub fn shutdown(&self) {
        self.driver.scheduler.close();
    }
    pub fn is_shutdown(&self) -> bool {
        self.driver.scheduler.timers.lock().unwrap().closed
    }
    pub fn pending_deadlines(&self) -> usize {
        self.driver.scheduler.timers.lock().unwrap().pending.len()
    }

    pub(super) fn start<R: Send + 'static>(
        &self,
        clients: &Arc<ClientCalls>,
        timeout: Duration,
    ) -> Result<(Call<R>, CallResponder<R>), CallError> {
        if timeout.is_zero() {
            return Err(CallError::TimedOut);
        }
        let at = Instant::now()
            .checked_add(timeout)
            .ok_or(CallError::InvalidOptions)?;
        let scheduler = &self.driver.scheduler;
        let key = {
            let mut timers = scheduler.timers.lock().unwrap();
            if timers.closed {
                return Err(CallError::ContextClosed);
            }
            let id = timers.next_id;
            timers.next_id = id.checked_add(1).ok_or(CallError::AtCapacity)?;
            DeadlineKey { at, id }
        };
        let state = Arc::new(CallState::new(
            key,
            Arc::downgrade(scheduler),
            Arc::downgrade(clients),
        ));
        let pending: Arc<dyn Pending> = state.clone();
        // Insert client tracking before exposing the state to timer/shutdown.
        clients.insert(key, Arc::downgrade(&pending));
        {
            let mut timers = scheduler.timers.lock().unwrap();
            if timers.closed {
                drop(timers);
                state.fail(CallError::ContextClosed);
                return Err(CallError::ContextClosed);
            }
            timers.pending.insert(key, Arc::downgrade(&pending));
        }
        scheduler.changed.notify_one();
        let responder = CallResponder {
            state: Arc::downgrade(&state),
            answered: false,
        };
        Ok((Call { state }, responder))
    }
}
