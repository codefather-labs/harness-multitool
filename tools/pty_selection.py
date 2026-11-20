#!/usr/bin/env python3
"""Debug: selecting and copying text in a real window (CON-188). A drag with the left button over the transcript and
over the composer copies what was dragged over: a fake wl-copy, first on PATH with WAYLAND_DISPLAY set, writes down
what reaches it, and the window's toast says 'Copied'. A plain click copies nothing. The mouse is sent as SGR events.
Needs pyte. Usage: tools/pty_selection.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, pty, select, struct, tempfile, termios, time
import pyte

COLS, ROWS = 120, 40
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-sel-root")); home = tempfile.mkdtemp(prefix="mt-sel-home")
bindir = home + "/bin"; os.makedirs(bindir); os.makedirs(home + "/cfg/harness-multitool")
copied = home + "/copied"
# The fake clipboard program: each copy is one record, separated by a line of '---'.
open(bindir + "/wl-copy", "w").write(f"#!/bin/sh\ncat >> {copied}\nprintf '\\n---\\n' >> {copied}\n")
os.chmod(bindir + "/wl-copy", 0o755)
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color",
           MULTITOOL_GRAPHICS="off", WAYLAND_DISPLAY="wayland-test", PATH=bindir + ":" + os.environ.get("PATH", ""))
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(sys.argv[1], [sys.argv[1], "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)

def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return

def text(): return "\n".join(screen.display)

def find(needle):
    for y, line in enumerate(screen.display):
        x = line.find(needle)
        if x >= 0: return x, y
    raise AssertionError(f"{needle!r} is not on the screen:\n{text()}")

# SGR mouse: 1-based cells; M is a press or a drag (32 + button), m a release.
def press(x, y): os.write(fd, f"\x1b[<0;{x + 1};{y + 1}M".encode()); pump(0.08)
def move(x, y): os.write(fd, f"\x1b[<32;{x + 1};{y + 1}M".encode()); pump(0.08)
def release(x, y): os.write(fd, f"\x1b[<0;{x + 1};{y + 1}m".encode()); pump(0.3)
def records(): return [r.strip("\n") for r in open(copied).read().split("\n---\n") if r.strip()] if os.path.exists(copied) else []

def drag_over(needle):
    x, y = find(needle)
    press(x, y); move(x + len(needle) // 2, y); move(x + len(needle) - 1, y); release(x + len(needle) - 1, y)
    end = time.time() + 5
    while time.time() < end and not any(needle in r for r in records()): pump(0.05)

results = []
def check(name, ok, why=""):
    results.append((name, ok)); print(("PASS " if ok else "FAIL ") + name + ("" if ok else ": " + why))

try:
    # The window's first frame, however long the start takes.
    end = time.time() + 15
    while time.time() < end and "Welcome to harness-multitool" not in text(): pump(0.1)
    pump(0.3)
    # 1. The transcript: the welcome card's title.
    drag_over("Welcome to harness-multitool")
    check("transcript_copied", any(r == "Welcome to harness-multitool" for r in records()), repr(records()))
    pump(0.3)
    check("toast_said_copied", "Copied 28 characters" in text(), text()[-800:])
    check("osc52_not_shown", "]52;" not in text())
    # 2. The composer: typed text, dragged over.
    os.write(fd, b"\x1b"); pump(0.2)  # Esc clears the selection
    for c in "copy me please": os.write(fd, c.encode()); pump(0.02)
    pump(0.3)
    drag_over("copy me please")
    check("composer_copied", any(r == "copy me please" for r in records()), repr(records()))
    # 3. A plain click copies nothing more.
    count = len(records())
    x, y = find("Welcome to harness-multitool")
    press(x + 2, y); release(x + 2, y); pump(0.5)
    check("click_copies_nothing", len(records()) == count, repr(records()))
    # 4. A drag over two rows copies both, joined by a line break.
    x1, y1 = find("Welcome to harness-multitool"); x2, y2 = find("plugin-first coding harness")
    press(x1, y1); move(x2 + 5, y2); release(x2 + len("plugin-first coding harness") - 1, y2)
    end = time.time() + 5
    while time.time() < end and len(records()) == count: pump(0.05)
    two = records()[-1] if len(records()) > count else ""
    check("two_rows_copied", two == "Welcome to harness-multitool\nplugin-first coding harness", repr(two))
finally:
    try: os.kill(pid, 9)
    except OSError: pass

print("RESULTS", "ALL PASSED" if all(ok for _, ok in results) else "FAILED", f"({sum(ok for _, ok in results)}/{len(results)})")
sys.exit(0 if all(ok for _, ok in results) else 1)
