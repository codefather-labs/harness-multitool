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

/// How far a long piece of work has got, for the user to watch (CON-136, CON-144): the thread
/// doing it binds a sink, and what does the work reports as it goes. Every report reaches the
/// sink; how often they are passed on is the binder's (harness_session::throttled), since telling
/// the time is the platform's (INV-1).
pub mod progress {
    use std::cell::RefCell;
    use std::sync::Arc;

    /// What is counted.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum Unit {
        Messages,
        Bytes,
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct Progress {
        /// What is being done, in words for the user: "Reading Claude Code's copy".
        pub what: String,
        pub done: u64,
        /// 0 while it is not known.
        pub total: u64,
        pub unit: Unit,
    }

    impl Progress {
        /// The last report of its work.
        pub fn finished(&self) -> bool {
            self.total > 0 && self.done >= self.total
        }
    }

    type Sink = Arc<dyn Fn(Progress) + Send + Sync>;

    thread_local! {
        static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    }

    pub fn bind(sink: Arc<dyn Fn(Progress) + Send + Sync>) {
        SINK.with(|cell| *cell.borrow_mut() = Some(sink));
    }

    /// Reports `done` of `total` to this thread's sink, if one is bound.
    pub fn report(what: &str, done: u64, total: u64, unit: Unit) {
        let Some(sink) = SINK.with(|cell| cell.borrow().clone()) else { return };
        sink(Progress { what: what.to_string(), done, total, unit });
    }

    /// How many bytes a counting reader reads between two reports: a report costs more than the
    /// read of a few kilobytes it would follow.
    pub const BYTES_PER_REPORT: u64 = 1 << 20;

    /// A reader that reports the bytes read through it, out of `total`: at its first read, every
    /// megabyte, and at its end.
    pub struct Counting<R> {
        inner: R,
        what: &'static str,
        done: u64,
        total: u64,
        reported: Option<u64>,
    }

    impl<R> Counting<R> {
        pub fn new(inner: R, what: &'static str, total: u64) -> Self {
            Counting { inner, what, done: 0, total, reported: None }
        }
    }

    impl<R: std::io::Read> std::io::Read for Counting<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.done += n as u64;
            // The end of the input is the last report, whatever the total said.
            let end = n == 0;
            if end || self.reported.is_none_or(|at| self.done - at >= BYTES_PER_REPORT) {
                self.reported = Some(self.done);
                let total = if end { self.done } else { self.total.max(self.done) };
                report(self.what, self.done, total, Unit::Bytes);
            }
            Ok(n)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Mutex;

        #[test]
        fn a_counting_reader_reports_to_its_threads_sink_and_ends_whole() {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let into = seen.clone();
            bind(Arc::new(move |p: Progress| into.lock().unwrap().push(p)));
            let mut text = String::new();
            std::io::Read::read_to_string(&mut Counting::new(&b"hello world"[..], "Reading", 999), &mut text).unwrap();
            let seen = seen.lock().unwrap();
            let last = seen.last().unwrap();
            assert_eq!((last.done, last.total), (11, 11), "the end is the whole, whatever was said");
            assert!(last.finished() && !seen[0].finished() || seen.len() == 1);
        }
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
