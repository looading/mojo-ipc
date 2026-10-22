mod child;
mod config;
mod health;
mod participant;

use crate::ServiceManager;
use crate::manager::SharedRemote;
use child::ManagedChild;
pub use config::*;
use health::{HealthMonitor, start_probe};
use mojo_ipc::Runtime;
pub use participant::{
    SUPERVISOR_ATTEMPT_ENV, SUPERVISOR_SERVER_ENV, connect_to_supervisor, watch_supervisor,
};
use std::{
    collections::BTreeMap,
    io,
    process::ExitStatus,
    time::{Duration, Instant},
};

struct Process {
    spec: ProcessSpec,
    snapshot: ProcessSnapshot,
    child: Option<ManagedChild>,
    due: Instant,
    startup_deadline: Instant,
    health: Option<HealthMonitor<SharedRemote>>,
}

impl Process {
    fn event(&self, kind: ProcessEventKind, events: &mut Vec<ProcessEvent>) {
        events.push(ProcessEvent {
            id: self.snapshot.id,
            kind,
        });
    }

    fn finish(&mut self, success: bool, error: Option<String>, events: &mut Vec<ProcessEvent>) {
        // All callers remove/reap the old child before considering a retry.
        debug_assert!(self.child.is_none());
        self.health = None;
        self.snapshot.pid = None;
        if let Some(error) = error {
            self.snapshot.last_error = Some(error.clone());
            self.event(ProcessEventKind::AttemptFailed { error }, events);
        }
        if let Some(delay) = self.spec.restart.delay(success, self.snapshot.restarts) {
            if let Some(due) = Instant::now().checked_add(delay) {
                self.snapshot.state = ProcessState::Backoff;
                self.due = due;
                self.event(
                    ProcessEventKind::RestartScheduled {
                        restart: self.snapshot.restarts + 1,
                        delay,
                    },
                    events,
                );
                return;
            }
            self.snapshot.last_error =
                Some("restart deadline exceeds the monotonic clock range".into());
        }
        self.snapshot.state = if success {
            ProcessState::Stopped
        } else {
            ProcessState::Failed
        };
        self.event(
            if success {
                ProcessEventKind::Stopped { forced: false }
            } else {
                ProcessEventKind::Failed
            },
            events,
        );
    }

    fn exited(&mut self, status: ExitStatus, events: &mut Vec<ProcessEvent>) {
        self.event(
            ProcessEventKind::Exited {
                code: status.code(),
                success: status.success(),
            },
            events,
        );
    }

    fn fail_live_child(&mut self, reason: &str, events: &mut Vec<ProcessEvent>) {
        self.fail_live_child_with_grace(reason, Duration::ZERO, events);
    }

    fn fail_live_child_with_grace(
        &mut self,
        reason: &str,
        grace: Duration,
        events: &mut Vec<ProcessEvent>,
    ) {
        self.health = None;
        match self.child.as_mut().unwrap().stop(grace) {
            Ok((status, _)) => {
                self.exited(status, events);
                self.health = None;
                self.child.take();
                self.finish(false, Some(reason.to_owned()), events);
            }
            Err(error) => {
                self.quarantine(format!("{reason}; child cleanup failed: {error}"), events)
            }
        }
    }

    fn quarantine(&mut self, error: String, events: &mut Vec<ProcessEvent>) {
        // Retain an unconfirmed child handle; never launch a replacement while
        // cleanup is uncertain. Explicit stop can retry; Drop still owns it.
        self.health = None;
        self.snapshot.state = ProcessState::Failed;
        self.snapshot.last_error = Some(error.clone());
        self.event(ProcessEventKind::AttemptFailed { error }, events);
        self.event(ProcessEventKind::Failed, events);
    }

    fn poll(
        &mut self,
        runtime: &Runtime,
        manager: &ServiceManager,
        events: &mut Vec<ProcessEvent>,
    ) {
        let now = Instant::now();
        if matches!(
            self.snapshot.state,
            ProcessState::Stopped | ProcessState::Failed
        ) {
            return;
        }
        if matches!(
            self.snapshot.state,
            ProcessState::Queued | ProcessState::Backoff
        ) {
            if now < self.due {
                return;
            }
            if self.snapshot.state == ProcessState::Backoff {
                self.snapshot.restarts += 1;
            }
            self.snapshot.attempts += 1;
            self.snapshot.health_checks = 0;
            self.snapshot.health_failures = 0;
            self.snapshot.health = if self.spec.health.is_some() {
                HealthStatus::Unknown
            } else {
                HealthStatus::Disabled
            };
            self.health = self.spec.health.clone().map(|p| HealthMonitor::new(p, now));
            let Some(deadline) = now.checked_add(self.spec.startup_timeout) else {
                self.quarantine(
                    "startup deadline exceeds the monotonic clock range".into(),
                    events,
                );
                return;
            };
            match ManagedChild::launch(
                runtime,
                manager,
                &self.spec,
                self.snapshot.attempts,
                deadline,
            ) {
                Ok(child) => {
                    let pid = child.child.id();
                    self.child = Some(child);
                    self.snapshot.pid = Some(pid);
                    self.snapshot.state = ProcessState::Starting;
                    self.startup_deadline = deadline;
                    self.event(
                        ProcessEventKind::Started {
                            pid,
                            attempt: self.snapshot.attempts,
                        },
                        events,
                    );
                }
                Err(error) => self.finish(
                    false,
                    Some(format!("process launch failed: {error}")),
                    events,
                ),
            }
            return;
        }
        match self.child.as_mut().unwrap().try_wait() {
            Ok(Some(status)) => {
                self.exited(status, events);
                self.health = None;
                self.child.take();
                self.finish(
                    status.success(),
                    (!status.success()).then(|| format!("process exited: {status}")),
                    events,
                );
                return;
            }
            Err(error) => {
                self.quarantine(format!("cannot inspect child exit: {error}"), events);
                return;
            }
            Ok(None) => {}
        }
        let child = self.child.as_mut().unwrap();
        let connection = child.connection.as_ref().unwrap();
        if !connection.is_open() {
            let lost = *child.control_lost_at.get_or_insert(now);
            self.health = None;
            self.snapshot.state = ProcessState::Stopping;
            if now.saturating_duration_since(lost) >= self.spec.shutdown_grace {
                self.fail_live_child("control connection closed but process did not exit", events);
            }
            return;
        }
        if self.snapshot.state == ProcessState::Starting {
            if connection.has_services_by(
                self.spec.required.iter().map(|r| (&r.key, r.version)),
                self.startup_deadline,
                self.spec.health.is_some(),
            ) {
                self.snapshot.state = ProcessState::Ready;
                self.event(ProcessEventKind::Ready, events);
            } else if now >= self.startup_deadline {
                self.fail_live_child("startup registration readiness timed out", events);
            }
            return;
        }
        if let Some(health) = &mut self.health {
            let target = connection.health_probe();
            let outcome = health.poll(now, target.as_ref().map(|t| t.0), || {
                start_probe(target.unwrap().1)
            });
            if let Some(outcome) = outcome {
                self.snapshot.health_checks = outcome.checks;
                self.snapshot.health_failures = outcome.failures;
                self.snapshot.health = if outcome.error.is_none() {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                };
                if let Some(error) = outcome.error {
                    self.snapshot.last_error = Some(error.into());
                    self.event(
                        ProcessEventKind::HealthCheckFailed {
                            failures: outcome.failures,
                            error: error.into(),
                        },
                        events,
                    );
                    if outcome.fatal {
                        self.fail_live_child_with_grace(error, self.spec.shutdown_grace, events);
                    }
                } else if outcome.newly_healthy {
                    self.event(ProcessEventKind::Healthy, events);
                }
            }
        }
    }
}

/// Host-driven supervision, independent of Tokio/UI runtimes. Call `poll` on a
/// blocking-capable host thread: process bootstrap may block up to its startup
/// timeout. Never call poll/stop on the native dispatch sequence. `add` queues
/// work; only polling performs launches, exit checks, readiness checks and retries.
/// Dropping the supervisor closes admission and kills/reaps direct children.
pub struct ProcessSupervisor {
    runtime: Runtime,
    manager: ServiceManager,
    processes: BTreeMap<ProcessId, Process>,
    next_id: u64,
}

impl ProcessSupervisor {
    pub fn new(runtime: &Runtime, manager: &ServiceManager) -> Self {
        Self {
            runtime: runtime.clone(),
            manager: manager.clone(),
            processes: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add(&mut self, spec: ProcessSpec) -> io::Result<ProcessId> {
        spec.validate()?;
        let id = ProcessId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("process identifier space exhausted"))?;
        let now = Instant::now();
        let snapshot = ProcessSnapshot {
            id,
            identity: spec.identity.clone(),
            state: ProcessState::Queued,
            pid: None,
            attempts: 0,
            restarts: 0,
            health: if spec.health.is_some() {
                HealthStatus::Unknown
            } else {
                HealthStatus::Disabled
            },
            health_checks: 0,
            health_failures: 0,
            last_error: None,
        };
        self.processes.insert(
            id,
            Process {
                spec,
                snapshot,
                child: None,
                due: now,
                startup_deadline: now,
                health: None,
            },
        );
        Ok(id)
    }

    fn host_thread(&self) -> io::Result<()> {
        if self
            .runtime
            .dispatch_runner()
            .runs_tasks_in_current_sequence()
        {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "process supervision must not block the native dispatch sequence",
            ))
        } else {
            Ok(())
        }
    }

    pub fn poll(&mut self) -> io::Result<Vec<ProcessEvent>> {
        self.host_thread()?;
        let mut events = Vec::new();
        for process in self.processes.values_mut() {
            process.poll(&self.runtime, &self.manager, &mut events);
        }
        Ok(events)
    }

    pub fn snapshot(&self, id: ProcessId) -> Option<ProcessSnapshot> {
        self.processes.get(&id).map(|p| p.snapshot.clone())
    }

    /// Disable retries and close admission, allow shutdown grace, then kill/reap.
    /// On cleanup error the child remains owned and the call can be retried.
    pub fn stop(&mut self, id: ProcessId) -> io::Result<ProcessEvent> {
        self.host_thread()?;
        let process = self
            .processes
            .get_mut(&id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown supervised process"))?;
        process.health = None;
        process.snapshot.health = if process.spec.health.is_some() {
            HealthStatus::Unknown
        } else {
            HealthStatus::Disabled
        };
        process.snapshot.state = ProcessState::Stopped;
        let forced = if let Some(child) = &mut process.child {
            match child.stop(process.spec.shutdown_grace) {
                Ok((_, forced)) => forced,
                Err(error) => {
                    process.snapshot.state = ProcessState::Failed;
                    process.snapshot.last_error = Some(format!("explicit stop failed: {error}"));
                    return Err(error);
                }
            }
        } else {
            false
        };
        process.child.take();
        process.snapshot.pid = None;
        Ok(ProcessEvent {
            id,
            kind: ProcessEventKind::Stopped { forced },
        })
    }

    /// Attempt every stop even when another child's cleanup reports an error.
    pub fn stop_all(&mut self) -> io::Result<()> {
        self.host_thread()?;
        let ids: Vec<_> = self.processes.keys().copied().collect();
        let mut first_error = None;
        for id in ids {
            if let Err(error) = self.stop(id) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
