//! Real processes exercise the reusable supervisor's failure paths.
use crate::{Result, key};
use mojo_ipc::Runtime;
use mojo_service::{
    AccessPolicy, ProcessEventKind, ProcessId, ProcessSnapshot, ProcessSpec, ProcessState,
    ProcessSupervisor, RestartPolicy, ServiceManager,
};
use std::{
    thread,
    time::{Duration, Instant},
};

fn spec(role: &str) -> Result<ProcessSpec> {
    Ok(ProcessSpec::new(
        role,
        std::env::current_exe()?,
        AccessPolicy::new()
            .allow_publish(key())
            .allow_publish(crate::private_key()),
    )
    .args(["--role", role])
    .startup_timeout(Duration::from_secs(1))
    .shutdown_grace(Duration::from_millis(50)))
}

fn until(
    supervisor: &mut ProcessSupervisor,
    id: ProcessId,
    predicate: impl Fn(&ProcessSnapshot) -> bool,
) -> Result<ProcessSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        supervisor.poll()?;
        let snapshot = supervisor.snapshot(id).unwrap();
        if predicate(&snapshot) {
            return Ok(snapshot);
        }
        if Instant::now() >= deadline {
            return Err(format!("supervision state timed out: {snapshot:?}").into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn run(runtime: &Runtime) -> Result<()> {
    let manager = ServiceManager::new(runtime);
    let mut supervisor = ProcessSupervisor::new(runtime, &manager);
    let cancelled = supervisor.add(spec("bootstrap-hang")?)?;
    supervisor.stop(cancelled)?;
    supervisor.poll()?;
    assert_eq!(supervisor.snapshot(cancelled).unwrap().attempts, 0);
    let missing = supervisor.add(
        ProcessSpec::new(
            "missing-executable",
            std::env::temp_dir().join(format!("mojo-nonexistent-{}", std::process::id())),
            AccessPolicy::new(),
        )
        .restart_policy(RestartPolicy::OnFailure {
            max_restarts: 1,
            backoff: Duration::ZERO,
        }),
    )?;
    let snapshot = until(&mut supervisor, missing, |s| {
        s.state == ProcessState::Failed
    })?;
    assert_eq!(
        (snapshot.attempts, snapshot.restarts, snapshot.pid),
        (2, 1, None)
    );
    assert!(snapshot.last_error.unwrap().contains("launch failed"));
    let crash = supervisor.add(
        spec("crash-loop")?.restart_policy(RestartPolicy::OnFailure {
            max_restarts: 2,
            backoff: Duration::from_millis(50),
        }),
    )?;
    let snapshot = until(&mut supervisor, crash, |s| s.state == ProcessState::Failed)?;
    assert_eq!(
        (snapshot.attempts, snapshot.restarts, snapshot.pid),
        (3, 2, None)
    );

    let clean = supervisor.add(
        spec("clean-exit")?.restart_policy(RestartPolicy::OnFailure {
            max_restarts: 2,
            backoff: Duration::ZERO,
        }),
    )?;
    let snapshot = until(&mut supervisor, clean, |s| s.state == ProcessState::Stopped)?;
    assert_eq!((snapshot.attempts, snapshot.restarts), (1, 0));

    let pending = supervisor.add(spec("crash-loop")?.restart_policy(RestartPolicy::Always {
        max_restarts: 3,
        backoff: Duration::from_millis(100),
    }))?;
    until(&mut supervisor, pending, |s| {
        s.state == ProcessState::Backoff
    })?;
    supervisor.stop(pending)?;
    thread::sleep(Duration::from_millis(150));
    supervisor.poll()?;
    let snapshot = supervisor.snapshot(pending).unwrap();
    assert_eq!(
        (snapshot.state, snapshot.attempts, snapshot.restarts),
        (ProcessState::Stopped, 1, 0)
    );

    let silent = supervisor.add(spec("silent")?.require(key(), 1))?;
    let snapshot = until(&mut supervisor, silent, |s| s.state == ProcessState::Failed)?;
    assert!(snapshot.last_error.unwrap().contains("readiness timed out"));
    assert!(snapshot.pid.is_none());

    let bootstrap = supervisor.add(spec("bootstrap-hang")?.require(key(), 1))?;
    let snapshot = until(&mut supervisor, bootstrap, |s| {
        s.state == ProcessState::Failed
    })?;
    // Named bootstrap can queue an invitation before the participant accepts it
    // on some platforms. Require application registration to bound that case too.
    let error = snapshot.last_error.unwrap();
    assert!(error.contains("launch failed") || error.contains("readiness timed out"));
    assert!(snapshot.pid.is_none());

    let control = supervisor.add(spec("close-control")?)?;
    let snapshot = until(&mut supervisor, control, |s| {
        s.state == ProcessState::Failed
    })?;
    assert!(snapshot.last_error.unwrap().contains("control connection"));
    assert!(snapshot.pid.is_none());

    let stubborn = supervisor.add(spec("silent")?)?;
    until(&mut supervisor, stubborn, |s| {
        s.state == ProcessState::Ready
    })?;
    let stopped = supervisor.stop(stubborn)?;
    assert!(matches!(
        stopped.kind,
        ProcessEventKind::Stopped { forced: true }
    ));
    assert!(supervisor.snapshot(stubborn).unwrap().pid.is_none());

    // Delayed host polling must recognize registrations made before deadline.
    let timely = supervisor.add(spec("provider")?.require(key(), 1))?;
    supervisor.poll()?;
    thread::sleep(Duration::from_millis(1200));
    supervisor.poll()?;
    assert_eq!(
        supervisor.snapshot(timely).unwrap().state,
        ProcessState::Ready
    );
    supervisor.stop(timely)?;
    // Conversely, a registration actually made after deadline cannot satisfy it.
    let late = supervisor.add(spec("late-provider")?.require(key(), 1))?;
    supervisor.poll()?;
    thread::sleep(Duration::from_millis(2000));
    supervisor.poll()?;
    let snapshot = supervisor.snapshot(late).unwrap();
    assert_eq!(snapshot.state, ProcessState::Failed);
    assert!(snapshot.last_error.unwrap().contains("readiness timed out"));
    supervisor.stop_all()?;
    println!(
        "PASS supervisor: restart budget, clean exit, cancelled backoff, startup deadlines, control loss, forced stop and registration timestamps"
    );
    Ok(())
}
