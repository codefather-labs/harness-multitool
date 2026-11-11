#!/usr/bin/env python3
"""CON-131 amended, CON-132 (journal#883, journal#884): a window whose daemon is away refuses what is
sent with a toast, keeps the text in the composer, shows a reconnecting toast with a pulsing bar and
a growing attempt count, and never hangs. A real window in a pty (read with pyte), started bare as
by hand, with a fake Responses endpoint for its model.
1. stopped daemon: refusal toast < 300 ms after Enter, the text back in the composer, no model call;
   20 keys typed each shown < 200 ms; the reconnecting toast with 'attempt N' growing; the window
   starts a daemon by itself and says 'Reconnected'; the message then sends once.
2. frozen daemon (SIGSTOP, its socket still open): one send costs <= 2.5 s, then the window counts
   it lost and shows the reconnecting toast; the next send is refused < 300 ms; keys still < 200 ms;
   SIGCONT: 'Reconnected'.
Needs pyte. Usage: tools/pty_daemon_down.py <absolute path to multitool> [a directory for pictures]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, http.server, json, pty, re, select, shutil, signal, struct, subprocess, tempfile, termios, threading, time
import pyte

bodies = []
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        emit({"type": "response.output_text.delta", "delta": f"Answer {len(bodies)}."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()

bin_ = sys.argv[1]; keep = sys.argv[2] if len(sys.argv) > 2 else None
COLS, ROWS = 120, 34
home = tempfile.mkdtemp(prefix="mt-down-home"); root = os.path.realpath(tempfile.mkdtemp(prefix="mt-down-root-"))
os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="local"\nmodel="m"\nplan_first="off"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_port}"\n\n[session.auth.local]\nnone=true\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
address = home + "/data/harness-multitool/daemon/address.json"
def cli(*args): return subprocess.run([bin_, *args], env=env, capture_output=True, text=True, cwd=root, timeout=60)
def daemon_pid(): return json.load(open(address))["pid"]
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.01)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def flat(): return re.sub(r"\s+", " ", "\n".join(screen.display))
def composer(): return "\n".join(screen.display[-4:])
def until(what, check, sec):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.01)
    assert check(), f"{what}:\n" + "\n".join(l.rstrip() for l in screen.display)
    return time.time()
def type_(text):
    for ch in text: os.write(fd, ch.encode()); pump(0.005)
def key_latencies(word):
    worst = 0.0
    for ch in word:
        t0 = time.time(); os.write(fd, ch.encode())
        typed = composer()
        until(f"the key {ch!r} shows", lambda: composer() != typed, 2)
        worst = max(worst, time.time() - t0)
    return worst
def clear_composer():
    for _ in range(40): os.write(fd, b"\x7f")
    pump(0.3)
def shot(name):
    if not keep: return
    out = os.path.join(keep, name)
    p = subprocess.Popen([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    start = time.time()
    while p.poll() is None and time.time() - start < 20: pump(0.05)

pump(3.0); os.write(fd, b"\r"); pump(1.5); os.write(fd, b"n"); pump(1.0)
until("connected at start", lambda: "Connected to the daemon, pid" in flat(), 15)
first = daemon_pid()

# 1. Stopped.
assert "stopped the daemon" in cli("daemon", "stop").stdout
until("the loss is told", lambda: "Connection to the daemon lost" in flat(), 2)
until("the reconnecting toast", lambda: "Reconnecting to the daemon" in flat(), 3)
calls = len(bodies)
type_("hello while down"); t0 = time.time(); os.write(fd, b"\r")
t = until("the refusal toast", lambda: "Not sent: no connection to the daemon" in flat(), 2)
print(f"stopped: refusal shown {(t - t0) * 1000:.0f} ms after Enter")
assert t - t0 < 0.3, t - t0
assert "hello while down" in composer(), "the text stays in the composer:\n" + composer()
assert len(bodies) == calls, "nothing reached the model"
worst = key_latencies(" abcdefghijklmnopqrs")
print(f"stopped: slowest of 20 keys shown in {worst * 1000:.0f} ms")
assert worst < 0.2, worst
m = re.search(r"attempt (\d+)", flat()); a1 = int(m.group(1)) if m else 0
until("the attempt count grows", lambda: (re.search(r"attempt (\d+)", flat()) and int(re.search(r"attempt (\d+)", flat()).group(1)) > a1), 5)
print("stopped: attempts", a1, "->", re.search(r"attempt (\d+)", flat()).group(1))
until("the window started a daemon and says so", lambda: "Reconnected to the daemon, pid" in flat(), 15)
assert "Reconnecting to the daemon" not in flat(), "the reconnecting toast went"
second = daemon_pid(); assert second != first
shot("down-reconnected.png")
clear_composer(); calls = len(bodies)
type_("hello again"); os.write(fd, b"\r")
until("the message runs once it is back", lambda: len(bodies) == calls + 1, 15)
pump(1.5); assert len(bodies) == calls + 1, "sent once"
print("stopped: reconnected to", second, "and the next message went once")

# 2. Frozen.
os.kill(second, signal.SIGSTOP)
try:
    clear_composer(); type_("to a frozen daemon"); t0 = time.time(); os.write(fd, b"\r")
    until("the window counts the frozen daemon lost", lambda: "Connection to the daemon lost" in flat() or "Reconnecting to the daemon" in flat(), 4)
    cost = time.time() - t0
    print(f"frozen: the send cost {cost:.2f} s before the window counted the daemon lost")
    assert cost <= 2.5, cost
    pump(0.5); clear_composer(); type_("once more"); t0 = time.time(); os.write(fd, b"\r")
    t = until("refused at once while frozen", lambda: "Not sent: no connection to the daemon" in flat(), 2)
    print(f"frozen: next send refused {(t - t0) * 1000:.0f} ms after Enter")
    assert t - t0 < 0.3, t - t0
    worst = key_latencies(" abcdefghij")
    print(f"frozen: slowest key {worst * 1000:.0f} ms")
    assert worst < 0.2, worst
finally:
    os.kill(second, signal.SIGCONT)
until("reconnected after it thaws", lambda: "Reconnected to the daemon, pid" in flat(), 15)
shot("frozen-reconnected.png")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(1.0)
cli("daemon", "stop")
print("ok")
