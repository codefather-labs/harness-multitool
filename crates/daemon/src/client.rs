//! A window's side of the daemon: find it or start it, register, hear events.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Address, Numbered, address_path};

#[derive(Clone)]
pub struct Client {
    base: String,
    admin: String,
    agent: ureq::Agent,
}

/// A registered window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub window: String,
    pub url: String,
}

impl Client {
    pub fn new(address: &Address) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(40))).build().into();
        Client { base: format!("http://127.0.0.1:{}", address.port), admin: address.admin.clone(), agent }
    }

    /// The daemon of this data directory, if one answers.
    pub fn find(data_dir: &Path) -> Option<Client> {
        let text = std::fs::read_to_string(address_path(data_dir)).ok()?;
        let client = Client::new(&serde_json::from_str(&text).ok()?);
        client.healthy().then_some(client)
    }

    /// Finds the daemon, or starts `program daemon` detached from this terminal and waits for it.
    pub fn find_or_start(data_dir: &Path, program: &Path) -> Result<Client, String> {
        if let Some(client) = Client::find(data_dir) {
            return Ok(client);
        }
        let mut command = std::process::Command::new(program);
        command.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            command.creation_flags(DETACHED_PROCESS);
        }
        command.spawn().map_err(|e| format!("starting the daemon: {e}"))?;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if let Some(client) = Client::find(data_dir) {
                return Ok(client);
            }
        }
        Err("the daemon did not start".to_string())
    }

    fn healthy(&self) -> bool {
        let quick: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(2))).build().into();
        quick.get(format!("{}/health", self.base)).call().is_ok_and(|r| r.status() == 200)
    }

    pub fn register(&self, root: &str) -> Result<Registered, String> {
        let mut response = self
            .agent
            .post(format!("{}/register", self.base))
            .header("authorization", &format!("Bearer {}", self.admin))
            .send(json!({"root": root}).to_string())
            .map_err(|e| format!("registering with the daemon: {e}"))?;
        let body: Value = response.body_mut().read_json().map_err(|e| format!("the daemon's answer: {e}"))?;
        match (body["window"].as_str(), body["url"].as_str()) {
            (Some(window), Some(url)) => Ok(Registered { window: window.to_string(), url: url.to_string() }),
            _ => Err(format!("the daemon refused: {body}")),
        }
    }

    pub fn port(&self) -> &str {
        self.base.rsplit(':').next().unwrap_or_default()
    }

    /// The custom handlers the daemon's build has: (name, types).
    pub fn handlers(&self) -> Result<Vec<(String, Vec<String>)>, String> {
        let mut response = self
            .agent
            .get(format!("{}/handlers", self.base))
            .header("authorization", &format!("Bearer {}", self.admin))
            .call()
            .map_err(|e| format!("reaching the daemon: {e}"))?;
        let body: Value = response.body_mut().read_json().map_err(|e| format!("the daemon's answer: {e}"))?;
        Ok(body["handlers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| (h["name"].as_str().unwrap_or_default().to_string(), h["types"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect()))
            .collect())
    }

    /// The daemon reads the projects' callback files again.
    pub fn reload(&self) -> Result<Value, String> {
        let mut response = self
            .agent
            .post(format!("{}/reload", self.base))
            .header("authorization", &format!("Bearer {}", self.admin))
            .send("{}")
            .map_err(|e| format!("reaching the daemon: {e}"))?;
        response.body_mut().read_json().map_err(|e| format!("the daemon's answer: {e}"))
    }

    /// Events after `after`, waiting a while for some.
    pub fn poll(&self, window: &str, after: u64) -> Result<Vec<Numbered>, String> {
        let mut response = self
            .agent
            .get(format!("{}/events?window={window}&after={after}", self.base))
            .header("authorization", &format!("Bearer {}", self.admin))
            .call()
            .map_err(|e| format!("polling the daemon: {e}"))?;
        let body: Value = response.body_mut().read_json().map_err(|e| format!("the daemon's answer: {e}"))?;
        serde_json::from_value(body["events"].clone()).map_err(|e| format!("the daemon's events: {e}"))
    }
}
