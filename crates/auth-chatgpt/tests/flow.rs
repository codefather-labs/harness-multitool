//! The sign-in and refresh against a fake issuer on real HTTP, with real signatures.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_api::Auth;
use harness_auth_chatgpt::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};

const KEY: &str = include_str!("fixtures/test_only_key.pem");
const MODULUS: &str = include_str!("fixtures/test_only_modulus.txt");
const CLIENT: &str = "oaiapp_test";
const ACCESS: &str = "ACCESS-SECRET";
const REFRESH: &str = "REFRESH-SECRET";

#[derive(Default)]
struct Fake {
    /// What the token endpoint answers for the code exchange: changes to `claims` and the rest.
    claims: Value,
    scope: String,
    forms: Vec<String>,
    refresh_error: Option<&'static str>,
    refreshes: u32,
    revoked: bool,
}

fn fake_issuer() -> (String, Arc<Mutex<Fake>>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
    let fake = Arc::new(Mutex::new(Fake { scope: "openid chatgpt.tokens.use.direct".into(), ..Default::default() }));
    for _ in 0..4 {
        let (server, state, base) = (server.clone(), fake.clone(), url.clone());
        std::thread::spawn(move || {
            while let Ok(mut request) = server.recv() {
                let path = request.url().to_string();
                let mut body = String::new();
                std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();
                let mut s = state.lock().unwrap();
                let (status, reply) = match path.as_str() {
                    "/.well-known/openid-configuration" => (200, json!({"jwks_uri": format!("{base}/jwks"), "revocation_endpoint": format!("{base}/revoke")})),
                    "/jwks" => (200, json!({"keys": [{"kty": "RSA", "kid": "k1", "alg": "RS256", "n": MODULUS.trim(), "e": "AQAB"}]})),
                    "/revoke" => {
                        s.revoked = true;
                        (200, json!({}))
                    }
                    "/api/accounts/oauth/token" => {
                        s.forms.push(body.clone());
                        if body.contains("grant_type=refresh_token") {
                            s.refreshes += 1;
                            std::thread::sleep(Duration::from_millis(100));
                            match s.refresh_error {
                                Some(code) => (400, json!({"error": code})),
                                None => (200, json!({"access_token": format!("{ACCESS}-{}", s.refreshes), "refresh_token": format!("{REFRESH}-{}", s.refreshes), "expires_in": 3600})),
                            }
                        } else {
                            let mut header = Header::new(Algorithm::RS256);
                            header.kid = Some("k1".into());
                            let id_token = encode(&header, &s.claims, &EncodingKey::from_rsa_pem(KEY.as_bytes()).unwrap()).unwrap();
                            (200, json!({"access_token": ACCESS, "refresh_token": REFRESH, "id_token": id_token, "token_type": "Bearer", "expires_in": 3600, "scope": s.scope}))
                        }
                    }
                    _ => (404, json!({})),
                };
                drop(s);
                request.respond(tiny_http::Response::from_string(reply.to_string()).with_status_code(status)).unwrap();
            }
        });
    }
    (url, fake)
}

fn query(url: &str, name: &str) -> String {
    let (_, query) = url.split_once('?').unwrap();
    query.split('&').find_map(|kv| kv.strip_prefix(&format!("{name}="))).unwrap_or("").to_string()
}

fn get(url: &str) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    agent.get(url).call().unwrap().status().as_u16()
}

fn exp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600
}

fn good_claims(issuer: &str, nonce: &str) -> Value {
    json!({"iss": issuer, "aud": CLIENT, "exp": exp(), "sub": "user-1", "email": "a@example.com", "nonce": nonce})
}

/// Plays the browser: the fake issuer signs in and redirects to the callback.
fn sign_in(store: &Store, issuer: &str, fake: &Arc<Mutex<Fake>>, change: impl FnOnce(&mut Value)) -> Result<Credential, String> {
    let pending = begin_login(store, issuer, 0).unwrap();
    let (state, nonce) = (query(&pending.url, "state"), query(&pending.url, "nonce"));
    let redirect = query(&pending.url, "redirect_uri").replace("%3A", ":").replace("%2F", "/");
    let mut claims = good_claims(issuer, &nonce);
    change(&mut claims);
    fake.lock().unwrap().claims = claims;
    let callback = format!("{redirect}?code=the-code&state={state}&client_id={CLIENT}");
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        get(&callback);
    });
    pending.finish(store, Duration::from_secs(10))
}

fn dirs() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    (dir, store)
}

fn files_hold(dir: &Path, secret: &str) -> bool {
    std::fs::read_to_string(dir.join("auth/chatgpt/credentials.json")).is_ok_and(|t| t.contains(secret))
}

#[test]
fn a_sign_in_registers_validates_and_saves_owner_only() {
    let (url, fake) = fake_issuer();
    let (dir, store) = dirs();
    let pending = begin_login(&store, &url, 0).unwrap();
    for (name, want) in [("client_id", "dynamic_agent_client"), ("agent_name_hint", "harness-multitool"), ("response_type", "code"), ("code_challenge_method", "S256"), ("resource", "https%3A%2F%2Fapi.openai.com%2Fv1")] {
        assert_eq!(query(&pending.url, name), want, "{name}");
    }
    assert!(query(&pending.url, "scope").contains("chatgpt.tokens.use.direct"));
    assert!(query(&pending.url, "redirect_uri").starts_with("http%3A%2F%2F127.0.0.1%3A"));
    assert!(query(&pending.url, "ext_agent_host_id").starts_with("urn%3Auuid%3A"));
    drop(pending);

    let credential = sign_in(&store, &url, &fake, |_| {}).unwrap();
    assert_eq!((credential.client_id.as_str(), credential.subject.as_str()), (CLIENT, "user-1"));
    assert!(files_hold(dir.path(), REFRESH));
    let forms = fake.lock().unwrap().forms.clone();
    assert!(forms[0].contains("grant_type=authorization_code") && forms[0].contains("code_verifier=") && forms[0].contains(&format!("client_id={CLIENT}")), "{forms:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("auth/chatgpt/credentials.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // The next sign-in is the same client, with no name hint, and the same host.
    let host = query(&begin_login(&store, &url, 0).unwrap().url, "ext_agent_host_id");
    let again = begin_login(&store, &url, 0).unwrap();
    assert_eq!(query(&again.url, "client_id"), CLIENT);
    assert_eq!(query(&again.url, "agent_name_hint"), "");
    assert_eq!(query(&again.url, "ext_agent_host_id"), host);
}

#[test]
fn a_bad_sign_in_saves_nothing() {
    let (url, fake) = fake_issuer();
    type Change = Box<dyn FnOnce(&mut Value)>;
    let bad: [(&str, Change); 4] = [
        ("wrong nonce", Box::new(|c| c["nonce"] = json!("other"))),
        ("wrong audience", Box::new(|c| c["aud"] = json!("someone-else"))),
        ("expired", Box::new(|c| c["exp"] = json!(1000))),
        ("wrong issuer", Box::new(|c| c["iss"] = json!("https://evil.example"))),
    ];
    for (what, change) in bad {
        let (dir, store) = dirs();
        assert!(sign_in(&store, &url, &fake, change).is_err(), "{what}");
        assert!(store.load().is_none() && !files_hold(dir.path(), REFRESH), "{what}");
    }
    let (_dir, store) = dirs();
    fake.lock().unwrap().scope = "openid profile".into();
    let answer = sign_in(&store, &url, &fake, |_| {}).err().unwrap();
    assert!(answer.contains("plan usage was not granted"), "{answer}");
    assert!(store.load().is_none());
}

#[test]
fn a_callback_with_a_wrong_state_is_ignored_and_a_refusal_stops() {
    let (url, _fake) = fake_issuer();
    let (_dir, store) = dirs();
    let pending = begin_login(&store, &url, 0).unwrap();
    let state = query(&pending.url, "state");
    let redirect = query(&pending.url, "redirect_uri").replace("%3A", ":").replace("%2F", "/");
    std::thread::spawn(move || {
        assert_eq!(get(&format!("{redirect}?code=stolen&state=wrong&client_id=x")), 400);
        assert_eq!(get(&format!("{redirect}?error=access_denied&state={state}")), 200);
    });
    let answer = pending.finish(&store, Duration::from_secs(10)).err().unwrap();
    assert!(answer.contains("access_denied"), "{answer}");
    assert!(store.load().is_none());
}

fn expire(dir: &Path) {
    let path = dir.join("auth/chatgpt/credentials.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    c["saved_at_s"] = json!(1000);
    std::fs::write(path, c.to_string()).unwrap();
}

#[test]
fn tokens_refresh_near_expiry_and_rotate() {
    let (url, fake) = fake_issuer();
    let (dir, store) = dirs();
    sign_in(&store, &url, &fake, |_| {}).unwrap();
    let auth = ChatGptAuth::new(dir.path());
    assert_eq!(pollster::block_on(auth.credentials("x")).unwrap().expose(), ACCESS);
    assert_eq!(fake.lock().unwrap().refreshes, 0, "still fresh");

    expire(dir.path());
    let auth = ChatGptAuth::new(dir.path());
    assert_eq!(pollster::block_on(auth.credentials("x")).unwrap().expose(), format!("{ACCESS}-1"));
    assert!(files_hold(dir.path(), &format!("{REFRESH}-1")), "the rotating token is saved");
    assert!(fake.lock().unwrap().forms.last().unwrap().contains(&format!("refresh_token={REFRESH}")));
    assert!(!fake.lock().unwrap().forms.last().unwrap().contains("dynamic_agent_client"));
}

#[test]
fn several_processes_refresh_once() {
    let (url, fake) = fake_issuer();
    let (dir, store) = dirs();
    sign_in(&store, &url, &fake, |_| {}).unwrap();
    expire(dir.path());
    let path = dir.path().to_path_buf();
    let threads: Vec<_> = (0..4).map(|_| { let p = path.clone(); std::thread::spawn(move || pollster::block_on(ChatGptAuth::new(&p).credentials("x")).unwrap().expose().to_string()) }).collect();
    let tokens: Vec<String> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(fake.lock().unwrap().refreshes, 1, "{tokens:?}");
    assert!(tokens.iter().all(|t| t == &format!("{ACCESS}-1")), "{tokens:?}");
}

#[test]
fn a_terminal_refresh_error_asks_for_a_sign_in_and_hides_the_tokens() {
    let (url, fake) = fake_issuer();
    let (dir, store) = dirs();
    sign_in(&store, &url, &fake, |_| {}).unwrap();
    expire(dir.path());
    fake.lock().unwrap().refresh_error = Some("refresh_token_reused");
    let error = pollster::block_on(ChatGptAuth::new(dir.path()).credentials("x")).err().unwrap().to_string();
    assert!(error.contains("multitool login chatgpt"), "{error}");
    assert!(!error.contains("SECRET"), "{error}");
    assert!(store.load().is_none(), "unusable tokens are cleared");

    // Not signed in is the same advice.
    let error = pollster::block_on(ChatGptAuth::new(dir.path()).credentials("x")).err().unwrap().to_string();
    assert!(error.contains("multitool login chatgpt"), "{error}");
}

#[test]
fn signing_out_revokes_and_keeps_the_client() {
    let (url, fake) = fake_issuer();
    let (_dir, store) = dirs();
    sign_in(&store, &url, &fake, |_| {}).unwrap();
    assert!(logout(&store).unwrap());
    assert!(fake.lock().unwrap().revoked);
    assert!(store.load().is_none());
    assert_eq!(query(&begin_login(&store, &url, 0).unwrap().url, "client_id"), CLIENT);
}
