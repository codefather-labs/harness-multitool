#!/usr/bin/env python3
"""Debug, CON-181: a window leaves its agent in the real TUI. A fake model whose first answer runs
`sleep 5.1` (asked with the permission card, allowed) and whose second says DONE. Each case starts a
window in a pty, sends 'work', allows the call, and leaves while the command runs:
- /exit, 3 (finish the work): the window quits; the model is asked again (DONE) after it;
- /exit, 1 (stop): nothing more is asked;
- /exit, 4 (keep): DONE comes, and the daemon still holds the window's conversations afterwards;
- /exit, Space, 2 (remember finish-command): shared.toml gets on_exit, and the next /exit quits
  with no dialog;
- the window killed (SIGKILL) and, apart, its pty closed (SIGHUP, as an ssh drop): the default
  'finish the work' lets DONE come.
The daemon's lost-window time is cut to 3 s (MULTITOOL_WINDOW_GONE_SECS). The tool's process is
gone after each case. Needs pyte. Usage: tools/pty_leave.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, http.server, json, pty, select, signal, struct, subprocess, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text, tool=None):
    ev = [{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}]
    if tool:
        ev += [{"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": tool[0], "name": tool[1], "input": {}}}, {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": json.dumps(tool[2])}}, {"type": "content_block_stop", "index": 1}]
    ev += [{"type": "message_delta", "delta": {"stop_reason": "tool_use" if tool else "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}]
    return sse(ev)
calls = []
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
        calls.append(body)
        b = turn("DONE") if "tool_result" in body else turn("Working.", (f"s{len(calls)}", "shell", {"command": "sleep 5.1; echo slept"}))
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-leave-root"); home = tempfile.mkdtemp(prefix="mt-leave-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", MULTITOOL_WINDOW_GONE_SECS="3")

class Window:
    def __init__(self):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.clear(); os.environ.update(env)
            os.execv(sys.argv[1], [sys.argv[1], "--new"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.screen = pyte.Screen(120, 40); self.stream = pyte.ByteStream(self.screen); self.gone = False
    def pump(self, sec):
        end = time.time() + sec
        while time.time() < end and not self.gone:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.stream.feed(os.read(self.fd, 65536))
                except OSError: self.gone = True
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display)
    def wait(self, cond, sec):
        end = time.time() + sec
        while time.time() < end and not cond(): self.pump(0.2)
        return cond()
    def key(self, seq):
        """An escape sequence written whole, so it is not read as a lone Esc."""
        os.write(self.fd, seq.encode()); time.sleep(0.1)
    def type(self, s):
        for c in s: os.write(self.fd, c.encode()); time.sleep(0.05)
    def exited(self, sec):
        end = time.time() + sec
        while time.time() < end:
            self.pump(0.2)
            try:
                if os.waitpid(self.pid, os.WNOHANG)[0]: return True
            except ChildProcessError: return True
        return False
    def start_work(self):
        assert self.wait(lambda: "Enter send" in self.text(), 20), self.text()
        self.type("work\r")
        assert self.wait(lambda: "Permission required" in self.text(), 20), self.text()
        # The card takes no key for a moment after it shows (PermissionView.locked).
        for _ in range(10):
            self.pump(0.8); self.type("1")
            if self.wait(lambda: "Permission required" not in self.text(), 2): break
        assert "Permission required" not in self.text(), self.text()
        self.pump(1.0)

def tool_processes(): return subprocess.run(["pgrep", "-f", "sleep 5.1"], capture_output=True, text=True).stdout.split()
def daemon_windows():
    out = subprocess.run([sys.argv[1], "daemon", "--status"], env=env, capture_output=True, text=True).stdout
    try: return json.loads(out)["conversations"]["windows"]
    except Exception: return None
def calls_after(before, sec, want):
    end = time.time() + sec
    while time.time() < end and len(calls) - before < want: time.sleep(0.2)
    time.sleep(1.5)
    return len(calls) - before

results = {}
def leave_case(name, keys, want_calls):
    before = len(calls)
    w = Window(); w.start_work()
    w.type("/exit"); w.pump(0.5)
    open(proof_dir + f"/typed-{name}.txt", "w").write(w.text())
    w.type("\r")
    dialog = w.wait(lambda: "Leaving multitool" in w.text(), 10)
    open(proof_dir + f"/dialog-{name}.txt", "w").write(w.text())
    w.type(keys)
    quit_ = w.exited(10)
    got = calls_after(before, 15, want_calls)
    print(f"{name}: dialog {dialog}, quit {quit_}, model calls {got}")
    results[f"{name}_dialog_then_quit"] = dialog and quit_
    results[f"{name}_model_calls_{want_calls}"] = got == want_calls
    return w

proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-leave-proof")); os.makedirs(proof_dir, exist_ok=True)
leave_case("finish_work", "3", 2)
leave_case("stop", "1", 1)
leave_case("keep", "4", 2)
time.sleep(2)
kept = daemon_windows()
print("daemon windows after keep:", kept)
results["keep_conversations_stay_in_the_daemon"] = bool(kept) and kept >= 1
# Remember: Space, then 2 (finish-command); the next /exit asks nothing.
leave_case("remember", " 2", 1)
shared = open(home + "/mt-home/shared.toml").read() if os.path.exists(home + "/mt-home/shared.toml") else ""
results["remember_saved_on_exit"] = 'on_exit = "finish-command"' in shared
w = Window(); assert w.wait(lambda: "Enter send" in w.text(), 20)
w.type("/exit\r")
results["remembered_exit_asks_nothing"] = w.exited(10) and "Leaving multitool" not in w.text()
# Back to asking for the loss cases.
open(home + "/mt-home/shared.toml", "w").write(shared.replace('on_exit = "finish-command"', 'on_exit = "ask"'))
# Lost: SIGKILL.
before = len(calls); w = Window(); w.start_work()
os.kill(w.pid, signal.SIGKILL); os.waitpid(w.pid, 0)
got = calls_after(before, 20, 2); print("sigkill: model calls", got)
results["sigkill_work_finishes"] = got == 2
# Lost: the pty closed under it, as an ssh drop (SIGHUP).
before = len(calls); w = Window(); w.start_work()
os.close(w.fd)
try: os.waitpid(w.pid, 0)
except ChildProcessError: pass
got = calls_after(before, 20, 2); print("sighup: model calls", got)
results["sighup_work_finishes"] = got == 2
# Settings > Behaviour: 'On exit' changed from Ask to Stop and saved (CON-181).
w = Window(); assert w.wait(lambda: "Enter send" in w.text(), 20)
w.type("/settings\r"); w.pump(2)
for _ in range(20):
    if "BEHAVIOUR" in w.text(): break
    w.key("\x1b[B"); w.pump(0.3)
w.key("\x1b[C"); w.pump(0.5)
w.type("\r"); w.pump(0.3)
w.type("\x13"); w.pump(1)
open(proof_dir + "/behaviour-page.txt", "w").write(w.text())
shared = open(home + "/mt-home/shared.toml").read() if os.path.exists(home + "/mt-home/shared.toml") else ""
results["behaviour_page_saves_on_exit"] = 'on_exit = "stop"' in shared and "Saved." in w.text()
w.key("\x1b"); w.pump(0.8)
w.type("/exit\r")
results["saved_on_exit_quits_without_asking"] = w.exited(10)
time.sleep(1)
results["no_tool_process_left"] = not tool_processes()
print("\nRESULTS:", json.dumps(results))
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
