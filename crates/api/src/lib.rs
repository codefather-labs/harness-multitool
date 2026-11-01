//! The interfaces of harness-multitool: one module per interface (docs/spec/interfaces.md),
//! the events of the bus (IF-0.2) and the fold over the session log (INV-5).
//!
//! This crate touches no platform (INV-1): files, processes, network, clock, storage and the
//! terminal are interfaces too ([`platform`]). Futures are boxed and not `Send`, so the same
//! traits serve a native thread and the browser's single thread.

use std::future::Future;
use std::pin::Pin;

pub mod event;
pub mod interfaces;
pub mod model;
pub mod platform;
pub mod state;

pub use event::{Event, LogEntry};
pub use interfaces::*;
pub use model::*;
pub use state::{SessionState, TaskState};

/// A boxed future, the return type of every asynchronous interface method.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// What a plugin reports when it fails. Never carries a secret (INV-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError(pub String);

impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        PluginError(message.into())
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PluginError {}

pub type PluginResult<T> = Result<T, PluginError>;
