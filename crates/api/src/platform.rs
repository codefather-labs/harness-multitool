//! Platform interfaces (IF-20..IF-25, INV-1): the only way to files, processes, network, time,
//! storage and the terminal. Native implementations live outside the core; the browser build
//! brings its own.

use std::rc::Rc;

use crate::{BoxFuture, PluginResult};

/// IF-20.
pub trait Fs {
    fn read(&self, path: &str) -> BoxFuture<'_, PluginResult<Vec<u8>>>;
    fn write(&self, path: &str, bytes: &[u8]) -> BoxFuture<'_, PluginResult<()>>;
    /// Appends, creating the file and its directory when missing.
    fn append(&self, path: &str, bytes: &[u8]) -> BoxFuture<'_, PluginResult<()>>;
    fn list(&self, path: &str) -> BoxFuture<'_, PluginResult<Vec<String>>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// IF-21. Absent in the browser, where tools that need it are not offered.
pub trait Process {
    fn run(&self, command: &str, cwd: Option<&str>, env: &[(String, String)]) -> BoxFuture<'_, PluginResult<ProcessOutput>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// A response whose body arrives in chunks, for streamed model turns.
pub trait NetBody {
    /// The next chunk; `None` at the end.
    fn next(&mut self) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>>;
}

pub struct NetResponse {
    pub status: u16,
    pub body: Box<dyn NetBody>,
}

/// IF-22.
pub trait Net {
    fn send(&self, request: NetRequest) -> BoxFuture<'_, PluginResult<NetResponse>>;
}

/// IF-23.
pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep(&self, ms: u64) -> BoxFuture<'_, ()>;
}

/// IF-24.
pub trait Storage {
    fn get(&self, key: &str) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>>;
    fn put(&self, key: &str, value: &[u8]) -> BoxFuture<'_, PluginResult<()>>;
}

/// IF-25.
pub trait Terminal {
    fn size(&self) -> (u16, u16);
}

/// What a plugin may reach of the platform. `process` is `None` where there are no processes.
#[derive(Clone)]
pub struct Platform {
    pub fs: Rc<dyn Fs>,
    pub process: Option<Rc<dyn Process>>,
    pub net: Rc<dyn Net>,
    pub clock: Rc<dyn Clock>,
    pub storage: Rc<dyn Storage>,
}
