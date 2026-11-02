#!/usr/bin/env python3
"""Debug: multitool debug screenshot, against a real TUI in a pty. Reads the PNG's header, not the screen,
so it needs no pyte.
Usage: tools/pty_screenshot.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, hashlib, pty, select, struct, subprocess, tempfile, termios, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-ss-root")); home = tempfile.mkdtemp(prefix="mt-ss-home"); os.makedirs(home + "/cfg/harness-multitool")
bin_ = sys.argv[1]; COLS, ROWS = 120, 40
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
call = lambda out: subprocess.run([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, capture_output=True, text=True, timeout=30)
none = call(home + "/none.png")
assert none.returncode != 0 and not os.path.exists(home + "/none.png"), none
print("with no window it says so:", none.stderr.strip()[:80])
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
done = {"stop": False}
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def shot(name):
    out = home + "/" + name
    # The window draws only while it is pumped.
    p = subprocess.Popen([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    start = time.time()
    while p.poll() is None and time.time() - start < 20: pump(0.05)
    stdout, stderr = p.communicate(timeout=1)
    assert p.returncode == 0, stderr
    assert stdout.strip() == out, stdout
    return out, time.time() - start
pump(2)
first, took = shot("a.png")
data = open(first, "rb").read()
assert data[:8] == b"\x89PNG\r\n\x1a\n", data[:8]
width, height = struct.unpack(">II", data[16:24])
assert width % COLS == 0 and height % ROWS == 0, (width, height)
print(f"a picture of {COLS}x{ROWS} cells, {width}x{height} px, in {took:.2f} s")
for c in "some typed text": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5)
second, _ = shot("b.png")
digest = lambda p: hashlib.sha256(open(p, "rb").read()).hexdigest()
assert digest(first) != digest(second), "the picture did not change with the screen"
print("it follows the screen")
again, _ = shot("a.png")
assert digest(again) == digest(second), "a second call overwrites, and shows the same state"
print("a second call overwrites"); print("ok")
for c in "\x15/exit\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5)
