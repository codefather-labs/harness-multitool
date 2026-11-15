#!/usr/bin/env python3
"""Debug, CON-167: `multitool --dir <root> --new` opens a session tab at once, with no start page, and
`multitool --dir <root> --resume <id>` opens that conversation, its transcript shown. The real TUI in a
pty, a fake model, a temporary home. Needs pyte. Usage: tools/pty_open_flags.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
replies = ["FIRST-REPLY-FROM-THE-MODEL", "SECOND-REPLY"]
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        b = turn(replies.pop(0) if replies else "more")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-flags-root"); home = tempfile.mkdtemp(prefix="mt-flags-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
elsewhere = tempfile.mkdtemp(prefix="mt-flags-cwd")
COLS, ROWS = 120, 40; bin_ = sys.argv[1]
env = dict(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")

def window(args):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(elsewhere); os.environ.update(env)
        os.execv(bin_, [bin_] + args)
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
    def close():
        for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.01)
        pump(1.0)
        try: os.kill(pid, signal.SIGTERM)
        except ProcessLookupError: pass
    return pump, text, wait, fd, close

results = {}
# --new: a session tab at once, in --dir's folder, not the start page.
pump, text, wait, fd, close = window(["--dir", root, "--new"])
results["new_is_a_session"] = wait(lambda: "Enter send" in text() and os.path.basename(root) in text(), 20)
for c in "hello there\r": os.write(fd, c.encode()); time.sleep(0.01)
results["new_reached_the_model"] = wait(lambda: "FIRST-REPLY-FROM-THE-MODEL" in text(), 20)
print("===== --new =====\n" + text())
close()
logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
print("session logs:", logs)
results["one_session_logged_under_the_root"] = len(logs) == 1 and os.path.basename(root).replace("_", "-") in logs[0].replace("_", "-")
session = os.path.basename(logs[0])[:-len(".jsonl")] if logs else "none"

# --resume: that conversation, its transcript shown.
pump, text, wait, fd, close = window(["--dir", root, "--resume", session])
results["resume_shows_the_transcript"] = wait(lambda: "hello there" in text() and "FIRST-REPLY-FROM-THE-MODEL" in text(), 20)
print("===== --resume " + session + " =====\n" + text())
close()
print("\nRESULTS:", json.dumps(results))
data = home + "/data/harness-multitool"
try: os.kill(json.load(open(data + "/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
assert all(results.values()), results
print("ALL PASSED")
