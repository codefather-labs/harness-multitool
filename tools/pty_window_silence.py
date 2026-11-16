#!/usr/bin/env python3
"""Debug, journal#1251: a window that is frozen for longer than the daemon's WINDOW_GONE (30 s), as a
laptop's lid or a stopped terminal does, then goes on. Its next prompt must be answered, and a message
from Telegram (a fake Bot API) must reach its session. The real TUI in a pty and the real daemon, a fake
model, a temporary home. Needs pyte. Usage: tools/pty_window_silence.py <absolute path to multitool> [both]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
replies = ["REPLY-BEFORE-THE-FREEZE", "REPLY-AFTER-THE-FREEZE", "REPLY-TO-TELEGRAM"]
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        b = turn(replies.pop(0) if replies else "more")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

TOKEN = "123456:PTYSECRET"
updates = []
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path.endswith("/getUpdates"):
            time.sleep(0.2)
            reply = {"ok": True, "result": [u for u in updates if u["update_id"] >= body["offset"]]}
        else:
            reply = {"ok": True, "result": {"message_id": 7}}
        b = json.dumps(reply).encode(); self.send_response(200); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
    def handle(self):
        try: super().handle()
        except BrokenPipeError: pass
bot = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Bot); threading.Thread(target=bot.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-silence-root"); home = tempfile.mkdtemp(prefix="mt-silence-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[telegram]\nbase_url="http://127.0.0.1:{bot.server_address[1]}"\nallowed_users=[42]\n\n[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
COLS, ROWS = 120, 40; bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(MULTITOOL_TELEGRAM_TOKEN=TOKEN, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(bin_, [bin_, "--new"])
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
def wait(cond, sec):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(0.2)
    return cond()
def type_(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.01)

results = {}
assert wait(lambda: "Enter send" in text(), 20), text()
type_("before\r")
results["answered_before"] = wait(lambda: "REPLY-BEFORE-THE-FREEZE" in text(), 20)
# The lid closes: the window's process is frozen past WINDOW_GONE.
both = len(sys.argv) > 2 and sys.argv[2] == "both"
daemon_pid = json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"]
if both:
    # As a machine's sleep does: the daemon and the window frozen together; the daemon thaws first.
    print("freezing the daemon and the window for 40 s"); os.kill(daemon_pid, signal.SIGSTOP); os.kill(pid, signal.SIGSTOP); time.sleep(40)
    os.kill(daemon_pid, signal.SIGCONT); time.sleep(6); os.kill(pid, signal.SIGCONT); print("thawed, the daemon 6 s before the window")
else:
    print("freezing the window for 40 s"); os.kill(pid, signal.SIGSTOP); time.sleep(40); os.kill(pid, signal.SIGCONT); print("thawed")
pump(3)
type_("after\r")
results["answered_after_the_freeze"] = wait(lambda: "REPLY-AFTER-THE-FREEZE" in text(), 25)
print("===== after the freeze =====\n" + text())
# A Telegram message: bound by itself to the only session, and answered.
updates.append({"update_id": 1, "message": {"message_id": 1, "from": {"id": 42}, "chat": {"id": -100}, "text": "from the phone"}})
results["telegram_reached_it"] = wait(lambda: "REPLY-TO-TELEGRAM" in text(), 25)
print("===== after Telegram =====\n" + text())
print("\nRESULTS:", json.dumps(results))
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.01)
pump(1)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
