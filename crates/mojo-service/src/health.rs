use crate::{ServiceError, Status, control::HealthProbe};
use mojo_ipc::{PendingRemote, Receiver, Runtime};

/// Retain this receiver while the participant exposes its health endpoint.
pub struct HealthBinding {
    _receiver: Receiver<HealthState>,
}
struct HealthState(Box<dyn FnMut() -> bool + Send + 'static>);
#[allow(non_snake_case)]
impl HealthProbe for HealthState {
    fn Check(&mut self, callback: impl FnOnce(bool) + Send + 'static) {
        callback((self.0)());
    }
}
mojo_ipc::register_mojom_state_object_impls!(impl HealthProbe for HealthState);

/// Run a short application health check on the participant's dispatch sequence.
/// A constant true checks dispatch responsiveness only. Offload blocking work;
/// a blocked check prevents all other work on that sequence from progressing.
pub fn bind_health(
    runtime: &Runtime,
    check: impl FnMut() -> bool + Send + 'static,
) -> Result<(PendingRemote<dyn HealthProbe>, HealthBinding), ServiceError> {
    let (remote, receiver) =
        PendingRemote::<dyn HealthProbe>::new_pipe().ok_or(ServiceError(Status::Unavailable))?;
    let receiver = receiver.bind_with_options(
        HealthState(Box::new(check)),
        Some(runtime.dispatch_runner()),
        None,
    );
    Ok((
        remote,
        HealthBinding {
            _receiver: receiver,
        },
    ))
}
