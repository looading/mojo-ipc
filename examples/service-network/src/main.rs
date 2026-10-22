//! Real broker/provider/consumer fixture. Requires a trusted native SDK image.
use mojo_ipc::{PendingReceiver, Role, Runtime};
use mojo_service::{
    AccessPolicy, CallClient, CallContext, CallError, HealthPolicy, ManagerConnection,
    ProcessEventKind, ProcessSpec, ProcessState, ProcessSupervisor, RestartPolicy,
    SUPERVISOR_ATTEMPT_ENV, SUPERVISOR_SERVER_ENV, ServiceConnection, ServiceDirectory,
    ServiceError, ServiceKey, ServiceManager, Status, bind_factory, bind_health,
    connect_to_supervisor, watch_supervisor,
};
use std::{
    error::Error,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
use echo::Echo;
mod call_checks;
mod control_checks;
mod discovery;
mod health_checks;
mod supervisor_checks;
mod watch_checks;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TIMEOUT: Duration = Duration::from_secs(15);

fn key() -> ServiceKey {
    ServiceKey::new("echo", "demo.services.Echo")
}
fn private_key() -> ServiceKey {
    ServiceKey::new("private", "demo.services.Echo")
}

fn replacement_key() -> ServiceKey {
    ServiceKey::new("health-replaced", "demo.services.Echo")
}

struct EchoService(u32);
#[allow(non_snake_case)]
impl Echo for EchoService {
    fn Ping(&mut self, text: String, callback: impl FnOnce(String, u32, u32) + Send + 'static) {
        callback(text, self.0, std::process::id());
    }
    fn Crash(&mut self, _callback: impl FnOnce() + Send + 'static) {
        std::process::exit(17);
    }
}
mojo_ipc::register_mojom_state_object_impls!(impl Echo for EchoService);

fn response<T: Send + 'static>(send: impl FnOnce(mpsc::Sender<T>)) -> Result<T> {
    let (tx, rx) = mpsc::channel();
    send(tx);
    Ok(rx.recv_timeout(TIMEOUT)?)
}

fn provider(
    runtime: &Runtime,
    pipe: mojo_ipc::system::message_pipe::MessageEndpoint,
    instance: u32,
    health_mode: &str,
) -> Result<()> {
    let (closed, disconnected) = mpsc::channel();
    let mut directory = ServiceDirectory::bind(
        runtime,
        pipe,
        Some(Box::new(move || {
            let _ = closed.send(());
        })),
    );
    let mode = health_mode.to_owned();
    let mut checks = 0;
    let (probe, _health) = bind_health(runtime, move || {
        checks += 1;
        if mode == "health-recover" && instance == 1 {
            // Deliberately block the participant sequence while the OS process
            // remains alive. The host must time out and reap this process.
            thread::sleep(Duration::from_secs(30));
        }
        mode != "health-unhealthy" && !(mode == "health-transient" && checks == 1)
    })?;
    response(|tx| {
        directory.set_health_probe(probe, move |result| {
            let _ = tx.send(result);
        })
    })??;
    if health_mode == "health-replace" {
        let (replacement, binding) = bind_health(runtime, || true)?;
        response(|tx| {
            directory.set_health_probe(replacement, move |result| {
                let _ = tx.send(result);
            })
        })??;
        // The old receiver's delayed disconnect must not remove its replacement.
        drop(_health);
        return serve_provider(
            runtime,
            directory,
            disconnected,
            instance,
            binding,
            Some(Duration::from_millis(200)),
        );
    }
    serve_provider(
        runtime,
        directory,
        disconnected,
        instance,
        _health,
        (health_mode == "health-late-replace").then_some(Duration::from_millis(1200)),
    )
}

fn serve_provider(
    runtime: &Runtime,
    mut directory: ServiceDirectory,
    disconnected: mpsc::Receiver<()>,
    instance: u32,
    _health: mojo_service::HealthBinding,
    replacement_delay: Option<Duration>,
) -> Result<()> {
    let mut factories = Vec::new();
    for service_key in [key(), private_key()] {
        let (factory, binding) = bind_factory(runtime, move |pipe| {
            // Factory callbacks already run on the SDK sequence. Self-owned
            // receivers release themselves when each consumer disconnects.
            PendingReceiver::<dyn Echo>::new(pipe).bind_self_owned(EchoService(instance));
        })?;
        let generation = response(|tx| {
            directory.register(&service_key, 1, factory, move |result| {
                let _ = tx.send(result);
            })
        })??;
        println!(
            "registered {} instance={instance} generation={generation}",
            service_key.name
        );
        factories.push(binding);
    }
    let _replacement = if let Some(delay) = replacement_delay {
        // Host polling continues for the live replacement test and is delayed
        // past the startup deadline for the late-registration evidence test.
        thread::sleep(delay);
        let live = delay < Duration::from_secs(1);
        let mut checks = 0;
        let (probe, binding) = bind_health(runtime, move || {
            checks += 1;
            !live || checks > 1
        })?;
        response(|tx| {
            directory.set_health_probe(probe, move |result| {
                let _ = tx.send(result);
            })
        })??;
        drop(_health);
        let (factory, marker) = bind_factory(runtime, drop)?;
        response(|tx| {
            directory.register(&replacement_key(), 1, factory, move |result| {
                let _ = tx.send(result);
            })
        })??;
        Some((binding, marker))
    } else {
        None
    };
    disconnected.recv_timeout(Duration::from_secs(45))?;
    Ok(())
}

fn connect(
    directory: &mut ServiceDirectory,
    service_key: &ServiceKey,
    version: u32,
) -> Result<std::result::Result<ServiceConnection<dyn Echo>, ServiceError>> {
    response(|tx| {
        directory.connect::<dyn Echo>(service_key, version, move |result| {
            let _ = tx.send(result);
        })
    })
}

fn consumer(
    runtime: &Runtime,
    pipe: mojo_ipc::system::message_pipe::MessageEndpoint,
) -> Result<()> {
    let mut directory = ServiceDirectory::bind(runtime, pipe, None);
    let mut subscription = discovery::subscribe(&mut directory, 64)?;
    let connection = discovery::connect(&mut directory, &mut subscription, 0)?;
    let services = response(|tx| {
        directory.list(move |services| {
            let _ = tx.send(services);
        })
    })?;
    assert_eq!(services.len(), 1, "private service must be hidden");
    assert_eq!(services[0].name, "echo");
    assert_eq!(services[0].provider, "provider");
    assert!(matches!(
        connect(&mut directory, &private_key(), 1)?,
        Err(ServiceError(Status::PermissionDenied))
    ));
    assert!(matches!(
        connect(&mut directory, &key(), 2)?,
        Err(ServiceError(Status::IncompatibleVersion))
    ));
    let (factory, _binding) = bind_factory(runtime, |_| {})?;
    let denied = response(|tx| {
        directory.register(&key(), 1, factory, move |result| {
            let _ = tx.send(result);
        })
    })?;
    assert_eq!(denied, Err(ServiceError(Status::PermissionDenied)));

    let first_generation = connection.generation;
    let denied = response(|tx| {
        directory.unregister(&key(), first_generation, move |result| {
            let _ = tx.send(result);
        })
    })?;
    assert_eq!(denied, Err(ServiceError(Status::PermissionDenied)));
    let calls = CallContext::new()?;
    let mut echo = CallClient::bind(runtime, &calls, connection.remote, 8)?;
    let (text, instance, first_pid) = echo
        .call(TIMEOUT, |remote, reply| {
            remote.Ping("before restart".into(), move |text, instance, pid| {
                reply.respond((text, instance, pid));
            });
        })?
        .wait()?;
    assert_eq!((text.as_str(), instance), ("before restart", 1));
    assert_ne!(first_pid, std::process::id());
    let crashed = echo.call(TIMEOUT, |remote, reply| {
        remote.Crash(move || {
            reply.respond(());
        })
    })?;
    assert_eq!(crashed.wait(), Err(CallError::Disconnected));
    drop(echo);

    let connection = discovery::connect(&mut directory, &mut subscription, first_generation)?;
    assert!(connection.generation > first_generation);
    let mut echo = CallClient::bind(runtime, &calls, connection.remote, 8)?;
    let (text, instance, second_pid) = echo
        .call(TIMEOUT, |remote, reply| {
            remote.Ping("after restart".into(), move |text, instance, pid| {
                reply.respond((text, instance, pid));
            });
        })?
        .wait()?;
    assert_eq!((text.as_str(), instance), ("after restart", 2));
    assert_ne!(first_pid, second_pid);
    println!(
        "PASS consumer: permissions, discovery, endpoint transfer, typed calls, disconnect and generation-based reconnect"
    );
    Ok(())
}

/// Named one-shot bootstrap lets the host retain real Child handles for kill /
/// wait cleanup on every failure path. It does not require a multi-client listener.
struct Worker {
    child: Child,
    connection: Option<ManagerConnection>,
}

impl Worker {
    fn start(
        runtime: &Runtime,
        manager: &ServiceManager,
        role: &str,
        instance: u32,
        policy: AccessPolicy,
    ) -> Result<Self> {
        let suffix = format!("mojo-service-{}-{role}-{instance}", std::process::id());
        let name = if cfg!(target_os = "linux") {
            std::env::temp_dir()
                .join(suffix)
                .to_str()
                .ok_or("non-UTF8 temporary directory")?
                .to_owned()
        } else {
            suffix
        };
        let mut listener = runtime.listen_once(&name)?;
        let mut command = Command::new(std::env::current_exe()?);
        command.env(SUPERVISOR_SERVER_ENV, &name);
        command.stdin(Stdio::piped());
        if role == "parked" {
            command.stdout(Stdio::piped());
        }
        let child = command
            .args([
                "--role",
                role,
                "--server",
                &name,
                "--instance",
                &instance.to_string(),
            ])
            .spawn()?;
        let mut worker = Self {
            child,
            connection: None,
        };
        let pipe = listener.accept(u64::from(worker.child.id()), TIMEOUT)?;
        worker.connection = Some(manager.attach(role, policy, pipe));
        Ok(worker)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.connection.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn broker(runtime: &Runtime) -> Result<()> {
    let manager = ServiceManager::new(runtime);
    let policy = AccessPolicy::new()
        .allow_publish(key())
        .allow_publish(private_key());
    let mut supervisor = ProcessSupervisor::new(runtime, &manager);
    let program = std::env::current_exe()?;
    let provider = supervisor.add(
        ProcessSpec::new("provider", &program, policy)
            .args(["--role", "provider"])
            .require(key(), 1)
            .health_check(HealthPolicy {
                interval: Duration::from_secs(1),
                timeout: Duration::from_millis(250),
                failure_threshold: 3,
            })
            .restart_policy(RestartPolicy::OnFailure {
                max_restarts: 1,
                backoff: Duration::from_millis(50),
            }),
    )?;
    let consumer = supervisor.add(
        ProcessSpec::new(
            "consumer",
            &program,
            AccessPolicy::new().allow_connect(key()),
        )
        .args(["--role", "consumer"]),
    )?;
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut provider_crashed = false;
    let mut consumer_succeeded = false;
    loop {
        for event in supervisor.poll()? {
            if let ProcessEventKind::Exited { code, success } = event.kind {
                if event.id == provider && code == Some(17) {
                    provider_crashed = true;
                }
                if event.id == consumer && success {
                    consumer_succeeded = true;
                }
            }
        }
        let p = supervisor.snapshot(provider).unwrap();
        let c = supervisor.snapshot(consumer).unwrap();
        assert_ne!(
            p.state,
            ProcessState::Failed,
            "provider failed: {:?}",
            p.last_error
        );
        assert_ne!(
            c.state,
            ProcessState::Failed,
            "consumer failed: {:?}",
            c.last_error
        );
        if c.state == ProcessState::Stopped {
            assert!(provider_crashed && consumer_succeeded);
            assert_eq!((p.attempts, p.restarts), (2, 1));
            break;
        }
        if Instant::now() >= deadline {
            return Err("supervised network timed out".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let stopped = supervisor.stop(provider)?;
    assert!(matches!(
        stopped.kind,
        ProcessEventKind::Stopped { forced: false }
    ));
    println!(
        "PASS service-network: broker + two applications, provider crash/restart, all children reaped"
    );
    Ok(())
}

fn argument(name: &str) -> Option<String> {
    std::env::args().skip_while(|arg| arg != name).nth(1)
}

fn main() -> Result<()> {
    let role = argument("--role").unwrap_or_else(|| "broker".into());
    let is_broker = matches!(
        role.as_str(),
        "broker"
            | "control-checks"
            | "parent-death-check"
            | "supervisor-checks"
            | "call-checks"
            | "watch-checks"
            | "health-checks"
    );
    if !is_broker {
        // The broker retains Child.stdin as a liveness lease. EOF also arrives
        // after a hard broker kill, when its normal Worker::Drop cannot run.
        // Install before native initialization so a stuck connect is covered.
        watch_supervisor()?;
    }
    let library = std::env::var_os("MOJO_RUNTIME_LIBRARY")
        .ok_or("set MOJO_RUNTIME_LIBRARY to the trusted pinned native SDK library")?;
    // SAFETY: the fixture operator supplies the trusted SDK image for this pin.
    let runtime = unsafe {
        Runtime::load(
            library,
            if is_broker {
                Role::Broker
            } else {
                Role::Participant
            },
        )?
    };
    match role.as_str() {
        "broker" => broker(&runtime),
        "control-checks" => control_checks::run(&runtime),
        "supervisor-checks" => supervisor_checks::run(&runtime),
        "call-checks" => call_checks::run(&runtime),
        "watch-checks" => watch_checks::run(&runtime),
        "health-checks" => health_checks::run(&runtime),
        "parent-death-check" => {
            let manager = ServiceManager::new(&runtime);
            let mut worker = Worker::start(&runtime, &manager, "parked", 1, AccessPolicy::new())?;
            // Confirm the participant finished native connect and is parked.
            let stdout = worker
                .child
                .stdout
                .take()
                .ok_or("missing participant stdout")?;
            let ready = response(|tx| {
                thread::spawn(move || {
                    use std::io::BufRead;
                    let mut line = String::new();
                    let result = std::io::BufReader::new(stdout)
                        .read_line(&mut line)
                        .map(|_| line);
                    let _ = tx.send(result);
                });
            })??;
            assert_eq!(ready.trim(), "PARKED");
            println!("participant admitted; terminating broker without destructors");
            std::process::exit(23);
        }
        "bootstrap-hang" => {
            thread::sleep(Duration::from_secs(30));
            Err("bootstrap watchdog elapsed".into())
        }
        "provider"
        | "late-provider"
        | "consumer"
        | "parked"
        | "silent"
        | "crash-loop"
        | "clean-exit"
        | "close-control"
        | "health-unhealthy"
        | "health-transient"
        | "health-recover"
        | "health-replace"
        | "health-late-replace" => {
            let pipe = connect_to_supervisor(&runtime)?;
            if role == "parked" {
                // Deliberately ignore directory/pipe disconnect. Only the
                // parent-liveness watcher can end this participant.
                let _pipe = pipe;
                println!("PARKED");
                // Longer than the test's 10-second EOF bound: a broken watcher
                // must fail the test without leaving this fixture alive forever.
                thread::sleep(Duration::from_secs(30));
                std::process::exit(24);
            } else if matches!(role.as_str(), "silent" | "close-control") {
                let _pipe = if role == "close-control" {
                    drop(pipe);
                    None
                } else {
                    Some(pipe)
                };
                thread::sleep(Duration::from_secs(30));
                Err("uncooperative fixture watchdog elapsed".into())
            } else if role == "crash-loop" {
                std::process::exit(19);
            } else if role == "clean-exit" {
                Ok(())
            } else if role == "provider" || role == "late-provider" || role.starts_with("health-") {
                if role == "late-provider" {
                    thread::sleep(Duration::from_millis(1500));
                }
                provider(
                    &runtime,
                    pipe,
                    argument("--instance")
                        .or_else(|| std::env::var(SUPERVISOR_ATTEMPT_ENV).ok())
                        .ok_or("missing --instance")?
                        .parse()?,
                    &role,
                )
            } else {
                consumer(&runtime, pipe)
            }
        }
        _ => Err("unknown --role".into()),
    }
}
