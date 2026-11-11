#!/usr/bin/env python3
"""Debug, real programs (journal#926, CON-134, CON-144): one window, one tab, its conversation moved
between adapters through Settings > Provider as the user does it. At each step the tab names the
code words said so far and is given a new one; the status line must show that step's adapter. Spends
a little of the owner's plans. Claude Code and Codex run as the user signed them in.
What it leaves behind is its own only: Claude Code's transcripts and Codex's rollouts of its
temporary project folder are removed at the end.
Run it capped: systemd-run --user --scope -p MemoryMax=4G -p MemorySwapMax=0 ...
Usage: tools/real_move.py <absolute path to multitool> [claude codex claude]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, re, select, shutil, signal, struct, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; chain = sys.argv[2:] or ["claude", "codex", "claude"]
ROW = {"claude": "claude-code", "codex": "codex"}
ADAPTER = {"claude": "claude-code", "codex": "codex"}
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-rm-root")); home = tempfile.mkdtemp(prefix="mt-rm-home"); os.makedirs(home + "/cfg/harness-multitool")
first = chain[0]
open(home + "/cfg/harness-multitool/config.toml", "w").write(
    f'[session]\nprovider="{ADAPTER[first]}"\nmodel="{"claude-haiku-5-5" if first == "claude" else ""}"\nplan_first="off"\n\n'
    '[session.providers.claude-code]\nkind="claude-code"\n\n[session.providers.codex]\nkind="codex"\ncontext_window=272000\n')
WORDS = ["лазурит", "кварцит", "малахит", "гранат", "обсидиан"]
COLS, ROWS = 130, 46
pid, fd = pty.fork()
if pid == 0:
    # The user's own Claude Code and Codex, signed in: not the folders of the profile this runs under.
    for name in ("CODEX_HOME", "CLAUDE_CONFIG_DIR", "MULTITOOL_CHATGPT_DIR", "MULTITOOL_USERS_CLAUDE_JSON"): os.environ.pop(name, None)
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(bin_, [bin_, "preview", "welcome"])
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
def status(): return next((l for l in text().splitlines() if "ADAPTER" in l), "")
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)

def move_to(row):
    """Settings > Provider: the row named `row`, by its index in the list as drawn, chosen and saved."""
    typed("/settings\r"); pump(1.0)
    rows = [m.group(1) for l in text().splitlines() for m in [re.search(r"\( [ ●] \)  (\S+(?: \S+)?)", l)] if m]
    names = [r.split("   ")[0].strip() for r in rows]
    index = next(i for i, n in enumerate(names) if n.startswith(row))
    if os.environ.get("MT_DUMP"): print("rows:", names, "->", index, flush=True)
    os.write(fd, b"\t"); pump(0.3)
    for _ in range(12): os.write(fd, b"\x1b[A"); pump(0.03)
    for _ in range(index): os.write(fd, b"\x1b[B"); pump(0.05)
    os.write(fd, b"\r"); pump(0.4)
    if os.environ.get("MT_DUMP"): print("\n".join(l for l in text().splitlines() if "( " in l), flush=True)
    os.write(fd, b"\x13"); pump(0.6)
    if os.environ.get("MT_DUMP"): print("AFTER SAVE:\n" + text(), flush=True)
    if "Esc close" in text(): os.write(fd, b"\x1b"); pump(0.3)
    # The move: its toast comes and goes (CON-144), whatever it says as the work goes on.
    moving = lambda t: any(w in t for w in ("Bringing the conversation in line", "Claude Code's copy", "Codex's copy", "starting…", "Catching up"))
    end = time.time() + 180
    pump(0.5)
    while time.time() < end and (moving(text()) or f"ADAPTER  {row}" not in status()):
        pump(0.2)
    pump(1.0)

pump(2.5)
results = []
for step, name in enumerate(chain):
    if step:
        move_to(ROW[name])
    on = status()
    known = WORDS[:step]
    ask = (f"Какие кодовые слова я называл раньше в этой переписке? Перечисли их. " if step else "") + f"Новое кодовое слово: {WORDS[step]}. Ничего не запускай, просто ответь коротко, инструментов не вызывай."
    typed(ask + "\r")
    t0 = time.time(); pump(2.0)
    while time.time() - t0 < 240:
        pump(0.3)
        if "Waiting for your answer" in text(): os.write(fd, b"\x1b[C\r"); pump(0.5)
        if "Esc stops" not in text() and time.time() - t0 > 4: break
    pump(1.0)
    s = text(); tail = s[s.rfind("ASSISTANT"):] if "ASSISTANT" in s else s[-700:]
    found = [w for w in known if w in tail]
    adapter_ok = f"ADAPTER  {ADAPTER[name]}" in on
    ok = found == known and adapter_ok
    results.append((name, round(time.time() - t0, 1), known, found, adapter_ok, ok))
    print(f"--- step {step + 1}: {name} ({'on ' + ADAPTER[name] if adapter_ok else 'WRONG ADAPTER: ' + on.strip()[:80]}), {time.time() - t0:.1f}s; asked for {known}, named {found}: {'ok' if ok else 'MISSED'}", flush=True)
    print("\n".join(l[:120] for l in tail.splitlines()[:6]), flush=True)
    if os.environ.get("MT_DUMP"): print("SCREEN:\n" + s, flush=True)
print(json.dumps(results, ensure_ascii=False))
os.kill(pid, signal.SIGTERM); pump(0.5)
try: os.waitpid(pid, 0)
except OSError: pass
# Only its own: the transcripts of its temp folder, and the rollouts whose session_meta names it.
slug = "".join(c if c.isascii() and c.isalnum() else "-" for c in root)
shutil.rmtree(os.path.expanduser("~/.claude/projects/") + slug, ignore_errors=True)
for path in glob.glob(os.path.expanduser("~/.codex/sessions/**/rollout-*.jsonl"), recursive=True):
    try:
        with open(path) as f: meta = json.loads(f.readline())
        if meta.get("type") == "session_meta" and meta["payload"].get("cwd") == root: os.remove(path)
    except (OSError, ValueError, KeyError): pass
shutil.rmtree(home, ignore_errors=True); shutil.rmtree(root, ignore_errors=True)
sys.exit(0 if all(r[5] for r in results) else 1)
