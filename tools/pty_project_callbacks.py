#!/usr/bin/env python3
"""Debug, CON-183: a project's custom callback in the real TUI. A temp project whose
.harness/callbacks/echo holds a compiled Rust handler (the SDK's test-echo, built by cargo test -p
harness-callback). One window, two tabs on two conversations:
- the window says at opening that a handler waits for approval;
- in tab 1, Settings > Callbacks: a then a approves it, Space subscribes tab 1's conversation;
- a POST of a `ping` to tab 1's URL is answered 202 started, and tab 1's transcript shows the
  handler's message; tab 2's does not;
- in tab 2, the Callbacks page shows echo approved but not subscribed, and a ping to its URL is 400.
Needs pyte. Usage: tools/pty_project_callbacks.py <absolute path to multitool> <absolute path to test-echo>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, re, select, shutil, signal, struct, tempfile, termios, threading, time, urllib.request, urllib.error
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        b = turn("Noted.")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-pcb-root"); home = tempfile.mkdtemp(prefix="mt-pcb-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
folder = root + "/.harness/callbacks/echo"; os.makedirs(folder)
open(folder + "/config.toml", "w").write('handler = "handler"\ndescription = "says it back"\ntypes = ["ping"]\n')
shutil.copy(sys.argv[2], folder + "/handler"); os.chmod(folder + "/handler", 0o755)
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-pcb-proof")); os.makedirs(proof_dir, exist_ok=True)

class Window:
    def __init__(self):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.clear(); os.environ.update(env)
            os.execv(sys.argv[1], [sys.argv[1], "--new"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 140, 0, 0))
        self.screen = pyte.Screen(140, 50); self.stream = pyte.ByteStream(self.screen); self.seen = ""
    def pump(self, sec):
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.stream.feed(os.read(self.fd, 65536))
                except OSError: return
                self.seen += self.text() + "\n"
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display)
    def wait(self, cond, sec):
        end = time.time() + sec
        while time.time() < end and not cond(): self.pump(0.2)
        return cond()
    def key(self, seq): os.write(self.fd, seq.encode()); time.sleep(0.15)
    def type(self, s):
        for c in s: os.write(self.fd, c.encode()); time.sleep(0.05)
    def say(self, words):
        self.type(words + "\r")
        assert self.wait(lambda: "Noted." in self.text(), 20), self.text()
    def callbacks(self):
        """Settings, the Callbacks section, the page focused, the cursor on the last row."""
        self.type("/settings\r"); self.pump(2)
        for _ in range(20):
            if "CALLBACKS   profile" in self.text(): break
            self.key("\x1b[B"); self.pump(0.3)
        self.key("\x1b[C"); self.pump(0.5)
        for _ in range(30): self.key("\x1b[B")
        self.pump(1.5)
    def url(self):
        # The page's column, its lines joined: a long URL wraps over two.
        page = "".join(l.split("│")[2].strip() for l in self.text().splitlines() if l.count("│") >= 3)
        m = re.search(r"(http://127\.0\.0\.1:\d+/callback/[0-9a-f-]{36})", page)
        return m.group(1) if m else None

def masked(text): return re.sub(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{0,12}", "<token>", text)

def post(url, body):
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers={"content-type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=5) as r: return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e: return e.code, json.loads(e.read() or b"null")

results = {}
w = Window()
assert w.wait(lambda: "Enter send" in w.text(), 25), w.text()
results["toast_at_opening"] = w.wait(lambda: "callback handler to approve" in w.seen, 5)
w.say("hello from tab one")
# Tab 2, its own conversation.
w.key("\x14"); assert w.wait(lambda: "Enter send" in w.text(), 10)
w.say("hello from tab two")
w.callbacks()
open(proof_dir + "/tab2-before.txt", "w").write(masked(w.text()))
url2 = w.url()
w.key("\x1b"); w.pump(0.8)
# Tab 1: approve (a, a) and subscribe (Space).
w.key("\x1b1"); w.pump(1)
w.callbacks()
url1 = w.url()
open(proof_dir + "/tab1-unapproved.txt", "w").write(masked(w.text()))
results["listed_not_approved"] = "not approved" in w.text() and "echo" in w.text()
w.type("a"); w.pump(0.8)
results["approval_asks_naming_the_path"] = "a again approves" in w.text() and folder + "/handler:" in w.text()
open(proof_dir + "/tab1-asking.txt", "w").write(masked(w.text()))
w.type("a"); w.pump(1.5)
w.type(" "); w.pump(1.5)
open(proof_dir + "/tab1-subscribed.txt", "w").write(masked(w.text()))
results["approved_and_subscribed"] = bool(re.search(r"● on\s+echo\s+approved", w.text()))
w.key("\x1b"); w.pump(0.8)
print("urls", url1 and url1.split("/callback/")[0], url2 and url2.split("/callback/")[0], "distinct", url1 != url2)
results["two_urls"] = bool(url1 and url2 and url1 != url2)
code, said = post(url1, {"uuid": "pty-1", "type": "ping", "payload": {"n": 1}})
print("tab 1 ping:", code, said)
results["tab1_started_202"] = code == 202 and said.get("result") == "started"
results["tab1_gets_the_handlers_message"] = w.wait(lambda: "echo ping for" in w.text(), 15)
open(proof_dir + "/tab1-after-ping.txt", "w").write(masked(w.text()))
code2, said2 = post(url2, {"uuid": "pty-2", "type": "ping"})
print("tab 2 ping:", code2, said2)
results["tab2_not_subscribed_400"] = code2 == 400 and "not subscribed" in said2.get("error", "")
# Tab 2: nothing came, and its page shows echo approved, not subscribed.
w.key("\x1b2"); w.pump(1.5)
results["tab2_got_nothing"] = "echo ping for" not in w.text()
w.callbacks()
open(proof_dir + "/tab2-after.txt", "w").write(masked(w.text()))
results["tab2_page_approved_off"] = bool(re.search(r"○ off\s+echo\s+approved", w.text()))
w.key("\x1b"); w.pump(0.5)
log = open(home + "/data/harness-multitool/daemon/triggers.jsonl").read()
open(proof_dir + "/triggers.jsonl", "w").write(masked(log))
results["log_started_ran_handled"] = all(f'"outcome":"{o}"' in log for o in ["started", "ran", "handled"])
print("\nRESULTS:", json.dumps(results))
os.kill(w.pid, signal.SIGKILL)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
shutil.rmtree(root, ignore_errors=True); shutil.rmtree(home, ignore_errors=True)
print("ALL PASSED" if all(results.values()) else "FAILED")
