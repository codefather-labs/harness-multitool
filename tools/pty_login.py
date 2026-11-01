#!/usr/bin/env python3
"""Debug: the Auth tab in a real TUI in a pty, against a fake OpenAI: Login, cancel, a sign-in with a
real signature, the plan's models, Ctrl+S, a message that goes with the plan token, and a key typed
masked. Needs pyte and openssl. Usage: tools/pty_login.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import base64, fcntl, hashlib, http.server, json, pty, re, select, struct, subprocess, tempfile, termios, threading, time, urllib.parse, urllib.request
import pyte

here = os.path.dirname(os.path.abspath(__file__))
KEY = here + "/../crates/auth-chatgpt/tests/fixtures/test_only_key.pem"
MODULUS = open(here + "/../crates/auth-chatgpt/tests/fixtures/test_only_modulus.txt").read().strip()
ACCESS, REFRESH, API_KEY = "PLAN-ACCESS-SECRET", "PLAN-REFRESH-SECRET", "sk-TYPED-SECRET-KEY"
b64 = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()

def id_token(iss, aud, nonce):
    head = b64(json.dumps({"alg": "RS256", "typ": "JWT", "kid": "k1"}).encode())
    claims = b64(json.dumps({"iss": iss, "aud": aud, "exp": int(time.time()) + 3600, "sub": "user-1", "email": "you@example.com", "nonce": nonce}).encode())
    signed = subprocess.run(["openssl", "dgst", "-sha256", "-sign", KEY], input=f"{head}.{claims}".encode(), capture_output=True, check=True).stdout
    return f"{head}.{claims}.{b64(signed)}"

state = {"nonce": "", "responses": [], "revoked": False}
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, code, body, kind="application/json"):
        b = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(code); self.send_header("content-type", kind); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        base = f"http://127.0.0.1:{srv.server_address[1]}"
        if self.path == "/.well-known/openid-configuration": return self.reply(200, {"jwks_uri": base + "/jwks", "revocation_endpoint": base + "/revoke"})
        if self.path == "/jwks": return self.reply(200, {"keys": [{"kty": "RSA", "kid": "k1", "n": MODULUS, "e": "AQAB"}]})
        if self.path == "/v1/models":
            assert self.headers["authorization"] == f"Bearer {ACCESS}"
            return self.reply(200, {"models": [{"slug": "plan-model-a", "display_name": "A", "visibility": "list"}, {"slug": "plan-model-b", "display_name": "B", "visibility": "list"}, {"slug": "hidden", "visibility": "hide"}]})
        self.reply(404, {})
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        if self.path == "/revoke": state["revoked"] = True; return self.reply(200, b"")
        if self.path == "/api/accounts/oauth/token":
            form = urllib.parse.parse_qs(body.decode())
            return self.reply(200, {"access_token": ACCESS, "refresh_token": REFRESH, "id_token": id_token(f"http://127.0.0.1:{srv.server_address[1]}", form["client_id"][0], state["nonce"]), "token_type": "Bearer", "expires_in": 3600, "scope": "openid email chatgpt.tokens.use.direct"})
        if self.path == "/v1/responses":
            state["responses"].append((self.headers["authorization"], json.loads(body)))
            events = [{"type": "response.output_text.delta", "delta": "Answered on the plan."}, {"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}}]
            return self.reply(200, "".join(f"data: {json.dumps(e)}\n\n" for e in events).encode(), "text/event-stream")
        self.reply(404, {})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{srv.server_address[1]}"

root = tempfile.mkdtemp(prefix="mt-login-root"); home = tempfile.mkdtemp(prefix="mt-login-home"); os.makedirs(home + "/cfg/harness-multitool")
browser = home + "/browser.sh"
open(browser, "w").write(f'#!/bin/sh\nprintf %s "$1" > {home}/url\n'); os.chmod(browser, 0o755)
cfg = home + "/cfg/harness-multitool/config.toml"
open(cfg, "w").write(f'[session.providers.chatgpt]\nkind="openai-responses"\nbase_url="{base}/v1"\n\n[session.auth.chatgpt]\noauth="chatgpt"\nissuer="{base}"\n')
COLS, ROWS = 130, 46; bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", MULTITOOL_BROWSER=browser, TERM="xterm-256color", MULTITOOL_GRAPHICS="off"); os.environ.pop("OPENAI_API_KEY", None)
    os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def show(t): print(f"\n===== {t} =====\n{text()}")
def keys(*ks):
    for k in ks: os.write(fd, k if isinstance(k, bytes) else k.encode()); pump(0.1)
DOWN, UP, TAB, ENTER, ESC = b"\x1b[B", b"\x1b[A", b"\t", b"\r", b"\x1b"
pump(1.5)
for _ in range(50):
    if "Ask about the repo" in text(): break
    pump(0.2)
for c in "/settings\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5); assert "Settings" in text(), text()
assert "claude-subscription" in text() and "not available" in text(), "the option stays, disabled"
keys(DOWN, TAB); show("the Auth tab")
assert "gpt-subscription" in text() and "Enter a key" in text(), text()
assert "claude-subscription" in text()

# Choose the plan: the button becomes Login. Cancel frees the port for the next attempt.
keys(DOWN, DOWN, ENTER); pump(0.2); show("claude-subscription pressed: a toast")
assert "forbidden by Anthropic" in text(), text()
keys(UP, ENTER, DOWN, DOWN); assert "Login" in text() and "Not signed in" in text(), text()
keys(ENTER); pump(0.5); show("Login pressed"); assert "Cancel" in text(), text()
first = open(home + "/url").read(); os.remove(home + "/url"); assert first.startswith(base + "/api/accounts/authorize?"), first
keys(ENTER); pump(0.6); show("cancelled"); assert "cancelled" in text() and "Login" in text(), text()
keys(ENTER); pump(0.5); assert "Could not start" not in text(), "the port is free again: " + text()
url = open(home + "/url").read(); q = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)
assert q["client_id"] == ["dynamic_agent_client"] and q["agent_name_hint"] == ["harness-multitool"], q
state["nonce"] = q["nonce"][0]
urllib.request.urlopen(f"{q['redirect_uri'][0]}?code=c&state={q['state'][0]}&client_id=oaiapp_test").read()
pump(1.5); show("signed in"); assert "Signed in as you@example.com" in text() and "plan-model-a" in text(), text()
keys(ENTER); assert "plan-model-b" in text(), text()
keys(b"\x13"); pump(0.8); show("saved"); assert "New sessions use chatgpt / plan-model-b" in text(), text()
saved = open(cfg).read(); assert 'provider = "chatgpt"' in saved and "plan-model-b" in saved, saved
assert ACCESS not in saved

for c in "hello\r": os.write(fd, c.encode()); time.sleep(0.005)
for _ in range(50):
    if state["responses"]: break
    pump(0.2)
pump(1.0); show("a message on the plan"); auth, body = state["responses"][0]
assert auth == f"Bearer {ACCESS}" and body["model"] == "plan-model-b" and body["store"] is False and body["stream"] is True, (auth, body)
assert "Answered on the plan." in text(), text()

# A key for another provider: typed masked, saved owner-only, on no screen.
for c in "/settings\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5); keys(DOWN, TAB, UP, UP, UP, UP, ENTER); pump(0.2)
keys(DOWN, DOWN, DOWN); show("back on api-key"); assert "Enter a key" in text(), text()
keys(ENTER)
for c in API_KEY: os.write(fd, c.encode()); time.sleep(0.005)
pump(0.3); show("typing a key"); assert API_KEY not in text() and "•" in text(), text()
keys(ENTER); pump(0.6); show("key saved"); assert "Saved for" in text() and API_KEY not in text(), text()
saved = open(cfg).read(); assert API_KEY in saved, saved
assert oct(os.stat(cfg).st_mode & 0o777) == "0o600", oct(os.stat(cfg).st_mode)
keys(ESC); pump(0.2)
data = home + "/data/harness-multitool"
for dirpath, _, files in os.walk(data):
    for f in files:
        if f.endswith(".jsonl") or f.endswith(".json") and "credentials" not in f:
            t = open(os.path.join(dirpath, f), errors="ignore").read()
            assert ACCESS not in t and REFRESH not in t and API_KEY not in t, f
creds = data + "/auth/chatgpt/credentials.json"; assert oct(os.stat(creds).st_mode & 0o777) == "0o600"
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5); print("exit:", os.waitpid(pid, os.WNOHANG)); print("ok")
