#!/usr/bin/env python3
"""journal#913: Codex CLI as a delegate in a real window. A fake `codex` (or the real one with
MT_REAL_CODEX=1) runs each message; the fake writes down how it was run.
Fake: the first message runs `codex exec --json ... -` with the prompt on stdin, never on the command
line, the harness's MCP servers given with -c; the second resumes the thread the first started
(`resume T1 -`); on profile max it runs with max's own CODEX_HOME, whose sessions/ links the user's;
on default with no own sign-in, the user's own Codex folder; the answer reaches the transcript.
Real: two messages to the real Codex on the owner's plan, the second recalls the first's word.
Needs pyte. Usage: tools/pty_codex.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, json, pty, re, select, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; real = os.environ.get("MT_REAL_CODEX") == "1"
home = tempfile.mkdtemp(prefix="mt-codex-home"); mt = home + "/mt-home"; log = home + "/codex-log"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-codex-root-"))
users_codex = home + "/dot-codex"; os.makedirs(users_codex + "/sessions")
fake = home + "/codex"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if args[:2] == ["login", "status"]:
    signed = os.path.exists(os.path.join(os.environ.get("CODEX_HOME", ""), "signed-in"))
    print("Logged in" if signed else "Not logged in"); sys.exit(0 if signed else 1)
prompt = sys.stdin.read()
resumed = args[args.index("resume") + 1] if "resume" in args else None
open({log!r}, "a").write(json.dumps({{"args": args, "prompt": prompt, "codex_home": os.environ.get("CODEX_HOME"), "resumed": resumed}}) + "\\n")
tid = resumed or "T1"
for e in [{{"type": "thread.started", "thread_id": tid}}, {{"type": "turn.started"}},
          {{"type": "item.completed", "item": {{"id": "i1", "type": "agent_message", "text": "FAKE-ANSWER to " + prompt.strip().splitlines()[-1]}}}},
          {{"type": "turn.completed", "usage": {{"input_tokens": 10, "output_tokens": 3}}}}]:
    print(json.dumps(e), flush=True)
''')
os.chmod(fake, 0o755)
binary = "codex" if real else fake
settings = f'[session]\nprovider="codex"\nmodel="{"gpt-6.1-sol" if real else "m"}"\nplan_first="off"\n\n[session.providers.codex]\nkind="codex"\nbinary="{binary}"\nsandbox="read-only"\n'
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE") and k not in ("MULTITOOL_CHATGPT_DIR", "CODEX_HOME")}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
if not real:
    env["CODEX_HOME"] = users_codex
os.makedirs(home + "/cfg/harness-multitool"); open(home + "/cfg/harness-multitool/config.toml", "w").write(settings)
def cli(*args): return subprocess.run([bin_, *args], env=env, capture_output=True, text=True, cwd=root, timeout=60)
assert cli("profile", "add", "max").returncode == 0
open(mt + "/profiles/max/profile.toml", "w").write(settings)
open(mt + "/profiles/default/profile.toml", "w").write(settings)
COLS, ROWS = 120, 40
def window(*args):
    s = pyte.Screen(COLS, ROWS); st = pyte.ByteStream(s)
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, *args, "preview", "welcome"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return s, st, fd
def pump(w, t):
    s, st, fd = w; e = time.time() + t
    while time.time() < e:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: st.feed(os.read(fd, 65536))
            except OSError: return
def flat(w): return re.sub(r"\s+", " ", "\n".join(w[0].display))
def seen(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def say(w, text, expect, sec=20 if not real else 180):
    for ch in text: os.write(w[2], ch.encode()); pump(w, 0.01)
    os.write(w[2], b"\r")
    end = time.time() + sec
    while time.time() < end and expect not in flat(w): pump(w, 0.1)
    assert expect in flat(w), f"{expect!r} not on screen:\n" + "\n".join(l.rstrip() for l in w[0].display)
def close(w): os.write(w[2], b"\x03"); pump(w, 0.3); os.write(w[2], b"\x03"); pump(w, 0.5)

if real:
    w = window(); pump(w, 3)
    say(w, "Remember the word PELICAN-7 and reply only OK.", "OK")
    say(w, "Which word did I ask you to remember? Reply with the word only.", "PELICAN-7")
    print("real Codex: the second message recalled the first's word")
    close(w)
else:
    w = window(); pump(w, 3)
    say(w, "first message", "FAKE-ANSWER to first message")
    say(w, "second message", "FAKE-ANSWER to second message")
    a, b = seen()[-2], seen()[-1]
    print("first run args:", " ".join(a["args"][:6]), "...", a["args"][-1])
    assert a["args"][:3] == ["exec", "--json", "--skip-git-repo-check"] and a["args"][-1] == "-", a["args"]
    assert not any("first message" in x for x in a["args"]), "the prompt is on stdin, never on the command line"
    assert a["prompt"].rstrip().endswith("first message")
    assert any(x.startswith("mcp_servers.harness.command=") for x in a["args"]), "the harness's tools go with it"
    assert a["resumed"] is None and b["resumed"] == "T1", (a["resumed"], b["resumed"])
    assert a["codex_home"] == users_codex, f"default not signed in on its own: the user's Codex folder: {a['codex_home']}"
    print("second run resumed", b["resumed"], "on the user's own Codex folder", b["codex_home"])
    close(w)
    w = window("--profile", "max"); pump(w, 3)
    say(w, "on max", "FAKE-ANSWER to on max")
    c = seen()[-1]
    assert c["codex_home"] == mt + "/profiles/max/codex", c["codex_home"]
    assert os.readlink(mt + "/profiles/max/codex/sessions") == users_codex + "/sessions"
    print("profile max ran with its own CODEX_HOME", c["codex_home"], "sessions shared:", os.readlink(c["codex_home"] + "/sessions"))
    close(w)
subprocess.run([bin_, "daemon", "stop"], env=env, capture_output=True, timeout=30)
print("ok")
