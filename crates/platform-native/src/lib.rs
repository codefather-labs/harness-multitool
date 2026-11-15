//! The native platform (IF-20..IF-24). Blocking calls behind ready futures: each session's loop
//! has its own thread, so blocking it blocks nothing else.

pub mod children;
pub mod scope;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_api::platform::{Clock, Fs, Net, NetBody, NetRequest, NetResponse, Platform, Process, ProcessOutput, Storage};
use harness_api::{BoxFuture, PluginError, PluginResult};

fn ready<'a, T: 'a>(value: T) -> BoxFuture<'a, T> {
    Box::pin(std::future::ready(value))
}

fn io<T>(what: &str, path: &str, r: std::io::Result<T>) -> PluginResult<T> {
    r.map_err(|e| PluginError::new(format!("{what} {path}: {e}")))
}

pub struct NativeFs;

impl Fs for NativeFs {
    fn read(&self, path: &str) -> BoxFuture<'_, PluginResult<Vec<u8>>> {
        ready(io("reading", path, std::fs::read(path)))
    }

    fn write(&self, path: &str, bytes: &[u8]) -> BoxFuture<'_, PluginResult<()>> {
        ready(io("writing", path, ensure_parent(path).and_then(|_| std::fs::write(path, bytes))))
    }

    fn append(&self, path: &str, bytes: &[u8]) -> BoxFuture<'_, PluginResult<()>> {
        let result = ensure_parent(path).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(path)).and_then(|mut f| f.write_all(bytes));
        ready(io("appending to", path, result))
    }

    fn list(&self, path: &str) -> BoxFuture<'_, PluginResult<Vec<String>>> {
        let result = std::fs::read_dir(path).and_then(|entries| {
            let mut names = Vec::new();
            for entry in entries {
                let entry = entry?;
                let mut name = entry.file_name().to_string_lossy().into_owned();
                if entry.file_type()?.is_dir() {
                    name.push('/');
                }
                names.push(name);
            }
            names.sort();
            Ok(names)
        });
        ready(io("listing", path, result))
    }
}

fn ensure_parent(path: &str) -> std::io::Result<()> {
    match Path::new(path).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => std::fs::create_dir_all(dir),
        _ => Ok(()),
    }
}

#[derive(Default)]
pub struct NativeProcess {
    /// What commands run with instead of this process's environment (a window's, in the daemon).
    pub environment: Option<std::sync::Arc<std::collections::BTreeMap<String, String>>>,
}

impl Process for NativeProcess {
    fn run(&self, command: &str, cwd: Option<&str>, env: &[(String, String)]) -> BoxFuture<'_, PluginResult<ProcessOutput>> {
        let mut c = shell_command(command);
        if let Some(cwd) = cwd {
            c.current_dir(cwd);
        }
        if let Some(base) = &self.environment {
            c.env_clear();
            c.envs(base.iter());
        }
        c.envs(env.iter().map(|(k, v)| (k, v)));
        c.stdin(std::process::Stdio::null());
        ready(run_stoppable(c))
    }

    fn start(&self, command: &str, cwd: Option<&str>, output: &str) -> PluginResult<Box<dyn harness_api::platform::RunningCommand>> {
        let fail = |e: std::io::Error| PluginError::new(format!("starting a command: {e}"));
        if let Some(dir) = std::path::Path::new(output).parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let file = std::fs::File::create(output).map_err(fail)?;
        let mut c = shell_command(command);
        if let Some(cwd) = cwd {
            c.current_dir(cwd);
        }
        if let Some(base) = &self.environment {
            c.env_clear();
            c.envs(base.iter());
        }
        c.stdin(std::process::Stdio::null()).stdout(file.try_clone().map_err(fail)?).stderr(file);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let child = c.spawn().map_err(fail)?;
        Ok(Box::new(Running { child, output: output.to_string(), ended: false }))
    }
}

struct Running {
    child: std::process::Child,
    output: String,
    ended: bool,
}

impl harness_api::platform::RunningCommand for Running {
    fn finished(&mut self) -> Option<Option<i32>> {
        let status = self.child.try_wait().ok()??;
        self.ended = true;
        Some(status.code())
    }

    fn tail(&self) -> String {
        const TAIL: usize = 2000;
        let bytes = std::fs::read(&self.output).unwrap_or_default();
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(TAIL)..]).into_owned()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.ended {
            end_group(&mut self.child);
        }
    }
}

fn shell_command(command: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// What a command had said, once it has ended; it gets this long to finish saying it, as something
/// it left running can hold the pipe open for ever.
const PIPE_GRACE: Duration = Duration::from_secs(1);
const TERM_GRACE: Duration = Duration::from_secs(1);
const CHECK: Duration = Duration::from_millis(30);

/// A command in a group of its own, so a stop ends what it started too.
fn run_stoppable(mut c: std::process::Command) -> PluginResult<ProcessOutput> {
    use std::process::Stdio;
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| PluginError::new(format!("running a command: {e}")))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut buffer = [0u8; 8192];
            while let Ok(read) = pipe.read(&mut buffer) {
                if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                    break;
                }
            }
        });
        receiver
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = loop {
        collect(&out, &mut stdout);
        collect(&err, &mut stderr);
        if let Some(status) = child.try_wait().map_err(|e| PluginError::new(format!("running a command: {e}")))? {
            break status;
        }
        if harness_api::platform::stopping::asked() {
            end_group(&mut child);
            return Err(PluginError::new(harness_api::TOOL_STOPPED));
        }
        std::thread::sleep(CHECK);
    };
    let until = std::time::Instant::now() + PIPE_GRACE;
    loop {
        let closed = [collect(&out, &mut stdout), collect(&err, &mut stderr)];
        if closed == [true, true] || std::time::Instant::now() >= until {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(ProcessOutput { code: status.code(), stdout: String::from_utf8_lossy(&stdout).into_owned(), stderr: String::from_utf8_lossy(&stderr).into_owned() })
}

/// What a pipe has said so far joins the rest. True once its reader has finished.
fn collect(pipe: &std::sync::mpsc::Receiver<Vec<u8>>, into: &mut Vec<u8>) -> bool {
    loop {
        match pipe.try_recv() {
            Ok(chunk) => into.extend(chunk),
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return true,
        }
    }
}

/// Asks the command and what it started to end, then makes them.
fn end_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let signal = |name: &str| {
            let _ = std::process::Command::new("kill").args([name, "--", &group]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        };
        signal("-TERM");
        let until = std::time::Instant::now() + TERM_GRACE;
        while std::time::Instant::now() < until {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(CHECK);
        }
        signal("-KILL");
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub struct NativeNet {
    agent: ureq::Agent,
}

impl Default for NativeNet {
    fn default() -> Self {
        // A status is an answer, not an error: providers read their own error bodies.
        let config = ureq::Agent::config_builder().http_status_as_error(false).timeout_connect(Some(Duration::from_secs(30))).build();
        NativeNet { agent: config.into() }
    }
}

/// A response's body, read on a thread of its own so a stop need not wait for the next byte. A body
/// given up on ends its thread when the server next sends or closes.
struct ReaderBody(std::sync::mpsc::Receiver<Result<Vec<u8>, String>>);

impl ReaderBody {
    fn new(mut reader: Box<dyn Read + Send>) -> ReaderBody {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(read) => {
                        if sender.send(Ok(buffer[..read].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Err(e.to_string()));
                        return;
                    }
                }
            }
        });
        ReaderBody(receiver)
    }
}

impl NetBody for ReaderBody {
    fn next(&mut self) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>> {
        ready(loop {
            match self.0.recv_timeout(CHECK) {
                Ok(Ok(chunk)) => break Ok(Some(chunk)),
                Ok(Err(e)) => break Err(PluginError::new(format!("reading a response: {e}"))),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if harness_api::platform::stopping::asked() => break Err(PluginError::new(harness_api::TOOL_STOPPED)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        })
    }
}

impl Net for NativeNet {
    fn send(&self, request: NetRequest) -> BoxFuture<'_, PluginResult<NetResponse>> {
        // A model can think a long while before its first byte: a stop does not wait for that.
        let (agent, (sender, receiver)) = (self.agent.clone(), std::sync::mpsc::channel());
        std::thread::spawn(move || {
            let _ = sender.send(send_now(&agent, request));
        });
        ready(loop {
            match receiver.recv_timeout(CHECK) {
                Ok(answer) => break answer.map(|(status, reader)| NetResponse { status, body: Box::new(ReaderBody::new(reader)) }),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err(PluginError::new("the request ended with no answer")),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if harness_api::platform::stopping::asked() => break Err(PluginError::new(harness_api::TOOL_STOPPED)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        })
    }
}

fn send_now(agent: &ureq::Agent, request: NetRequest) -> PluginResult<(u16, Box<dyn Read + Send>)> {
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        // An error here names the host at most: headers, and so credentials, stay out of it.
        let fail = |e: String| PluginError::new(format!("request to {}: {e}", host(&request.url)));
        let response = match request.body.clone() {
            Some(body) => agent.run(builder.body(body).map_err(|e| fail(e.to_string()))?),
            None => agent.run(builder.body(()).map_err(|e| fail(e.to_string()))?),
        }
        .map_err(|e| fail(e.to_string()))?;
        let status = response.status().as_u16();
        let reader = response.into_body().into_reader();
        Ok((status, Box::new(reader)))
}

fn host(url: &str) -> &str {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url)
}

pub struct NativeClock;

impl Clock for NativeClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }

    fn sleep(&self, ms: u64) -> BoxFuture<'_, ()> {
        std::thread::sleep(Duration::from_millis(ms));
        ready(())
    }
}

/// Keys as files under a directory.
pub struct FileStorage(pub PathBuf);

impl FileStorage {
    fn path(&self, key: &str) -> PathBuf {
        let safe: String = key.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
        self.0.join(safe)
    }
}

impl Storage for FileStorage {
    fn get(&self, key: &str) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>> {
        ready(match std::fs::read(self.path(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PluginError::new(format!("reading key {key}: {e}"))),
        })
    }

    fn put(&self, key: &str, value: &[u8]) -> BoxFuture<'_, PluginResult<()>> {
        let result = std::fs::create_dir_all(&self.0).and_then(|_| std::fs::write(self.path(key), value));
        ready(io("writing key", key, result))
    }
}

pub fn platform(data_dir: PathBuf) -> Platform {
    Platform {
        fs: Rc::new(NativeFs),
        process: Some(Rc::new(NativeProcess::default())),
        net: Rc::new(NativeNet::default()),
        clock: Rc::new(NativeClock),
        storage: Rc::new(FileStorage(data_dir)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &str) -> ProcessOutput {
        let process = NativeProcess { environment: None };
        let mut future = process.run(command, None, &[]);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(result) => result.unwrap(),
            std::task::Poll::Pending => panic!("it blocks, so it is ready"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_command_is_heard_whole() {
        let out = run("printf out; printf err >&2; exit 3");
        assert_eq!((out.stdout.as_str(), out.stderr.as_str(), out.code), ("out", "err", Some(3)));
        for _ in 0..50 {
            assert_eq!(run("echo quick").stdout, "quick\n", "a short answer is not lost");
        }
        let large = run("yes 0123456789 | head -c 3000000");
        assert_eq!(large.stdout.len(), 3_000_000, "more than a pipe holds");
    }

    #[test]
    #[cfg(unix)]
    fn what_a_command_leaves_running_does_not_hold_it_up() {
        let start = std::time::Instant::now();
        let out = run("sleep 30 & echo went on");
        assert_eq!(out.stdout, "went on\n");
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    #[cfg(unix)]
    fn a_stop_ends_a_command_and_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let (sender, receiver) = std::sync::mpsc::channel();
        let command = format!("sleep 41 & touch {}; wait", marker.display());
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = asked.clone();
        std::thread::spawn(move || {
            harness_api::platform::stopping::bind(std::sync::Arc::new(move || flag.load(std::sync::atomic::Ordering::SeqCst)));
            let process = NativeProcess { environment: None };
            let mut future = process.run(&command, None, &[]);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            let answer = match future.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(result) => result.map(|_| ()).map_err(|e| e.to_string()),
                std::task::Poll::Pending => Err("pending".to_string()),
            };
            sender.send(answer).unwrap();
        });
        while !marker.exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
        asked.store(true, std::sync::atomic::Ordering::SeqCst);
        let start = std::time::Instant::now();
        let answer = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(answer.unwrap_err(), harness_api::TOOL_STOPPED);
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[test]
    #[cfg(unix)]
    fn a_command_left_running_is_watched_and_ended_with_what_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("sub").join("out.log");
        let process = NativeProcess { environment: None };
        let mut quick = process.start("echo said; exit 4", None, &output.to_string_lossy()).unwrap();
        let start = std::time::Instant::now();
        let code = loop {
            if let Some(code) = quick.finished() {
                break code;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(code, Some(4));
        assert_eq!(quick.tail(), "said\n");
        let mut long = process.start("sleep 43", None, &output.to_string_lossy()).unwrap();
        assert_eq!(long.finished(), None);
        let start = std::time::Instant::now();
        drop(long);
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    }
}
