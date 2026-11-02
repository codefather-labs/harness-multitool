#!/usr/bin/env python3
"""Debug: the window waits for the daemon while a conversation is synced (CON-81), in a real TUI in a pty,
with a slow, large conversation of a fake Claude Code. The moon is on screen and typing does nothing
until the daemon signals; then the window is free. A failure says so and leaves the adapter as it was.
Needs pyte and --features claude-code.
Usage: tools/pty_sync.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, signal, struct, tempfile, termios, time, tomllib
import pyte

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-sy-root")); home = tempfile.mkdtemp(prefix="mt-sy-home"); os.makedirs(home + "/bin"); os.makedirs(home + "/cfg/harness-multitool")
claude_dir = home + "/claude"; folder = claude_dir + "/projects/" + "".join(c if c.isascii() and c.isalnum() else "-" for c in root); os.makedirs(folder)
cid = "11111111-2222-4333-8444-555555555555"; form = f"{folder}/{cid}.jsonl"
# Big enough to take a moment: a few hundred megabytes of a real shape.
filler = "x" * 200_000
with open(form, "w") as f:
    parent = None
    for i in range(int(os.environ.get("TURNS", "2500"))):
        u = f"u{i}"; a = f"a{i}"
        f.write(json.dumps({"type": "user", "uuid": u, "parentUuid": parent, "isSidechain": False, "cwd": root, "sessionId": cid, "timestamp": "2026-11-01T10:00:00.000Z", "message": {"role": "user", "content": f"question {i} {filler}"}}) + "\n")
        f.write(json.dumps({"type": "assistant", "uuid": a, "parentUuid": u, "isSidechain": False, "cwd": root, "sessionId": cid, "timestamp": "2026-11-01T10:00:00.000Z", "message": {"id": "m" + a, "role": "assistant", "content": [{"type": "text", "text": f"answer {i}"}]}}) + "\n")
        parent = a
print("conversation:", os.path.getsize(form) // 1_000_000, "MB")
open(home + "/bin/claude", "w").write(f'''#!/bin/sh
echo "{{\\"type\\":\\"system\\",\\"subtype\\":\\"init\\",\\"session_id\\":\\"{cid}\\",\\"model\\":\\"fake-model\\"}}"
while IFS= read -r line; do
  echo '{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"A new answer."}}}}}}'
  echo '{{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.0,"usage":{{}}}}'
done
''')
os.chmod(home + "/bin/claude", 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=claude_dir, PATH=home + "/bin:" + os.environ["PATH"], TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
bin_ = sys.argv[1]; COLS, ROWS = 130, 46
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
def until(what, check, sec=120):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + text()
until("the conversation is listed", lambda: "question 0" in text(), 30)
t0 = time.time(); os.write(fd, b"r")
until("the window waits, saying so", lambda: "Bringing in the conversation" in text(), 10); print("the moon is on; it says what it is doing")
for c in "typed while waiting\r": os.write(fd, c.encode()); time.sleep(0.003)
until("the conversation is drawn", lambda: f"answer {int(os.environ.get('TURNS','2500'))-1}" in text() or "BROUGHT IN" in text())
pump(1.0); assert "typed while waiting" not in text(), "typing did nothing: " + text()
print(f"open in {time.time() - t0:.1f} s; what was typed meanwhile went nowhere")
for c in "and now\r": os.write(fd, c.encode()); time.sleep(0.003)
until("the window is free again", lambda: "A new answer" in text(), 20); print("the window is free again"); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.3)
