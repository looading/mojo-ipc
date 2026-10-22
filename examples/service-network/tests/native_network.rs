use std::{
    io::Read,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn three_process_service_network_recovers_after_provider_crash() {
    run_fixture("broker");
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn directory_and_factory_disconnect_invalidate_discovery() {
    run_fixture("control-checks");
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn supervisor_bounds_retries_timeouts_and_shutdown() {
    run_fixture("supervisor-checks");
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn calls_bound_waiting_and_validate_late_responses() {
    run_fixture("call-checks");
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn subscriptions_bound_dynamic_discovery_and_resynchronize() {
    run_fixture("watch-checks");
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn health_checks_bound_unresponsive_processes_and_recover_discovery() {
    run_fixture("health-checks");
}

#[test]
#[ignore = "requires MOJO_RUNTIME_LIBRARY pointing to the trusted native SDK for this host"]
fn hard_broker_exit_closes_participant_stdio() {
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_service-network"))
            .args(["--role", "parent-death-check"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = collect(child.0.stdout.take().unwrap());
    let stderr = collect(child.0.stderr.take().unwrap());
    let status = child.wait(Duration::from_secs(25));
    // The dedicated participant is parked and ignores control disconnect. Its
    // inherited stderr stays open until the parent-liveness watcher exits it.
    let stdout = stdout
        .recv_timeout(Duration::from_secs(10))
        .expect("broker stdout stayed open")
        .unwrap();
    let stderr = stderr
        .recv_timeout(Duration::from_secs(10))
        .expect("participant survived broker death")
        .unwrap();
    assert_eq!(
        status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(String::from_utf8_lossy(&stdout).contains("participant admitted"));
}

fn collect(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream.read_to_end(&mut bytes).map(|_| bytes);
        let _ = tx.send(result);
    });
    rx
}

struct ChildGuard(Child);

impl ChildGuard {
    fn wait(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "service-network fixture exceeded its bounded waits"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_fixture(role: &str) {
    assert!(
        std::env::var_os("MOJO_RUNTIME_LIBRARY").is_some(),
        "set MOJO_RUNTIME_LIBRARY before running native tests"
    );
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_service-network"))
            .args(["--role", role])
            .spawn()
            .unwrap(),
    );
    let status = child.wait(Duration::from_secs(75));
    assert!(status.success(), "service-network exited with {status}");
}
