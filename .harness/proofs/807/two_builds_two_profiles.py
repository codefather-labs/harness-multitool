import sys, os; sys.path.insert(0, "/home/codefather-server/Documents/projects/vladcraft-multitool/tools"); import pty_cleanup
import fcntl, pty, select, struct, subprocess, tempfile, termios, time, glob
bin_ = sys.argv[1]; out = sys.argv[2]; os.makedirs(out, exist_ok=True)
home = tempfile.mkdtemp(prefix="mt-two-"); mt = home + "/mt-home"; os.makedirs(mt + "/profiles")
import shutil
for p in ["claude-personal", "gpt-25"]:
    real = os.path.expanduser("~/.harness/profiles/" + p); os.makedirs(mt + "/profiles/" + p)
    shutil.copy(real + "/profile.toml", mt + "/profiles/" + p + "/profile.toml")
    for sub in ["claude", "chatgpt"]:
        if os.path.isdir(real + "/" + sub): os.symlink(real + "/" + sub, mt + "/profiles/" + p + "/" + sub)
os.makedirs(mt + "/profiles/default")
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-two-root-"))
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}; env.update(MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", XDG_CONFIG_HOME=home + "/cfg", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
fds = []
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select(fds, [], [], 0.02)
        for fd in r:
            try: os.read(fd, 65536)
            except OSError: pass
def window(profile, b=None):
    b = b or bin_
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(b, [b, "--profile", profile])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0)); fds.append(fd); return fd
def typ(fd, text):
    for ch in text: os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
def shot(name, pidwin=None):
    p = subprocess.Popen([bin_, "debug", "screenshot", "--out", f"{out}/{name}"] + (["--window", pidwin] if pidwin else []), env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    s = time.time()
    while p.poll() is None and time.time() - s < 20: pump(0.05)
    print(name, p.returncode, p.communicate()[1][-300:])
print("home", home, "root", root)
a = window("claude-personal", "/tmp/mt-two/old-multitool"); pump(4); shot("a0.png")
os.write(a, b"\r"); pump(2); os.write(a, b"n"); pump(3)            # start page: new conversation
shot("a1.png"); typ(a, "Say just the word ALPHA."); pump(25); shot("a2.png")
b = window("gpt-25"); pump(4)
os.write(b, b"\r"); pump(2); os.write(b, b"n"); pump(3)
shot("b1.png"); typ(b, "Say just the word BRAVO."); pump(40); shot("b2.png")
typ(a, "Say just the word CHARLIE."); pump(25); shot("c.png")
import json
print("daemon:", open(home + "/data/harness-multitool/daemon/address.json").read()[:0]); [print(f, [json.loads(l).get("event",{}) and None for l in []]) for f in []]
print("files:"); [print(f) for f in sorted(glob.glob(home + "/data/**", recursive=True)) if os.path.isfile(f)]
import re
for f in sorted(glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")):
    t = open(f).read(); print(f[-30:], re.findall(r'"provider":"([a-z-]+)"', t)[:1], re.findall(r'"text":"([^"]{0,40})"', t))
def ours():
    out = []
    for p in os.listdir("/proc"):
        if not p.isdigit(): continue
        try:
            cmd = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0"); envb = open(f"/proc/{p}/environ", "rb").read()
        except OSError: continue
        if len(cmd) > 1 and cmd[1] == b"daemon" and (b"XDG_DATA_HOME=" + (home + "/data").encode() + b"\0") in envb: out.append((p, cmd[0].decode()))
    return out
print("OURS daemons:", ours())
time.sleep(1)
for fd in fds: os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.5)
