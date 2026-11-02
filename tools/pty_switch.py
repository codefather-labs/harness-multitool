#!/usr/bin/env python3
"""Debug: changing the adapter in the settings (CON-79), in a real TUI in a pty, with a fake provider
and a fake delegate. During a run the controls refuse and nothing changes. After it the choice is
saved by the harness, the status line shows it, and the adapter's form exists at once, before any
message, holding what was said. Going back and on, the form gets those turns too. Needs pyte and
--features claude-code.
Usage: tools/pty_switch.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, http.server, json, pty, re, select, signal, stat, struct, tempfile, termios, threading, time, tomllib
import pyte

bodies = []; slow = threading.Event()
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode()); n = len(bodies)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        if n == 1: slow.set(); time.sleep(7); emit({"type": "response.output_text.delta", "delta": "The slow answer."})
        else: emit({"type": "response.output_text.delta", "delta": f"Fake answer {n}."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-sw-root")); home = tempfile.mkdtemp(prefix="mt-sw-home"); os.makedirs(home + "/cfg/harness-multitool"); claude_dir = home + "/claude"; os.makedirs(claude_dir)
fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/bin/sh
echo "$@" >> {home}/calls
cat > /dev/null
echo '{{"type":"system","subtype":"init","session_id":"unused","model":"fake-model"}}'
echo '{{"type":"assistant","message":{{"id":"m1","role":"assistant","content":[{{"type":"text","text":"Delegate answer."}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.0,"usage":{{}},"modelUsage":{{"fake-model":{{"contextWindow":777000}}}}}}'
''')
os.chmod(fake, 0o755)
config = home + "/cfg/harness-multitool/config.toml"
open(config, "w").write(f'[session]\nprovider="local"\nmodel="m"\nplan_first="off"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.local]\nnone=true\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=claude_dir, TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
bin_ = sys.argv[1]; COLS, ROWS = 130, 46
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
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
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
    pump(0.6)
def click(name):
    for y, line in enumerate(screen.display):
        x = line.find(name)
        if x >= 0:
            os.write(fd, f"\x1b[<0;{x + 2};{y + 1}M\x1b[<0;{x + 2};{y + 1}m".encode()); pump(0.4); return
    raise AssertionError(f"no {name} on screen: " + text())
def saved(): return tomllib.load(open(config, "rb"))["session"]
def forms(): return glob.glob(claude_dir + "/projects/**/*.jsonl", recursive=True)
def until(what, check, sec=10):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.1)
    assert check(), what + ": " + text()
pump(1.5); typed("go\r"); assert slow.wait(10), text()
typed("/settings\r"); until("the settings open during a run", lambda: "claude-code" in text())
click("claude-code"); assert "A run is on" in text(), "the provider row refuses: " + text()
assert saved()["provider"] == "local"
os.write(fd, b"\x13"); pump(0.5); assert "A run is on" in text(), "Ctrl+S refuses: " + text(); assert saved()["provider"] == "local"
print("during a run: the row and the save refuse, nothing changed")
os.write(fd, b"\x1b"); pump(0.5)
until("the run ended", lambda: "The slow answer" in text() and "Esc stops" not in text(), 15)
assert not forms(), "no form until it is asked for"
typed("/settings\r"); click("claude-code"); pump(0.3)
os.write(fd, b"\x13"); pump(1.0)
assert saved()["provider"] == "claude-code", "saved by the harness"
assert "ADAPTER  claude-code" in text(), "the status line says so: " + text()
until("the form exists at once", lambda: forms() and "The slow answer" in open(forms()[0]).read())
assert not os.path.exists(home + "/calls"), "no message was needed"
form = forms()[0]; cid = os.path.basename(form)[:-6]; print("after it: saved, shown, and the form holds the conversation before any message:", cid)
typed("next\r"); until("the delegate answers", lambda: "Delegate answer" in text())
assert f"--resume {cid}" in open(home + "/calls").read(), "it goes on in that form"
pump(0.5); assert "/ 777k" in text() or "777" in text(), "the window is the model's own: " + text()
typed("/settings\r"); click("(   )  Local"); os.write(fd, b"\x13"); pump(1.0)
assert saved()["provider"] == "local", "back: " + text(); typed("and on\r"); until("the provider answers", lambda: "Fake answer" in text())
until("the form has those turns too", lambda: "and on" in open(form).read() and "Fake answer" in open(form).read())
assert len(forms()) == 1
assert "Delegate answer" in bodies[-1], "the provider has the delegate's turn"
print("back and on: one form, each side has the other's turns"); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.3)
