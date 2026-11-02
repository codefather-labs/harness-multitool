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
/// Whether the run on this thread has been asked to stop. A session's loop is one thread whose calls
/// block: a command or a silent model would hold it past any check the loop can make, so what blocks
/// looks here (CON-95). A thread with none set is never stopped.
pub mod stopping {
    use std::cell::RefCell;
    use std::sync::Arc;

    thread_local! {
        static ASKED: RefCell<Option<Arc<dyn Fn() -> bool + Send + Sync>>> = const { RefCell::new(None) };
    }

    pub fn bind(asked: Arc<dyn Fn() -> bool + Send + Sync>) {
        ASKED.with(|cell| *cell.borrow_mut() = Some(asked));
    }

    pub fn asked() -> bool {
        ASKED.with(|cell| cell.borrow().as_ref().is_some_and(|asked| asked()))
    }
}

pub trait Process {
    fn run(&self, command: &str, cwd: Option<&str>, env: &[(String, String)]) -> BoxFuture<'_, PluginResult<ProcessOutput>>;
    /// A command left running, what it says going to the file `output`. Dropping what comes back
    /// ends it.
    fn start(&self, _command: &str, _cwd: Option<&str>, _output: &str) -> PluginResult<Box<dyn RunningCommand>> {
        Err(crate::PluginError::new("this platform cannot leave a command running"))
    }
}

/// A command left running.
pub trait RunningCommand {
    /// Its exit code once it has ended: the outer `None` while it runs, the inner where it has none.
    fn finished(&mut self) -> Option<Option<i32>>;
    /// The end of what it has said.
    fn tail(&self) -> String;
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
