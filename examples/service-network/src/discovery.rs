//! Example event-driven discovery; snapshots are only the subscription boundary.
use crate::{Result, TIMEOUT, key, response};
use mojo_service::{ServiceChangeKind, ServiceConnection, ServiceDirectory, ServiceSubscription};

pub fn subscribe(directory: &mut ServiceDirectory, capacity: usize) -> Result<ServiceSubscription> {
    Ok(response(|tx| {
        directory.watch(capacity, move |result| {
            let _ = tx.send(result);
        })
    })??)
}

pub fn connect(
    directory: &mut ServiceDirectory,
    subscription: &mut ServiceSubscription,
    after: u64,
) -> Result<ServiceConnection<dyn crate::echo::Echo>> {
    let mut seen = after;
    loop {
        let initial = subscription
            .initial_services()
            .iter()
            .find(|info| {
                info.name == key().name
                    && info.interface_name == key().interface_name
                    && info.generation > seen
            })
            .map(|info| info.generation);
        let generation = if let Some(generation) = initial {
            generation
        } else {
            let batch = subscription.recv_timeout(TIMEOUT)?;
            let Some(change) = batch.changes.into_iter().find(|change| {
                change.kind == ServiceChangeKind::Added
                    && change.service.name == key().name
                    && change.service.interface_name == key().interface_name
                    && change.service.generation > seen
            }) else {
                continue;
            };
            change.service.generation
        };
        seen = generation;
        match crate::connect(directory, &key(), 1)? {
            Ok(connection) if connection.generation > after => return Ok(connection),
            Ok(_) | Err(mojo_service::ServiceError(mojo_service::Status::NotFound)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}
