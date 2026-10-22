//! Real Mojo response slots, typed late responses and returned-handle cleanup.
use crate::{Result, TIMEOUT, echo::CallProbe, response};
use mojo_ipc::{
    PendingRemote, Receiver, Remote, Runtime,
    system::{message_pipe::MessageEndpoint, mojo_types::MojoError},
};
use mojo_service::{CallClient, CallContext, CallError};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

type Reply = Box<dyn FnOnce(u64) + Send>;
type PipeReply = Box<dyn FnOnce(MessageEndpoint) + Send>;

#[derive(Default)]
struct Held {
    values: Vec<(u64, Reply)>,
    pipes: Vec<PipeReply>,
    peers: Vec<MessageEndpoint>,
}

struct ProbeState(Arc<Mutex<Held>>);
#[allow(non_snake_case)]
impl CallProbe for ProbeState {
    fn Echo(&mut self, value: u64, callback: impl FnOnce(u64) + Send + 'static) {
        callback(value);
    }
    fn Hold(&mut self, value: u64, callback: impl FnOnce(u64) + Send + 'static) {
        self.0
            .lock()
            .unwrap()
            .values
            .push((value, Box::new(callback)));
    }
    fn HoldPipe(&mut self, callback: impl FnOnce(MessageEndpoint) + Send + 'static) {
        self.0.lock().unwrap().pipes.push(Box::new(callback));
    }
    fn Pending(&mut self, callback: impl FnOnce(u32) + Send + 'static) {
        let count = {
            let held = self.0.lock().unwrap();
            (held.values.len() + held.pipes.len()) as u32
        };
        callback(count);
    }
    fn Flush(&mut self, callback: impl FnOnce(u32) + Send + 'static) {
        let (values, pipes) = {
            let mut held = self.0.lock().unwrap();
            (
                std::mem::take(&mut held.values),
                std::mem::take(&mut held.pipes),
            )
        };
        let count = (values.len() + pipes.len()) as u32;
        for (value, reply) in values {
            reply(value);
        }
        for reply in pipes {
            if let Ok((endpoint, peer)) = MessageEndpoint::create_pipe() {
                self.0.lock().unwrap().peers.push(peer);
                reply(endpoint);
            }
        }
        callback(count);
    }
}
mojo_ipc::register_mojom_state_object_impls!(impl CallProbe for ProbeState);

fn pipe(
    runtime: &Runtime,
    shared: &Arc<Mutex<Held>>,
) -> Result<(PendingRemote<dyn CallProbe>, Receiver<ProbeState>)> {
    let (remote, receiver) =
        PendingRemote::<dyn CallProbe>::new_pipe().ok_or("cannot create probe pipe")?;
    Ok((
        remote,
        receiver.bind_with_options(
            ProbeState(shared.clone()),
            Some(runtime.dispatch_runner()),
            None,
        ),
    ))
}

fn until(mut ready: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while !ready()? {
        if Instant::now() >= deadline {
            return Err("call fixture condition timed out".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn held(control: &mut Remote<dyn CallProbe>, count: u32) -> Result<()> {
    until(|| {
        Ok(response(|tx| {
            control.Pending(move |n| {
                let _ = tx.send(n);
            })
        })? == count)
    })
}

fn flush(control: &mut Remote<dyn CallProbe>, client: &CallClient<dyn CallProbe>) -> Result<()> {
    response(|tx| {
        control.Flush(move |n| {
            let _ = tx.send(n);
        })
    })?;
    until(|| Ok(client.outstanding_requests() == 0))
}

pub fn run(runtime: &Runtime) -> Result<()> {
    let context = CallContext::new()?;
    let shared = Arc::new(Mutex::new(Held::default()));
    let (pending, receiver) = pipe(runtime, &shared)?;
    let mut client = CallClient::bind(runtime, &context, pending, 2)?;
    let (control, _control_receiver) = pipe(runtime, &shared)?;
    let mut control = control.bind_with_options(Some(runtime.dispatch_runner()), None);

    let mut sent = false;
    assert!(matches!(
        client.call::<()>(Duration::ZERO, |_, _| sent = true),
        Err(CallError::TimedOut)
    ));
    assert!(!sent);
    assert_eq!(
        client
            .call(TIMEOUT, |remote, reply| remote
                .Echo(42, reply.into_callback()))?
            .wait(),
        Ok(42)
    );

    let timed = client.call(Duration::from_millis(50), |remote, reply| {
        remote.Hold(1, reply.into_callback())
    })?;
    assert_eq!(timed.wait(), Err(CallError::TimedOut));
    let cancelled = client.call(TIMEOUT, |remote, reply| {
        remote.Hold(2, reply.into_callback())
    })?;
    let cancel = cancelled.cancellation_handle();
    assert!(thread::spawn(move || cancel.cancel()).join().unwrap());
    assert_eq!(cancelled.wait(), Err(CallError::Cancelled));
    assert_eq!(client.pending_calls(), 0);
    assert_eq!(context.pending_deadlines(), 0);
    assert_eq!(client.outstanding_requests(), 2);
    let mut sent = false;
    assert!(matches!(
        client.call::<()>(TIMEOUT, |_, _| sent = true),
        Err(CallError::AtCapacity)
    ));
    assert!(!sent);
    held(&mut control, 2)?;
    flush(&mut control, &client)?;
    assert!(client.is_connected());
    assert_eq!(
        client
            .call(TIMEOUT, |remote, reply| remote
                .Echo(43, reply.into_callback()))?
            .wait(),
        Ok(43)
    );

    let abandoned = client.call(TIMEOUT, |remote, reply| {
        remote.Hold(3, reply.into_callback())
    })?;
    drop(abandoned);
    assert_eq!(client.pending_calls(), 0);
    held(&mut control, 1)?;
    flush(&mut control, &client)?;

    let pipe_reply = client.call(TIMEOUT, |remote, reply| {
        remote.HoldPipe(reply.into_callback())
    })?;
    pipe_reply.cancel();
    assert_eq!(pipe_reply.wait(), Err(CallError::Cancelled));
    held(&mut control, 1)?;
    flush(&mut control, &client)?;
    let peer = shared
        .lock()
        .unwrap()
        .peers
        .pop()
        .ok_or("no returned-handle peer")?;
    until(|| Ok(matches!(peer.read(), Err(MojoError::FailedPrecondition))))?;

    let wrong_thread = client.call(TIMEOUT, |remote, reply| {
        remote.Hold(4, reply.into_callback())
    })?;
    let (tx, rx) = mpsc::channel();
    assert!(runtime.dispatch_runner().post_task(move || {
        let _ = tx.send(wrong_thread.wait());
    }));
    assert_eq!(rx.recv_timeout(TIMEOUT)?, Err(CallError::WouldBlock));
    held(&mut control, 1)?;
    flush(&mut control, &client)?;

    let disconnected = client.call(TIMEOUT, |remote, reply| {
        remote.Hold(5, reply.into_callback())
    })?;
    held(&mut control, 1)?;
    drop(receiver);
    assert_eq!(disconnected.wait(), Err(CallError::Disconnected));
    assert_eq!(client.outstanding_requests(), 0);

    let (pending, _receiver) = pipe(runtime, &shared)?;
    let mut local = CallClient::bind(runtime, &context, pending, 2)?;
    let disconnected = local.call(TIMEOUT, |remote, reply| {
        remote.Hold(6, reply.into_callback())
    })?;
    drop(local);
    assert_eq!(disconnected.wait(), Err(CallError::Disconnected));

    let (pending, _receiver) = pipe(runtime, &shared)?;
    let mut local = CallClient::bind(runtime, &context, pending, 2)?;
    let shutdown = local.call(TIMEOUT, |remote, reply| {
        remote.Hold(7, reply.into_callback())
    })?;
    context.shutdown();
    assert_eq!(shutdown.wait(), Err(CallError::ContextClosed));
    assert!(matches!(
        local.call::<()>(TIMEOUT, |_, _| panic!("shutdown must not send")),
        Err(CallError::ContextClosed)
    ));
    // Release server-owned callbacks even for connections already closed.
    response(|tx| {
        control.Flush(move |n| {
            let _ = tx.send(n);
        })
    })?;
    assert_eq!(context.pending_deadlines(), 0);
    println!(
        "PASS calls: typed completion, deadlines/cancellation, late replies/handles, bounded slots, peer/client loss and context shutdown"
    );
    Ok(())
}
