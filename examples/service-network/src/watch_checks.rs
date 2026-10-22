//! Actual control-pipe subscriptions, bounded long polls and replacement cleanup.
use crate::{Result, TIMEOUT, discovery::subscribe, key, private_key, response};
use mojo_ipc::{PendingRemote, Runtime, system::message_pipe::MessageEndpoint};
use mojo_service::{
    AccessPolicy, FactoryBinding, ManagerConnection, ServiceChangeKind, ServiceDirectory,
    ServiceError, ServiceManager, ServiceSubscription, ServiceWatchBatch, Status, WatchError,
    bind_factory,
    control::{Directory as DirectoryInterface, ServiceWatch as WatchInterface, WatchStatus},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Weak, mpsc},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

fn attach(
    runtime: &Runtime,
    manager: &ServiceManager,
    provider: bool,
) -> Result<(ServiceDirectory, ManagerConnection)> {
    let (client, server) = MessageEndpoint::create_pipe()?;
    let policy = if provider {
        AccessPolicy::new()
            .allow_publish(key())
            .allow_publish(private_key())
    } else {
        AccessPolicy::new().allow_connect(key())
    };
    let lease = manager.attach(
        if provider { "provider" } else { "consumer" },
        policy,
        server,
    );
    Ok((ServiceDirectory::bind(runtime, client, None), lease))
}

fn publish(
    runtime: &Runtime,
    directory: &mut ServiceDirectory,
    private: bool,
) -> Result<(u64, FactoryBinding)> {
    let (factory, binding) = bind_factory(runtime, drop)?;
    let generation = response(|tx| {
        directory.register(
            &if private { private_key() } else { key() },
            1,
            factory,
            move |result| {
                let _ = tx.send(result);
            },
        )
    })??;
    Ok((generation, binding))
}

fn remove(directory: &mut ServiceDirectory, generation: u64) -> Result<()> {
    response(|tx| {
        directory.unregister(&key(), generation, move |result| {
            let _ = tx.send(result);
        })
    })??;
    Ok(())
}

fn change(batch: ServiceWatchBatch, kind: ServiceChangeKind, generation: u64, revision: u64) {
    assert_eq!(batch.revision, revision);
    assert_eq!(batch.changes.len(), 1);
    let event = &batch.changes[0];
    assert_eq!(event.kind, kind);
    assert_eq!(event.revision, revision);
    assert_eq!(event.service.generation, generation);
    assert_eq!(event.service.name, key().name);
}

struct Signal(mpsc::Sender<()>);
impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
}

fn next_async(watch: &mut ServiceSubscription) -> Result<ServiceWatchBatch> {
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(Signal(tx)));
    let mut next = watch.next();
    match Pin::new(&mut next).poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(result) => Ok(result?),
        Poll::Pending => {
            rx.recv_timeout(TIMEOUT)?;
            match Pin::new(&mut next).poll(&mut Context::from_waker(&waker)) {
                Poll::Ready(result) => Ok(result?),
                Poll::Pending => Err("watch wake lost its batch".into()),
            }
        }
    }
}

pub fn run(runtime: &Runtime) -> Result<()> {
    let manager = ServiceManager::new(runtime);
    let (mut provider, _provider_lease) = attach(runtime, &manager, true)?;
    let (mut viewer, viewer_lease) = attach(runtime, &manager, false)?;
    let (first, _first_binding) = publish(runtime, &mut provider, false)?;
    let (_, hidden_binding) = publish(runtime, &mut provider, true)?;
    let mut watch = subscribe(&mut viewer, 4)?;
    assert_eq!(watch.initial_services().len(), 1);
    assert_eq!(watch.initial_services()[0].generation, first);
    assert!(matches!(
        response(|tx| viewer.watch(0, move |r| {
            let _ = tx.send(r);
        }))?,
        Err(ServiceError(Status::InvalidArgument))
    ));

    let (tx, rx) = mpsc::channel();
    assert!(runtime.dispatch_runner().post_task(move || {
        let result = watch.recv_timeout(Duration::ZERO);
        let _ = tx.send((watch, result));
    }));
    let (mut watch, wrong_thread) = rx.recv_timeout(TIMEOUT)?;
    assert!(matches!(wrong_thread, Err(WatchError::WouldBlock)));
    for _ in 0..32 {
        assert!(matches!(
            watch.recv_timeout(Duration::ZERO),
            Err(WatchError::TimedOut)
        ));
        assert_eq!(watch.outstanding_requests(), 1);
    }
    remove(&mut provider, first)?;
    change(
        watch.recv_timeout(TIMEOUT)?,
        ServiceChangeKind::Removed,
        first,
        1,
    );
    let (second, second_binding) = publish(runtime, &mut provider, false)?;
    change(next_async(&mut watch)?, ServiceChangeKind::Added, second, 2);

    // A dropped borrowing Future releases its task Waker, but retains its RPC.
    let (tx, _rx) = mpsc::channel();
    let signal = Arc::new(Signal(tx));
    let weak: Weak<Signal> = Arc::downgrade(&signal);
    let waker = Waker::from(signal);
    let mut abandoned = watch.next();
    assert!(
        Pin::new(&mut abandoned)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    drop(abandoned);
    drop(waker);
    assert!(weak.upgrade().is_none());
    assert_eq!(watch.outstanding_requests(), 1);
    drop(second_binding);
    change(
        watch.recv_timeout(TIMEOUT)?,
        ServiceChangeKind::Removed,
        second,
        3,
    );

    assert!(matches!(
        watch.recv_timeout(Duration::ZERO),
        Err(WatchError::TimedOut)
    ));
    let mut replacement = subscribe(&mut viewer, 4)?;
    assert!(matches!(
        watch.recv_timeout(TIMEOUT),
        Err(WatchError::Disconnected)
    ));
    drop(watch);
    assert!(replacement.initial_services().is_empty());
    drop(hidden_binding);
    assert!(matches!(
        replacement.recv_timeout(Duration::from_millis(20)),
        Err(WatchError::TimedOut)
    ));
    let (third, _third_binding) = publish(runtime, &mut provider, false)?;
    change(
        replacement.recv_timeout(TIMEOUT)?,
        ServiceChangeKind::Added,
        third,
        1,
    );

    // No Next requests while these changes occur: capacity one must overflow.
    let mut slow = subscribe(&mut viewer, 1)?;
    remove(&mut provider, third)?;
    let (fourth, _fourth_binding) = publish(runtime, &mut provider, false)?;
    assert!(matches!(
        slow.recv_timeout(TIMEOUT),
        Err(WatchError::ResyncRequired)
    ));
    assert!(matches!(
        slow.recv_timeout(Duration::ZERO),
        Err(WatchError::ResyncRequired)
    ));
    let mut resynced = subscribe(&mut viewer, 4)?;
    assert_eq!(resynced.initial_services().len(), 1);
    assert_eq!(resynced.initial_services()[0].generation, fourth);
    assert!(matches!(
        resynced.recv_timeout(Duration::ZERO),
        Err(WatchError::TimedOut)
    ));
    drop(viewer);
    assert!(matches!(
        resynced.recv_timeout(TIMEOUT),
        Err(WatchError::Disconnected)
    ));
    drop(viewer_lease);

    let (mut viewer, lease) = attach(runtime, &manager, false)?;
    let mut revoked = subscribe(&mut viewer, 4)?;
    assert!(matches!(
        revoked.recv_timeout(Duration::ZERO),
        Err(WatchError::TimedOut)
    ));
    drop(lease);
    assert!(matches!(
        revoked.recv_timeout(TIMEOUT),
        Err(WatchError::Disconnected)
    ));

    // Raw protocol misuse must get Busy without losing the original Next.
    let (client, server) = MessageEndpoint::create_pipe()?;
    let _lease = manager.attach(
        "raw-viewer",
        AccessPolicy::new().allow_connect(key()),
        server,
    );
    let mut directory = PendingRemote::<dyn DirectoryInterface>::new(client)
        .bind_with_options(Some(runtime.dispatch_runner()), None);
    let (remote, receiver) =
        PendingRemote::<dyn WatchInterface>::new_pipe().ok_or("no raw watch pipe")?;
    let status = response(|tx| {
        directory.Watch(4, receiver, move |status, _| {
            let _ = tx.send(status);
        })
    })?;
    assert_eq!(status, Status::Ok);
    let mut raw = remote.bind_with_options(Some(runtime.dispatch_runner()), None);
    let (tx, rx) = mpsc::channel();
    raw.Next(move |status, revision, changes| {
        let _ = tx.send((status, revision, changes));
    });
    let busy = response(|tx| {
        raw.Next(move |status, _, _| {
            let _ = tx.send(status);
        })
    })?;
    assert_eq!(busy, WatchStatus::Busy);
    remove(&mut provider, fourth)?;
    let (status, revision, changes) = rx.recv_timeout(TIMEOUT)?;
    assert_eq!(status, WatchStatus::Updates);
    change(
        ServiceWatchBatch { revision, changes },
        ServiceChangeKind::Removed,
        fourth,
        1,
    );

    println!(
        "PASS watches: atomic snapshot, filtered changes, async/timeout reuse, replacement, overflow/resync, disconnect/lease and single pending Next"
    );
    Ok(())
}
