#!/usr/bin/env python3
"""Debug, real programs (journal#1076): plan mode on the Codex delegate, in one real window on a
Codex row with the real `codex` signed in as the user did it.
(a) /plan: the status line says MODE plan; the agent, asked to create probe.txt, cannot: the file
    does not exist after its turn (Codex runs with -s read-only).
(b) /plan off: MODE normal; the same ask creates probe.txt.
Spends a little of the owner's plan. Leaves only its own behind: its temp folders, and the Codex
rollouts whose session_meta names its project folder are removed.
Run it capped: systemd-run --user --scope -p MemoryMax=4G -p MemorySwapMax=0 ...
Usage: tools/real_codex_plan.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, select, shutil, signal, struct, tempfile, termios, time
import pyte

bin_ = sys.argv[1]
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-cp-root")); home = tempfile.mkdtemp(prefix="mt-cp-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="codex"\nmodel=""\nplan_first="off"\n\n[session.providers.codex]\nkind="codex"\ncontext_window=272000\n')
COLS, ROWS = 160, 50
probe = os.path.join(root, "probe.txt")
pid, fd = pty.fork()
if pid == 0:
    for name in ("CODEX_HOME", "CLAUDE_CONFIG_DIR", "MULTITOOL_CHATGPT_DIR", "MULTITOOL_USERS_CLAUDE_JSON"): os.environ.pop(name, None)
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.03)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def status(): return next((l for l in text().splitlines() if "ADAPTER" in l), "")
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
def turn(ask):
    typed(ask + "\r")
    t0 = time.time(); pump(3.0)
    while time.time() - t0 < 300:
        pump(0.3)
        if "Waiting for your answer" in text(): os.write(fd, b"\x1b[C\r"); pump(0.5)
        if "Esc stops" not in text() and time.time() - t0 > 6: break
    pump(1.5)

ASK = "Create a file named probe.txt in the current folder containing the single word hi. Do it now, with a shell command or a patch, then say DONE or say why you could not."
checks = []
pump(3.0)
typed("/plan\r"); pump(1.5)
checks.append(("/plan puts the tab in plan mode", "MODE  plan" in status() or "MODE plan" in " ".join(status().split())))
turn(ASK)
checks.append(("in plan mode the file is not written", not os.path.exists(probe)))
print("plan answer:", " ".join(text()[text().rfind("ASSISTANT"):].split())[:400], flush=True)
pump(float(os.environ.get("MT_SETTLE", "0")))
typed("/plan off\r")
# It takes effect when the run on now ends: after a turn the run still brings the form in line,
# some seconds (journal#1083). The ask goes in at once, as a user would type it.
turn(ASK)
if os.environ.get("MT_DUMP"): print("AFTER /plan off status:", " ".join(status().split()), "\nSCREEN TAIL:\n" + "\n".join(text().splitlines()[-14:]), flush=True)
checks.append(("/plan off returns to normal", "MODE normal" in " ".join(status().split())))
checks.append(("out of plan mode the file is written", os.path.exists(probe)))
print("normal answer:", " ".join(text()[text().rfind("ASSISTANT"):].split())[:300], flush=True)
# The log: plan mode off is said before the second ask, not after it (journal#1083).
order = []
for log in glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl"):
    for line in open(log):
        e = json.loads(line)
        if e.get("event") == "plan_mode" and e.get("on") is False: order.append("off")
        if e.get("event") == "user_message": order.append("ask")
checks.append(("the log has plan mode off before the second ask", order[-2:] == ["off", "ask"] and order.count("ask") == 2))
print("log order:", order, flush=True)
for name, ok in checks: print(("ok   " if ok else "FAIL ") + name, flush=True)
os.kill(pid, signal.SIGTERM); pump(0.5)
try: os.waitpid(pid, 0)
except OSError: pass
for path in glob.glob(os.path.expanduser("~/.codex/sessions/**/rollout-*.jsonl"), recursive=True):
    try:
        with open(path) as f: meta = json.loads(f.readline())
        if meta.get("type") == "session_meta" and meta["payload"].get("cwd") == root: os.remove(path)
    except (OSError, ValueError, KeyError): pass
keep = os.environ.get("MT_KEEP")
if keep and not all(ok for _, ok in checks): shutil.copytree(home + "/data", keep, dirs_exist_ok=True, ignore=shutil.ignore_patterns("*.sock"))
shutil.rmtree(home, ignore_errors=True); shutil.rmtree(root, ignore_errors=True)
sys.exit(0 if all(ok for _, ok in checks) else 1)
