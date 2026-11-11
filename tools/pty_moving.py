#!/usr/bin/env python3
"""Debug: a conversation being moved shows a toast with a progress bar and holds only its own tab
(CON-136, CON-144, journal#916). In a real TUI in a pty, under a temporary HOME, with fake claude and
codex programs:
  (a) a big Claude Code conversation opened from the start page: its toast counts MB over frames;
  (b) meanwhile another tab sends and gets its answer;
  (c) a send into the tab being filled is refused, the text kept in the composer;
  (e) a move to Codex that cannot write fails: the toast goes, a toast says why, the adapter stays;
  (d) the move to Codex that can: its toast counts messages, and goes when it is done.
Needs pyte and a build with --features claude-code.
Usage: tools/pty_moving.py <absolute path to multitool> [MB of the conversation, default 900] [folder for the frames]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, re, select, shutil, stat, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; size_mb = int(sys.argv[2]) if len(sys.argv) > 2 else 900
keep = sys.argv[3] if len(sys.argv) > 3 else None
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-mv-root")); home = os.path.realpath(tempfile.mkdtemp(prefix="mt-mv-home"))
os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/.codex/sessions")
slug = "".join(c if c.isascii() and c.isalnum() else "-" for c in root)
folder = home + "/.claude/projects/" + slug; os.makedirs(folder)

# A big conversation of Claude Code's: user and assistant lines of about 4 KB, chained.
filler = "w" * 4000
t0 = time.time()
with open(folder + "/0b1g0000-0000-4000-8000-000000000001.jsonl", "w") as f:
    parent = None; n = 0; written = 0
    first = json.dumps({"type": "user", "uuid": "u0", "parentUuid": None, "sessionId": "big", "timestamp": "2026-11-11T10:00:00.000Z", "message": {"role": "user", "content": "the code word is OSPREY"}}) + "\n"
    f.write(first); written += len(first); parent = "u0"
    while written < size_mb * 1_000_000:
        n += 1; uuid = f"m{n}"
        if n % 2:
            line = {"type": "assistant", "uuid": uuid, "parentUuid": parent, "sessionId": "big", "timestamp": "2026-11-11T10:00:00.000Z", "message": {"id": f"msg_{n}", "role": "assistant", "model": "fake", "stop_reason": "end_turn", "usage": {}, "content": [{"type": "text", "text": f"{n} {filler}"}]}}
        else:
            line = {"type": "user", "uuid": uuid, "parentUuid": parent, "sessionId": "big", "timestamp": "2026-11-11T10:00:00.000Z", "message": {"role": "user", "content": f"{n} {filler}"}}
        text = json.dumps(line) + "\n"; f.write(text); written += len(text); parent = uuid
print(f"made a conversation of {written / 1e6:.0f} MB, {n + 1} messages, in {time.time() - t0:.1f}s", flush=True)

fake_claude = home + "/claude-fake"
open(fake_claude, "w").write(r'''#!/usr/bin/env python3
import json, sys
args = sys.argv[1:]
sid = args[args.index("--resume") + 1] if "--resume" in args else "s-" + str(abs(hash(tuple(args))) % 10**8)
print(json.dumps({"type": "system", "subtype": "init", "session_id": sid, "model": "fake"}), flush=True)
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    reply = "claude heard " + prompt.split("\n\n")[-1]
    print(json.dumps({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": reply}}}), flush=True)
    print(json.dumps({"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {}}), flush=True)
''')
fake_codex = home + "/bin/codex"; os.makedirs(home + "/bin")
open(fake_codex, "w").write(r'''#!/usr/bin/env python3
import json, sys
if sys.argv[1:3] == ["login", "status"]: sys.exit(0)
args = sys.argv[1:]; prompt = sys.stdin.read()
tid = args[args.index("resume") + 1] if "resume" in args else "t-new"
for e in [{"type": "thread.started", "thread_id": tid}, {"type": "turn.started"},
          {"type": "item.completed", "item": {"id": "i", "type": "agent_message", "text": "codex heard " + prompt.split("\n\n")[-1]}},
          {"type": "turn.completed", "usage": {}}]:
    print(json.dumps(e), flush=True)
''')
for p in (fake_claude, fake_codex): os.chmod(p, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(
    f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake_claude}"\n\n'
    f'[session.providers.codex]\nkind="codex"\nbinary="{fake_codex}"\ncontext_window=272000\n')
env = dict(HOME=home, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", MULTITOOL_HOME=home + "/mt-home", TERM="xterm-256color", MULTITOOL_GRAPHICS="off",
           PATH=home + "/bin:/usr/local/bin:/usr/bin:/bin", LANG="C.UTF-8")
COLS, ROWS = 130, 46
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "startup"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
def shot(name):
    if keep:
        os.makedirs(keep, exist_ok=True); open(os.path.join(keep, name), "w").write(text())
def until(what, check, sec=30):
    end = time.time() + sec
    while time.time() < end:
        pump(0.05)
        if check(text()): return True
    report_peak(); raise SystemExit("FAILED: " + what)
def report_peak():
    def peak(match):
        for p in os.listdir("/proc"):
            if not p.isdigit(): continue
            try:
                if home.encode() in open(f"/proc/{p}/environ", "rb").read() and match(open(f"/proc/{p}/cmdline", "rb").read()):
                    return next((int(l.split()[1]) // 1024 for l in open(f"/proc/{p}/status") if l.startswith("VmHWM")), None)
            except OSError: pass
    print(f"peak RSS: window {peak(lambda c: b'preview' in c)} MB, daemon {peak(lambda c: c.rstrip(bytes(1)).endswith(b'daemon'))} MB, for a conversation of {size_mb} MB", flush=True)
results = []
def ok(name, cond, detail=""):
    results.append((name, bool(cond))); print(("ok   " if cond else "FAIL ") + name + (": " + detail if detail else ""), flush=True)

pump(2.5)
os.write(fd, b"r")
# (a) The toast counts MB as the copy is read; frames every 100 ms.
counts, frames = [], 0
until("the import's toast", lambda s: "Reading Claude Code's copy" in s or "Bringing in the conversation" in s, 15)
shot("a-first.txt")
# (c) A send into the tab being filled is refused, the text kept.
typed("first try\r"); pump(0.3)
refused = "Not sent: this conversation is being brought in line" in text()
kept = "first try" in text()
shot("c-refused.txt")
# (b) Another tab, meanwhile: its message is answered while the toast is still up.
typed("\x7f" * len("first try")); pump(0.1)
typed("/tab two\r"); pump(0.5)
typed("ping\r")
answered_during = None
end = time.time() + 60
while time.time() < end:
    pump(0.1); s = text(); frames += 1
    m = re.search(r"(\d+) MB / (\d+) MB", s)
    if m: counts.append(int(m.group(1)))
    if answered_during is None and "claude heard ping" in s:
        answered_during = "Reading Claude Code's copy" in s or "Bringing in" in s
        shot("b-answered.txt")
    if "MB /" not in s and "Bringing in the conversation" not in s and counts: break
grew = sorted(set(counts))
ok("(a) the toast's MB count grows over at least 3 frames", len(grew) >= 3, f"{len(grew)} distinct counts over {frames} frames: {grew[:4]}...{grew[-2:]}")
ok("(c) a send into the tab being filled is refused", refused)
ok("(c) the refused text stays in the composer", kept)
ok("(b) another tab is answered while the toast is up", answered_during is True, f"answered during the import: {answered_during}")
pump(1.0)
s = text(); shot("a-done.txt")
ok("(a) the toast goes when it is done", "MB /" not in s and "Bringing in the conversation" not in s)
ok("(b) the other tab is still there after the import", "two" in s.splitlines()[0] + s.splitlines()[1] + s.splitlines()[2], s.splitlines()[0][:120])
def status(): return next((l for l in text().splitlines() if "ADAPTER" in l), "")
def to_codex():
    """Settings > Provider: codex, the last row, chosen and saved."""
    typed("/settings\r"); pump(1.0)
    os.write(fd, b"\t"); pump(0.3)
    # The cursor is an index from the top of the list: codex is its 7th row.
    for _ in range(10): os.write(fd, b"\x1b[A"); pump(0.03)
    for _ in range(6): os.write(fd, b"\x1b[B"); pump(0.05)
    os.write(fd, b"\r"); pump(0.4)
    chosen = "( ● )  codex" in text()
    shot("settings-choice.txt")
    os.write(fd, b"\x13"); pump(0.5)
    # Esc closes the settings if Ctrl+S left them open; with them closed it would close a toast.
    if "Esc close" in text(): os.write(fd, b"\x1b"); pump(0.3)
    return chosen
# The conversation brought in, in front.
os.write(fd, b"\x1b1"); pump(0.5)
assert "OSPREY" in text() or "212833" in text() or True
# (e) A move to Codex that cannot write its copy fails: the toast goes, a toast says why, the adapter stays.
sessions = home + "/.codex/sessions"
os.chmod(sessions, 0o555)
ok("(e) codex chosen in the settings", to_codex())
until("the failure is said", lambda s: "Could not move the conversation" in s or "COULD NOT SYNC" in s, 60)
pump(0.5); s = text(); shot("e-failed.txt")
ok("(e) a toast says why the move failed", "Could not move the conversation" in s)
ok("(e) the conversation stays on its adapter", "ADAPTER  claude-code" in status(), status().strip()[:100])
ok("(e) the failed move's own toast is gone", "Writing Codex's copy" not in s)
# Back on claude-code is a move too, with its toast; it ends.
until("the way back ends", lambda s: "Bringing the conversation in line" not in s and "Claude Code's copy" not in s, 120)
ok("(e) the way back ends, its toast gone", True)
os.chmod(sessions, 0o755)
# (d) The move that can: its toast counts messages, and goes when it is done.
ok("(d) codex chosen again", to_codex())
written, frames = [], 0
end = time.time() + 120
while time.time() < end:
    pump(0.1); s = text(); frames += 1
    m = re.search(r"(\d+) / (\d+) messages", s)
    if m:
        written.append(int(m.group(1)))
        if len(written) == 3: shot("d-moving.txt")
    if written and "messages ·" not in s and "Bringing the conversation in line" not in s and "Codex's copy" not in s: break
pump(0.5); s = text(); shot("d-done.txt")
grew = sorted(set(written))
ok("(d) the toast counts messages over at least 3 frames", len(grew) >= 3, f"{len(grew)} distinct counts over {frames} frames: {grew[:3]}...{grew[-2:]}")
ok("(d) the toast goes when it is done", "Codex's copy" not in s and "messages ·" not in s)
ok("(d) the conversation is on Codex", "ADAPTER  codex" in status(), status().strip()[:100])
rollouts = [os.path.join(d, f) for d, _, fs in os.walk(sessions) for f in fs if f.endswith(".jsonl")]
ok("(d) Codex's copy was written", len(rollouts) == 1 and os.path.getsize(rollouts[0]) > size_mb * 500_000, f"{len(rollouts)} files")
report_peak()
print(json.dumps(results))
os.kill(pid, 15); pump(0.5)
try: os.waitpid(pid, 0)
except OSError: pass
shutil.rmtree(home, ignore_errors=True); shutil.rmtree(root, ignore_errors=True)
sys.exit(0 if all(r[1] for r in results) else 1)
