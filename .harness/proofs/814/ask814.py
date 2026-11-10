import sys, os; sys.path.insert(0, "/home/codefather-server/Documents/projects/vladcraft-multitool/tools"); import pty_cleanup
import fcntl, pty, select, struct, subprocess, tempfile, termios, time, shutil, re, glob
bin_ = sys.argv[1]; out = sys.argv[2]; os.makedirs(out, exist_ok=True)
home = tempfile.mkdtemp(prefix="mt-814-"); mt = home + "/mt-home"; p = mt + "/profiles/claude-personal"; os.makedirs(p); os.makedirs(mt + "/profiles/default")
real = os.path.expanduser("~/.harness/profiles/claude-personal")
toml = open(real + "/profile.toml").read(); toml = re.sub(r'(?m)^permissions = "skip"\n', "", toml); open(p + "/profile.toml", "w").write(toml)
assert "skip" not in toml
os.symlink(real + "/claude", p + "/claude")
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-814-root-")); target = "/tmp/mt814-" + str(os.getpid())
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}; env.update(MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", XDG_CONFIG_HOME=home + "/cfg", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
fds = []
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select(fds, [], [], 0.02)
        for fd in r:
            try: os.read(fd, 65536)
            except OSError: pass
def shot(name):
    pr = subprocess.Popen([bin_, "debug", "screenshot", "--out", f"{out}/{name}"], env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    s = time.time()
    while pr.poll() is None and time.time() - s < 20: pump(0.05)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "--profile", "claude-personal"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0)); fds.append(fd)
pump(4); os.write(fd, b"\r"); pump(2); os.write(fd, b"n"); pump(3)
for ch in f"Run exactly this one shell command with your Bash tool, nothing else: touch {target} . Then say DONE.": os.write(fd, ch.encode()); pump(0.01)
os.write(fd, b"\r")
end = time.time() + 90
while time.time() < end and not os.path.exists(target):
    pump(1)
    logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
    if logs and "program-ask" in "".join(open(l).read() for l in logs): break
pump(3); shot("asked.png"); print("file before answer:", os.path.exists(target))
os.write(fd, b"\r"); pump(1)
end = time.time() + 60
while time.time() < end and not os.path.exists(target): pump(1)
pump(15); shot("answered.png"); print("file after Allow once:", os.path.exists(target))
for l in glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl"):
    t = open(l).read(); print("texts:", re.findall(r'"text":"([^"]{1,80})"', t)[-6:])
if os.path.exists(target): os.remove(target)
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.5)
