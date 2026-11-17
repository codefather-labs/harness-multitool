#!/usr/bin/env python3
"""CON-182 (journal#1356): no delegate outlives the daemon that started it, however it ends. A window
on a fake Claude Code whose turn takes 60 s and ignores SIGTERM (a delegate busy in a tool):
- SIGTERM to the daemon, as systemd's stop sends: the daemon ends cleanly and its delegate with it;
- SIGKILL to the daemon, which nothing can catch: the next daemon, started by the window, reaps the
  dead daemon's scope, and the delegate is gone.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_daemon_restart.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, json, pty, select, struct, subprocess, tempfile, termios, time, urllib.request

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-dsig-home"); mt = home + "/mt-home"; users = home + "/dot-claude"; log = home + "/log"; fake = home + "/fake-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-dsig-root-")); os.makedirs(users + "/projects")
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys, time
if sys.argv[1:2] == ["auth"]: print(json.dumps({{"loggedIn": False}})); sys.exit(1)
args = sys.argv[1:]
resumed = args[args.index("--resume") + 1] if "--resume" in args else None
sid = resumed or "sess-" + str(os.getpid())
print(json.dumps({{"type": "system", "subtype": "init", "session_id": sid, "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"text": text, "resumed": resumed, "session": sid, "pid": os.getpid(), "ppid": os.getppid(), "at": time.time()}}) + "\\n")
    # A slow turn that also ignores SIGTERM, as a real delegate busy with a tool may: its daemon
    # ends only after the grace and a KILL (journal#1054).
    if text.endswith("slow"):
        import signal; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)
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
def alive(pid):
    try: os.kill(pid, 0)
    except ProcessLookupError: return False
    try: return open(f"/proc/{pid}/stat").read().split(")")[1].split()[0] != "Z"
    except OSError: return False
def busy_delegate(word):
    """A new turn ending in `word`, and the pid of the delegate busy with it."""
    before = len([s for s in seen() if s["text"].endswith(word)])
    type_(fd, word); end = time.time() + 30
    while time.time() < end and len([s for s in seen() if s["text"].endswith(word)]) == before: pump(0.05)
    got = [s for s in seen() if s["text"].endswith(word)]
    assert len(got) > before, f"no new {word!r} turn: {seen()}"
    return got[-1]["pid"]
results = {}
say(fd, "one")
# 1. SIGTERM, as systemd sends when its ExecStop cannot reach the daemon.
slow = busy_delegate("slow")
daemon_pid, _ = daemon()
os.kill(daemon_pid, 15)
end = time.time() + 12
while time.time() < end and (alive(slow) or alive(daemon_pid)): pump(0.1)
results["sigterm_daemon_ends"] = not alive(daemon_pid)
results["sigterm_delegate_ends"] = not alive(slow)
print("SIGTERM: daemon", daemon_pid, "alive" if alive(daemon_pid) else "gone", "; delegate", slow, "alive" if alive(slow) else "gone")
if alive(slow): os.kill(slow, 9)
# 2. SIGKILL: the next daemon reaps the scope the dead one left.
pump(8)                                                   # the window starts a new daemon
say(fd, "two", 40)
slow = busy_delegate("slow")
daemon_pid, _ = daemon()
os.kill(daemon_pid, 9)
# The window starts the next daemon when it next needs one, which reaps first; the scope's starter
# is gone either way. Asked for at once here with a message.
pump(3)
type_(fd, "wake")
for i in range(6):
    pump(3)
    procs = subprocess.run(["bash", "-c", f"for p in $(pgrep -f 'multitool daemon'); do tr '\\0' ' ' < /proc/$p/environ 2>/dev/null | grep -q '{home}' && echo $p $(ps -o stat= -p $p); done"], capture_output=True, text=True).stdout.split("\n")
    try: addr = daemon()
    except Exception as e: addr = e
    print(f"t+{3*(i+1)}s daemons of this home: {[p for p in procs if p]} address {addr}", flush=True)
end = time.time() + 30
while time.time() < end and alive(slow): pump(0.2)
try: print("daemon now:", daemon(), "was", daemon_pid)
except Exception as e: print("no daemon address:", e)
results["sigkill_delegate_reaped_by_the_next_daemon"] = not alive(slow)
print("SIGKILL: delegate", slow, "alive" if alive(slow) else "gone")
if alive(slow): os.kill(slow, 9)
print("RESULTS:", json.dumps(results))
os.kill(pid, 9)
try:
    a = json.load(open(address)); os.kill(a["pid"], 15)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
