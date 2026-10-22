use super::{HealthPolicy, HealthStatus};
use crate::{
    control::HealthProbe,
    manager::{ManagedRemote, SharedRemote},
};
use std::{sync::mpsc, time::Instant};

type Reply = Result<bool, &'static str>;
struct StampedReply {
    received_at: Instant,
    result: Reply,
}

pub(super) struct ProbeTask<K> {
    receiver: mpsc::Receiver<StampedReply>,
    // Keep a replaced endpoint alive only until this sample is canceled.
    _keepalive: K,
}
struct Pending<K> {
    token: u64,
    task: ProbeTask<K>,
}

pub(super) struct Outcome {
    pub error: Option<&'static str>,
    pub failures: u32,
    pub checks: u64,
    pub newly_healthy: bool,
    pub fatal: bool,
}

/// No timer thread and no callback owns the monitor. Old replies can only write
/// into their bounded channel, never mutate a later attempt or replacement.
pub(super) struct HealthMonitor<K> {
    policy: HealthPolicy,
    next_due: Instant,
    deadline: Option<Instant>,
    pending: Option<Pending<K>>,
    status: HealthStatus,
    checks: u64,
    failures: u32,
}

impl<K> HealthMonitor<K> {
    pub fn new(policy: HealthPolicy, now: Instant) -> Self {
        Self {
            policy,
            next_due: now,
            deadline: None,
            pending: None,
            status: HealthStatus::Unknown,
            checks: 0,
            failures: 0,
        }
    }

    fn complete(&mut self, now: Instant, result: Reply) -> Outcome {
        self.pending = None;
        self.deadline = None;
        // Policy durations are validated. Saturating at now handles the remote
        // possibility that the clock range runs out after a long process life.
        self.next_due = now.checked_add(self.policy.interval).unwrap_or(now);
        self.checks = self.checks.saturating_add(1);
        let error = match result {
            Ok(true) => None,
            Ok(false) => Some("health probe returned unhealthy"),
            Err(e) => Some(e),
        };
        let newly_healthy = error.is_none() && self.status != HealthStatus::Healthy;
        if error.is_none() {
            self.failures = 0;
            self.status = HealthStatus::Healthy;
        } else {
            self.failures = self.failures.saturating_add(1);
            self.status = HealthStatus::Unhealthy;
        }
        Outcome {
            error,
            failures: self.failures,
            checks: self.checks,
            newly_healthy,
            fatal: self.failures >= self.policy.failure_threshold,
        }
    }

    pub fn poll(
        &mut self,
        now: Instant,
        target: Option<u64>,
        send: impl FnOnce() -> Result<ProbeTask<K>, &'static str>,
    ) -> Option<Outcome> {
        if now < self.next_due {
            return None;
        }
        let deadline = *self
            .deadline
            .get_or_insert_with(|| now.checked_add(self.policy.timeout).unwrap_or(now));
        if self
            .pending
            .as_ref()
            .is_some_and(|p| Some(p.token) != target)
        {
            // Preserve the current time window and failure history on replacement.
            self.pending = None;
        }
        if let Some(pending) = &self.pending {
            match pending.task.receiver.try_recv() {
                Ok(reply) => {
                    let result = if reply.received_at <= deadline {
                        reply.result
                    } else {
                        Err("health probe timed out")
                    };
                    return Some(self.complete(now, result));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Some(self.complete(now, Err("health probe disconnected")));
                }
                Err(mpsc::TryRecvError::Empty) if now >= deadline => {
                    return Some(self.complete(now, Err("health probe timed out")));
                }
                Err(mpsc::TryRecvError::Empty) => return None,
            }
        }
        if now >= deadline {
            return Some(self.complete(now, Err("health probe timed out")));
        }
        let Some(token) = target else {
            return Some(self.complete(now, Err("health probe unavailable")));
        };
        match send() {
            Ok(task) => {
                self.pending = Some(Pending { token, task });
                None
            }
            Err(error) => Some(self.complete(now, Err(error))),
        }
    }
}

struct ReplyGuard(Option<mpsc::SyncSender<StampedReply>>);
impl ReplyGuard {
    fn respond(mut self, healthy: bool) {
        if let Some(sender) = self.0.take() {
            let _ = sender.try_send(StampedReply {
                received_at: Instant::now(),
                result: Ok(healthy),
            });
        }
    }
}
impl Drop for ReplyGuard {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.try_send(StampedReply {
                received_at: Instant::now(),
                result: Err("health probe disconnected"),
            });
        }
    }
}

pub(super) fn start_probe(remote: SharedRemote) -> Result<ProbeTask<SharedRemote>, &'static str> {
    let (sender, receiver) = mpsc::sync_channel(1);
    {
        let mut locked = remote.lock().unwrap();
        let ManagedRemote::Health(probe) = &mut *locked else {
            return Err("invalid health endpoint");
        };
        if !probe.is_connected() {
            return Err("health probe disconnected");
        }
        // A timed-out callback remains a transport slot until reply/disconnect.
        // Never enqueue another check behind a still outstanding request.
        if probe.pending_response_count() != 0 {
            return Err("previous health probe still outstanding");
        }
        let reply = ReplyGuard(Some(sender));
        probe.Check(move |healthy| reply.respond(healthy));
    }
    Ok(ProbeTask {
        receiver,
        _keepalive: remote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    const STEP: Duration = Duration::from_millis(10);
    fn monitor(now: Instant) -> HealthMonitor<()> {
        HealthMonitor::new(
            HealthPolicy {
                interval: STEP,
                timeout: STEP,
                failure_threshold: 2,
            },
            now,
        )
    }
    fn task() -> (mpsc::SyncSender<StampedReply>, ProbeTask<()>) {
        let (tx, receiver) = mpsc::sync_channel(1);
        (
            tx,
            ProbeTask {
                receiver,
                _keepalive: (),
            },
        )
    }
    fn never() -> Result<ProbeTask<()>, &'static str> {
        panic!("must not send twice")
    }
    #[test]
    fn timely_reply_survives_late_host_poll_but_late_reply_times_out() {
        let now = Instant::now();
        let mut m = monitor(now);
        let (tx, task) = task();
        assert!(m.poll(now, Some(1), || Ok(task)).is_none());
        tx.send(StampedReply {
            received_at: now + STEP,
            result: Ok(true),
        })
        .unwrap();
        let out = m.poll(now + STEP * 3, Some(1), never).unwrap();
        assert!(out.newly_healthy && out.error.is_none());
        let (tx, task) = self::task();
        assert!(m.poll(now + STEP * 4, Some(1), || Ok(task)).is_none());
        tx.send(StampedReply {
            received_at: now + STEP * 6,
            result: Ok(true),
        })
        .unwrap();
        let out = m.poll(now + STEP * 7, Some(1), never).unwrap();
        assert_eq!(out.error, Some("health probe timed out"));
    }
    #[test]
    fn one_pending_check_and_timeout_do_not_repeat_before_interval() {
        let now = Instant::now();
        let mut m = monitor(now);
        let (_tx, task) = task();
        m.poll(now, Some(1), || Ok(task));
        assert!(m.poll(now + STEP / 2, Some(1), never).is_none());
        assert_eq!(m.poll(now + STEP, Some(1), never).unwrap().failures, 1);
        assert!(m.poll(now + STEP + STEP / 2, Some(1), never).is_none());
        let out = m
            .poll(now + STEP * 2, Some(1), || {
                Err("previous health probe still outstanding")
            })
            .unwrap();
        assert!(out.fatal);
        assert_eq!((out.failures, out.checks), (2, 2));
    }
    #[test]
    fn replacements_preserve_deadline_and_ignore_old_replies() {
        let now = Instant::now();
        let mut m = monitor(now);
        let (old, task) = task();
        m.poll(now, Some(1), || Ok(task));
        let (_new, task) = self::task();
        m.poll(now + STEP / 2, Some(2), || Ok(task));
        assert!(
            old.try_send(StampedReply {
                received_at: now,
                result: Ok(true)
            })
            .is_err()
        );
        assert_eq!(m.poll(now + STEP, Some(2), never).unwrap().failures, 1);
        let out = m.poll(now + STEP * 2, None, never).unwrap();
        assert!(out.fatal); // endpoint churn does not reset failures
    }
    #[test]
    fn success_resets_consecutive_failures_and_emits_only_transitions() {
        let now = Instant::now();
        let mut m = monitor(now);
        assert_eq!(m.poll(now, None, never).unwrap().failures, 1);
        for (i, newly) in [(1, true), (3, false)] {
            let (tx, task) = task();
            m.poll(now + STEP * i, Some(2), || Ok(task));
            tx.send(StampedReply {
                received_at: now + STEP * i,
                result: Ok(true),
            })
            .unwrap();
            let out = m.poll(now + STEP * (i + 1), Some(2), never).unwrap();
            assert_eq!(out.failures, 0);
            assert_eq!(out.newly_healthy, newly);
        }
    }
    #[test]
    fn callback_drop_reports_disconnect_without_waiting_for_timeout() {
        let (tx, task) = task();
        drop(ReplyGuard(Some(tx)));
        assert_eq!(
            task.receiver.try_recv().unwrap().result,
            Err("health probe disconnected")
        );
    }
}
