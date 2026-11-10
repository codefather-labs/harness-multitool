#!/usr/bin/env python3
"""Profiles (CON-121, CON-122): two windows open at once, one on `default` and one on `work`, served by
one daemon. A fake Claude Code writes down, for each message, the Claude Code directory and the ChatGPT
folder it was started with. Each window's delegate must see only its own profile's. Also: an unknown
profile refuses to start and names the profiles. Reads no screen, so it needs no pyte. Needs
--features claude-code.
Usage: tools/pty_profiles.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, struct, subprocess, tempfile, termios, time

bin_ = sys.argv[1]
home = tempfile.mkdtemp(prefix="mt-prof-home"); mt = home + "/mt-home"
log = home + "/log"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    text = json.loads(line)["message"]["content"][0]["text"]
    seen = {{"text": text, "claude": os.environ.get("CLAUDE_CONFIG_DIR"), "chatgpt": os.environ.get("MULTITOOL_CHATGPT_DIR")}}
    open({log!r}, "a").write(json.dumps(seen) + "\\n")
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
settings = f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n'
# `default` comes from the old config on the first start; `work` is written as a profile of its own.
os.makedirs(home + "/cfg/harness-multitool"); open(home + "/cfg/harness-multitool/config.toml", "w").write(settings)
os.makedirs(mt + "/profiles/work"); open(mt + "/profiles/work/profile.toml", "w").write(settings)
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k != "MULTITOOL_CHATGPT_DIR"}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")

bad = subprocess.run([bin_, "--profile", "nope", "docs", "check"], env=env, capture_output=True, text=True, cwd=home)
assert bad.returncode != 0 and 'no profile "nope"' in bad.stderr and "default, work" in bad.stderr, (bad.returncode, bad.stderr)
print("unknown profile refused:", bad.stderr.strip().splitlines()[-1])

def window(profile):
    root = os.path.realpath(tempfile.mkdtemp(prefix=f"mt-prof-{profile}-"))
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

a = window(None); fds.append(a); pump(2.0)
b = window("work"); fds.append(b); pump(2.0)
for round_ in range(3):
    da = say(a, f"from-default-{round_}"); db = say(b, f"from-work-{round_}")
    assert da["claude"] is None, f"default runs on the user's own Claude login: {da}"
    assert da["chatgpt"] == home + "/data/harness-multitool/auth/chatgpt", da  # default keeps it where it was (journal#754)
    assert db["claude"] == mt + "/profiles/work/claude", db
    assert db["chatgpt"] == mt + "/profiles/work/chatgpt", db
    print(f"round {round_}: default saw", da["claude"], da["chatgpt"], "| work saw", db["claude"], db["chatgpt"])
addresses = [f for f in os.listdir(home + "/data/harness-multitool") if f.startswith("daemon")]
print("daemon files:", sorted(addresses))
print("ok")
for fd in fds:
    os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
