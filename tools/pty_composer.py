#!/usr/bin/env python3
"""Debug: the composer's cursor in a real TUI: arrows, Home, End, Delete and Backspace edit where the cursor is, and
what is sent is what shows. A fake delegate writes down what reaches it. Reads no screen, so it needs no pyte. Needs
--features claude-code.
Usage: tools/pty_composer.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, struct, tempfile, termios, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-cp-root")); home = tempfile.mkdtemp(prefix="mt-cp-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, sys
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-1", "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    open({log!r}, "a").write(json.loads(line)["message"]["content"][0]["text"] + "\\n")
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(sys.argv[1], [sys.argv[1], "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 60, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def press(*keys):
    for k in keys: os.write(fd, k.encode()); pump(0.04)
UP, DOWN, RIGHT, LEFT, HOME, END, DEL, BS = "\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D", "\x1b[H", "\x1b[F", "\x1b[3~", "\x7f"
def sent(): return open(log).read().split("\n")[:-1] if os.path.exists(log) else []
def expect(want):
    count = len(sent())
    press("\r")
    end = time.time() + 15
    while time.time() < end and len(sent()) == count: pump(0.05)
    assert sent() and sent()[-1] == want, f"wanted {want!r}, got {sent()[-1:]!r}"
    pump(0.4)
    print("sent:", want)
pump(1.5)
press(*"abd", LEFT, "c"); expect("abcd")
press(*"bcd", HOME, "a", END, "e"); expect("abcde")
press(*"axxb", LEFT, BS, LEFT, DEL); expect("ab")
press(*"ac", LEFT, LEFT, RIGHT, "b"); expect("abc")
press(*"привт", LEFT, "е"); expect("привет")
# Wider than the window: Up goes to the row above, and Down comes back.
long = "x" * 80
press(*long, UP, "U", DOWN, "D")
pump(0.2)
press("\r")
end = time.time() + 15
while time.time() < end and not (sent() and sent()[-1].startswith("x") and len(sent()[-1]) == 82): pump(0.05)
line = sent()[-1]
assert len(line) == 82 and "U" in line and line.index("U") < 80 and "D" in line and line.index("U") < line.index("D"), line
print("sent:", line)
print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
