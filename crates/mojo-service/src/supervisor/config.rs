use crate::{AccessPolicy, ServiceKey};
use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

/// Automatic retries are bounded over the entire lifetime of one process spec.
/// `max_restarts` excludes the initial attempt. Backoff is fixed, including for
/// launch errors and startup readiness failures. Explicit stop never restarts.
#[derive(Clone, Debug, Default)]
pub enum RestartPolicy {
    #[default]
    Never,
    OnFailure {
        max_restarts: u32,
        backoff: Duration,
    },
    Always {
        max_restarts: u32,
        backoff: Duration,
    },
}

impl RestartPolicy {
    pub(super) fn delay(&self, success: bool, restarts: u32) -> Option<Duration> {
        match self {
            Self::Never => None,
            Self::OnFailure { .. } if success => None,
            Self::OnFailure {
                max_restarts,
                backoff,
            }
            | Self::Always {
                max_restarts,
                backoff,
            } if restarts < *max_restarts => Some(*backoff),
            _ => None,
        }
    }
}

/// Continuous checks are opt-in. Polling drives their scheduling; timeout is
/// measured from request submission and responses carry their receive time.
#[derive(Clone, Debug)]
pub struct HealthPolicy {
    pub interval: Duration,
    pub timeout: Duration,
    pub failure_threshold: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    Disabled,
    Unknown,
    Healthy,
    Unhealthy,
}

/// Required startup registration, checked against this exact admitted session.
#[derive(Clone, Debug)]
pub struct ServiceRequirement {
    pub key: ServiceKey,
    pub version: u32,
}

/// Host-controlled executable and supervision policy. Arguments may be non-UTF8;
/// stdin is reserved for the opt-in `watch_supervisor` parent-liveness helper.
#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub(super) identity: String,
    pub(super) program: PathBuf,
    pub(super) args: Vec<OsString>,
    pub(super) policy: AccessPolicy,
    pub(super) required: Vec<ServiceRequirement>,
    pub(super) restart: RestartPolicy,
    pub(super) startup_timeout: Duration,
    pub(super) shutdown_grace: Duration,
    pub(super) health: Option<HealthPolicy>,
}

impl ProcessSpec {
    pub fn new(
        identity: impl Into<String>,
        program: impl Into<PathBuf>,
        policy: AccessPolicy,
    ) -> Self {
        Self {
            identity: identity.into(),
            program: program.into(),
            args: Vec::new(),
            policy,
            required: Vec::new(),
            restart: RestartPolicy::Never,
            startup_timeout: Duration::from_secs(15),
            shutdown_grace: Duration::from_secs(1),
            health: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Require this session to register the exact key/version before the startup
    /// deadline. With no requirements or health checks, Ready means admission only; it does
    /// not establish an application handshake or business readiness.
    pub fn require(mut self, key: ServiceKey, version: u32) -> Self {
        self.required.push(ServiceRequirement { key, version });
        self
    }

    pub fn restart_policy(mut self, policy: RestartPolicy) -> Self {
        self.restart = policy;
        self
    }
    pub fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Require an explicitly registered HealthProbe before Ready, then check
    /// continuously. Threshold failures consume the existing restart budget.
    pub fn health_check(mut self, policy: HealthPolicy) -> Self {
        self.health = Some(policy);
        self
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        let valid_identity = !self.identity.is_empty()
            && self.identity.len() <= 256
            && !self.identity.chars().any(char::is_control);
        if !valid_identity || self.program.as_os_str().is_empty() || self.startup_timeout.is_zero()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "nonempty identity/program and nonzero startup timeout required",
            ));
        }
        if self
            .required
            .iter()
            .any(|r| !r.key.is_valid() || !self.policy.can_publish(&r.key))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "startup services require valid keys and publish permission",
            ));
        }
        let mut keys = std::collections::BTreeSet::new();
        if self.required.iter().any(|r| !keys.insert(&r.key)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "duplicate startup service key",
            ));
        }
        if let Some(health) = &self.health {
            if health.failure_threshold == 0
                || [health.interval, health.timeout]
                    .into_iter()
                    .any(|d| d.is_zero() || Instant::now().checked_add(d).is_none())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "health interval/timeout must be nonzero and representable, threshold must be positive",
                ));
            }
        }
        let backoff = match self.restart {
            RestartPolicy::Never => Duration::ZERO,
            RestartPolicy::OnFailure { backoff, .. } | RestartPolicy::Always { backoff, .. } => {
                backoff
            }
        };
        if [self.startup_timeout, self.shutdown_grace, backoff]
            .into_iter()
            .any(|d| Instant::now().checked_add(d).is_none())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "supervision duration exceeds the monotonic clock range",
            ));
        }
        Ok(())
    }
}

/// Identifies a process spec within its owning supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessId(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Queued,
    Starting,
    Ready,
    Stopping,
    Backoff,
    Stopped,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ProcessSnapshot {
    pub id: ProcessId,
    pub identity: String,
    pub state: ProcessState,
    pub pid: Option<u32>,
    pub attempts: u64,
    pub restarts: u32,
    pub health: HealthStatus,
    /// Completed samples in the current launch attempt.
    pub health_checks: u64,
    /// Consecutive failed samples, reset by a successful check or new attempt.
    pub health_failures: u32,
    /// Retained after recovery for diagnostics; not a current health indicator.
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProcessEvent {
    pub id: ProcessId,
    pub kind: ProcessEventKind,
}

#[derive(Clone, Debug)]
pub enum ProcessEventKind {
    Started { pid: u32, attempt: u64 },
    Ready,
    Healthy,
    HealthCheckFailed { failures: u32, error: String },
    Exited { code: Option<i32>, success: bool },
    AttemptFailed { error: String },
    RestartScheduled { restart: u32, delay: Duration },
    Failed,
    Stopped { forced: bool },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_is_optional_and_rejects_unbounded_or_zero_settings() {
        let base = ProcessSpec::new("provider", "program", AccessPolicy::new());
        assert!(base.health.is_none());
        for (interval, timeout, failure_threshold) in [
            (Duration::ZERO, Duration::from_secs(1), 1),
            (Duration::from_secs(1), Duration::ZERO, 1),
            (Duration::from_secs(1), Duration::MAX, 1),
            (Duration::MAX, Duration::from_secs(1), 1),
            (Duration::from_secs(1), Duration::from_secs(1), 0),
        ] {
            assert!(
                base.clone()
                    .health_check(HealthPolicy {
                        interval,
                        timeout,
                        failure_threshold
                    })
                    .validate()
                    .is_err()
            );
        }
        assert!(
            base.health_check(HealthPolicy {
                interval: Duration::from_secs(1),
                timeout: Duration::from_secs(1),
                failure_threshold: 2
            })
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn failure_retries_obey_fixed_backoff_and_lifetime_budget() {
        let policy = RestartPolicy::OnFailure {
            max_restarts: 2,
            backoff: Duration::from_secs(3),
        };
        assert_eq!(policy.delay(false, 0), Some(Duration::from_secs(3)));
        assert_eq!(policy.delay(false, 1), Some(Duration::from_secs(3)));
        assert_eq!(policy.delay(false, 2), None);
        assert_eq!(policy.delay(true, 0), None);
        assert_eq!(RestartPolicy::Never.delay(false, 0), None);
    }
    #[test]
    fn always_can_restart_success_but_zero_budget_cannot_restart() {
        let policy = RestartPolicy::Always {
            max_restarts: 1,
            backoff: Duration::ZERO,
        };
        assert_eq!(policy.delay(true, 0), Some(Duration::ZERO));
        assert_eq!(policy.delay(true, 1), None);
        assert_eq!(
            RestartPolicy::OnFailure {
                max_restarts: 0,
                backoff: Duration::ZERO
            }
            .delay(false, 0),
            None
        );
    }
    #[test]
    fn rejects_unpublishable_readiness_and_invalid_deadlines() {
        let base = ProcessSpec::new("provider", "program", AccessPolicy::new());
        assert!(
            base.clone()
                .require(ServiceKey::new("echo", "demo.Echo"), 1)
                .validate()
                .is_err()
        );
        assert!(
            base.clone()
                .startup_timeout(Duration::ZERO)
                .validate()
                .is_err()
        );
        assert!(
            base.clone()
                .shutdown_grace(Duration::MAX)
                .validate()
                .is_err()
        );
        assert!(
            base.clone()
                .restart_policy(RestartPolicy::Always {
                    max_restarts: 1,
                    backoff: Duration::MAX
                })
                .validate()
                .is_err()
        );
        assert!(base.validate().is_ok());
    }

    #[test]
    fn rejects_duplicate_readiness_keys_including_conflicting_versions() {
        let key = ServiceKey::new("echo", "demo.Echo");
        for second_version in [1, 2] {
            let spec = ProcessSpec::new(
                "provider",
                "program",
                AccessPolicy::new().allow_publish(key.clone()),
            )
            .require(key.clone(), 1)
            .require(key.clone(), second_version);
            assert_eq!(
                spec.validate().unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
