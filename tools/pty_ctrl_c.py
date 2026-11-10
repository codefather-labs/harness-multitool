#!/usr/bin/env python3
"""Debug: Ctrl+C in a real TUI with a fake delegate (CON-86): during a run it stops the run and the queued
message goes on, with the window alive; idle, it takes a second press within 2 s to quit, and the
terminal comes back. Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_ctrl_c.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, struct, tempfile, termios, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-cc-root")); home = tempfile.mkdtemp(prefix="mt-cc-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, signal, sys, time
def asked(*_):
    time.sleep(1)  # as long as a real one takes to end
    sys.exit(0)
signal.signal(signal.SIGTERM, asked)
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-9", "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(prompt + "\\n")
    print(json.dumps({{"type": "user", "isReplay": True, "message": {{"role": "user", "content": [{{"type": "text", "text": prompt}}]}}}}), flush=True)
    if prompt.split("\\n")[-1].startswith("slow"): time.sleep(60)  # by what it was just asked
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
bin_ = sys.argv[1]
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def prompts(): return open(log).read().split("\n") if os.path.exists(log) else []
gone = {"status": None}
def alive():
    if gone["status"] is not None: return False
    done, status = os.waitpid(pid, os.WNOHANG)
    if done == pid: gone["status"] = status; return False
    return True
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + json.dumps(prompts())
pump(1.5)
typed("slow one\r")
until("the run is on", lambda: "slow one" in prompts())
pump(0.3)
typed("queued two\r"); pump(0.5)
assert "queued two" not in prompts(), "it waits for the slow run"
os.write(fd, b"\x03")
until("the queued message goes to the agent", lambda: "queued two" in prompts())
assert alive(), "Ctrl+C during a run closed the window"
print("Ctrl+C stopped the run, the window stayed, the queued message went on")
pump(1.5)
typed("slow three\r")
until("the next run is on", lambda: "slow three" in prompts())
pump(0.3)
typed("queued four\r"); pump(0.5)
os.write(fd, b"\x03"); pump(0.2); os.write(fd, b"\x03")
pump(3)
assert alive(), "two presses during a run closed the window"
assert "queued four" not in prompts(), "the second press did not cancel the answer: " + json.dumps(prompts())
print("two quick presses cancelled the answer too")
typed("five\r")
until("the next message goes", lambda: "five" in prompts())
pump(1)
typed("slow six\r")
until("a slow one", lambda: "slow six" in prompts())
pump(0.3)
typed("slow seven\r"); pump(0.5)
os.write(fd, b"\x03")
until("its queue goes on", lambda: "slow seven" in prompts())
pump(0.5)
os.write(fd, b"\x03")
pump(2.5)
assert alive(), "a press during the queued answer closed the window"
typed("eight\r")
until("the answer was cancelled and the next message goes", lambda: "eight" in prompts(), 10)
print("a press during the queued answer stopped it")
pump(1.5)
os.write(fd, b"\x03"); pump(0.5)
assert alive(), "one idle Ctrl+C closed the window"
pump(2.5)
os.write(fd, b"\x03"); pump(0.5)
assert alive(), "a press after 2 s counts as a first"
print("an idle Ctrl+C only asks, and the asking runs out")
os.write(fd, b"\x03")
until("a second press quits", lambda: not alive(), 5)
attrs = termios.tcgetattr(fd)
assert attrs[3] & termios.ECHO and attrs[1] & termios.ONLCR, "the terminal did not come back"
print("a second press within 2 s quit, and the terminal came back"); print("ok")
