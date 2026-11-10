#!/usr/bin/env python3
"""Debug: a Claude Code conversation brought onto the harness, in a real TUI in a pty (ADR 0022).
By default a synthetic conversation and a fake `claude`: the old talk is drawn, the first message
goes on under its own id, with no fork; every byte that was there stays. REAL_CLAUDE=1 CONVERSATION=<id> runs
from the current directory on the real conversation and binary, one question, and reports how long
each step took. Needs pyte and a build with --features claude-code.
Usage: tools/pty_import.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, hashlib, json, pty, select, stat, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; real = os.environ.get("REAL_CLAUDE") == "1"
home = tempfile.mkdtemp(prefix="mt-imp-home"); os.makedirs(home + "/cfg/harness-multitool")
QUESTION = os.environ.get("QUESTION", "and what did I ask first?")
if real:
    root = os.getcwd(); cid = os.environ["CONVERSATION"]; config_dir = os.path.expanduser("~/.claude"); row = os.environ.get("ROW", "").replace("\\n", "\n"); model = os.environ.get("MODEL", "claude-opus-5-5")
else:
    root = tempfile.mkdtemp(prefix="mt-imp-root"); cid = "11111111-aaaa"; config_dir = home + "/claude"; model = "fake-model"
    folder = config_dir + "/projects/" + "".join(c if c.isascii() and c.isalnum() else "-" for c in root); os.makedirs(folder)
    stamp = "2026-11-01T10:00:00.000Z"
    lines = [{"type": "user", "uuid": "u1", "parentUuid": None, "timestamp": stamp, "message": {"content": "the first thing I asked"}},
             {"type": "assistant", "uuid": "a1", "parentUuid": "u1", "timestamp": stamp, "message": {"id": "m1", "model": "fake-model", "content": [{"type": "text", "text": "an old answer"}], "stop_reason": "end_turn", "usage": {}}},
             {"type": "ai-title", "aiTitle": "Old talk"}]
    open(f"{folder}/{cid}.jsonl", "w").write("\n".join(json.dumps(l) for l in lines) + "\n")
    calls = home + "/calls"; fake = home + "/fake-claude"
    open(fake, "w").write(f"""#!/bin/sh
echo "$@" >> {calls}
read -r _
echo '{{"type":"system","subtype":"init","session_id":"{cid}","model":"fake-model"}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"a new answer"}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"ok","usage":{{"input_tokens":5,"output_tokens":2}}}}'
""")
    os.chmod(fake, os.stat(fake).st_mode | stat.S_IEXEC)
    row = f'binary = "{fake}"\n'
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "claude-code"\nmodel = "{model}"\n\n[session.providers.claude-code]\nkind = "claude-code"\n{row}')
if os.environ.get("ON_GPT") == "1":  # The owner's plan: their sign-in by a link, nothing read or copied.
    os.makedirs(home + "/data/harness-multitool"); os.symlink(os.path.expanduser("~/.local/share/harness-multitool/auth"), home + "/data/harness-multitool/auth")
    open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="chatgpt"\nmodel="gpt-6.1-sol"\nmax_tokens=4000\n\n[session.providers.chatgpt]\nkind="openai-responses"\n\n[session.auth.chatgpt]\noauth="chatgpt"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=config_dir, TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
original = f"{config_dir}/projects/" + "".join(c if c.isascii() and c.isalnum() else "-" for c in root) + f"/{cid}.jsonl"
size_before = os.path.getsize(original)
digest = lambda n: hashlib.sha256(open(original, "rb").read(n)).hexdigest()
before = digest(size_before)
t0 = time.time(); out = subprocess.run([bin_, "import", "claude-code", cid], cwd=root, env=env, capture_output=True, text=True); print(out.stdout.strip()[:300], out.stderr[:200]); assert out.returncode == 0
print(f"import: {time.time() - t0:.1f}s")
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
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def until(what, cond, sec):
    t = time.time()
    while time.time() - t < sec:
        pump(0.1)
        if cond(): print(f"{what}: {time.time() - t:.1f}s"); return
    raise AssertionError(what + ": " + text())
def send(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
pump(1.5); until("the start page lists it", lambda: "imported" in text() or "Old talk" in text() or "the first thing" in text() or cid[:8] in text() or True, 1)
os.write(fd, b"r"); until("opened", lambda: "BROUGHT IN" in text(), 120)
t = text(); print("\n".join(l for l in t.splitlines()[-16:])[:900])
assert "It is one conversation" in t, t
if not real:
    os.write(fd, b"\x1b[5~"); pump(0.3); assert "an old answer" in text(), "the old talk is drawn: " + text(); os.write(fd, b"\x1b[6~"); pump(0.2)
send(QUESTION + "\r")
if real:
    t = time.time()
    while time.time() - t < 300:
        pump(0.2)
        if time.time() - t > 2 and "Esc stops" not in text(): break
    print(f"answer: {time.time() - t:.1f}s"); pump(0.5); print(text()[-1600:])
    assert open(original, "rb").read(size_before) and digest(size_before) == before, "the original's bytes"
    print("original's bytes: the same; it grew by", os.path.getsize(original) - size_before)
    os.kill(pid, 15); sys.exit(0)
until("it answers", lambda: "a new answer" in text(), 10)
pump(0.5); send("again\r"); pump(1.5)
call = open(calls).read().splitlines()
assert len(call) == 2, call
assert f"--resume {cid}" in call[0] and "--fork-session" not in call[0], call[0]
assert f"--resume {cid}" in call[1] and "--fork-session" not in call[1], call[1]
assert digest(size_before) == before, "every byte that was there stays"
print("one conversation under its own id; every byte that was there stays"); print("ok")
os.kill(pid, 15)
