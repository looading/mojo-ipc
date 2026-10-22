//! Native control-plane lifecycle checks, separate from subprocess orchestration.
use crate::{Result, TIMEOUT, key, response};
use mojo_ipc::{Runtime, system::message_pipe::MessageEndpoint};
use mojo_service::{
    AccessPolicy, ManagerConnection, ServiceDirectory, ServiceError, ServiceManager, Status,
    bind_factory,
};
use std::{
    thread,
    time::{Duration, Instant},
};

fn attach(
    runtime: &Runtime,
    manager: &ServiceManager,
    publish: bool,
) -> Result<(ServiceDirectory, ManagerConnection)> {
    let (client, server) = MessageEndpoint::create_pipe()?;
    let policy = if publish {
        AccessPolicy::new().allow_publish(key())
    } else {
        AccessPolicy::new().allow_connect(key())
    };
    let connection = manager.attach(
        if publish { "provider" } else { "consumer" },
        policy,
        server,
    );
    Ok((ServiceDirectory::bind(runtime, client, None), connection))
}

fn wait_removed(directory: &mut ServiceDirectory) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let services = response(|tx| {
            directory.list(move |services| {
                let _ = tx.send(services);
            })
        })?;
        if services.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("disconnected registration was not removed".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn run(runtime: &Runtime) -> Result<()> {
    let manager = ServiceManager::new(runtime);
    let (mut provider, _provider_lease) = attach(runtime, &manager, true)?;
    let (mut consumer, _consumer_lease) = attach(runtime, &manager, false)?;
    let (factory, binding) = bind_factory(runtime, drop)?;
    let old = response(|tx| {
        provider.register(&key(), 1, factory, move |result| {
            let _ = tx.send(result);
        })
    })??;
    let services = response(|tx| {
        consumer.list(move |services| {
            let _ = tx.send(services);
        })
    })?;
    assert_eq!(services.len(), 1);
    // Factory loss must invalidate discovery even while its directory is alive.
    drop(binding);
    wait_removed(&mut consumer)?;

    let (factory, _binding) = bind_factory(runtime, drop)?;
    let new = response(|tx| {
        provider.register(&key(), 1, factory, move |result| {
            let _ = tx.send(result);
        })
    })??;
    assert!(new > old);
    let stale = response(|tx| {
        provider.unregister(&key(), old, move |result| {
            let _ = tx.send(result);
        })
    })?;
    assert_eq!(stale, Err(ServiceError(Status::StaleGeneration)));
    response(|tx| {
        provider.unregister(&key(), new, move |result| {
            let _ = tx.send(result);
        })
    })??;
    wait_removed(&mut consumer)?;

    let (factory, _binding) = bind_factory(runtime, drop)?;
    response(|tx| {
        provider.register(&key(), 1, factory, move |result| {
            let _ = tx.send(result);
        })
    })??;
    // Leave the host lease alive: this checks peer directory disconnect itself.
    drop(provider);
    wait_removed(&mut consumer)?;

    let (mut replacement, lease) = attach(runtime, &manager, true)?;
    let (factory, _binding) = bind_factory(runtime, drop)?;
    response(|tx| {
        replacement.register(&key(), 1, factory, move |result| {
            let _ = tx.send(result);
        })
    })??;
    // Conversely, leave the peer alive while revoking its host lease.
    drop(lease);
    wait_removed(&mut consumer)?;
    println!(
        "PASS control checks: factory loss, unregister, stale generation, directory loss and host lease removal"
    );
    Ok(())
}
