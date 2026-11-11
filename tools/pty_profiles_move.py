#!/usr/bin/env python3
"""CON-127 (journal#830): the profile chosen in the settings is the window's and every tab's, and the
next bare start's.
- claude-code: a tab on default says 'one', Use moves the window onto max with Enter, the same tab says
  'two': its delegate runs with max's CLAUDE_CONFIG_DIR and resumes the same session id;
- a bare `multitool` started next runs on max; `--profile default` runs on default for that run and
  leaves max kept;
- an API adapter: a tab on default says 'alpha' with default's key, Use max, 'beta' goes with max's key
  and still carries 'alpha'.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_profiles_move.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, http.server, json, pty, select, struct, subprocess, tempfile, termios, threading, time, tomllib

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-pmove-home"); mt = home + "/mt-home"; users = home + "/dot-claude"; log = home + "/log"; fake = home + "/fake-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-pmove-root-")); os.makedirs(users + "/projects")
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys
d = os.environ.get("CLAUDE_CONFIG_DIR")
if sys.argv[1:2] == ["auth"]:
    yes = os.path.exists(os.path.join(d or "/nonexistent", "signed-in")); print(json.dumps({{"loggedIn": yes}})); sys.exit(0 if yes else 1)
args = sys.argv[1:]
resumed = args[args.index("--resume") + 1] if "--resume" in args else None
sid = resumed or "sess-" + str(os.getpid())
print(json.dumps({{"type": "system", "subtype": "init", "session_id": sid, "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"text": text, "claude": d, "resumed": resumed, "session": sid}}) + "\\n")
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "session_id": sid, "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
requests = []
class Api(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
        requests.append({"auth": self.headers.get("authorization", ""), "body": body})
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        emit({"type": "response.output_text.delta", "delta": f"Answer {len(requests)}."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
api = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Api); threading.Thread(target=api.serve_forever, daemon=True).start()

def settings(provider, key):
    return (f'[session]\nprovider="{provider}"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n\n'
            f'[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{api.server_address[1]}"\n\n[session.auth.local]\nkey="{key}"\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=users)
def cli(*args): return subprocess.run([bin_, *args], env=env, capture_output=True, text=True, cwd=root)
assert cli("profile", "add", "max").returncode == 0
def profiles_on(provider):
    open(mt + "/profiles/default/profile.toml", "w").write(settings(provider, "key-default"))
    open(mt + "/profiles/max/profile.toml", "w").write(settings(provider, "key-max"))
profiles_on("claude-code")
os.makedirs(mt + "/profiles/max/claude", exist_ok=True); open(mt + "/profiles/max/claude/signed-in", "w").write("yes")
def kept(): return tomllib.load(open(mt + "/shared.toml", "rb")).get("profile")

fds = []
def window(*args):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, *args, "preview", "welcome"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0)); fds.append(fd); pump(2.5)
    return fd
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select(fds, [], [], 0.02)
        for fd in r:
            try: os.read(fd, 65536)
            except OSError: pass
def press(fd, *ks):
    for k in ks: os.write(fd, k.encode()); pump(0.15)
def seen(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def type_(fd, text):
    for ch in text: os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
def say(fd, text):
    count = len(seen()); type_(fd, text)
    end = time.time() + 20
    while time.time() < end and len(seen()) == count: pump(0.05)
    got = [s for s in seen() if s["text"].endswith(text)]
    assert got, f"{text!r} did not reach a delegate: {seen()}"
    return got[-1]
def ask_api(fd, text):
    count = len(requests); type_(fd, text)
    end = time.time() + 20
    while time.time() < end and len(requests) == count: pump(0.05)
    assert len(requests) > count, f"{text!r} reached no API request"
    pump(0.5)
    return requests[-1]
DOWN = "\x1b[B"
def use_second_profile(fd):
    press(fd, "\x1b[<0;112;33M", "\x1b[<0;112;33m"); pump(0.8)    # the Settings button
    press(fd, *[DOWN] * 9); press(fd, "\t"); pump(0.5)            # Profiles, then into the page
    press(fd, DOWN, "\r"); pump(2.0)                              # max, Enter is Use
    press(fd, "\x1b"); pump(0.3); press(fd, "\x1b"); pump(0.5)    # out of the settings
def close(fd):
    os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(1.0); fds.remove(fd)

a = window()
one = say(a, "one")
assert one["claude"] == users, f"a bare first start runs on default, the user's own Claude Code: {one}"
use_second_profile(a)
two = say(a, "two")
assert two["claude"] == mt + "/profiles/max/claude", f"after Use the started tab runs on max: {two}"
assert two["resumed"] == one["session"], f"the same conversation goes on: {one} -> {two}"
assert kept() == "max", kept()
print("started tab moved:", one["session"], "->", "resumed", two["resumed"], "under", two["claude"])
close(a)

b = window()
three = say(b, "three")
assert three["claude"] == mt + "/profiles/max/claude", f"a bare start runs on the kept profile: {three}"
close(b)
c = window("--profile", "default")
four = say(c, "four")
assert four["claude"] == users and kept() == "max", (four, kept())
close(c)
print("bare start on the kept max; --profile default for one run, max still kept")

profiles_on("local")
open(mt + "/shared.toml", "w").write(open(mt + "/shared.toml").read().replace('profile = "max"', 'profile = "default"'))
d = window()
alpha = ask_api(d, "alpha")
assert "key-default" in alpha["auth"], alpha["auth"]
use_second_profile(d)
beta = ask_api(d, "beta")
assert "key-max" in beta["auth"], f"after Use the API tab signs with max's key: {beta['auth']}"
assert "alpha" in beta["body"] and "beta" in beta["body"], "the API tab keeps its history"
print("API tab moved: key-default -> key-max, history kept")
close(d)
print("ok")
