#!/usr/bin/env python3
"""CON-155 (journal#1096, journal#1095): a daemon whose folder was removed does not live on. A window
on a fake Claude Code starts its daemon in a temporary home and sends a slow message, whose delegate
ignores SIGTERM. The window is closed, and the home removed under the daemon, as a test script did,
with no stop. Within 20 s (two checks of 5 s, up to 5 s for its runs to end, 3 s grace for a delegate) both the daemon and its delegate must be gone (the delegate KILLed after
the 3 s grace of CON-128), and the folder must not be brought back.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_daemon_orphan.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, signal, struct, tempfile, termios, time

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-orph-home"); users = home + "/dot-claude"; log = home + "/log"; fake = home + "/fake-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-orph-root-")); os.makedirs(users + "/projects")
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, signal, sys, time
if sys.argv[1:2] == ["auth"]: print(json.dumps({{"loggedIn": False}})); sys.exit(1)
sid = "sess-" + str(os.getpid())
print(json.dumps({{"type": "system", "subtype": "init", "session_id": sid, "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid()}}) + "\\n")
    # Busy and deaf to SIGTERM, as a delegate deep in a tool may be.
    signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(120)
''')
os.chmod(fake, 0o755)
os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=users)
address = home + "/data/harness-multitool/daemon/address.json"

def pump(fd, sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def alive(p):
    try:
        os.kill(p, 0)
        return open(f"/proc/{p}/stat").read().split()[2] != "Z"
    except OSError: return False

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0)); pump(fd, 2.5)
for ch in "slow": os.write(fd, ch.encode()); pump(fd, 0.02)
os.write(fd, b"\r")
end = time.time() + 20
while time.time() < end and not os.path.exists(log): pump(fd, 0.1)
assert os.path.exists(log), "the slow message reached no delegate"
delegate = json.loads(open(log).readline())["pid"]
daemon = json.load(open(address))["pid"]
print("daemon", daemon, "delegate", delegate, "running", flush=True)

# The window goes, then its folder, under a daemon nobody stops: what a test script did.
os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0)
pty_cleanup._rmtree(home)
gone_at = time.time()
end = gone_at + 30
while time.time() < end and (alive(daemon) or alive(delegate)): time.sleep(0.2)
took = time.time() - gone_at
checks = [
    ("the daemon stopped by itself", not alive(daemon)),
    ("its delegate, deaf to SIGTERM, did not outlive it", not alive(delegate)),
    ("the folder was not brought back", not os.path.exists(address)),
    ("within 20 s", took < 20),
]
print(f"daemon alive {alive(daemon)}, delegate alive {alive(delegate)}, after {took:.1f} s", flush=True)
for name, ok in checks: print(("ok   " if ok else "FAIL ") + name, flush=True)
for p in (daemon, delegate):
    if alive(p): os.kill(p, signal.SIGKILL)
sys.exit(0 if all(ok for _, ok in checks) else 1)
