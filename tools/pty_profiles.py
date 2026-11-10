#!/usr/bin/env python3
"""Profiles (CON-120..CON-126), with a fake Claude Code standing in for `claude`:
- `multitool profile add max` makes a clean profile whose claude/projects links to the user's store;
- `multitool --profile max login claude-code` runs `<claude> auth login` in max's own Claude Code
  directory, with nothing else of the harness's Claude Code environment; `profile list` then says so;
- windows on default and max open at once on one daemon; each delegate sees only its own profile's
  Claude Code directory and ChatGPT folder; default runs on the user's own Claude Code until its own
  is signed in, and on its own after;
- a transcript plain claude made in the user's store is seen from max's directory through the link,
  and the user's store gains nothing from the harness;
- an unknown profile refuses to start and names the profiles.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_profiles.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, json, pty, select, struct, subprocess, tempfile, termios, time

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-prof-home"); mt = home + "/mt-home"; users = home + "/dot-claude"
log = home + "/log"; calls = home + "/auth-calls"; fake = home + "/fake-claude"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-prof-root-"))
slug = "".join(c if c.isascii() and c.isalnum() else "-" for c in root)
os.makedirs(f"{users}/projects/{slug}"); open(f"{users}/projects/{slug}/plain.jsonl", "w").write("{}\n")
def tree(d): return sorted((os.path.relpath(os.path.join(p, f), d), os.path.getsize(os.path.join(p, f))) for p, _, fs in os.walk(d) for f in fs)
users_before = tree(users)
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys
d = os.environ.get("CLAUDE_CONFIG_DIR")
if sys.argv[1:2] == ["auth"]:
    open({calls!r}, "a").write(json.dumps({{"argv": sys.argv[1:], "dir": d, "claude_env": sorted(k for k in os.environ if k.startswith("CLAUDE") and k != "CLAUDE_CONFIG_DIR")}}) + "\\n")
    mark = os.path.join(d or "/nonexistent", "signed-in")
    if sys.argv[2] == "login": open(mark, "w").write("yes")
    if sys.argv[2] == "logout" and os.path.exists(mark): os.remove(mark)
    if sys.argv[2] == "status":
        yes = os.path.exists(mark); print(json.dumps({{"loggedIn": yes}})); sys.exit(0 if yes else 1)
    sys.exit(0)
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    sees = os.path.exists(os.path.join(d or {users!r}, "projects", {slug!r}, "plain.jsonl"))
    open({log!r}, "a").write(json.dumps({{"text": text, "claude": d, "chatgpt": os.environ.get("MULTITOOL_CHATGPT_DIR"), "sees_plain": sees}}) + "\\n")
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
settings = f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n'
os.makedirs(home + "/cfg/harness-multitool"); open(home + "/cfg/harness-multitool/config.toml", "w").write(settings)
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=users)
def cli(*args, extra=None):
    return subprocess.run([bin_, *args], env={**env, **(extra or {})}, capture_output=True, text=True, cwd=root)

bad = cli("--profile", "nope", "docs", "check")
assert bad.returncode != 0 and 'no profile "nope"' in bad.stderr, (bad.returncode, bad.stderr)
print("unknown profile refused:", bad.stderr.strip().splitlines()[-1])
made = cli("profile", "add", "max"); assert made.returncode == 0, made.stderr
# max's settings: the same fake delegate (a new profile is clean; this is what its settings page would save).
open(mt + "/profiles/max/profile.toml", "w").write(settings)
login = cli("--profile", "max", "login", "claude-code", extra={"CLAUDE_CODE_SOMETHING": "1"})
assert login.returncode == 0 and "signed in" in login.stdout, (login.stdout, login.stderr)
first = [c for c in map(json.loads, open(calls)) if c["argv"][1] == "login"][0]  # status checks at each start come first
assert first["argv"] == ["auth", "login"] and first["dir"] == mt + "/profiles/max/claude" and first["claude_env"] == [], first
assert os.readlink(mt + "/profiles/max/claude/projects") == users + "/projects"
print("login ran:", first["argv"], "in", first["dir"], "with no other CLAUDE variable")
listed = cli("profile", "list").stdout; print(listed.rstrip())
assert any("default" in l and "Claude Code: not signed in" in l for l in listed.splitlines()), listed
assert any("max" in l and "Claude Code: signed in" in l for l in listed.splitlines()), listed

def window(profile):
    args = [bin_] + (["--profile", profile] if profile else []) + ["preview", "welcome"]
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, args)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 100, 0, 0))
    return fd
fds = []
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select(fds, [], [], 0.02)
        for fd in r:
            try: os.read(fd, 65536)
            except OSError: pass
def seen(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def say(fd, text):
    count = len(seen())
    for ch in text: os.write(fd, ch.encode()); pump(0.02)
    os.write(fd, b"\r")
    end = time.time() + 20
    while time.time() < end and len(seen()) == count: pump(0.05)
    got = [s for s in seen() if s["text"].endswith(text)]  # the first message carries the harness's note before it
    assert got, f"{text!r} did not reach a delegate: {seen()}"
    return got[-1]
def close(fd):
    os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3); fds.remove(fd)

a = window(None); fds.append(a); pump(2.0)
b = window("max"); fds.append(b); pump(2.0)
for round_ in range(3):
    da = say(a, f"from-default-{round_}"); db = say(b, f"from-max-{round_}")
    assert da["claude"] == users, f"default runs on the user's own Claude Code until signed in: {da}"
    assert da["chatgpt"] == mt + "/profiles/default/chatgpt", da
    assert db["claude"] == mt + "/profiles/max/claude" and db["chatgpt"] == mt + "/profiles/max/chatgpt", db
    assert da["sees_plain"] and db["sees_plain"], (da, db)
    print(f"round {round_}: default on {da['claude']} | max on {db['claude']}; both see plain claude's transcript")
close(a)
signed = cli("login", "claude-code"); assert signed.returncode == 0, signed.stderr
c = window(None); fds.append(c); pump(2.0)
dc = say(c, "default-after-sign-in")
assert dc["claude"] == mt + "/profiles/default/claude" and dc["sees_plain"], dc
print("default after its sign-in runs on", dc["claude"], "and still sees plain claude's transcript")
out = cli("--profile", "max", "logout", "claude-code"); assert out.returncode == 0 and "not signed in" in out.stdout, out.stdout
assert tree(users) == users_before, ("the user's store changed", users_before, tree(users))
print("the user's Claude Code store unchanged:", users_before)
print("daemon files:", sorted(f for f in os.listdir(home + "/data/harness-multitool") if f.startswith("daemon")))
print("ok")
for fd in list(fds): close(fd)
