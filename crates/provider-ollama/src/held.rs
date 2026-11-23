//! Which Ollama models the sessions of this process hold (CON-190 point 11, journal#1686).
//!
//! A model is kept loaded while a session uses it, and unloaded when the last one using it ends,
//! never by time. A session holds (server, model) from its first request on it, through its
//! provider, until the provider is dropped with the session's engine. The count is this process's:
//! the daemon's, or a window's that runs its sessions itself. A daemon that stops unloads what is
//! still held ([`release_all`]); one that died without stopping left its holds in a ledger file,
//! and the next one unloads them as it starts ([`take_leftovers`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_api::platform::{Net, NetRequest};
use serde_json::{Value, json};

/// (base URL, model).
pub type Key = (String, String);

static HELD: Mutex<BTreeMap<Key, usize>> = Mutex::new(BTreeMap::new());
static LEDGER: Mutex<Option<PathBuf>> = Mutex::new(None);

fn held() -> std::sync::MutexGuard<'static, BTreeMap<Key, usize>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// The held set, written to the ledger when there is one.
fn write_ledger(held: &BTreeMap<Key, usize>) {
    let Some(path) = LEDGER.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    let list: Vec<Value> = held.keys().map(|(url, model)| json!({"base_url": url, "model": model})).collect();
    let staging = path.with_extension("partial");
    if std::fs::write(&staging, json!(list).to_string()).is_ok() {
        let _ = std::fs::rename(&staging, &path);
    }
}

/// One more session holds `model` on `url`.
pub fn acquire(url: &str, model: &str) {
    let mut held = held();
    *held.entry((url.to_string(), model.to_string())).or_insert(0) += 1;
    write_ledger(&held);
}

/// A session lets go of `model` on `url`; true when it was the last, so the model is to be
/// unloaded.
pub fn release(url: &str, model: &str) -> bool {
    let mut held = held();
    let key = (url.to_string(), model.to_string());
    let last = match held.get_mut(&key) {
        Some(n) if *n > 1 => {
            *n -= 1;
            false
        }
        Some(_) => {
            held.remove(&key);
            true
        }
        None => false,
    };
    write_ledger(&held);
    last
}

/// How many sessions hold `model` on `url`.
pub fn count(url: &str, model: &str) -> usize {
    held().get(&(url.to_string(), model.to_string())).copied().unwrap_or(0)
}

/// Ollama's own way to unload a model at once: no messages, keep_alive 0 (its docs/api.md,
/// 'Unload a model').
pub fn unload_request(url: &str, model: &str) -> NetRequest {
    let body = json!({"model": model, "messages": [], "keep_alive": 0});
    NetRequest { method: "POST".into(), url: format!("{url}/api/chat"), headers: vec![("content-type".to_string(), "application/json".to_string())], body: Some(body.to_string().into_bytes()) }
}

/// Asks Ollama to unload, and waits for its answer. A failure is said on stderr and nothing more:
/// a server that is gone holds nothing.
pub fn unload(net: &dyn Net, url: &str, model: &str) {
    match pollster::block_on(net.send(unload_request(url, model))) {
        Ok(response) if response.status == 200 => {}
        Ok(response) => eprintln!("ollama: unloading {model} at {url}: answered {}", response.status),
        Err(e) => eprintln!("ollama: unloading {model} at {url}: {}", e.0),
    }
}

/// Unloads everything still held, for a process that ends without its sessions' threads
/// unwinding (a daemon's stop, journal#1032).
pub fn release_all(net: &dyn Net) {
    let keys: Vec<Key> = {
        let mut held = held();
        let keys = held.keys().cloned().collect();
        held.clear();
        write_ledger(&held);
        keys
    };
    for (url, model) in keys {
        unload(net, &url, &model);
    }
}

/// For a daemon starting: unloads what the ledger at `path` says a daemon before it held and never
/// let go (it died without stopping), then keeps this one's holds there. Answers what it unloaded.
pub fn take_leftovers(path: &Path, net: &dyn Net) -> Vec<Key> {
    let left: Vec<Key> = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| Some((v["base_url"].as_str()?.to_string(), v["model"].as_str()?.to_string())))
        .collect();
    for (url, model) in &left {
        unload(net, url, model);
    }
    *LEDGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_path_buf());
    write_ledger(&held());
    left
}
