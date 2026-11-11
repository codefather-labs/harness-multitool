#!/usr/bin/env python3
"""journal#824: Use on the Profiles tab of a window that runs its sessions itself (Local, because the
daemon found is of another build and busy). A fake daemon answers /health with another build and
`busy` true, so the window cannot replace it and stays Local. The window starts on default, Use moves
it onto max with Enter, and its next message must reach a delegate run with max's Claude Code folder.
Before the fix the window answered 'Still on default: this window runs its sessions itself'.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_profiles_local.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, http.server, json, pty, select, struct, subprocess, tempfile, termios, threading, time

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-plocal-home"); mt = home + "/mt-home"; users = home + "/dot-claude"; log = home + "/log"; fake = home + "/fake-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-plocal-root-")); os.makedirs(users + "/projects")
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys
d = os.environ.get("CLAUDE_CONFIG_DIR")
if sys.argv[1:2] == ["auth"]:
    yes = os.path.exists(os.path.join(d or "/nonexistent", "signed-in")); print(json.dumps({{"loggedIn": yes}})); sys.exit(0 if yes else 1)
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"text": text, "claude": d, "parent": os.getppid()}}) + "\\n")
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
settings = f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n'
os.makedirs(home + "/cfg/harness-multitool"); open(home + "/cfg/harness-multitool/config.toml", "w").write(settings)
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=users)
def cli(*args): return subprocess.run([bin_, *args], env=env, capture_output=True, text=True, cwd=root)
assert cli("profile", "add", "max").returncode == 0
open(mt + "/profiles/max/profile.toml", "w").write(settings)
os.makedirs(mt + "/profiles/max/claude", exist_ok=True); open(mt + "/profiles/max/claude/signed-in", "w").write("yes")

# The busy daemon of another build: the window must not stop it, so it runs Local.
asked = []
class Old(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, body, code=200):
        data = json.dumps(body).encode(); self.send_response(code); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        asked.append(("GET", self.path)); self.reply({"ok": True, "build": "another-build"} if self.path == "/health" else {"error": "no"}, 200 if self.path == "/health" else 404)
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)) or 0); asked.append(("POST", self.path, body[:200]))
        self.reply({"busy": True} if self.path == "/conversations" else {"error": "no"}, 200 if self.path == "/conversations" else 404)
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Old); threading.Thread(target=server.serve_forever, daemon=True).start()
os.makedirs(home + "/data/harness-multitool/daemon")
open(home + "/data/harness-multitool/daemon/address.json", "w").write(json.dumps({"port": server.server_address[1], "admin": "x", "pid": os.getpid()}))

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def press(*ks):
    for k in ks: os.write(fd, k.encode()); pump(0.15)
def seen(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def say(text):
    count = len(seen())
    for ch in text: os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
    end = time.time() + 20
    while time.time() < end and len(seen()) == count: pump(0.05)
    got = [s for s in seen() if s["text"].endswith(text)]
    assert got, f"{text!r} did not reach a delegate: {seen()}"
    return got[-1]
DOWN = "\x1b[B"
pump(2.5)
assert ("POST", "/shutdown") not in [a[:2] for a in asked], "the busy daemon of another build was asked to stop"
press("\x1b[<0;112;33M", "\x1b[<0;112;33m"); pump(0.8)          # the Settings button
press(*[DOWN] * 9); press("\t"); pump(0.5)                      # Profiles, then into the page
press(DOWN, "\r"); pump(1.5)                                    # max, Enter is Use
press("\x1b"); pump(0.3); press("\x1b"); pump(0.5)              # out of the settings
got = say("hello-after-use")
print("delegate ran with", got["claude"], "parent", got["parent"], "window", pid)
assert got["parent"] == pid, f"the window ran it itself (Local): {got}"
assert got["claude"] == mt + "/profiles/max/claude", f"after Use the window's new session runs on max's Claude Code: {got}"
print("OK: a Local window moved onto max with Enter, and its session ran on max's Claude Code folder")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.5)
server.shutdown()
