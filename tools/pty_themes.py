#!/usr/bin/env python3
"""Debug, CON-180: the light themes in the real TUI. A window in a pty with a temporary home; F2 is
pressed through the themes, and after each press the screen's cell backgrounds (pyte keeps them)
are read: each new theme's background colour must fill most of the screen.
Needs pyte. Usage: tools/pty_themes.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import collections, fcntl, json, pty, select, signal, struct, tempfile, termios, time, tomllib
import pyte

here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "themes")
order = ["dark", "light", "paper", "sand", "blossom", "mist", "nord", "monokai", "cyan"]
bg = {t: tomllib.load(open(f"{here}/{t}.toml", "rb"))["background"].lstrip("#").lower() for t in order}

root = tempfile.mkdtemp(prefix="mt-themes-root"); home = tempfile.mkdtemp(prefix="mt-themes-home"); os.makedirs(home + "/cfg/harness-multitool")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", COLORTERM="truecolor", MULTITOOL_GRAPHICS="off")
    os.execv(sys.argv[1], [sys.argv[1], "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
screen = pyte.Screen(120, 40); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def most_bg():
    counts = collections.Counter(screen.buffer[y][x].bg for y in range(screen.lines) for x in range(screen.columns))
    colour, n = counts.most_common(1)[0]
    return colour, n / (screen.lines * screen.columns)
end = time.time() + 20
while time.time() < end and "Enter send" not in text(): pump(0.2)
pump(1)
results, seen = {}, {}
first, _ = most_bg()
seen["start"] = first
# Find where the window starts in the cycle, then go round once.
start = next((t for t in order if bg[t] == first), "dark")
at = order.index(start)
for _ in range(len(order)):
    os.write(fd, b"\x1bOQ"); pump(0.8)
    at = (at + 1) % len(order)
    colour, share = most_bg()
    seen[order[at]] = (colour, round(share, 2))
    if order[at] in ("paper", "sand", "blossom", "mist", "light"):
        results[f"{order[at]}_background_fills_the_screen"] = colour == bg[order[at]] and share > 0.4
print("expected backgrounds:", {t: bg[t] for t in ("light", "paper", "sand", "blossom", "mist")})
print("seen (most common cell background, its share):", json.dumps(seen))
print("RESULTS:", json.dumps(results))
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.01)
pump(1)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if results and all(results.values()) else "FAILED")
