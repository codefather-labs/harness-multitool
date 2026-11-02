#!/usr/bin/env python3
"""Debug: the owner's way onto the harness: no flags, no configuration files, no import by hand. The
start page lists Claude Code's conversation of this folder; the settings choose Claude Code and how it
runs; the conversation opens linked. With a fake `claude` on the PATH. Needs pyte and --features
claude-code.
Usage: tools/pty_flow.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, signal, struct, tempfile, termios, time, tomllib
import pyte

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-fl-root")); home = tempfile.mkdtemp(prefix="mt-fl-home"); os.makedirs(home + "/bin")
claude_dir = home + "/claude"; folder = claude_dir + "/projects/" + "".join(c if c.isascii() and c.isalnum() else "-" for c in root); os.makedirs(folder)
cid = "11111111-2222-4333-8444-555555555555"; form = f"{folder}/{cid}.jsonl"
def line(u, parent, kind, content):
    return json.dumps({"type": kind, "uuid": u, "parentUuid": parent, "isSidechain": False, "cwd": root, "sessionId": cid, "timestamp": "2026-11-01T10:00:00.000Z", "message": {"id": "m" + u, "role": kind, "content": content}}) + "\n"
open(form, "w").write(line("a1", None, "user", "the old question") + line("a2", "a1", "assistant", [{"type": "text", "text": "the old answer"}]))
open(home + "/bin/claude", "w").write(f'''#!/bin/sh
echo "$@" >> {home}/calls
echo "{{\\"type\\":\\"system\\",\\"subtype\\":\\"init\\",\\"session_id\\":\\"{cid}\\",\\"model\\":\\"fake-model\\"}}"
while IFS= read -r line; do
  echo '{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"The new answer."}}}}}}'
  echo '{{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.0,"usage":{{}}}}'
done
''')
os.chmod(home + "/bin/claude", 0o755)
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
def click(name):
    for y, l in enumerate(screen.display):
        x = l.find(name)
        if x >= 0:
            os.write(fd, f"\x1b[<0;{x + 2};{y + 1}M\x1b[<0;{x + 2};{y + 1}m".encode()); pump(0.5); return
    raise AssertionError(f"no {name} on screen: " + text())
def until(what, check, sec=10):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.1)
    assert check(), what + ": " + text()
pump(2)
until("the conversation is on the start page", lambda: "the old question" in text()); print("Claude Code's conversation is on the start page")
os.write(fd, b"s"); pump(1); until("the settings", lambda: "claude-code" in text())
click("claude-code"); pump(0.3); assert "Your environment" in text(), text()
click("No permission prompts"); pump(0.2)
os.write(fd, b"\x13"); pump(1)
c = tomllib.load(open(home + "/cfg/harness-multitool/config.toml", "rb"))["session"]; row = c["providers"]["claude-code"]
assert c["provider"] == "claude-code" and row["environment"] == "user" and row["permissions"] == "skip", c
print("chosen and saved by the harness")
until("back on the start page, showing it", lambda: "the old question" in text() and "claude-code" in text())
os.write(fd, b"r"); until("it opens", lambda: "the old answer" in text() or "BROUGHT IN" in text()); pump(1)
assert "ADAPTER  claude-code" in text(), text()
for c_ in "and now\r": os.write(fd, c_.encode()); time.sleep(0.005)
until("it answers", lambda: "The new answer" in text())
call = open(home + "/calls").read(); assert f"--resume {cid}" in call and "--fork-session" not in call and "--dangerously-skip-permissions" in call and "stream-json" in call, call
print("it goes on in that conversation, as chosen"); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.3)
