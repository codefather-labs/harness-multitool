#!/usr/bin/env python3
"""Debug: the conversation lives in the daemon (ADR 0024). A real TUI in a pty is killed in the middle
of a slow reply: the daemon finishes the run and the log has it; a prompt nobody can answer is
denied; a new TUI opens the conversation with the reply in it and goes on. Needs pyte.
Usage: tools/pty_daemon.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, http.server, json, pty, select, signal, struct, subprocess, tempfile, termios, threading, time
import pyte

bodies = []; start_slow = threading.Event()
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode()); n = len(bodies)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        if n == 1:
            emit({"type": "response.output_text.delta", "delta": "Starting. "}); start_slow.set(); time.sleep(3)
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": json.dumps({"command": "touch made-by-orphan"})}})
        elif n == 2:
            emit({"type": "response.output_text.delta", "delta": "Finished with nobody watching."})
        else:
            emit({"type": "response.output_text.delta", "delta": "Going on in the new window."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
root = tempfile.mkdtemp(prefix="mt-dm-root"); home = tempfile.mkdtemp(prefix="mt-dm-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="fake"\nmodel="m"\nplan_first="off"\n\n[session.providers.fake]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.fake]\nnone=true\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
bin_ = sys.argv[1]; COLS, ROWS = 130, 46
def tui(page):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", page])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return pid, fd, pyte.Screen(COLS, ROWS)
pid, fd, screen = tui("welcome"); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def status(): return json.loads(subprocess.run([bin_, "daemon", "--status"], env=env, capture_output=True, text=True).stdout)
def log():
    files = glob.glob(home + "/data/**/sessions/**/*.jsonl", recursive=True)
    return open(files[0]).read() if files else ""
pump(1.5)
for c in "go\r": os.write(fd, c.encode()); time.sleep(0.005)
assert start_slow.wait(10), "the model was reached: " + text()
stats = status(); print("in the daemon:", stats["conversations"]); assert stats["conversations"]["busy"] == 1, stats
os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0); print("the TUI is killed in the middle of the reply")
assert not os.path.exists(root + "/made-by-orphan")
end = time.time() + 45
while time.time() < end and len(bodies) < 2 and "Finished with nobody" not in log(): time.sleep(0.2)
# The tool's prompt waits for the window until the daemon calls it gone.
assert not os.path.exists(root + "/made-by-orphan"), "a prompt nobody can answer is not allowed"
end = time.time() + 45
while time.time() < end and "Finished with nobody" not in log(): time.sleep(0.2)
assert "Finished with nobody" in log(), "the daemon finished the run: " + log()[-400:]
print("the daemon finished the run; the log has the reply")
end = time.time() + 5
while time.time() < end and status()["conversations"]["busy"]: time.sleep(0.1)
assert status()["conversations"]["busy"] == 0
pid, fd, screen = tui("startup"); stream = pyte.ByteStream(screen); pump(1.5)
os.write(fd, b"r"); pump(1.5); assert "Finished with nobody watching" in text(), "a new window shows it: " + text()
for c in "and now\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(2.0); assert "Going on in the new window" in text(), text()
assert "Finished with nobody watching" in bodies[-1], "the model got it too"
print("a new window shows it and goes on"); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.3)
