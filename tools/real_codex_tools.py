#!/usr/bin/env python3
"""Debug, real programs (journal#1068, journal#1069): one window on a Codex row, the real `codex`
signed in as the user did it. The Codex delegate is asked to name the tools of the harness's `mode`
server, to say whether it has the window's own tools (game_invite, agent_list), and to write a
journal entry. Each must hold: the mode tools there, the window tools there, the entry written.
Spends a little of the owner's plan. Leaves only its own behind: its temp folders, and the Codex
rollouts whose session_meta names its project folder are removed.
Run it capped: systemd-run --user --scope -p MemoryMax=4G -p MemorySwapMax=0 ...
Usage: tools/real_codex_tools.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, re, select, shutil, signal, struct, tempfile, termios, time
import pyte

bin_ = sys.argv[1]
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-ct-root")); home = tempfile.mkdtemp(prefix="mt-ct-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="codex"\nmodel=""\nplan_first="off"\n\n[session.providers.codex]\nkind="codex"\ncontext_window=272000\n')
COLS, ROWS = 160, 50
pid, fd = pty.fork()
if pid == 0:
    # The user's own Codex, signed in: not the folders of the profile this runs under.
    for name in ("CODEX_HOME", "CLAUDE_CONFIG_DIR", "MULTITOOL_CHATGPT_DIR", "MULTITOOL_USERS_CLAUDE_JSON", "MULTITOOL_OWN_CLAUDE_CONFIG_DIR", "MULTITOOL_OWN_CODEX_HOME"): os.environ.pop(name, None)
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
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)

pump(3.0)
ask = ("Do not run shell commands and do not edit files. Answer in four short lines. "
       "Line 1: MODE= then the names of the tools of the MCP server named mode, comma-separated, without prefixes. "
       "Line 2: WIN=yes if you have both the harness tools game_invite and agent_list, else WIN=no. "
       "Then call the harness tool journal_add with body 'codex window check', category 'finding', tags ['check'], and write Line 3: JOURNAL= then the id it answered. "
       "Last, call the tool plan_enter of the mode server, and write Line 4: PLAN=ok if it answered that plan mode is on, else PLAN= then its error.")
typed(ask + "\r")
t0 = time.time(); pump(3.0)
while time.time() - t0 < 300:
    pump(0.3)
    if "Waiting for your answer" in text(): os.write(fd, b"\x1b[C\r"); pump(0.5)
    if "Esc stops" not in text() and time.time() - t0 > 6: break
pump(1.5)
s = text(); tail = s[s.rfind("ASSISTANT"):] if "ASSISTANT" in s else s[-1500:]
flat = " ".join(tail.split())
mode = re.search(r"MODE=\s*([^\n]*?)(?=\s+WIN=|$)", flat)
win = re.search(r"WIN=\s*(\w+)", flat)
jid = re.search(r"JOURNAL=\s*(\d+)", flat)
plan = re.search(r"PLAN=\s*(\S+)", flat)
def status(): return " ".join(next((l for l in text().splitlines() if "ADAPTER" in l), "").split())
# The mode goes on when the run ends, and after a turn it still brings the form in line (journal#1083).
t1 = time.time()
while "MODE plan" not in status() and time.time() - t1 < 30: pump(0.5)
status = status()
mode_tools = mode.group(1) if mode else ""
checks = [
    ("the mode server's tools are there (plan_enter, plan_exit, debug)", all(t in mode_tools for t in ("plan_enter", "plan_exit", "debug"))),
    ("the window's own tools are there (game_invite, agent_list)", bool(win and win.group(1).lower() == "yes")),
    ("journal_add answered an id", bool(jid)),
    # Only this window's daemon knows the window: another answers 'this window is not open here' (journal#1084).
    ("plan_enter reached this window's daemon", bool(plan and plan.group(1).lower().startswith("ok"))),
    ("the status line says MODE plan", "MODE plan" in status),
]
print("answer:", flat[:700], flush=True)
print("status:", status, flush=True)
for name, ok in checks: print(("ok   " if ok else "FAIL ") + name, flush=True)
os.kill(pid, signal.SIGTERM); pump(0.5)
try: os.waitpid(pid, 0)
except OSError: pass
for path in glob.glob(os.path.expanduser("~/.codex/sessions/**/rollout-*.jsonl"), recursive=True):
    try:
        with open(path) as f: meta = json.loads(f.readline())
        if meta.get("type") == "session_meta" and meta["payload"].get("cwd") == root: os.remove(path)
    except (OSError, ValueError, KeyError): pass
shutil.rmtree(home, ignore_errors=True); shutil.rmtree(root, ignore_errors=True)
sys.exit(0 if all(ok for _, ok in checks) else 1)
