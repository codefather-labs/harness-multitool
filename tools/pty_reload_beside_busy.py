#!/usr/bin/env python3
"""CON-130 (journal#857, journal#848): windows of two builds share one daemon, and a reload never waits
on another window's run. Two copies of the build stand for two builds (same protocol, other size or
time): window A opens on copy 1 and starts the daemon; window B opens on copy 2 and must join that
daemon (both windows in its stats, no Local), though A then runs a slow turn there. B asks /reload
during A's turn: B's process must be replaced within 2 s, and A's turn must go on and end in the
same daemon, which is not replaced while it runs.
Needs pyte. Usage: tools/pty_reload_beside_busy.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, http.server, json, pty, select, shutil, struct, subprocess, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}, {"type": "message_stop"}])
calls = []
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        calls.append(body)
        last = json.dumps(body["messages"][-1])
        if "slow one" in last: time.sleep(8)
        b = turn(f"Answer number {len(calls)}.")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()

home = tempfile.mkdtemp(prefix="mt-rbb-home"); root = os.path.realpath(tempfile.mkdtemp(prefix="mt-rbb-root"))
os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/b1"); os.makedirs(home + "/b2")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nplan_first="off"\n\n[session.providers.anthropic]\nbase_url="http://127.0.0.1:{srv.server_port}"\n')
one, two = home + "/b1/multitool", home + "/b2/multitool"
shutil.copy(sys.argv[1], one); shutil.copy(sys.argv[1], two); os.chmod(one, 0o755); os.chmod(two, 0o755)
os.utime(two, (time.time() + 60, time.time() + 60))            # another build: another time
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k not in ("MULTITOOL_CHATGPT_DIR", "MULTITOOL_RELOAD_BUILD")}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
COLS, ROWS = 110, 36
class Win:
    def __init__(self, binary):
        self.screen = pyte.Screen(COLS, ROWS); self.stream = pyte.ByteStream(self.screen)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(binary, [binary, "preview", "welcome"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display)
    def typ(self, s):
        for ch in s: os.write(self.fd, ch.encode()); pump(0.01)
wins = []
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([w.fd for w in wins], [], [], 0.02)
        for w in wins:
            if w.fd in r:
                try: w.stream.feed(os.read(w.fd, 65536))
                except OSError: pass
def stats():
    out = subprocess.run([sys.argv[1], "daemon", "--status"], env=env, capture_output=True, text=True, cwd=root)
    return json.loads(out.stdout)
a = Win(one); wins.append(a); pump(3)
a.typ("hello from a\r"); pump(3)
before = stats()
b = Win(two); wins.append(b); pump(3)
b.typ("hello from b\r"); pump(3)
now = stats()
assert now["pid"] == before["pid"], (before, now)
assert now["conversations"]["windows"] == 2, f"window B of another build joined the daemon, no Local: {now}"
print("two builds, one daemon:", now["pid"], "windows", now["conversations"]["windows"], "socket clients", now["socket_clients"])
a.typ("a slow one\r"); pump(1.0)
assert stats()["conversations"]["busy"] >= 1, "A's turn is on in the daemon"
t0 = time.time(); b.typ("/reload\r")
while time.time() - t0 < 5 and "--handoff" not in open(f"/proc/{b.pid}/cmdline").read(): pump(0.05)
swapped = time.time() - t0
assert "--handoff" in open(f"/proc/{b.pid}/cmdline").read(), "B was not replaced"
print(f"B swapped {swapped:.2f} s after /reload, beside A's slow turn")
assert swapped < 2.0, swapped
assert stats()["pid"] == before["pid"], "the busy daemon was not replaced"
end = time.time() + 20
while time.time() < end and "Answer number" not in a.text().split("a slow one")[-1]: pump(0.1)
assert "Answer number" in a.text().split("a slow one")[-1], "A's slow turn went on and ended:\n" + a.text()
print("A's slow turn ended in the same daemon", stats()["pid"])
for w in wins: os.write(w.fd, b"\x03"); pump(0.3); os.write(w.fd, b"\x03"); pump(0.5)
subprocess.run([sys.argv[1], "daemon", "stop"], env=env, capture_output=True, cwd=root)
print("ok")
