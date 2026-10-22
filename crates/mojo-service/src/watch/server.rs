use super::WatchBuffer;
use crate::{
    ServiceChange,
    control::{ServiceWatch, WatchStatus},
    manager::Shared,
};
use mojo_ipc::{PendingReceiver, Receiver, SequencedTaskRunnerHandle};
use std::sync::{Arc, Weak};

struct WatchSession(Arc<WatchBuffer>);
#[allow(non_snake_case)]
impl ServiceWatch for WatchSession {
    fn Next(
        &mut self,
        callback: impl FnOnce(WatchStatus, u64, Vec<ServiceChange>) + Send + 'static,
    ) {
        self.0.next(Box::new(callback));
    }
}
mojo_ipc::register_mojom_state_object_impls!(impl ServiceWatch for WatchSession);

type WeakRegistry = Weak<std::sync::Mutex<crate::registry::Registry<crate::manager::SharedRemote>>>;

fn detach(registry: &WeakRegistry, owner: u64, token: u64, buffer: &WatchBuffer) {
    let removed = registry
        .upgrade()
        .and_then(|registry| registry.lock().unwrap().remove_watch(owner, token));
    if let Some(entry) = removed {
        entry.buffer.close();
    }
    buffer.close();
}

pub(crate) struct WatchBinding {
    _receiver: Receiver<WatchSession>,
    buffer: Arc<WatchBuffer>,
    registry: WeakRegistry,
    owner: u64,
    token: u64,
}

impl WatchBinding {
    pub fn new(
        registry: &Shared,
        owner: u64,
        token: u64,
        buffer: Arc<WatchBuffer>,
        receiver: PendingReceiver<dyn ServiceWatch>,
        runner: SequencedTaskRunnerHandle,
    ) -> Self {
        let weak_registry = Arc::downgrade(registry);
        let closed_registry = weak_registry.clone();
        let closed_buffer = buffer.clone();
        let receiver = receiver.bind_with_options(
            WatchSession(buffer.clone()),
            Some(runner),
            Some(Box::new(move || {
                detach(&closed_registry, owner, token, &closed_buffer);
            })),
        );
        Self {
            _receiver: receiver,
            buffer,
            registry: weak_registry,
            owner,
            token,
        }
    }
}

impl Drop for WatchBinding {
    fn drop(&mut self) {
        detach(&self.registry, self.owner, self.token, &self.buffer);
    }
}
