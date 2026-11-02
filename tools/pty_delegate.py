#!/usr/bin/env python3
"""Debug: Claude Code as a delegate in a real TUI in a pty, by a fake `claude` script, or the real one
with REAL_CLAUDE=1. Needs pyte and a binary built with --features claude-code; the other pty scripts
expect the default build, whose provider list has one row less.
Usage: tools/pty_delegate.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, stat, struct, tempfile, termios, time
import pyte

real = os.environ.get("REAL_CLAUDE") == "1"
root = tempfile.mkdtemp(prefix="mt-cc-root"); home = tempfile.mkdtemp(prefix="mt-cc-home"); os.makedirs(home + "/cfg/harness-multitool")
open(root + "/note.txt", "w").write("the secret word is mango\n")
calls = home + "/calls"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/bin/sh
IFS= read -r prompt
printf '%s' "$*" | tr '\\n' ' ' >> {calls}; echo >> {calls}
echo '{{"type":"system","subtype":"init","session_id":"sess-1","model":"fake-model"}}'
case "$prompt" in
  *slow*) echo '{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"Working"}}}}}}'; exec sleep 30;;
esac
echo '{{"type":"stream_event","event":{{"type":"content_block_start","content_block":{{"type":"thinking"}}}}}}'
sleep 0.5
echo '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Read","input":{{"file_path":"note.txt"}}}}]}}}}'
echo '{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"the secret word is mango"}}]}}}}'
sleep 0.3
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"The word is **mango**."}}}}}}'
echo '{{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.0125,"usage":{{"input_tokens":4,"output_tokens":6,"cache_read_input_tokens":90}}}}'
''')
os.chmod(fake, os.stat(fake).st_mode | stat.S_IEXEC)
binary = "" if real else f'binary = "{fake}"\n'
model = "claude-haiku-5-5"
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "claude-code"\nmodel = "{model}"\n\n[session.providers.claude-code]\nkind = "claude-code"\ncontext_window = 200000\n{binary}')
COLS, ROWS = 120, 40; bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def show(t): print(f"\n===== {t} =====\n{text()}")
def typ(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
def until(what, cond, sec=60):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(0.1)
    assert cond(), what + ": " + text()
status = lambda: next((l for l in screen.display if "ADAPTER" in l), "")
pump(1.5); assert "claude-code" in status(), status()
typ("Read note.txt with the Read tool, then say the secret word, in bold.\r")
until("the answer", lambda: "mango" in text() and "Esc stops" not in status()); pump(0.3); show("first message")
assert "**" not in text() and "Read" in text(), text()
print("status:", status().strip()); assert "by Claude Code" in status() and "TOKENS" in status(), status()
typ("Now say the word again, one word.\r")
until("the second answer", lambda: "USER" in text() and text().count("mango") >= 2 and "Esc stops" not in status()); pump(0.3); show("second message")
if not real:
    lines = open(calls).read().splitlines()
    print("calls:", [l[:40] for l in lines])
    assert "--resume" not in lines[0] and "--resume sess-1" in lines[1], lines
    assert "--setting-sources" in lines[0] and "--strict-mcp-config" in lines[0], lines[0]
    # Esc ends it at once, and the tab goes on.
    typ("be slow\r")
    until("it runs", lambda: "Esc stops" in status() and "Working" in text())
    os.write(fd, b"\x1b"); t0 = time.time()
    until("it stopped", lambda: "Esc stops" not in status(), 5); print("stopped in %.1fs" % (time.time() - t0))
    typ("once more\r")
    until("it goes on", lambda: "once more" in text() and text().count("mango") >= 3 and "Esc stops" not in status()); show("after a stop")
    # The settings show the row, and it says who signs in.
    typ("/settings\r"); pump(0.5); os.write(fd, b"\t"); pump(0.2)
    assert "claude-code" in text() and "Signed in by Claude Code" in text(), text()
    os.write(fd, b"\x1b"); pump(0.2)
typ("/exit\r"); pump(0.5); print("exit:", os.waitpid(pid, os.WNOHANG)); print("ok")
