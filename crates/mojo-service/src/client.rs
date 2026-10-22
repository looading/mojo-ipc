use crate::{
    ServiceError, ServiceInfo, ServiceKey, Status,
    control::{Directory, HealthProbe, ServiceFactory},
};
use mojo_ipc::{
    PendingRemote, Remote, Runtime, SequencedTaskRunnerHandle,
    bindings::interface::DynMojomInterface, system::message_pipe::MessageEndpoint,
};

/// A typed business endpoint and the provider generation selected by discovery.
pub struct ServiceConnection<T: DynMojomInterface + ?Sized> {
    pub remote: PendingRemote<T>,
    pub generation: u64,
}

/// Callback client for a host-admitted directory connection.
///
/// Callbacks execute on the SDK dispatch runner. Like the underlying bindings,
/// disconnect drops pending success callbacks; observe the disconnect handler
/// and bound host waits. No request is automatically replayed after reconnect.
pub struct ServiceDirectory {
    remote: Remote<dyn Directory>,
    runner: SequencedTaskRunnerHandle,
}

impl ServiceDirectory {
    pub fn bind(
        runtime: &Runtime,
        pipe: MessageEndpoint,
        disconnect_handler: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Self {
        Self {
            runner: runtime.dispatch_runner(),
            remote: PendingRemote::<dyn Directory>::new(pipe)
                .bind_with_options(Some(runtime.dispatch_runner()), disconnect_handler),
        }
    }

    pub fn register(
        &mut self,
        key: &ServiceKey,
        version: u32,
        factory: PendingRemote<dyn ServiceFactory>,
        callback: impl FnOnce(Result<u64, ServiceError>) + Send + 'static,
    ) {
        self.remote.Register(
            key.name.clone(),
            key.interface_name.clone(),
            version,
            factory,
            move |status, generation| callback(crate::result(status, generation)),
        );
    }

    /// Only services authorized for this session's connect policy are visible.
    pub fn list(&mut self, callback: impl FnOnce(Vec<ServiceInfo>) + Send + 'static) {
        self.remote.List(callback);
    }

    /// Resolve an exact interface version and route a fresh receiver to its
    /// provider. Success acknowledges routing, not provider/business completion.
    /// The caller must use the interface type corresponding to `key`.
    pub fn connect<T: DynMojomInterface + ?Sized + 'static>(
        &mut self,
        key: &ServiceKey,
        version: u32,
        callback: impl FnOnce(Result<ServiceConnection<T>, ServiceError>) + Send + 'static,
    ) {
        let Some((remote, receiver)) = PendingRemote::<T>::new_pipe() else {
            let _ = self
                .runner
                .post_task(move || callback(Err(ServiceError(Status::Unavailable))));
            return;
        };
        self.remote.Connect(
            key.name.clone(),
            key.interface_name.clone(),
            version,
            receiver.into_endpoint(),
            move |status, generation| {
                callback(
                    crate::result(status, generation)
                        .map(|generation| ServiceConnection { remote, generation }),
                );
            },
        );
    }

    pub fn unregister(
        &mut self,
        key: &ServiceKey,
        generation: u64,
        callback: impl FnOnce(Result<(), ServiceError>) + Send + 'static,
    ) {
        self.remote.Unregister(
            key.name.clone(),
            key.interface_name.clone(),
            generation,
            move |status| callback(crate::result(status, 0).map(|_| ())),
        );
    }

    /// Replace this session's health endpoint. Keep its HealthBinding alive.
    pub fn set_health_probe(
        &mut self,
        probe: PendingRemote<dyn HealthProbe>,
        callback: impl FnOnce(Result<(), ServiceError>) + Send + 'static,
    ) {
        self.remote.SetHealthProbe(probe, move |status| {
            callback(crate::result(status, 0).map(|_| ()))
        });
    }

    pub fn is_disconnected(&self) -> bool {
        !self.remote.is_connected()
    }

    /// Atomically install one bounded watch and return its authorized snapshot.
    /// A new watch replaces this directory session's previous watch. Overflow
    /// requires resubscribing; watch waits are separate from the directory pipe.
    pub fn watch(
        &mut self,
        capacity: usize,
        callback: impl FnOnce(Result<crate::ServiceSubscription, ServiceError>) + Send + 'static,
    ) {
        if capacity == 0 || capacity > crate::MAX_WATCH_EVENTS {
            let _ = self
                .runner
                .post_task(move || callback(Err(ServiceError(Status::InvalidArgument))));
            return;
        }
        let Some((remote, receiver)) =
            PendingRemote::<dyn crate::control::ServiceWatch>::new_pipe()
        else {
            let _ = self
                .runner
                .post_task(move || callback(Err(ServiceError(Status::Unavailable))));
            return;
        };
        let runner = self.runner.clone();
        self.remote
            .Watch(capacity as u32, receiver, move |status, snapshot| {
                callback(
                    crate::result(status, 0)
                        .map(|_| crate::ServiceSubscription::bind(snapshot, remote, runner)),
                );
            });
    }
}
