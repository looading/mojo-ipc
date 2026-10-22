mod buffer;
mod client;
mod server;

pub(crate) use buffer::{WatchBuffer, clone_info};
pub use client::{NextServiceBatch, ServiceSubscription, ServiceWatchBatch, WatchError};
pub(crate) use server::WatchBinding;

/// Maximum queued service changes for one admitted directory session.
pub const MAX_WATCH_EVENTS: usize = 256;
