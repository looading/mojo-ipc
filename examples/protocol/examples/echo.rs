//! Real two-process fixture. Requires the actual GN-built native runtime.
//! MOJO_RUNTIME_LIBRARY=/absolute/path/to/library cargo run -p mojo-example-protocol --example echo
#![allow(non_snake_case)]
use ipc::{PendingReceiver, PendingRemote, Remote, Role, Runtime, SequencedTaskRunnerHandle};
use mojo_example_protocol::{
    common::{MessageKind, Record},
    echo::{Echo, Observer},
};
use std::{error::Error, sync::mpsc, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(15);

struct EchoService {
    runner: SequencedTaskRunnerHandle,
    observer: Option<Remote<dyn Observer>>,
}
impl Echo for EchoService {
    fn Ping(&mut self, text: String, callback: impl FnOnce(String) + Send + 'static) {
        callback(text);
    }
    fn RoundTrip(&mut self, record: Record, callback: impl FnOnce(Record) + Send + 'static) {
        callback(record);
    }
    fn Notify(&mut self, text: String) {
        if let Some(observer) = &mut self.observer {
            observer.OnEvent(text);
        }
    }
    fn SetObserver(&mut self, observer: PendingRemote<dyn Observer>) {
        self.observer = Some(observer.bind_with_options(Some(self.runner.clone()), None));
    }
}
ipc::register_mojom_state_object_impls!(impl Echo for EchoService);

struct ObserverService(mpsc::Sender<String>);
impl Observer for ObserverService {
    fn OnEvent(&mut self, text: String) {
        let _ = self.0.send(text);
    }
}
ipc::register_mojom_state_object_impls!(impl Observer for ObserverService);

fn main() -> Result<(), Box<dyn Error>> {
    let library = std::env::var_os("MOJO_RUNTIME_LIBRARY")
        .ok_or("set MOJO_RUNTIME_LIBRARY to the actual GN-built SDK native image")?;
    if let Some(token) = Runtime::inherited_token() {
        // SAFETY: this fixture's operator supplies the pinned SDK image.
        let runtime = unsafe { Runtime::load(&library, Role::Participant)? };
        // SAFETY: this child is launched only by the SDK helper below; the
        // inherited endpoint token is accepted exactly once.
        let pipe = unsafe { runtime.accept_inherited(&token)? };
        let runner = runtime.dispatch_runner();
        let (closed, disconnected) = mpsc::channel();
        let _receiver = PendingReceiver::<dyn Echo>::new(pipe).bind_with_options(
            EchoService {
                runner: runner.clone(),
                observer: None,
            },
            Some(runner),
            Some(Box::new(move || {
                let _ = closed.send(());
            })),
        );
        disconnected.recv_timeout(TIMEOUT)?;
        return Ok(());
    }
    // SAFETY: this fixture's operator supplies the pinned SDK image.
    let runtime = unsafe { Runtime::load(&library, Role::Broker)? };
    let executable = std::env::current_exe()?;
    let (mut child, pipe) = runtime.spawn_worker(
        executable.to_str().ok_or("fixture path must be UTF-8")?,
        &[],
    )?;
    let runner = runtime.dispatch_runner();
    let (closed, disconnected) = mpsc::channel();
    let mut remote = PendingRemote::<dyn Echo>::new(pipe).bind_with_options(
        Some(runner.clone()),
        Some(Box::new(move || {
            let _ = closed.send(());
        })),
    );
    let (send, receive) = mpsc::channel();
    remote.Ping(
        "Rust → Mojo → Rust / 类型化接口".into(),
        move |text| {
            let _ = send.send(text);
        },
    );
    assert_eq!(
        receive.recv_timeout(TIMEOUT)?,
        "Rust → Mojo → Rust / 类型化接口"
    );

    let expected = Record {
        id: 42,
        text: "跨进程 Record / Rust".into(),
        data: vec![0, 1, 255],
        kind: MessageKind::Binary,
        note: Some("aligned pointer".into()),
    };
    let (send, receive) = mpsc::channel();
    remote.RoundTrip(expected.clone(), move |record| {
        let _ = send.send(record);
    });
    assert_eq!(receive.recv_timeout(TIMEOUT)?, expected);

    let (observer_remote, observer_receiver) =
        PendingRemote::<dyn Observer>::new_pipe().ok_or("create observer pipe failed")?;
    let (send, receive) = mpsc::channel();
    let _observer = observer_receiver.bind_with_options(ObserverService(send), Some(runner), None);
    remote.SetObserver(observer_remote);
    remote.Notify("typed endpoint transfer works".into());
    assert_eq!(
        receive.recv_timeout(TIMEOUT)?,
        "typed endpoint transfer works"
    );
    assert_eq!(remote.pending_response_count(), 0);
    assert!(disconnected.try_recv().is_err());
    drop(remote);
    assert_eq!(child.wait(TIMEOUT)?, 0);
    println!(
        "PASS: two native Mojo processes, typed calls, imported Record and interface endpoint transfer"
    );
    Ok(())
}
