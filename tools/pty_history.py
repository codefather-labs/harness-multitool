#!/usr/bin/env python3
"""Debug, CON-199: Up on the composer's top row recalls what was sent, and Enter sends it again.
The real TUI on a fake model: send 'first' and 'second', press Up twice, see 'first' in the
composer, press Enter, and the model's third request ends with the user message 'first'.
Needs pyte. Usage: tools/pty_history.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, shutil, signal, struct, tempfile, termios, threading, time
import pyte

B = sys.argv[1]
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-hist-proof")); os.makedirs(proof_dir, exist_ok=True)

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
requests = []; lock = threading.Lock()
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        with lock: requests.append(body)
        b = turn("OK-%d" % len(requests))
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-hist-root"); home = tempfile.mkdtemp(prefix="mt-hist-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
ENV = dict(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(ENV); os.execv(B, [B, "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
screen = pyte.Screen(120, 40); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def composer():
    # The composer's box is near the bottom: the rows between its borders that hold text.
    rows = [l for l in screen.display[-8:]]
    return "\n".join(rows)
def typed(t):
    for c in t: os.write(fd, c.encode()); time.sleep(0.003)
def until(what, cond, sec=20):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(0.2)
    ok = cond(); print(("ok   " if ok else "FAIL ") + what); return ok

results = {}
end = time.time() + 30
while time.time() < end and "Enter send" not in text(): pump(0.3)
typed("first"); os.write(fd, b"\r")
results["first_answered"] = until("the first message answered", lambda: "OK-1" in text())
typed("second"); os.write(fd, b"\r")
results["second_answered"] = until("the second message answered", lambda: "OK-2" in text())
UP = b"\x1b[A"
os.write(fd, UP); pump(0.6)
results["up_shows_second"] = "second" in composer(); print(("ok   " if results["up_shows_second"] else "FAIL ") + "Up shows 'second' in the composer")
os.write(fd, UP); pump(0.6)
shot = composer()
results["up_up_shows_first"] = "first" in shot and "second" not in shot; print(("ok   " if results["up_up_shows_first"] else "FAIL ") + "Up again shows 'first'")
open(proof_dir + "/composer-after-up-up.txt", "w").write(shot)
os.write(fd, b"\r")
results["third_request"] = until("a third request reaches the model", lambda: len(requests) >= 3)
last_user = None
if len(requests) >= 3:
    users = [m for m in requests[2]["messages"] if m["role"] == "user"]
    content = users[-1]["content"]
    last_user = content if isinstance(content, str) else " ".join(c.get("text", "") for c in content if isinstance(c, dict))
results["first_sent_again"] = last_user is not None and last_user.strip() == "first"; print(("ok   " if results["first_sent_again"] else "FAIL ") + "the model's last user message is 'first': %r" % last_user)
open(proof_dir + "/screen.txt", "w").write(text())
try: os.kill(pid, signal.SIGTERM)
except Exception: pass
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
time.sleep(0.5)
shutil.rmtree(root, ignore_errors=True); shutil.rmtree(home, ignore_errors=True)
print("\nRESULTS:", json.dumps(results))
print("ALL PASSED" if all(results.values()) else "FAILED")
