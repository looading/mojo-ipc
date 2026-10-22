use crate::{
    AccessPolicy, ServiceKey, Status,
    control::{Directory, HealthProbe, ServiceFactory, ServiceWatch},
    registry::Registry,
    watch::{WatchBinding, WatchBuffer},
};
use mojo_ipc::{
    PendingReceiver, PendingRemote, Receiver, Remote, Runtime, SequencedTaskRunnerHandle,
    system::message_pipe::MessageEndpoint,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub(crate) enum ManagedRemote {
    Factory(Remote<dyn ServiceFactory>),
    Health(Remote<dyn HealthProbe>),
}
pub(crate) type SharedRemote = Arc<Mutex<ManagedRemote>>;
pub(crate) type Shared = Arc<Mutex<Registry<SharedRemote>>>;

/// Control plane for one local Mojo network. The host owns all attached sessions.
#[derive(Clone)]
pub struct ServiceManager {
    registry: Shared,
    runner: SequencedTaskRunnerHandle,
}

impl ServiceManager {
    pub fn new(runtime: &Runtime) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            runner: runtime.dispatch_runner(),
        }
    }

    /// Attach an already trusted bootstrap pipe. Identity/policy come from the
    /// host's launch or authenticated admission decision, never from the peer.
    /// Keep the returned connection alive to admit requests from this session.
    pub fn attach(
        &self,
        identity: impl Into<String>,
        policy: AccessPolicy,
        pipe: MessageEndpoint,
    ) -> ManagerConnection {
        let id = self.registry.lock().unwrap().open_session();
        let session = DirectorySession {
            registry: self.registry.clone(),
            runner: self.runner.clone(),
            id,
            identity: identity.into(),
            policy,
            watch: None,
        };
        let registry = self.registry.clone();
        let receiver = PendingReceiver::<dyn Directory>::new(pipe).bind_with_options(
            session,
            Some(self.runner.clone()),
            Some(Box::new(move || close_session(&registry, id))),
        );
        ManagerConnection {
            _receiver: receiver,
            registry: self.registry.clone(),
            id,
        }
    }
}

/// Host-owned admission lease. Dropping it closes control access and removes
/// this session's registrations. Already transferred business pipes are separate.
pub struct ManagerConnection {
    _receiver: Receiver<DirectorySession>,
    registry: Shared,
    id: u64,
}

impl ManagerConnection {
    pub(crate) fn is_open(&self) -> bool {
        self.registry.lock().unwrap().is_open(self.id)
    }

    pub(crate) fn has_services_by<'a>(
        &self,
        services: impl IntoIterator<Item = (&'a ServiceKey, u32)>,
        deadline: std::time::Instant,
        require_health: bool,
    ) -> bool {
        let registry = self.registry.lock().unwrap();
        registry.is_open(self.id)
            && (!require_health || registry.probe_registered_by(self.id, deadline))
            && services.into_iter().all(|(key, version)| {
                registry.owns_registration_by(self.id, key, version, deadline)
            })
    }
    pub(crate) fn health_probe(&self) -> Option<(u64, SharedRemote)> {
        let registry = self.registry.lock().unwrap();
        if !registry.is_open(self.id) {
            return None;
        }
        registry.probe(self.id).map(|p| (p.id, p.remote.clone()))
    }
}

impl Drop for ManagerConnection {
    fn drop(&mut self) {
        close_session(&self.registry, self.id);
    }
}

fn close_session(registry: &Shared, id: u64) {
    let (removed, watch, probe) = registry.lock().unwrap().close_session(id);
    if let Some(watch) = watch {
        watch.buffer.close();
    }
    // Endpoint destruction may schedule callbacks; release the registry first.
    drop(removed);
    drop(probe);
}

struct DirectorySession {
    registry: Shared,
    runner: SequencedTaskRunnerHandle,
    id: u64,
    identity: String,
    policy: AccessPolicy,
    watch: Option<WatchBinding>,
}

#[allow(non_snake_case)]
impl Directory for DirectorySession {
    fn Register(
        &mut self,
        name: String,
        interface_name: String,
        version: u32,
        factory: PendingRemote<dyn ServiceFactory>,
        callback: impl FnOnce(Status, u64) + Send + 'static,
    ) {
        let key = ServiceKey::new(name, interface_name);
        let registry = Arc::downgrade(&self.registry);
        let cleanup_key = key.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let cleanup_generation = generation.clone();
        let factory = factory.bind_with_options(
            Some(self.runner.clone()),
            Some(Box::new(move || {
                if let Some(registry) = registry.upgrade() {
                    let removed = registry
                        .lock()
                        .unwrap()
                        .remove_generation(&cleanup_key, cleanup_generation.load(Ordering::SeqCst));
                    drop(removed);
                }
            })),
        );
        let result = self.registry.lock().unwrap().register(
            self.id,
            &self.identity,
            &self.policy,
            key,
            version,
            Arc::new(Mutex::new(ManagedRemote::Factory(factory))),
        );
        match result {
            Ok(id) => {
                // Disconnect runs on this same runner, after Register returns.
                generation.store(id, Ordering::SeqCst);
                callback(Status::Ok, id);
            }
            Err((status, factory)) => {
                drop(factory);
                callback(status, 0);
            }
        }
    }

    fn SetHealthProbe(
        &mut self,
        probe: PendingRemote<dyn HealthProbe>,
        callback: impl FnOnce(Status) + Send + 'static,
    ) {
        let registry = Arc::downgrade(&self.registry);
        let owner = self.id;
        let token = Arc::new(AtomicU64::new(0));
        let cleanup_token = token.clone();
        let probe = probe.bind_with_options(
            Some(self.runner.clone()),
            Some(Box::new(move || {
                if let Some(registry) = registry.upgrade() {
                    let removed = registry
                        .lock()
                        .unwrap()
                        .remove_probe(owner, cleanup_token.load(Ordering::SeqCst));
                    drop(removed);
                }
            })),
        );
        let installed = self
            .registry
            .lock()
            .unwrap()
            .install_probe(self.id, Arc::new(Mutex::new(ManagedRemote::Health(probe))));
        match installed {
            Ok((id, old)) => {
                token.store(id, Ordering::SeqCst);
                drop(old);
                callback(Status::Ok);
            }
            Err((status, probe)) => {
                drop(probe);
                callback(status);
            }
        }
    }

    fn List(&mut self, callback: impl FnOnce(Vec<crate::ServiceInfo>) + Send + 'static) {
        let services = self.registry.lock().unwrap().list(self.id, &self.policy);
        callback(services);
    }

    fn Connect(
        &mut self,
        name: String,
        interface_name: String,
        version: u32,
        receiver: MessageEndpoint,
        callback: impl FnOnce(Status, u64) + Send + 'static,
    ) {
        let key = ServiceKey::new(name, interface_name);
        let target = self
            .registry
            .lock()
            .unwrap()
            .lookup(self.id, &self.policy, &key, version)
            .map(|entry| (entry.factory.clone(), entry.info.generation));
        match target {
            Ok((factory, generation)) => {
                // No registry lock is held during IPC or user callbacks.
                let status = match &mut *factory.lock().unwrap() {
                    ManagedRemote::Factory(factory) => {
                        factory.Bind(receiver);
                        Status::Ok
                    }
                    ManagedRemote::Health(_) => {
                        drop(receiver);
                        Status::Unavailable
                    }
                };
                callback(status, generation);
            }
            Err(status) => {
                drop(receiver);
                callback(status, 0);
            }
        }
    }

    fn Unregister(
        &mut self,
        name: String,
        interface_name: String,
        generation: u64,
        callback: impl FnOnce(Status) + Send + 'static,
    ) {
        let result = self.registry.lock().unwrap().unregister(
            self.id,
            &ServiceKey::new(name, interface_name),
            generation,
        );
        match result {
            Ok(entry) => {
                drop(entry);
                callback(Status::Ok);
            }
            Err(status) => callback(status),
        }
    }

    fn Watch(
        &mut self,
        capacity: u32,
        receiver: PendingReceiver<dyn ServiceWatch>,
        callback: impl FnOnce(Status, Vec<crate::ServiceInfo>) + Send + 'static,
    ) {
        let buffer = WatchBuffer::new(capacity as usize, self.runner.clone());
        let installed =
            self.registry
                .lock()
                .unwrap()
                .install_watch(self.id, &self.policy, buffer.clone());
        match installed {
            Ok(installed) => {
                if let Some(previous) = installed.previous {
                    previous.buffer.close();
                }
                let binding = WatchBinding::new(
                    &self.registry,
                    self.id,
                    installed.id,
                    buffer,
                    receiver,
                    self.runner.clone(),
                );
                let old = self.watch.replace(binding);
                drop(old);
                callback(Status::Ok, installed.snapshot);
            }
            Err(status) => {
                drop(receiver);
                callback(status, Vec::new());
            }
        }
    }
}
mojo_ipc::register_mojom_state_object_impls!(impl Directory for DirectorySession);
