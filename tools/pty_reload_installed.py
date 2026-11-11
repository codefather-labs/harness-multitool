#!/usr/bin/env python3
"""CON-129 (journal#843): /reload in a window that runs an installed copy, not the output of a local
build: nothing is built, and the window restarts in the same terminal on the file installed now. A
copy of the build is 'installed' outside target/; the window opens on it with a fake Claude Code and
says 'one'; a new copy is moved over the installed path as install.sh does; /reload. The window's
process (same pid) must then run the new file, and the tab, back with its conversation, says 'two'
resuming the same session. Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_reload_installed.py <absolute path to a multitool build>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, json, pty, select, shutil, struct, tempfile, termios, time

build = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-rli-home"); log = home + "/log"; fake = home + "/fake-claude"; users = home + "/dot-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-rli-root-")); os.makedirs(users + "/projects"); os.makedirs(home + "/inst")
installed = home + "/inst/multitool"
def install():
    shutil.copy2(build, installed + ".new"); os.chmod(installed + ".new", 0o755); os.replace(installed + ".new", installed)
    return os.stat(installed).st_ino
first = install()
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys
if sys.argv[1:2] == ["auth"]: print(json.dumps({{"loggedIn": False}})); sys.exit(1)
args = sys.argv[1:]
resumed = args[args.index("--resume") + 1] if "--resume" in args else None
sid = resumed or "sess-" + str(os.getpid())
print(json.dumps({{"type": "system", "subtype": "init", "session_id": sid, "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"text": text, "resumed": resumed, "session": sid}}) + "\\n")
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "session_id": sid, "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k not in ("MULTITOOL_CHATGPT_DIR", "MULTITOOL_RELOAD_BUILD")}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=users)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(installed, [installed, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0))
out = bytearray()
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: out.extend(os.read(fd, 65536))
            except OSError: return
def seen(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def type_(text):
    for ch in text: os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
def say(text):
    count = len(seen()); type_(text)
    end = time.time() + 20
    while time.time() < end and not [s for s in seen()[count:] if s["text"].endswith(text)]: pump(0.05)
    got = [s for s in seen() if s["text"].endswith(text)]
    assert got, f"{text!r} did not reach a delegate: {seen()}"
    return got[-1]
def exe_ino(): return os.stat(f"/proc/{pid}/exe").st_ino
pump(2.5)
assert exe_ino() == first
one = say("one")
second = install()                                   # a newer install over the running one, as install.sh does
assert second != first and exe_ino() == first
type_("/reload")
end = time.time() + 30
while time.time() < end and exe_ino() != second: pump(0.1)
assert exe_ino() == second, "the window did not restart on the installed file"
assert b"not the output of a local build" not in bytes(out)
print("restarted: pid", pid, "now runs inode", second, "(was", first, ")")
pump(3)
two = say("two")
assert two["resumed"] == one["session"], f"the conversation is back after the restart: {one} -> {two}"
print("the tab came back and resumed", two["resumed"])
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(1.0)
print("ok")
