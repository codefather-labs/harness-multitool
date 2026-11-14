#!/usr/bin/env python3
"""Debug: the harness inside a launcher that starts `claude` from the PATH and types messages in
(run.sh --under), with a fake provider. What the launcher gives reaches a session's tools; a message
reaches the model; a message during a permission prompt does not answer it; a real paste does not
send itself. Needs pyte. LAUNCHER is the launcher's command, with a NICK to send to.
Usage: LAUNCHER="<command>" SENDER="<command taking text>" tools/pty_full.py <repo> """
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, shlex, signal, struct, subprocess, tempfile, termios, threading, time
import pyte

bodies = []
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode()); n = len(bodies)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        call = lambda c: emit({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": f"c{n}", "name": "shell", "arguments": json.dumps({"command": c})}})
        if n == 1: call(os.environ["IDENTITY_COMMAND"])
        elif "FIRST-PING" in bodies[-1] and "made-by-prompt" not in bodies[-1]: call("touch made-by-prompt")
        else: emit({"type": "response.output_text.delta", "delta": f"Answer {n}."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-full-root")); home = tempfile.mkdtemp(prefix="mt-full-home"); os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/bin")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="fake"\nmodel="m"\nplan_first="off"\n\n[session.providers.fake]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.fake]\nnone=true\n')
# The launcher keeps its own configuration: only the harness gets these folders.
binary = sys.argv[1]
os.environ["MULTITOOL_SKIP"] = "1"
wrapper = home + "/harness"; open(wrapper, "w").write(f'#!/bin/sh\nXDG_CONFIG_HOME={home}/cfg MULTITOOL_HOME={home}/mt-home XDG_DATA_HOME={home}/data TERM=xterm-256color MULTITOOL_GRAPHICS=off exec {binary} "$@"\n'); os.chmod(wrapper, 0o755)
shim = home + "/bin/claude"; path = os.environ["PATH"]
open(shim, "w").write(f'#!/bin/sh\nexport PATH={shlex.quote(path)}\nexec {wrapper} preview welcome\n'); os.chmod(shim, 0o755)
drop = [k for k in os.environ if k.startswith("CLAUDE")]
env = {k: v for k, v in os.environ.items() if k not in drop}; env["PATH"] = home + "/bin:" + path; env["TERM"] = "xterm-256color"
COLS, ROWS = 130, 46
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); a = shlex.split(os.environ["LAUNCHER"]); os.execvp(a[0], a)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def until(what, check, sec=15):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.1)
    assert check(), what + ": " + text()
send = lambda t: subprocess.run(shlex.split(os.environ["SENDER"]) + [t], capture_output=True, text=True, check=True)
pump(4)
os.write(fd, b"\x1b[200~line one\nline two\x1b[201~"); pump(0.8)
assert not bodies and "line one" in text(), "a paste waits in the composer: " + text()
for _ in range(20): os.write(fd, b"\x7f")
pump(0.3)
for c in "go\r": os.write(fd, c.encode()); time.sleep(0.01)
# Enter, not a digit: the launcher takes a printable key for a line being typed, and holds messages back.
until("the tool's prompt", lambda: "1 once" in text()); pump(1.3); os.write(fd, b"\r")
until("the identity reached a tool", lambda: len(bodies) >= 2)
want = os.environ["IDENTITY_EXPECT"]; assert want in bodies[1], "the launcher's identity reached the tool: " + bodies[1][-300:]
print("what the launcher gives reached a session's tool"); until("idle", lambda: "Esc stops" not in text()); pump(1)
send("FIRST-PING"); until("its own card", lambda: "AGENT" in text() and "FIRST-PING" in text()); composer = [l for l in text().splitlines() if "Ask about" in l]; assert composer, "the composer stays empty: " + text()
until("a message reached the model", lambda: any("FIRST-PING" in b for b in bodies))
assert any("[callback:" in b and "FIRST-PING" in b for b in bodies), "marked by its kind, as from outside (CON-158)"
print("a message reached the model, marked as from outside")
until("the prompt from it", lambda: "1 once" in text()); pump(0.2)
send("SECOND-PING"); pump(5)
assert "1 once" in text(), "a message did not answer the prompt: " + text()
assert not os.path.exists(root + "/made-by-prompt"), "and the tool did not run"
print("a message during a prompt did not answer it")
os.write(fd, b"\x1b")
until("the waiting message goes in after the prompt", lambda: any("SECOND-PING" in b for b in bodies), 25)
print("it went in after the prompt"); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.5)
