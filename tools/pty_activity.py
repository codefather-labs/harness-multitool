#!/usr/bin/env python3
"""Debug: the activity line in a real TUI in a pty, with a slow fake model: waiting, thinking,
writing, waiting for your answer, running a tool, the seconds rising, and gone at the end.
Needs pyte. Usage: tools/pty_activity.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, re, select, struct, tempfile, termios, threading, time
import pyte

turn = {"n": 0}
bodies = []
class H(http.server.BaseHTTPRequestHandler):
    def emit(self, event, wait=0.0):
        time.sleep(wait)
        self.wfile.write(f"data: {json.dumps(event)}\n\n".encode()); self.wfile.flush()
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        turn["n"] += 1
        if turn["n"] == 1:
            self.emit({"type": "response.reasoning_summary_text.delta", "delta": "Let me think about it."}, 2.5)
            self.emit({"type": "response.output_text.delta", "delta": "I will run it."}, 2.0)
            self.emit({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": json.dumps({"command": "sleep 2"})}}, 1.5)
        else:
            self.emit({"type": "response.output_text.delta", "delta": "All done."}, 1.0)
        self.emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
root = tempfile.mkdtemp(prefix="mt-act-root"); home = tempfile.mkdtemp(prefix="mt-act-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="slow"\nmodel="slow-model"\n\n[session.providers.slow]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.slow]\nkey="k"\n')
COLS, ROWS = 130, 46; bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def line():
    return next((l.strip() for l in screen.display if "Esc stops" in l and "ADAPTER" in l), None)
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
pump(1.5); assert line() is None, "no line before a run: " + text()
for c in "go\r": os.write(fd, c.encode()); time.sleep(0.005)
seen, spins, seconds = [], set(), []
queued_stage = 0
end = time.time() + 30; answered = False
while time.time() < end:
    pump(0.05); l = line()
    if l is None:
        if seen and turn["n"] >= 2 and "All done." in text(): break
        continue
    m = re.search(r"(\S) ((?:Waiting|Thinking|Writing|Calling)[^·]*?)\s{2}(\d+)s\s+·\s+Esc stops", l)
    assert m, l
    spins.add(m.group(1)); seconds.append(int(m.group(3)))
    if not seen or seen[-1] != m.group(2): seen.append(m.group(2)); print("state:", l[-60:])
    # While it works: one message taken back with Up, one left in the queue.
    if queued_stage == 0 and m.group(2) == "Writing":
        for c in "drop me\r": os.write(fd, c.encode()); time.sleep(0.005)
        pump(0.2); assert "⧗ drop me" in text(), "in sight: " + text()
        os.write(fd, b"\x1b[A"); pump(0.2); assert "⧗" not in text() and "drop me" in text(), "back in the composer: " + text()
        for _ in "drop me": os.write(fd, b"\x7f")
        for c in "and one more thing\r": os.write(fd, c.encode()); time.sleep(0.005)
        pump(0.2); assert "⧗ and one more thing" in text() and "↑ to change" in text(), text(); print("queued:", [l.strip() for l in screen.display if "⧗" in l])
        queued_stage = 1
    if m.group(2) == "Waiting for your answer" and "tool: shell" in text() and not answered:
        pump(1.0); os.write(fd, b"1"); answered = True
assert queued_stage == 1
assert "⧗" not in text() and "and one more thing" in text(), "delivered, its card up: " + text()
assert "and one more thing" in bodies[1] and "drop me" not in "".join(bodies), bodies[1][:300]
assert len(bodies) == 2, "woven in, no extra run"
print("states:", seen); print("spinner frames:", len(spins), "seconds:", seconds[0], "to", seconds[-1])
for want in ["Waiting for slow-model", "Thinking", "Writing", "Waiting for your answer", "Calling shell"]:
    assert want in seen, (want, seen)
assert seen.index("Thinking") < seen.index("Writing") < seen.index("Waiting for your answer") < seen.index("Calling shell"), seen
assert len(spins) == 8 and all(c in "🌑🌒🌓🌔🌕🌖🌗🌘" for c in spins), spins
assert seconds == sorted(seconds) and seconds[-1] >= 8, seconds
pump(0.3); assert line() is None, "gone at the end: " + text()
assert "generating" not in text(), text()
status = next(l for l in screen.display if "ADAPTER" in l); print(status.strip()); assert "TOKENS  7 /" in status, status
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5); print("ok")
