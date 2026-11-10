#!/usr/bin/env python3
"""Debug: one conversation back and forth between Claude Code's own program and the harness (INV-20,
INV-21, ADR 0025). It starts in `claude -p`; the harness opens it on GPT, then on Claude Code; between
them `claude -p --resume` goes on with it. Each step names every word told on either side. The id and
the file stay one, and what was in the file stays byte for byte. Spends a little of the owner's plans;
the ChatGPT sign-in is linked, never read or copied. Needs pyte and --features claude-code.
Usage: tools/real_move.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, select, shutil, signal, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; MODEL = "claude-haiku-5-5"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-mv-root")); home = tempfile.mkdtemp(prefix="mt-mv-home"); os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/data/harness-multitool")
os.symlink(os.path.expanduser("~/.local/share/harness-multitool/auth"), home + "/data/harness-multitool/auth")
ROW = '\n[session.providers.claude-code]\nkind="claude-code"\n' + os.environ.get("ROW_EXTRA", "").replace("\\n", "\n")
CONFIG = {
    "gpt": '[session]\nprovider="chatgpt"\nmodel="gpt-6.1-sol"\nmax_tokens=4000\nplan_first="off"\n\n[session.providers.chatgpt]\nkind="openai-responses"\n\n[session.auth.chatgpt]\noauth="chatgpt"\n' + ROW,
    "claude": f'[session]\nprovider="claude-code"\nmodel="{MODEL}"\n' + ROW,
}
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
for name in list(env):
    if name == "CLAUDECODE" or name.startswith("CLAUDE_CODE_"): env.pop(name)
folder = os.path.expanduser("~/.claude/projects/") + "".join(c if c.isascii() and c.isalnum() else "-" for c in root)
WORDS = ["лазурит", "кварцит", "малахит", "гранат", "обсидиан", "базальт"]
results = []; COLS, ROWS = 130, 46
def ask_text(step):
    return (f"Какие кодовые слова я называл раньше в этой переписке? Перечисли все. " if step else "") + f"Новое кодовое слово: {WORDS[step]}. Запомни его. Ответь коротко, инструменты не используй."
def note(step, where, reply):
    known = WORDS[:step]; found = [w for w in known if w in reply]; ok = found == known
    results.append((where, known, found, ok)); print(f"--- step {step + 1}: {where}; asked for {known}, named {found}: {'ok' if ok else 'MISSED'}", flush=True)
    if not ok: print(reply[-900:], flush=True)
def claude(step, session=None):
    args = ["claude", "-p", "--model", MODEL, "--output-format", "json", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}', "--tools", ""] + (["--resume", session] if session else [])
    out = subprocess.run(args, cwd=root, env=env, input=ask_text(step), capture_output=True, text=True, timeout=300)
    reply = json.loads(out.stdout); note(step, "claude's own program", reply["result"]); return reply["session_id"]
def harness(step, name, first=False):
    open(home + "/cfg/harness-multitool/config.toml", "w").write(CONFIG[name])
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "startup"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
    def pump(sec):
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.02)
            if r:
                try: stream.feed(os.read(fd, 65536))
                except OSError: return
    text = lambda: "\n".join(l.rstrip() for l in screen.display).rstrip()
    pump(2.0); os.write(fd, b"r"); pump(2.5)
    if step:
        # A window killed a moment ago holds the session for a few seconds yet.
        end = time.time() + 12
        while time.time() < end and WORDS[step - 1] not in text(): pump(0.3)
        shown = text(); assert WORDS[step - 1] in shown, f"what was said outside is drawn on opening: {shown[-600:]}"
    for c in ask_text(step) + "\r": os.write(fd, c.encode()); time.sleep(0.002)
    t0 = time.time(); pump(2.0)
    while time.time() - t0 < 240:
        pump(0.3)
        if "Waiting for your answer" in text(): os.write(fd, b"\x1b[C\r"); pump(0.5)
        if "Esc stops" not in text() and time.time() - t0 > 4: break
    pump(1.0)
    shown = text(); tail = shown[shown.rfind("ASSISTANT"):] if "ASSISTANT" in shown else shown[-700:]
    note(step, f"harness on {name}", tail)
    if "CANNOT" in shown or "held" in shown: print(shown[-700:])
    os.kill(pid, signal.SIGTERM); pump(0.5)
    try: os.waitpid(pid, 0)
    except OSError: pass
    time.sleep(0.5)
try:
    cid = claude(0)
    path = f"{folder}/{cid}.jsonl"; print("conversation", cid)
    open(home + "/cfg/harness-multitool/config.toml", "w").write(CONFIG["gpt"])
    print(subprocess.run([bin_, "import", "claude-code", cid], cwd=root, env=env, capture_output=True, text=True).stdout.strip())
    def kept(before):
        now = open(path, "rb").read(); assert now[:len(before)] == before, "what was in the form stays byte for byte"; return now
    held = open(path, "rb").read()
    harness(1, "gpt"); held = kept(held); assert WORDS[1] in held.decode(), "the harness's turn on GPT is in Claude's file"
    assert claude(2, cid) == cid, "the same id"; held = kept(held)
    harness(3, "gpt"); held = kept(held); assert WORDS[3] in held.decode()
    harness(4, "claude"); held = kept(held); assert WORDS[4] in held.decode()
    assert claude(5, cid) == cid; held = kept(held)
    names = sorted(os.path.basename(f) for f in glob.glob(folder + "/*.jsonl")); assert names == [cid + ".jsonl"], names
    print("one id, one file:", names)
    logs = glob.glob(home + "/data/**/sessions/**/*.jsonl", recursive=True); assert len(logs) == 1, logs
    lines = [json.loads(l) for l in open(logs[0])]; assert [l["seq"] for l in lines] == list(range(len(lines))), "one numbering"
finally:
    print(json.dumps(results, ensure_ascii=False))
    if "mt-mv-root" in folder: shutil.rmtree(folder, ignore_errors=True)
ok = all(r[3] for r in results) and len(results) == 6
print("VERDICT", "ok" if ok else "FAILED"); sys.exit(0 if ok else 1)
