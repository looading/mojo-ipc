mod client;
mod state;
mod timer;

pub use client::CallClient;
pub use state::{Call, CallCancellation, CallResponder};
pub use timer::CallContext;

/// Local call completion/admission errors. Business errors stay in the response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallError {
    TimedOut,
    Cancelled,
    Disconnected,
    ContextClosed,
    AtCapacity,
    InvalidOptions,
    WouldBlock,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "service call: {:?}", self)
    }
}
impl std::error::Error for CallError {}

#[cfg(test)]
mod tests;
