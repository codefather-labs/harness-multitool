#!/usr/bin/env python3
"""Debug: ask_user in a real TUI in a pty, with a fake model: the panel with options above the
composer, arrows, Enter, a digit, a typed answer, and each reaching the model as the user's words.
Needs pyte. Usage: tools/pty_ask.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, struct, tempfile, termios, threading, time
import pyte

bodies = []
ASK = {"question": "What counts as a bad row?", "options": [{"label": "Not a number", "description": "negative values stay"}, "Not a number or negative", "Any empty column"]}
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        emit({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": f"c{len(bodies)}", "name": "ask_user", "arguments": json.dumps(ASK)}})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
root = tempfile.mkdtemp(prefix="mt-ask-root"); home = tempfile.mkdtemp(prefix="mt-ask-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="fake"\nmodel="m"\n\n[session.providers.fake]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.fake]\nnone=true\n')
COLS, ROWS = 130, 46; bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
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
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def send(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
def waits(n):
    end = time.time() + 8
    while time.time() < end:
        pump(0.1)
        if len(bodies) >= n and "↑↓ and Enter" in text(): return
    raise AssertionError(f"no panel after {n} requests: " + text())
def said(i):
    return json.loads(bodies[i])["input"][-1]
pump(1.5); send("go\r"); waits(1); pump(0.3)
t = text(); print("\n".join(l for l in t.splitlines() if "❯" in l or " ? " in l or "2. Not" in l or "↑↓" in l))
assert "❯ 1. Not a number" in t and "negative values stay" in t and "3. Any empty column" in t, t
os.write(fd, b"\x1b[B"); pump(0.2); assert "❯ 2. Not a number or negative" in text(), text()
os.write(fd, b"\x1b[A\x1b[A"); pump(0.2); assert "❯ 3. Any empty column" in text(), "wraps round: " + text()
os.write(fd, b"\x1b[B\x1b[B"); pump(0.2); send("\r"); pump(0.2)
waits(2); assert "Not a number or negative" in json.dumps(said(1), ensure_ascii=False), said(1); print("enter sent the label")
pump(0.3); send("3"); waits(3); assert "Any empty column" in json.dumps(said(2)), said(2); print("a digit sent the label")
pump(0.3); send("9"); pump(0.2); assert len(bodies) == 3, "9 is no option: it is typed"
send("x own words\r"); waits(4); assert "9x own words" in json.dumps(said(3)), said(3); print("typed text is the answer")
print("ok")
