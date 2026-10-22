//! Real health endpoints, hung dispatch sequence, restart and discovery recovery.
use crate::{Result, TIMEOUT, discovery::subscribe, key, private_key, replacement_key};
use mojo_ipc::{Runtime, system::message_pipe::MessageEndpoint};
use mojo_service::{
    AccessPolicy, HealthPolicy, HealthStatus, ProcessEvent, ProcessEventKind, ProcessId,
    ProcessSnapshot, ProcessSpec, ProcessState, ProcessSupervisor, RestartPolicy,
    ServiceChangeKind, ServiceDirectory, ServiceManager,
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
            .allow_publish(private_key())
            .allow_publish(replacement_key()),
    )
    .args(["--role", role])
    .require(key(), 1)
    .startup_timeout(Duration::from_secs(2))
    .shutdown_grace(Duration::from_millis(100))
    .health_check(HealthPolicy {
        interval: Duration::from_millis(50),
        timeout: Duration::from_millis(200),
        failure_threshold: 2,
    }))
}

fn until(
    supervisor: &mut ProcessSupervisor,
    id: ProcessId,
    predicate: impl Fn(&ProcessSnapshot) -> bool,
) -> Result<(ProcessSnapshot, Vec<ProcessEvent>)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        events.extend(supervisor.poll()?.into_iter().filter(|e| e.id == id));
        let snapshot = supervisor.snapshot(id).unwrap();
        if predicate(&snapshot) {
            return Ok((snapshot, events));
        }
        if Instant::now() >= deadline {
            return Err(format!("health state timed out: {snapshot:?}").into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn replacement_watch(
    runtime: &Runtime,
    manager: &ServiceManager,
) -> Result<(
    ServiceDirectory,
    mojo_service::ManagerConnection,
    mojo_service::ServiceSubscription,
)> {
    let (client, server) = MessageEndpoint::create_pipe()?;
    let lease = manager.attach(
        "replacement-viewer",
        AccessPolicy::new().allow_connect(replacement_key()),
        server,
    );
    let mut directory = ServiceDirectory::bind(runtime, client, None);
    let watch = subscribe(&mut directory, 4)?;
    assert!(watch.initial_services().is_empty());
    Ok((directory, lease, watch))
}

fn replaced(watch: &mut mojo_service::ServiceSubscription) -> Result<()> {
    let batch = watch.recv_timeout(TIMEOUT)?;
    assert_eq!(batch.changes.len(), 1);
    assert_eq!(batch.changes[0].kind, ServiceChangeKind::Added);
    assert_eq!(batch.changes[0].service.name, replacement_key().name);
    Ok(())
}

pub fn run(runtime: &Runtime) -> Result<()> {
    let manager = ServiceManager::new(runtime);
    let mut supervisor = ProcessSupervisor::new(runtime, &manager);
    for role in ["provider", "health-transient", "health-replace"] {
        let mut marker = if role == "health-replace" {
            Some(replacement_watch(runtime, &manager)?)
        } else {
            None
        };
        let id = supervisor.add(spec(role)?)?;
        let (s, events) = until(&mut supervisor, id, |s| {
            s.health_checks >= if role == "health-replace" { 8 } else { 3 }
        })?;
        if let Some((_, _, watch)) = &mut marker {
            replaced(watch)?;
        }
        assert_eq!(s.state, ProcessState::Ready);
        assert_eq!(
            (s.health, s.health_failures, s.attempts),
            (HealthStatus::Healthy, 0, 1)
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, ProcessEventKind::Healthy))
                .count(),
            if role == "health-replace" { 2 } else { 1 }
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, ProcessEventKind::HealthCheckFailed { .. }))
                .count(),
            usize::from(role != "provider")
        );
        let stopped = supervisor.stop(id)?;
        assert!(matches!(
            stopped.kind,
            ProcessEventKind::Stopped { forced: false }
        ));
        assert!(supervisor.snapshot(id).unwrap().pid.is_none());
    }

    // Enabled checks require an endpoint even if no business services are required.
    let missing = supervisor.add(
        ProcessSpec::new(
            "missing-probe",
            std::env::current_exe()?,
            AccessPolicy::new(),
        )
        .args(["--role", "silent"])
        .startup_timeout(Duration::from_millis(500))
        .health_check(HealthPolicy {
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(100),
            failure_threshold: 1,
        }),
    )?;
    let (s, _) = until(&mut supervisor, missing, |s| {
        s.state == ProcessState::Failed
    })?;
    assert!(s.last_error.unwrap().contains("readiness timed out"));
    assert_eq!((s.health_checks, s.pid), (0, None));

    let unhealthy = supervisor.add(spec("health-unhealthy")?.restart_policy(
        RestartPolicy::OnFailure {
            max_restarts: 1,
            backoff: Duration::from_millis(25),
        },
    ))?;
    let (s, events) = until(&mut supervisor, unhealthy, |s| {
        s.state == ProcessState::Failed
    })?;
    assert_eq!(
        (s.attempts, s.restarts, s.health_failures, s.pid),
        (2, 1, 2, None)
    );
    assert_eq!(s.health, HealthStatus::Unhealthy);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, ProcessEventKind::HealthCheckFailed { .. }))
            .count(),
        4
    );

    // Receive time, rather than the next host poll time, determines success.
    let timely = supervisor.add(spec("provider")?)?;
    until(&mut supervisor, timely, |s| s.state == ProcessState::Ready)?;
    supervisor.poll()?; // submit Check
    thread::sleep(Duration::from_millis(500));
    supervisor.poll()?;
    let s = supervisor.snapshot(timely).unwrap();
    assert_eq!(
        (s.health, s.health_checks, s.health_failures),
        (HealthStatus::Healthy, 1, 0)
    );
    supervisor.stop(timely)?;

    // Continuous registration survives a replacement after the startup deadline
    // even when the host has not polled to observe Ready yet.
    let (_marker_directory, _marker_lease, mut marker) = replacement_watch(runtime, &manager)?;
    let late_replace =
        supervisor.add(spec("health-late-replace")?.startup_timeout(Duration::from_secs(1)))?;
    supervisor.poll()?;
    replaced(&mut marker)?; // explicit acknowledgement after late replacement
    assert_eq!(
        supervisor.snapshot(late_replace).unwrap().state,
        ProcessState::Starting
    );
    supervisor.poll()?;
    assert_eq!(
        supervisor.snapshot(late_replace).unwrap().state,
        ProcessState::Ready
    );
    supervisor.stop(late_replace)?;

    let (client, server) = MessageEndpoint::create_pipe()?;
    let _viewer_lease = manager.attach("viewer", AccessPolicy::new().allow_connect(key()), server);
    let mut viewer = ServiceDirectory::bind(runtime, client, None);
    let mut watch = subscribe(&mut viewer, 16)?;
    assert!(watch.initial_services().is_empty());
    let hung = supervisor.add(spec("health-recover")?.restart_policy(
        RestartPolicy::OnFailure {
            max_restarts: 1,
            backoff: Duration::from_millis(25),
        },
    ))?;
    let (s, events) = until(&mut supervisor, hung, |s| {
        s.attempts == 2 && s.health == HealthStatus::Healthy
    })?;
    assert_eq!(
        (s.state, s.restarts, s.health_failures),
        (ProcessState::Ready, 1, 0)
    );
    let failures: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            ProcessEventKind::HealthCheckFailed { error, .. } => Some(error.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        failures,
        [
            "health probe timed out",
            "previous health probe still outstanding"
        ]
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, ProcessEventKind::Exited { success: false, .. }))
    );
    let mut changes = Vec::new();
    while changes.len() < 3 {
        changes.extend(watch.recv_timeout(TIMEOUT)?.changes);
    }
    assert_eq!(changes.len(), 3);
    assert_eq!(
        changes.iter().map(|c| c.kind.clone()).collect::<Vec<_>>(),
        [
            ServiceChangeKind::Added,
            ServiceChangeKind::Removed,
            ServiceChangeKind::Added
        ]
    );
    assert_eq!(changes[0].service.generation, changes[1].service.generation);
    assert_ne!(changes[0].service.generation, changes[2].service.generation);
    supervisor.stop(hung)?;
    supervisor.poll()?;
    assert_eq!(
        supervisor.snapshot(hung).unwrap().state,
        ProcessState::Stopped
    );
    supervisor.stop_all()?;
    println!(
        "PASS health: periodic checks, transient recovery, replacement, readiness, restart budget, receive timestamps and hung-process discovery recovery"
    );
    Ok(())
}
