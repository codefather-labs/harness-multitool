//! A debug wrapper around `Net`: every request body and response chunk goes to a file, so what a
//! model really received and answered can be read. Headers are not written: they carry keys
//! (INV-6).

use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

use harness_api::platform::{Net, NetBody, NetRequest, NetResponse};
use harness_api::{BoxFuture, PluginResult};

pub struct WireLog {
    inner: Rc<dyn Net>,
    path: Rc<PathBuf>,
}

impl WireLog {
    pub fn new(inner: Rc<dyn Net>, path: PathBuf) -> Self {
        WireLog { inner, path: Rc::new(path) }
    }
}

fn write(path: &PathBuf, text: &[u8]) {
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(text);
    }
}

struct LoggedBody {
    inner: Box<dyn NetBody>,
    path: Rc<PathBuf>,
}

impl NetBody for LoggedBody {
    fn next(&mut self) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>> {
        Box::pin(async move {
            let chunk = self.inner.next().await?;
            if let Some(bytes) = &chunk {
                write(&self.path, bytes);
            }
            Ok(chunk)
        })
    }
}

impl Net for WireLog {
    fn send(&self, request: NetRequest) -> BoxFuture<'_, PluginResult<NetResponse>> {
        Box::pin(async move {
            let mut head = format!("\n>>> {} {}\n", request.method, request.url).into_bytes();
            head.extend_from_slice(request.body.as_deref().unwrap_or_default());
            write(&self.path, &head);
            let response = self.inner.send(request).await?;
            write(&self.path, format!("\n<<< {}\n", response.status).as_bytes());
            Ok(NetResponse { status: response.status, body: Box::new(LoggedBody { inner: response.body, path: self.path.clone() }) })
        })
    }
}
