//! Sign-in with a ChatGPT plan (IF-2, CON-55): the flow OpenAI documents for open-source apps at
//! developers.openai.com/siwc/token-sharing-open-source. The harness registers a client of its
//! own; the user signs in on OpenAI's page, and the harness only ever holds the tokens it issues.
//! This crate has its own HTTP client: a wire log around `Platform.net` would record tokens (INV-6).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use harness_api::{Auth, BoxFuture, PluginError, PluginResult, Secret};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const NEW_CLIENT: &str = "dynamic_agent_client";
const APP_NAME: &str = "harness-multitool";
pub const SIGN_IN_AGAIN: &str = "sign in again: multitool login chatgpt";
/// Refresh this long before an access token expires.
const REFRESH_MARGIN_S: u64 = 300;
/// A lock older than this was left by a process that died.
const STALE_LOCK: Duration = Duration::from_secs(60);
/// Errors of a refresh that only a new sign-in cures.
const TERMINAL: [&str; 6] = ["invalid_grant", "invalid_refresh_token", "token_expired", "refresh_token_expired", "refresh_token_invalidated", "refresh_token_reused"];

/// One sign-in. It prints nothing and has no `Debug`, so it cannot reach a log by accident.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub email: Option<String>,
    pub issuer: String,
    pub subject: String,
    pub client_id: String,
    pub ext_agent_host_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub scopes: Vec<String>,
    pub saved_at_s: u64,
}

/// What stays after signing out: the same client is used for the same account.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Registration {
    client_id: String,
    subject: String,
    email: Option<String>,
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The files, owner-only and written atomically.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store { dir: data_dir.join("auth").join("chatgpt") }
    }

    /// The store in this folder itself: a profile's own (CON-122).
    pub fn at(dir: &Path) -> Store {
        Store { dir: dir.to_path_buf() }
    }

    fn write(&self, name: &str, text: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let temp = self.dir.join(format!("{name}.{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| format!("{}: {e}", temp.display()))?;
        std::io::Write::write_all(&mut file, text.as_bytes()).map_err(|e| format!("{}: {e}", temp.display()))?;
        drop(file);
        std::fs::rename(&temp, self.dir.join(name)).map_err(|e| format!("{name}: {e}"))
    }

    /// A stable id for this host, opaque and made once; never the user's identity.
    fn host_id(&self) -> Result<String, String> {
        if let Ok(id) = std::fs::read_to_string(self.dir.join("host_id")) {
            return Ok(id.trim().to_string());
        }
        let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        self.write("host_id", &id)?;
        Ok(id)
    }

    pub fn load(&self) -> Option<Credential> {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join("credentials.json")).ok()?).ok()
    }

    fn save(&self, credential: &Credential) -> Result<(), String> {
        self.write("credentials.json", &serde_json::to_string(credential).map_err(|e| e.to_string())?)
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(self.dir.join("credentials.json"));
    }

    fn registration(&self) -> Option<Registration> {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join("registration.json")).ok()?).ok()
    }

    /// Serializes refreshes between processes, since the refresh token rotates.
    fn lock(&self) -> Result<Lock, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.dir.join("refresh.lock");
        let start = Instant::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Lock { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > STALE_LOCK);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                    } else if start.elapsed() > Duration::from_secs(30) {
                        return Err("another process has been refreshing the sign-in for too long".to_string());
                    } else {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
    }
}

struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn agent(timeout_s: u64) -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(timeout_s))).build().into()
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode_query(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn form(fields: &[(&str, &str)]) -> String {
    fields.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

fn post_form(agent: &ureq::Agent, url: &str, fields: &[(&str, &str)]) -> Result<(u16, Value), String> {
    let sent = agent.post(url).header("content-type", "application/x-www-form-urlencoded").send(form(fields));
    let mut response = sent.map_err(|e| format!("reaching OpenAI: {e}"))?;
    let status = response.status().as_u16();
    Ok((status, response.body_mut().read_json().unwrap_or(Value::Null)))
}

/// The error code of a token endpoint's answer, in either form. Never its whole body.
fn error_code(body: &Value) -> String {
    body["error"]["code"].as_str().or(body["error"].as_str()).or(body["code"].as_str()).unwrap_or("unknown").to_string()
}

fn discovery(agent: &ureq::Agent, issuer: &str) -> Result<Value, String> {
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let mut response = agent.get(&url).call().map_err(|e| format!("reaching OpenAI: {e}"))?;
    response.body_mut().read_json().map_err(|e| format!("the OpenID configuration: {e}"))
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

/// Signature, issuer, audience, expiry and nonce (the sign-in guide's step 4).
fn validate_id_token(agent: &ureq::Agent, issuer: &str, token: &str, client_id: &str, nonce: &str) -> Result<Claims, String> {
    let config = discovery(agent, issuer)?;
    let jwks_uri = config["jwks_uri"].as_str().ok_or("the OpenID configuration has no jwks_uri")?;
    let mut response = agent.get(jwks_uri).call().map_err(|e| format!("reaching OpenAI: {e}"))?;
    let keys: Value = response.body_mut().read_json().map_err(|e| format!("the signing keys: {e}"))?;
    let header = decode_header(token).map_err(|e| format!("the ID token: {e}"))?;
    let list = keys["keys"].as_array().cloned().unwrap_or_default();
    let key = list.iter().find(|k| header.kid.is_none() || k["kid"].as_str() == header.kid.as_deref()).ok_or("the ID token's key is not among the published ones")?;
    let (n, e) = (key["n"].as_str().ok_or("a signing key has no n")?, key["e"].as_str().ok_or("a signing key has no e")?);
    let decoding = DecodingKey::from_rsa_components(n, e).map_err(|e| format!("a signing key: {e}"))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer.trim_end_matches('/')]);
    validation.set_audience(&[client_id]);
    let claims = decode::<Claims>(token, &decoding, &validation).map_err(|e| format!("the ID token did not validate: {e}"))?.claims;
    if claims.nonce.as_deref() != Some(nonce) {
        return Err("the ID token's nonce does not match this sign-in".to_string());
    }
    Ok(claims)
}

/// A sign-in under way: the browser has `url`, and `finish` waits for its callback.
pub struct Pending {
    issuer: String,
    server: tiny_http::Server,
    pub url: String,
    redirect_uri: String,
    state: String,
    nonce: String,
    verifier: String,
    /// The client of an earlier sign-in; `None` registers a new one.
    returning: Option<Registration>,
    host_id: String,
}

fn random() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Starts the callback listener and makes the URL for the user's browser.
pub fn begin_login(store: &Store, issuer: &str, port: u16) -> Result<Pending, String> {
    begin_login_for(store, issuer, port)
}

/// Where the address of the sign-in under way is kept, to copy it whole.
pub fn login_url_path(store: &Store) -> PathBuf {
    store.dir.join("login-url.txt")
}

/// A readable hint for a sign-in from a machine other than the browser's.
pub fn remote_hint(port: u16) -> Option<String> {
    std::env::var_os("SSH_CONNECTION").map(|_| format!("Over SSH the browser needs this machine's port: ssh -L {port}:127.0.0.1:{port} <this host>"))
}

fn begin_login_for(store: &Store, issuer: &str, port: u16) -> Result<Pending, String> {
    let host_id = store.host_id()?;
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| format!("listening on 127.0.0.1:{port}: {e}"))?;
    let port = server.server_addr().to_ip().map_or(port, |a| a.port());
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    let (state, nonce, verifier) = (random(), random(), random());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let returning = store.registration();
    let mut params: Vec<(&str, &str)> = vec![
        ("ext_agent_host_id", &host_id),
        ("response_type", "code"),
        ("redirect_uri", &redirect_uri),
        ("scope", SCOPE),
        ("resource", RESOURCE),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
    ];
    match &returning {
        Some(r) => {
            params.insert(0, ("client_id", &r.client_id));
            if let Some(email) = &r.email {
                params.push(("login_hint", email));
            }
        }
        None => {
            params.insert(0, ("client_id", NEW_CLIENT));
            params.push(("agent_name_hint", APP_NAME));
        }
    }
    let url = format!("{}/api/accounts/authorize?{}", issuer.trim_end_matches('/'), form(&params));
    store.write("login-url.txt", &url)?;
    Ok(Pending { issuer: issuer.trim_end_matches('/').to_string(), server, url, redirect_uri, state, nonce, verifier, returning, host_id })
}

impl Pending {
    /// Waits for the callback, then exchanges and checks everything before saving anything.
    pub fn finish(self, store: &Store, timeout: Duration) -> Result<Credential, String> {
        self.finish_or_cancel(store, timeout, &std::sync::atomic::AtomicBool::new(false))
    }

    /// The same; setting `cancel` ends the wait and frees the port.
    pub fn finish_or_cancel(self, store: &Store, timeout: Duration, cancel: &std::sync::atomic::AtomicBool) -> Result<Credential, String> {
        let outcome = self.wait(store, timeout, cancel);
        let _ = std::fs::remove_file(login_url_path(store));
        outcome
    }

    fn wait(self, store: &Store, timeout: Duration, cancel: &std::sync::atomic::AtomicBool) -> Result<Credential, String> {
        let deadline = Instant::now() + timeout;
        let query = loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("the sign-in was cancelled".to_string());
            }
            if Instant::now() >= deadline {
                return Err("the sign-in timed out".to_string());
            }
            let Some(request) = self.server.recv_timeout(Duration::from_millis(200)).map_err(|e| e.to_string())? else { continue };
            let url = request.url().to_string();
            let Some(("/auth/callback", query)) = url.split_once('?') else {
                let _ = request.respond(tiny_http::Response::from_string("").with_status_code(404));
                continue;
            };
            let params: Vec<(String, String)> = query.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), decode_query(v))).collect();
            let state_ok = params.iter().any(|(k, v)| k == "state" && *v == self.state);
            // Something else knocking on the port does not end the sign-in.
            let page = if state_ok { "You can close this window and go back to the terminal." } else { "This does not belong to the sign-in under way." };
            let _ = request.respond(tiny_http::Response::from_string(page).with_status_code(if state_ok { 200 } else { 400 }));
            if state_ok {
                break params;
            }
        };
        let param = |name: &str| query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        if let Some(error) = param("error") {
            return Err(format!("the sign-in was refused: {error}"));
        }
        let code = param("code").ok_or("the callback has no code")?;
        let client_id = match (&self.returning, param("client_id")) {
            (Some(r), Some(id)) if id != r.client_id => return Err("the callback names a different client".to_string()),
            (Some(r), _) => r.client_id.clone(),
            (None, Some(id)) if id != NEW_CLIENT => id.to_string(),
            (None, _) => return Err("the registration is incomplete: no client id came back".to_string()),
        };
        let http = agent(30);
        let token_url = format!("{}/api/accounts/oauth/token", self.issuer);
        let fields = [("grant_type", "authorization_code"), ("client_id", &client_id), ("code", code), ("code_verifier", &self.verifier), ("redirect_uri", &self.redirect_uri), ("resource", RESOURCE)];
        let (status, body) = post_form(&http, &token_url, &fields)?;
        if status != 200 {
            return Err(format!("the code exchange was refused ({status}): {}", error_code(&body)));
        }
        let text = |name: &str| body[name].as_str().map(str::to_string).ok_or(format!("the token answer has no {name}"));
        let id_token = text("id_token")?;
        let claims = validate_id_token(&http, &self.issuer, &id_token, &client_id, &self.nonce)?;
        if let Some(r) = self.returning.as_ref().filter(|r| r.subject != claims.sub) {
            return Err(format!("this is a different account than the one registered{}", r.email.as_ref().map_or(String::new(), |e| format!(" ({e})"))));
        }
        let scopes: Vec<String> = body["scope"].as_str().or(param("scope")).unwrap_or_default().split_whitespace().map(str::to_string).collect();
        if !scopes.iter().any(|s| s == PLAN_SCOPE) {
            return Err("plan usage was not granted, so this sign-in cannot run a model".to_string());
        }
        let credential = Credential {
            email: claims.email.clone(),
            issuer: self.issuer.clone(),
            subject: claims.sub.clone(),
            client_id: client_id.clone(),
            ext_agent_host_id: self.host_id.clone(),
            id_token,
            access_token: text("access_token")?,
            refresh_token: text("refresh_token")?,
            token_type: body["token_type"].as_str().unwrap_or("Bearer").to_string(),
            expires_in: body["expires_in"].as_u64().unwrap_or(3600),
            scopes,
            saved_at_s: now_s(),
        };
        store.write("registration.json", &serde_json::to_string(&Registration { client_id, subject: claims.sub, email: claims.email }).map_err(|e| e.to_string())?)?;
        store.save(&credential)?;
        Ok(credential)
    }
}

/// Signs out: revokes the refresh token, clears the tokens, keeps the registered client.
/// Returns false if the revocation could not be confirmed.
pub fn logout(store: &Store) -> Result<bool, String> {
    let Some(credential) = store.load() else { return Ok(true) };
    let http = agent(30);
    let confirmed = discovery(&http, &credential.issuer).ok().and_then(|c| c["revocation_endpoint"].as_str().map(str::to_string)).is_some_and(|url| {
        post_form(&http, &url, &[("token", &credential.refresh_token), ("token_type_hint", "refresh_token"), ("client_id", &credential.client_id)]).is_ok_and(|(status, _)| status == 200)
    });
    store.clear();
    Ok(confirmed)
}

/// `credentials()` for a provider on a ChatGPT plan.
pub struct ChatGptAuth {
    store: Store,
    http: ureq::Agent,
    /// Only to stop this process re-reading the file on every turn.
    cached: RefCell<Option<Credential>>,
}

impl ChatGptAuth {
    pub fn new(data_dir: &Path) -> ChatGptAuth {
        ChatGptAuth::with_store(Store::new(data_dir))
    }

    pub fn with_store(store: Store) -> ChatGptAuth {
        ChatGptAuth { store, http: agent(30), cached: RefCell::new(None) }
    }

    fn fresh(c: &Credential) -> bool {
        now_s() + REFRESH_MARGIN_S < c.saved_at_s + c.expires_in
    }

    fn token(&self) -> Result<String, String> {
        if let Some(c) = self.cached.borrow().as_ref().filter(|c| Self::fresh(c)) {
            return Ok(c.access_token.clone());
        }
        let current = self.store.load().ok_or(format!("not signed in to ChatGPT: {SIGN_IN_AGAIN}"))?;
        if Self::fresh(&current) {
            let token = current.access_token.clone();
            self.cached.replace(Some(current));
            return Ok(token);
        }
        let _lock = self.store.lock()?;
        // Another process may have refreshed while this one waited.
        let current = self.store.load().ok_or(format!("not signed in to ChatGPT: {SIGN_IN_AGAIN}"))?;
        if Self::fresh(&current) {
            let token = current.access_token.clone();
            self.cached.replace(Some(current));
            return Ok(token);
        }
        let url = format!("{}/api/accounts/oauth/token", current.issuer.trim_end_matches('/'));
        let fields = [("grant_type", "refresh_token"), ("client_id", &current.client_id), ("refresh_token", &current.refresh_token), ("resource", RESOURCE)];
        let (status, body) = post_form(&self.http, &url, &fields)?;
        if status != 200 {
            let code = error_code(&body);
            if TERMINAL.contains(&code.as_str()) {
                self.store.clear();
                self.cached.replace(None);
                return Err(format!("the ChatGPT sign-in has ended ({code}): {SIGN_IN_AGAIN}"));
            }
            return Err(format!("refreshing the ChatGPT sign-in failed ({status}): {code}"));
        }
        let (Some(access), Some(refresh)) = (body["access_token"].as_str(), body["refresh_token"].as_str()) else {
            return Err("the refresh answer has no tokens".to_string());
        };
        // The old refresh token is spent now: the new one is saved before anything else happens.
        let mut next = current;
        next.access_token = access.to_string();
        next.refresh_token = refresh.to_string();
        next.expires_in = body["expires_in"].as_u64().unwrap_or(3600);
        next.saved_at_s = now_s();
        if let Some(scope) = body["scope"].as_str() {
            next.scopes = scope.split_whitespace().map(str::to_string).collect();
        }
        if let Some(id_token) = body["id_token"].as_str() {
            next.id_token = id_token.to_string();
        }
        self.store.save(&next)?;
        let token = next.access_token.clone();
        self.cached.replace(Some(next));
        Ok(token)
    }
}

impl Auth for ChatGptAuth {
    fn credentials(&self, _provider: &str) -> BoxFuture<'_, PluginResult<Secret>> {
        let answer = self.token().map(Secret::new).map_err(PluginError::new);
        Box::pin(std::future::ready(answer))
    }
}

/// The sign-in's state without its tokens, for the settings.
pub fn status(store: &Store) -> (bool, String) {
    match store.load() {
        Some(c) => (true, format!("Signed in{}", c.email.map_or(String::new(), |e| format!(" as {e}")))),
        None => (false, "Not signed in".to_string()),
    }
}

/// What is not a chat model, on a server that lists everything it has.
const NOT_CHAT: [&str; 9] = ["embed", "whisper", "tts", "dall-e", "moderation", "image", "audio", "transcribe", "realtime"];

/// A model as its server describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedModel {
    pub id: String,
    pub name: String,
    pub context: Option<u64>,
    /// Price per 1M tokens, (input, output), where the server states one, as OpenRouter does.
    pub per_million: Option<(f64, f64)>,
}

/// The models a server offers, in its order. It reads both forms: a plan's `models` with `slug`
/// and `visibility`, and the usual `data` with `id`. No token means none sent.
pub fn list_models(base_url: &str, token: &str) -> Result<Vec<ListedModel>, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = agent(15).get(&url);
    if !token.is_empty() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    read_models(request, base_url)
}

/// The same from Anthropic's server, which takes its key in its own header.
pub fn list_anthropic_models(base_url: &str, key: &str, version: &str) -> Result<Vec<ListedModel>, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    read_models(agent(15).get(&url).header("x-api-key", key).header("anthropic-version", version), base_url)
}

fn read_models(request: ureq::RequestBuilder<ureq::typestate::WithoutBody>, base_url: &str) -> Result<Vec<ListedModel>, String> {
    let mut response = request.call().map_err(|e| format!("reaching {base_url}: {e}"))?;
    if response.status() != 200 {
        return Err(format!("listing the models of {base_url}: {}", response.status()));
    }
    let body: Value = response.body_mut().read_json().map_err(|e| format!("the model list: {e}"))?;
    let token_price = |v: &Value| v.as_str().and_then(|p| p.parse::<f64>().ok()).or(v.as_f64()).map(|p| p * 1_000_000.0);
    if let Some(list) = body["models"].as_array() {
        return Ok(list
            .iter()
            .filter(|m| m["visibility"] == "list")
            .filter_map(|m| Some(ListedModel { id: m["slug"].as_str()?.to_string(), name: m["display_name"].as_str().unwrap_or_default().to_string(), context: m["context_window"].as_u64(), per_million: None }))
            .collect());
    }
    let list = body["data"].as_array().cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            if NOT_CHAT.iter().any(|n| id.contains(n)) {
                return None;
            }
            let price = token_price(&m["pricing"]["prompt"]).zip(token_price(&m["pricing"]["completion"]));
            Some(ListedModel { id: id.to_string(), name: m["display_name"].as_str().or(m["name"].as_str()).unwrap_or_default().to_string(), context: m["context_length"].as_u64().or(m["context_window"].as_u64()), per_million: price })
        })
        .collect())
}
