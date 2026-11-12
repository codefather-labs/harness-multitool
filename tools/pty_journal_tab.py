#!/usr/bin/env python3
"""Debug: Settings > Journal (ADR 0028, journal#991) in a real TUI in a pty, under a temporary HOME,
with a fake gh: sending is turned on and saved for the project; r checks gh off the window's thread;
a todo written to the project's journal opens an issue through the daemon; every gh call names the
project's gh folder. Needs pyte. Run it capped.
Usage: tools/pty_journal_tab.py <absolute path to multitool> [folder for the frames]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, shutil, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; keep = sys.argv[2] if len(sys.argv) > 2 else None
home = os.path.realpath(tempfile.mkdtemp(prefix="mt-jt-home")); root = os.path.realpath(tempfile.mkdtemp(prefix="mt-jt-root"))
os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/bin")
subprocess.run(["git", "-C", root, "init", "-q"], check=True)
subprocess.run(["git", "-C", root, "remote", "add", "origin", "git@github.com:acme/widgets.git"], check=True)
log = home + "/gh.log"
open(home + "/bin/gh", "w").write(f'''#!/bin/sh
input=""
case "$*" in *--body-file*) input=$(cat);; esac
printf '%s|%s|%s\\n' "$GH_CONFIG_DIR" "$*" "$(echo $input | head -c 60)" >> {log}
case "$1 $2" in
  "api user") echo tester;;
  "repo view") echo "PRIVATE WRITE";;
  "issue create") echo "https://github.com/acme/widgets/issues/7";;
esac
''')
os.chmod(home + "/bin/gh", 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="local"\nmodel="m"\n\n[session.providers.local]\nkind="openai"\nbase_url="http://127.0.0.1:9/v1"\n\n[session.auth.local]\nnone=true\n')
env = dict(HOME=home, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", MULTITOOL_HOME=home + "/mt", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", PATH=home + "/bin:/usr/local/bin:/usr/bin:/bin", LANG="C.UTF-8")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
screen = pyte.Screen(130, 46); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.03)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
def shot(name):
    if keep: os.makedirs(keep, exist_ok=True); open(os.path.join(keep, name), "w").write(text())
results = []
def ok(name, cond, detail=""):
    results.append((name, bool(cond))); print(("ok   " if cond else "FAIL ") + name + (f": {detail}" if detail else ""), flush=True)
def until(check, sec):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.1)
    return check()

pump(2.5)
typed("/settings\r"); pump(1.0)
# The sections pane: down to Journal, found by what its page says.
for _ in range(20):
    if "Send todos to issues" in text(): break
    os.write(fd, b"\x1b[B"); pump(0.15)
pump(0.3)
ok("the Journal page opens", "Send todos to issues" in text() and "acme/widgets" in text())
ok("it reads files only until asked: gh not called yet", not os.path.exists(log) or "api user" not in open(log).read())
shot("journal-tab-opened.txt")
os.write(fd, b"\t"); pump(0.3)
for _ in range(2): os.write(fd, b"\x1b[B"); pump(0.1)
os.write(fd, b" "); pump(0.3)
os.write(fd, b"\x13"); pump(0.5)
ok("saved for the project, on", until(lambda: "Saved for this project: on" in text(), 5), [l for l in text().splitlines() if "Saved" in l][:1])
ok("the check found the account and the place", until(lambda: "tester" in text() and "private write" in text(), 10))
ok("sending is on", until(lambda: "on: todos go to issues" in text(), 10))
shot("journal-tab-on.txt")
settings = json.load(open([os.path.join(d, "settings.json") for d, _, fs in os.walk(home + "/data") if "settings.json" in fs and "journal-trackers" in d][0]))
ok("settings.json says enabled", settings.get("enabled") is True, settings)
# A todo written to the project's journal, as an agent writes one, goes to an issue.
call = [json.dumps(m) for m in ({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "journal_add", "arguments": {"body": "Ship the widget", "category": "todo", "tags": ["t"], "flags": ["todo"]}}})]
subprocess.run([bin_, "mcp", "--root", root], input="\n".join(call) + "\n", text=True, capture_output=True, env=env, cwd=root, timeout=30)
sent = until(lambda: os.path.exists(log) and "issue create --repo acme/widgets" in open(log).read(), 20)
ok("the todo opened an issue through the daemon", sent)
lines = open(log).read().splitlines() if os.path.exists(log) else []
ok("every gh call named the project's gh folder", lines and all("/journal-trackers/" in l.split("|")[0] and l.split("|")[0].endswith("/gh") for l in lines), lines[:3])
if keep: shutil.copy(log, os.path.join(keep, "gh.log"))
print(json.dumps(results))
os.kill(pid, 15); pump(0.5)
try: os.waitpid(pid, 0)
except OSError: pass
# The test's daemon goes too.
for p in os.listdir("/proc"):
    if p.isdigit():
        try:
            if home.encode() in open(f"/proc/{p}/environ", "rb").read(): os.kill(int(p), 15)
        except OSError: pass
shutil.rmtree(home, ignore_errors=True); shutil.rmtree(root, ignore_errors=True)
sys.exit(0 if all(r[1] for r in results) else 1)
