#!/usr/bin/env python3
"""Debug: a window takes over a run still on (CON-77). A real TUI in a pty is killed in the middle of a
reply; a new one opens the conversation at once: what was said is there, the prompt comes to it, the
rest of the run arrives live, and the conversation goes on. No line shows twice. Needs pyte.
Usage: tools/pty_attach.py <absolute path to multitool>"""
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
            emit({"type": "response.output_text.delta", "delta": "Starting. "}); start_slow.set(); time.sleep(1)
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": json.dumps({"command": "touch made-after-takeover"})}})
        elif n == 2:
            time.sleep(2); emit({"type": "response.output_text.delta", "delta": "Rest of the reply, live."})
        else:
            emit({"type": "response.output_text.delta", "delta": "Going on in the new window."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
root = tempfile.mkdtemp(prefix="mt-at-root"); home = tempfile.mkdtemp(prefix="mt-at-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="fake"\nmodel="m"\nplan_first="off"\n\n[session.providers.fake]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.fake]\nnone=true\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
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
time.sleep(1.5)
assert status()["conversations"]["busy"] == 1
os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0); print("the TUI is killed with a prompt about to come")
t0 = time.time()
pid, fd, screen = tui("startup"); stream = pyte.ByteStream(screen); pump(1.5)
os.write(fd, b"r"); pump(0.5)
end = time.time() + 12
while time.time() < end and "shell" not in text().lower(): pump(0.2)
print(f"opened and shown after {time.time() - t0:.1f} s")
assert not os.path.exists(root + "/made-after-takeover")
assert "touch made-after-takeover" in text(), "the prompt came to the new window: " + text()
print("the waiting prompt reached the new window")
pump(1.2); os.write(fd, b"1"); pump(0.3)
end = time.time() + 8
while time.time() < end and not os.path.exists(root + "/made-after-takeover"): pump(0.1)
assert os.path.exists(root + "/made-after-takeover"), "the answer reached the run: " + text()
end = time.time() + 8
while time.time() < end and "Rest of the reply, live" not in text(): pump(0.1)
assert "Rest of the reply, live" in text(), "the rest arrives live: " + text()
print("the answer reached the run; the rest arrived live")
assert text().count("Starting.") == 1, "what was said is there, once: " + text()
pump(1.0)
for c in "and now\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(2.0); assert "Going on in the new window" in text(), text()
assert "Rest of the reply" in bodies[-1], "the model got all of it"
lines = [json.loads(l) for l in log().splitlines()]
assert [l["seq"] for l in lines] == list(range(len(lines))), "one writer: one numbering"
print("the conversation goes on here; one numbering in the log"); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.3)
