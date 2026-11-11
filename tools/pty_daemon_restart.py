#!/usr/bin/env python3
"""CON-128 (journal#831): `multitool daemon restart` and `stop`, with a window open on a fake Claude Code.
- restart while idle: a new daemon on the same port, and the window's tab goes on in it, resuming its
  conversation;
- restart during a run: the command says the run was stopped, and the tab answers its next message;
- stop: no daemon answers; restart with none starts one.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_daemon_restart.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, json, pty, select, struct, subprocess, tempfile, termios, time, urllib.request

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-drs-home"); mt = home + "/mt-home"; users = home + "/dot-claude"; log = home + "/log"; fake = home + "/fake-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-drs-root-")); os.makedirs(users + "/projects")
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys, time
if sys.argv[1:2] == ["auth"]: print(json.dumps({{"loggedIn": False}})); sys.exit(1)
args = sys.argv[1:]
resumed = args[args.index("--resume") + 1] if "--resume" in args else None
sid = resumed or "sess-" + str(os.getpid())
print(json.dumps({{"type": "system", "subtype": "init", "session_id": sid, "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"text": text, "resumed": resumed, "session": sid}}) + "\\n")
    if text.endswith("slow"): time.sleep(60)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "session_id": sid, "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=users)
address = home + "/data/harness-multitool/daemon/address.json"
def cli(*args): return subprocess.run([bin_, *args], env=env, capture_output=True, text=True, cwd=root, timeout=60)
def daemon():
    a = json.load(open(address))
    return a["pid"], a["port"]
def healthy(port):
    try: return urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2).status == 200
    except Exception: return False

fds = []
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select(fds, [], [], 0.02)
        for fd in r:
            try: os.read(fd, 65536)
            except OSError: pass
def seen(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def type_(fd, text):
    for ch in text: os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
def say(fd, text, sec=20):
    count = len(seen()); type_(fd, text)
    end = time.time() + sec
    while time.time() < end and not [s for s in seen()[count:] if s["text"].endswith(text)]: pump(0.05)
    got = [s for s in seen() if s["text"].endswith(text)]
    assert got, f"{text!r} did not reach a delegate: {seen()}"
    return got[-1]

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0)); fds.append(fd); pump(2.5)

one = say(fd, "one")
old_pid, old_port = daemon()
out = cli("daemon", "restart"); print(out.stdout.strip())
assert out.returncode == 0 and "no run was going on" in out.stdout and "started the daemon" in out.stdout, (out.stdout, out.stderr)
new_pid, new_port = daemon()
assert new_pid != old_pid and new_port == old_port and healthy(new_port), (old_pid, new_pid, old_port, new_port)
pump(6)                                                   # the window finds the new daemon (REFIND_WAIT 5 s)
two = say(fd, "two")
assert two["resumed"] == one["session"], f"the conversation goes on in the new daemon: {one} -> {two}"
print("idle restart: pid", old_pid, "->", new_pid, "port", new_port, "kept; conversation", two["resumed"], "resumed")

type_(fd, "slow"); end = time.time() + 20
while time.time() < end and not [s for s in seen() if s["text"].endswith("slow")]: pump(0.05)
out = cli("daemon", "restart"); print(out.stdout.strip())
assert "1 window's run was stopped" in out.stdout, (out.stdout, out.stderr)
pump(6)
after = say(fd, "after-slow")
assert after["resumed"] == one["session"], after
print("busy restart: the run was stopped and said so; the tab answered next, same conversation")

out = cli("daemon", "stop"); print(out.stdout.strip())
assert "stopped the daemon" in out.stdout and "started" not in out.stdout and not healthy(new_port), out.stdout
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(1.0)   # the window, before it starts another
fds.remove(fd)
if healthy(daemon()[1]): cli("daemon", "stop")
out = cli("daemon", "restart"); print(out.stdout.strip())
assert "no daemon was running" in out.stdout and "started the daemon" in out.stdout and healthy(daemon()[1]), out.stdout
cli("daemon", "stop")
print("ok")
