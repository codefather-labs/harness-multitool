#!/usr/bin/env python3
"""CON-130 amended (journal#885): a busy daemon of another protocol is waited for, never worked around.
Two cases, each with a fake daemon in this script holding the data directory:
- protocol 2 on the socket: refuses hello with protocol 2, answers stop_if_idle with stopped:false
  while 'busy', stopped:true once free, and then goes away;
- a build from before the socket (protocol 0, HTTP only): /health answers, POST /shutdown with
  if_idle gets 409 while busy, 200 once free, and then it goes away.
A window started bare goes into a session and must: show the reconnecting toast naming the
protocol and that it is busy, show no 'runs its conversations itself' toast, refuse a send; once the
fake is free, start its own daemon (protocol 1), say 'Connected to the daemon, pid N' within 5 s,
and run a message (fake Responses endpoint).
Needs pyte. Usage: tools/pty_daemon_other_busy.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, http.server, json, pty, re, select, socket, struct, subprocess, tempfile, termios, threading, time
import pyte

bodies = []
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        for e in [{"type": "response.output_text.delta", "delta": "Answer."}, {"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}}]:
            self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()
bin_ = sys.argv[1]
COLS, ROWS = 120, 34

class Fake:
    """A daemon of another protocol: busy until `free_at`, then it agrees to stop and goes away."""
    def __init__(self, data, protocol, busy_for):
        self.data, self.protocol, self.free_at, self.asked, self.gone = data, protocol, time.time() + busy_for, [], False
        os.makedirs(data + "/daemon", exist_ok=True)
        fake = self
        class Http(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a): pass
            def reply(self, code, body):
                b = json.dumps(body).encode(); self.send_response(code); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
            def do_GET(self):
                if fake.gone: return self.reply(503, {})
                self.reply(200, {"ok": True, "build": "old"}) if self.path == "/health" else self.reply(404, {})
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
                fake.asked.append(("http", self.path, body))
                if self.path == "/shutdown" and '"if_idle":true' in body:
                    if time.time() < fake.free_at: return self.reply(409, {"busy": True})
                    self.reply(200, {"ok": True}); fake.go()
                else: self.reply(404, {})
        self.http = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Http); threading.Thread(target=self.http.serve_forever, daemon=True).start()
        address = {"port": self.http.server_address[1], "admin": "x", "pid": os.getpid()}
        self.sock_path = data + "/daemon/daemon.sock"
        if protocol:
            address.update(socket=self.sock_path, protocol=protocol)
            self.listener = socket.socket(socket.AF_UNIX); self.listener.bind(self.sock_path); self.listener.listen(16)
            threading.Thread(target=self.accept, daemon=True).start()
        json.dump(address, open(data + "/daemon/address.json", "w"))
    def accept(self):
        while not self.gone:
            try: conn, _ = self.listener.accept()
            except OSError: return
            threading.Thread(target=self.serve, args=(conn,), daemon=True).start()
    def serve(self, conn):
        f = conn.makefile("rwb")
        line = f.readline()
        if not line: return conn.close()
        msg = json.loads(line); self.asked.append(("socket", list(msg)[0], ""))
        if "stop_if_idle" in msg:
            free = time.time() >= self.free_at
            f.write((json.dumps({"stopped": free}) + "\n").encode()); f.flush(); conn.close()
            if free: self.go()
        else:
            f.write((json.dumps({"refused": {"protocol": self.protocol, "error": f"this daemon speaks protocol {self.protocol}, not 1"}}) + "\n").encode()); f.flush(); conn.close()
    def go(self):
        self.gone = True
        if self.protocol:
            try: self.listener.close(); os.remove(self.sock_path)
            except OSError: pass
        threading.Thread(target=self.http.shutdown, daemon=True).start()

def case(protocol):
    home = tempfile.mkdtemp(prefix=f"mt-ob{protocol}-home"); root = os.path.realpath(tempfile.mkdtemp(prefix=f"mt-ob{protocol}-root-"))
    os.makedirs(home + "/cfg/harness-multitool")
    open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="local"\nmodel="m"\nplan_first="off"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{model.server_port}"\n\n[session.auth.local]\nnone=true\n')
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
    env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    fake = Fake(home + "/data/harness-multitool", protocol, busy_for=12)
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
    def until(what, check, sec):
        end = time.time() + sec
        while time.time() < end and not check(): pump(0.02)
        assert check(), f"protocol {protocol}: {what}:\n" + "\n".join(l.rstrip() for l in screen.display)
    pump(3.0); os.write(fd, b"\r"); pump(1.5); os.write(fd, b"n"); pump(1.0)
    until("the reconnecting toast names the busy daemon", lambda: f"protocol {protocol}" in flat() and "Reconnecting to the daemon" in flat(), 8)
    assert "runs its conversations itself" not in flat(), "no Local"
    for ch in "hi": os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
    until("a send is refused while it waits", lambda: "Not sent: no connection to the daemon" in flat(), 2)
    print(f"protocol {protocol}: waiting, named in the toast, sending refused; asked to stop: {len(fake.asked)} times so far")
    assert any(a[1] in ("/shutdown", "stop_if_idle") for a in fake.asked), fake.asked
    assert not [a for a in fake.asked if a[1] == "/shutdown" and '"if_idle":true' not in a[2]], "never asked to stop outright"
    until("connected to this build's daemon once the other is free", lambda: "Connected to the daemon, pid" in flat() or "Reconnected to the daemon, pid" in flat(), 20)
    took = time.time() - fake.free_at
    print(f"protocol {protocol}: connected {took:.1f} s after the other daemon was free")
    assert took < 5, took
    address = json.load(open(home + "/data/harness-multitool/daemon/address.json"))
    assert address.get("protocol") == 1 and address["pid"] != os.getpid(), address
    for _ in range(10): os.write(fd, b"\x7f")
    pump(0.3); calls = len(bodies)
    for ch in "now it runs": os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
    until("the message runs in the new daemon", lambda: len(bodies) > calls, 15)
    print(f"protocol {protocol}: a message ran in daemon pid {address['pid']}")
    os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(1.0)
    subprocess.run([bin_, "daemon", "stop"], env=env, capture_output=True, cwd=root, timeout=30)

case(2)
case(0)
print("ok")
