//! Local application services over the existing Mojo IPC runtime.
//!
//! The host attaches trusted pipes with explicit identities and policies.
//! Applications publish factories, discover services and exchange business pipe
//! endpoints. No business messages are forwarded by the service manager.

mod calls;
mod client;
mod factory;
mod health;
mod manager;
mod policy;
mod registry;
mod supervisor;
mod watch;

include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));

pub use calls::{Call, CallCancellation, CallClient, CallContext, CallError, CallResponder};
pub use client::{ServiceConnection, ServiceDirectory};
pub use control::{ServiceChange, ServiceChangeKind};
pub use control::{ServiceInfo, Status};
pub use factory::{FactoryBinding, bind_factory};
pub use health::{HealthBinding, bind_health};
pub use manager::{ManagerConnection, ServiceManager};
pub use policy::{AccessPolicy, ServiceKey};
pub use supervisor::{
    HealthPolicy, HealthStatus, ProcessEvent, ProcessEventKind, ProcessId, ProcessSnapshot,
    ProcessSpec, ProcessState, ProcessSupervisor, RestartPolicy, SUPERVISOR_ATTEMPT_ENV,
    SUPERVISOR_SERVER_ENV, ServiceRequirement, connect_to_supervisor, watch_supervisor,
};
pub use watch::{
    MAX_WATCH_EVENTS, NextServiceBatch, ServiceSubscription, ServiceWatchBatch, WatchError,
};

/// An explicit directory failure. Business-operation errors use their own schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError(pub Status);

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "service directory: {:?}", self.0)
    }
}

impl std::error::Error for ServiceError {}

fn result(status: Status, generation: u64) -> Result<u64, ServiceError> {
    if status == Status::Ok {
        Ok(generation)
    } else {
        Err(ServiceError(status))
    }
}
