#!/usr/bin/env python3
"""journal#886: a map wider than its card pans by every gesture over it. `multitool preview maps` in an
80-column pty, read with pyte, the mind map card under the pointer: a left drag pans it, a sideways
wheel (or touchpad swipe) right pans it, Shift with the wheel pans it, the wheel left brings it back,
and the plain wheel still scrolls the transcript.
Needs pyte. Usage: tools/pty_map_pan.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, pty, select, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; COLS, ROWS = 80, 60
def run(gesture):
    home = tempfile.mkdtemp(prefix="mt-pan-home"); root = os.path.realpath(tempfile.mkdtemp(prefix="mt-pan-root"))
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    s = pyte.Screen(COLS, ROWS); st = pyte.ByteStream(s)
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "maps"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    def pump(t):
        e = time.time() + t
        while time.time() < e:
            r, _, _ = select.select([fd], [], [], 0.02)
            if r:
                try: st.feed(os.read(fd, 65536))
                except OSError: return
    def screen(): return [l.rstrip() for l in s.display]
    def sgr(b, x, y, up=False): os.write(fd, f"\x1b[<{b};{x + 1};{y + 1}{'m' if up else 'M'}".encode()); pump(0.1)
    pump(3)
    before = screen()
    head = [i for i, l in enumerate(before) if "MINDMAP" in l][0]
    r, c = head + 12, 40
    if gesture == "drag":
        sgr(0, c, r)
        for k in range(1, 21): sgr(32, c - k, r)
        sgr(0, c - 20, r, up=True)
    elif gesture == "wheel right": [sgr(67, c, r) for _ in range(6)]
    elif gesture == "shift+wheel": [sgr(69, c, r) for _ in range(6)]
    elif gesture == "wheel right then left": [sgr(67, c, r) for _ in range(6)] + [sgr(66, c, r) for _ in range(6)]
    elif gesture == "plain wheel": [sgr(64, c, r) for _ in range(3)]
    pump(1)
    after = screen()
    card = [i for i in range(head, head + 25) if before[i] != after[i]]
    moved_up = before[head] != after[head] and "MINDMAP" not in after[head]
    os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
    subprocess.run([bin_, "daemon", "stop"], env=env, capture_output=True, timeout=30)
    return len(card), moved_up
for gesture in ["drag", "wheel right", "shift+wheel"]:
    rows, scrolled = run(gesture)
    print(f"{gesture}: {rows} rows of the card changed")
    assert rows >= 10 and not scrolled, (gesture, rows, scrolled)
rows, _ = run("wheel right then left"); print(f"wheel right then left: {rows} rows differ from the start"); assert rows == 0
rows, scrolled = run("plain wheel"); print(f"plain wheel: the transcript scrolled: {scrolled}"); assert scrolled
print("ok")
