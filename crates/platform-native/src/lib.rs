//! The native platform (IF-20..IF-24). Blocking calls behind ready futures: each session's loop
//! has its own thread, so blocking it blocks nothing else.

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

pub struct NativeProcess;

impl Process for NativeProcess {
    fn run(&self, command: &str, cwd: Option<&str>, env: &[(String, String)]) -> BoxFuture<'_, PluginResult<ProcessOutput>> {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        if let Some(cwd) = cwd {
            c.current_dir(cwd);
        }
        c.envs(env.iter().map(|(k, v)| (k, v)));
        c.stdin(std::process::Stdio::null());
        ready(c.output().map_err(|e| PluginError::new(format!("running a command: {e}"))).map(|o| ProcessOutput {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }))
    }
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

struct ReaderBody(Box<dyn Read>);

impl NetBody for ReaderBody {
    fn next(&mut self) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>> {
        let mut buffer = vec![0u8; 8192];
        let result = match self.0.read(&mut buffer) {
            Ok(0) => Ok(None),
            Ok(n) => {
                buffer.truncate(n);
                Ok(Some(buffer))
            }
            Err(e) => Err(PluginError::new(format!("reading a response: {e}"))),
        };
        ready(result)
    }
}

impl Net for NativeNet {
    fn send(&self, request: NetRequest) -> BoxFuture<'_, PluginResult<NetResponse>> {
        ready(self.send_now(request))
    }
}

impl NativeNet {
    fn send_now(&self, request: NetRequest) -> PluginResult<NetResponse> {
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        // An error here names the host at most: headers, and so credentials, stay out of it.
        let fail = |e: String| PluginError::new(format!("request to {}: {e}", host(&request.url)));
        let response = match request.body.clone() {
            Some(body) => self.agent.run(builder.body(body).map_err(|e| fail(e.to_string()))?),
            None => self.agent.run(builder.body(()).map_err(|e| fail(e.to_string()))?),
        }
        .map_err(|e| fail(e.to_string()))?;
        let status = response.status().as_u16();
        let reader = response.into_body().into_reader();
        Ok(NetResponse { status, body: Box::new(ReaderBody(Box::new(reader))) })
    }
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
        process: Some(Rc::new(NativeProcess)),
        net: Rc::new(NativeNet::default()),
        clock: Rc::new(NativeClock),
        storage: Rc::new(FileStorage(data_dir)),
    }
}
