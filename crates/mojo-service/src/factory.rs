use crate::{ServiceError, Status, control::ServiceFactory};
use mojo_ipc::{PendingRemote, Receiver, Runtime, system::message_pipe::MessageEndpoint};

/// Owns a provider's factory receiver. Retain it while publishing the service.
pub struct FactoryBinding {
    _receiver: Receiver<FactoryState>,
}

struct FactoryState(Box<dyn FnMut(MessageEndpoint) + Send + 'static>);

#[allow(non_snake_case)]
impl ServiceFactory for FactoryState {
    fn Bind(&mut self, receiver: MessageEndpoint) {
        (self.0)(receiver);
    }
}
mojo_ipc::register_mojom_state_object_impls!(impl ServiceFactory for FactoryState);

/// Create a factory for one service key/version. `bind` receives each routed
/// business pipe on the dispatch runner; bind the known typed receiver and
/// retain its owner. Offload blocking work to your application's executor.
pub fn bind_factory(
    runtime: &Runtime,
    bind: impl FnMut(MessageEndpoint) + Send + 'static,
) -> Result<(PendingRemote<dyn ServiceFactory>, FactoryBinding), ServiceError> {
    let (remote, receiver) =
        PendingRemote::<dyn ServiceFactory>::new_pipe().ok_or(ServiceError(Status::Unavailable))?;
    let receiver = receiver.bind_with_options(
        FactoryState(Box::new(bind)),
        Some(runtime.dispatch_runner()),
        None,
    );
    Ok((
        remote,
        FactoryBinding {
            _receiver: receiver,
        },
    ))
}
