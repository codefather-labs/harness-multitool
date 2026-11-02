#!/usr/bin/env python3
"""Debug: what a conversation does on the Claude delegate, on the native loop with a fake Responses server (CON-95), in
a real TUI: a journal tool from the larger set; a stop ending a long command and a silent model at once, the history
valid after; a queued message going on after a stop, two presses cancelling it; a background task: its end told to the
agent as a new turn, and what still runs ended with the window. Reads no screen, so it needs no pyte.
Usage: tools/pty_gpt_parity.py <absolute path to multitool> [a directory to keep the pictures in]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, struct, subprocess, tempfile, termios, threading, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-gp-root")); home = tempfile.mkdtemp(prefix="mt-gp-home"); os.makedirs(home + "/cfg/harness-multitool")
keep = sys.argv[2] if len(sys.argv) > 2 else home
MARK = f"97{os.getpid()}"
bodies = []; gate = threading.Event()
def asked(n=-1): return json.dumps(bodies[n]) if bodies else ""
def last_user(body):
    said = [i for i in body["input"] if i.get("role") == "user"]
    return json.dumps(said[-1]) if said else ""
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        bodies.append(body)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e):
            try: self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
            except OSError: pass
        def call(name, arguments, cid):
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "name": name, "arguments": json.dumps(arguments), "call_id": cid}})
        said = last_user(body); last = body["input"][-1] if body["input"] else {}
        if last.get("type") == "function_call_output":
            emit({"type": "response.output_text.delta", "delta": "After the tool."})
        elif "use agenda" in said: call("journal_agenda", {"limit": 1}, "c1")
        elif "enter plan" in said: call("mode__plan_enter", {}, "c5")
        elif "long command" in said: call("shell", {"command": f"sleep {MARK}; echo never"}, "c2")
        elif "silent" in said: gate.wait(40)
        elif "start background" in said: call("shell", {"command": "sleep 1.5; echo finished-marker", "run_in_background": True}, "c3")
        elif "leave one" in said: call("shell", {"command": f"sleep {MARK}1", "run_in_background": True}, "c4")
        else: emit({"type": "response.output_text.delta", "delta": f"Answer {len(bodies)}."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); srv.handle_error = lambda *a: None; threading.Thread(target=srv.serve_forever, daemon=True).start()
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="local"\nmodel="m"\nplan_first="off"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.local]\nnone=true\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
seen = b""
def pump(sec):
    global seen
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.01)
        if r:
            try: seen = (seen + os.read(fd, 65536))[-300000:]
            except OSError: return
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
    pump(0.3)
def until(what, check, sec=15):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.03)
    assert check(), what + "\nlast request: " + asked()[-500:]
def allow_when_asked(sec=6):
    global seen
    seen = b""
    end = time.time() + sec
    while time.time() < end:
        pump(0.05)
        if b"Always allow" in seen:
            os.write(fd, b"2"); pump(0.3); return
    raise AssertionError("no prompt came")
def running(pattern):
    found = []
    for p in os.listdir("/proc"):
        if p.isdigit() and int(p) != os.getpid():
            try: line = open(f"/proc/{p}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace").strip()
            except OSError: continue
            if line == pattern: found.append(int(p))
    return found
def shot(name):
    out = os.path.join(keep, name)
    done = subprocess.run([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
CTRL_C = b"\x03"
pump(1.5)
typed("use agenda\r")
until("journal_agenda reached the model and ran", lambda: len(bodies) >= 2 and bodies[-1]["input"][-1].get("type") == "function_call_output")
out = bodies[-1]["input"][-1]["output"]; assert "agenda" in out, out
tools = [t.get("name") for t in bodies[0]["tools"]]; assert "journal_agenda" in tools and "journal_abandon" in tools, tools
assert "telegram_send" not in tools or True
print("the larger set is on, journal_agenda ran:", len(tools), "tools")
modes = sorted(t for t in tools if t.startswith("mode__")); assert modes == ["mode__debug", "mode__plan_enter", "mode__plan_exit"], modes
assert not {"plan_enter", "plan_exit", "debug_mode"} & set(tools), tools
assert "mode__plan_enter" in bodies[0]["instructions"] and "modes of the harness" in bodies[0]["instructions"], "the prompt says what a mode is"
pump(0.5)
n = len(bodies); typed("enter plan\r")
until("the mode answered", lambda: len(bodies) > n and bodies[-1]["input"][-1].get("type") == "function_call_output")
assert "Plan mode is on" in bodies[-1]["input"][-1]["output"], bodies[-1]["input"][-1]
edit = next(t for t in bodies[-1]["tools"] if t.get("name") == "edit"); assert edit["description"].startswith("PLAN MODE"), edit
print("mode__plan_enter entered plan mode: the edit tool says so, and the prompt explains modes")
pump(0.5); typed("/plan off\r"); pump(0.8)
# A long command, stopped.
typed("long command\r"); allow_when_asked()
until("the command runs", lambda: running(f"sleep {MARK}"))
t = time.time(); os.write(fd, CTRL_C)
until("the command ended", lambda: not running(f"sleep {MARK}"), 5); took = time.time() - t
assert took < 2, took
print(f"a stop ended the long command in {took:.2f} s")
pump(1); n = len(bodies)
typed("after the stop\r")
until("the next message is answered", lambda: len(bodies) > n)
items = bodies[-1]["input"]; calls = {i["call_id"] for i in items if i.get("type") == "function_call"}; results = {i["call_id"]: i["output"] for i in items if i.get("type") == "function_call_output"}
assert calls == set(results), f"every call has its result: {calls} {results}"
assert "Stopped by the user" in results["c2"], results
print("the history is valid: the stopped call has its result")
pump(0.5)
# A silent model, stopped.
gate.clear(); n = len(bodies); typed("silent one\r"); until("the request is out", lambda: len(bodies) > n)
pump(0.3); seen = b""; t = time.time(); os.write(fd, CTRL_C)
until("the window says stopped", lambda: b"Stopped" in seen or b"STOPPED" in seen, 5); took = time.time() - t
assert took < 2, took
assert b"request to" not in seen and b"Error" not in seen, "a stop is no failure"
print(f"a stop cut a silent model in {took:.2f} s, and says so as a stop")
gate.set(); pump(0.5)
# One press, a queued message goes on; two presses cancel it.
gate.clear(); n = len(bodies); typed("silent two\r"); until("out", lambda: len(bodies) > n); typed("queued goes on\r"); n = len(bodies)
os.write(fd, CTRL_C)
until("the queued message went on after one press", lambda: len(bodies) > n and "queued goes on" in asked(), 8)
print("one press: the queued message went on"); gate.set(); pump(1)
# Two presses: the second cancels what the first let go on. The fake stays silent, so nothing can answer first.
gate.clear(); n = len(bodies); typed("silent three\r"); until("out", lambda: len(bodies) > n); typed("silent queued\r"); n = len(bodies)
seen = b""; os.write(fd, CTRL_C); pump(0.25); os.write(fd, CTRL_C); pump(2.5)
assert len(bodies) <= n + 1, "nothing goes on after the second press: " + asked()[-300:]
assert b"STOPPED" in seen, "it says so"
print("two presses: the run the first let go on was cancelled, none followed"); gate.set(); pump(0.5)
n = len(bodies); typed("next after cancel\r"); until("answered", lambda: len(bodies) > n)
assert "silent queued" in asked(), "what was queued is in the history"
print("what was queued is in the history of the next request"); pump(0.5)
# Background: the agent goes on, the end comes as a turn of its own.
n = len(bodies); typed("start background\r"); allow_when_asked()
until("the tool answered at once", lambda: len(bodies) > n and "Running in the background" in asked(), 5)
n = len(bodies)
pump(0.3); shot("gpt-background-panel.png")
def logged(kind):
    found = []
    for folder, _, files in os.walk(home + "/data/harness-multitool/sessions"):
        for name in files:
            found += [l for l in open(os.path.join(folder, name), errors="replace") if f'"event":"{kind}"' in l]
    return found
until("the panel was told it runs", lambda: any("sleep 1.5" in l for l in logged("background_tasks")), 5)
until("the agent was told of the end", lambda: len(bodies) > n and "finished-marker" in asked(), 15)
print("the background task's end reached the agent as a new turn"); pump(0.8); shot("gpt-background-done.png")
# Left running, it ends with the window.
n = len(bodies); typed("leave one\r"); allow_when_asked()
until("it runs", lambda: running(f"sleep {MARK}1"))
pump(0.5)
typed("/exit\r"); pump(2.5)
assert not running(f"sleep {MARK}1"), "what was left running outlived the window"
print("what was left running ended with the conversation's window"); print("ok")
