use super::{Call, CallContext, CallError, CallResponder, state::ClientCalls};
use mojo_ipc::{PendingRemote, Remote, Runtime, bindings::interface::DynMojomInterface};
use std::{sync::Arc, time::Duration};

/// Bounded calls over one typed connection. Cancelled/timed-out sent requests
/// still occupy binding capacity until reply/disconnect. Drop/rebind to release
/// an unresponsive connection. No requests are automatically replayed.
pub struct CallClient<T: DynMojomInterface + ?Sized> {
    remote: Remote<T>,
    context: CallContext,
    calls: Arc<ClientCalls>,
    capacity: usize,
}

impl<T: DynMojomInterface + ?Sized + 'static> CallClient<T> {
    pub fn bind(
        runtime: &Runtime,
        context: &CallContext,
        remote: PendingRemote<T>,
        capacity: usize,
    ) -> Result<Self, CallError> {
        if capacity == 0 {
            return Err(CallError::InvalidOptions);
        }
        if context.is_shutdown() {
            return Err(CallError::ContextClosed);
        }
        Ok(Self {
            remote: remote.bind_with_options(Some(runtime.dispatch_runner()), None),
            context: context.clone(),
            calls: Arc::new(ClientCalls::default()),
            capacity,
        })
    }

    /// Send exactly one response-bearing RPC immediately, moving the responder
    /// into its callback. The send closure must not block or retain the remote.
    /// Admission errors occur before sending. Local cancellation/timeout cannot
    /// undo a remote side effect; the weak bridge still validates late replies.
    pub fn call<R: Send + 'static>(
        &mut self,
        timeout: Duration,
        send: impl FnOnce(&mut Remote<T>, CallResponder<R>),
    ) -> Result<Call<R>, CallError> {
        if self.context.is_shutdown() {
            return Err(CallError::ContextClosed);
        }
        if !self.remote.is_connected() {
            return Err(CallError::Disconnected);
        }
        if self.remote.pending_response_count() >= self.capacity
            || self.calls.len() >= self.capacity
        {
            return Err(CallError::AtCapacity);
        }
        let (call, responder) = self.context.start(&self.calls, timeout)?;
        send(&mut self.remote, responder);
        Ok(call)
    }

    pub fn is_connected(&self) -> bool {
        self.remote.is_connected()
    }
    /// Binding response slots, including locally cancelled/timed-out calls.
    pub fn outstanding_requests(&self) -> usize {
        self.remote.pending_response_count()
    }
    /// Local results still waiting (excludes completed/cancelled calls).
    pub fn pending_calls(&self) -> usize {
        self.calls.len()
    }
}

impl<T: DynMojomInterface + ?Sized> Drop for CallClient<T> {
    fn drop(&mut self) {
        self.calls.disconnect();
    }
}
