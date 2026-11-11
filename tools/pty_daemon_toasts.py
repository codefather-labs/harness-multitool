#!/usr/bin/env python3
"""CON-131 (journal#863): a window shows a toast every time its connection to the daemon is lost
and every time one is made, the first included. A real window in a pty on its own data directory;
the screen is read with pyte (no daemon is there to take a screenshot while it is down), and a
debug screenshot is kept after each connection as proof.
The window starts bare, on the start page, as the user starts it (journal#873), and goes into a new
session. Rounds: start -> 'Connected to the daemon, pid A'; `multitool daemon stop` -> 'Connection to the
daemon lost' within 1 s, one such toast however long it lasts; a daemon started again ->
'Reconnected to the daemon, pid B' within 2.5 s, B != A. Three rounds.
Usage: tools/pty_daemon_toasts.py <absolute path to multitool> [a directory to keep the pictures in]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, json, pty, re, select, shutil, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; keep = sys.argv[2] if len(sys.argv) > 2 else None
COLS, ROWS = 120, 34
home = tempfile.mkdtemp(prefix="mt-toast-home"); root = os.path.realpath(tempfile.mkdtemp(prefix="mt-toast-root-"))
os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="local"\nmodel="m"\nplan_first="off"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:9"\n\n[session.auth.local]\nnone=true\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
address = home + "/data/harness-multitool/daemon/address.json"
def cli(*args): return subprocess.run([bin_, *args], env=env, capture_output=True, text=True, cwd=root, timeout=60)
def daemon_pid(): return json.load(open(address))["pid"]
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    # As the user starts it: bare, on the start page (journal#873), then into a new session.
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def flat(): return re.sub(r"\s+", " ", text())
def until(what, check, sec):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), f"{what}:\n{text()}"
    return time.time()
shots = []
def shot(name):
    out = home + "/" + name
    p = subprocess.Popen([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    start = time.time()
    while p.poll() is None and time.time() - start < 20: pump(0.05)
    assert p.returncode == 0, p.communicate()[1]
    shots.append(out)
    if keep: shutil.copy(out, os.path.join(keep, name))

pump(3.0)
os.write(fd, b"\r"); pump(1.5)                     # the splash, then the start page
os.write(fd, b"n"); pump(1.0)                       # a new session
until("the first connection's toast, though the connection was made on the start page", lambda: "Connected to the daemon, pid" in flat(), 15)
first = daemon_pid()
assert f"pid {first}" in flat(), flat()
print("start:", re.search(r"Connected to the daemon, pid \d+", flat()).group(0))
shot("toast-connected.png")
pids = [first]
for round_ in range(1, 4):
    stopped = cli("daemon", "stop"); assert "stopped the daemon" in stopped.stdout, stopped.stdout + stopped.stderr
    t0 = time.time()
    t = until(f"round {round_}: the loss's toast", lambda: "Connection to the daemon lost" in flat(), 3)
    print(f"round {round_}: lost shown after {t - t0:.2f} s")
    assert t - t0 < 1.0, t - t0
    pump(4.5)                                    # the loss lasts: more tries, still the one toast
    assert flat().count("Connection to the daemon lost") == 1, flat()
    t0 = time.time()
    started = cli("daemon", "restart"); assert "started the daemon" in started.stdout, started.stdout + started.stderr
    t = until(f"round {round_}: the reconnection's toast", lambda: "Reconnected to the daemon, pid" in flat(), 6)
    now = daemon_pid(); pids.append(now)
    assert f"Reconnected to the daemon, pid {now}" in flat(), flat()
    print(f"round {round_}: reconnected to pid {now} shown {t - t0:.2f} s after the restart command")
    assert t - t0 < 2.5 + 1.0, t - t0               # the command itself takes up to a second
    shot(f"toast-reconnected-{round_}.png")
assert len(set(pids)) == 4, pids
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(1.0)
cli("daemon", "stop")
print("pids", pids, "ok")
