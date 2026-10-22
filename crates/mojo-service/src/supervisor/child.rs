use super::{ProcessSpec, SUPERVISOR_ATTEMPT_ENV, SUPERVISOR_SERVER_ENV};
use crate::{ManagerConnection, ServiceManager};
use mojo_ipc::Runtime;
use std::{
    io,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT_CHANNEL: AtomicU64 = AtomicU64::new(1);

pub(super) struct ManagedChild {
    pub child: Child,
    pub connection: Option<ManagerConnection>,
    pub control_lost_at: Option<Instant>,
    reaped: bool,
}

impl ManagedChild {
    pub fn launch(
        runtime: &Runtime,
        manager: &ServiceManager,
        spec: &ProcessSpec,
        attempt: u64,
        deadline: Instant,
    ) -> io::Result<Self> {
        let nonce = NEXT_CHANNEL
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| io::Error::other("bootstrap name space exhausted"))?;
        let suffix = format!("mojo-service-{}-{nonce}", std::process::id());
        let name = if cfg!(target_os = "linux") {
            std::env::temp_dir()
                .join(suffix)
                .into_os_string()
                .into_string()
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "non-UTF8 temporary directory")
                })?
        } else {
            suffix
        };
        let mut listener = runtime.listen_once(&name).map_err(io::Error::other)?;
        let child = Command::new(&spec.program)
            .args(&spec.args)
            .env(SUPERVISOR_SERVER_ENV, &name)
            .env(SUPERVISOR_ATTEMPT_ENV, attempt.to_string())
            .stdin(Stdio::piped())
            .spawn()?;
        let mut child = Self {
            child,
            connection: None,
            control_lost_at: None,
            reaped: false,
        };
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "startup deadline elapsed during spawn",
                )
            })?;
        let pipe = listener
            .accept(u64::from(child.child.id()), remaining)
            .map_err(io::Error::other)?;
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "startup deadline elapsed during bootstrap",
            ));
        }
        child.connection = Some(manager.attach(&spec.identity, spec.policy.clone(), pipe));
        Ok(child)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        self.reaped |= status.is_some();
        Ok(status)
    }

    /// Close control admission first, allowing the application's disconnect
    /// handler to flush and exit. Keep its stdin lease until the grace expires.
    pub fn stop(&mut self, grace: Duration) -> io::Result<(ExitStatus, bool)> {
        self.connection.take();
        let deadline = Instant::now().checked_add(grace).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "shutdown grace overflow")
        })?;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok((status, false));
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        match self.child.kill() {
            Ok(()) => {
                let status = self.child.wait()?;
                self.reaped = true;
                Ok((status, true))
            }
            Err(error) => {
                if let Some(status) = self.try_wait()? {
                    Ok((status, false))
                } else {
                    Err(error)
                }
            }
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.connection.take();
        self.child.stdin.take();
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
