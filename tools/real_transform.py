#!/usr/bin/env python3
"""Debug: one session across real providers, in a real TUI in a pty (ADR 0023). Each step resumes the
session on another provider, asks for the code words so far and gives a new one. Spends a little of
the owner's plans. The owner's ChatGPT sign-in is linked, never read or copied. Needs pyte, a local
Ollama and a build with --features claude-code.
Usage: tools/real_transform.py <absolute path to multitool> [gpt claude gemma gpt]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, re, select, shutil, signal, struct, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; chain = sys.argv[2:] or ["gpt", "claude", "gemma", "gpt"]
root = tempfile.mkdtemp(prefix="mt-tr-root"); home = tempfile.mkdtemp(prefix="mt-tr-home"); os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/data/harness-multitool")
os.symlink(os.path.expanduser("~/.local/share/harness-multitool/auth"), home + "/data/harness-multitool/auth")
CONFIG = {
    "gpt": '[session]\nprovider="chatgpt"\nmodel="gpt-6.1-sol"\nmax_tokens=4000\n\n[session.providers.chatgpt]\nkind="openai-responses"\n\n[session.auth.chatgpt]\noauth="chatgpt"\n',
    "claude": '[session]\nprovider="claude-code"\nmodel="claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind="claude-code"\n',
    "gemma": '[session]\nprovider="local"\nmodel="gemma4:e2b"\nmax_tokens=2000\n\n[session.providers.local]\nkind="openai"\nbase_url="http://127.0.0.1:11434/v1"\ncontext_window=32000\n\n[session.auth.local]\nnone=true\n',
}
WORDS = ["лазурит", "кварцит", "малахит", "гранат", "обсидиан"]
COLS, ROWS = 130, 46
results = []
for step, name in enumerate(chain):
    open(home + "/cfg/harness-multitool/config.toml", "w").write(CONFIG[name])
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
        os.execv(bin_, [bin_, "preview", "startup" if step else "welcome"])
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
    pump(2.0)
    if step: os.write(fd, b"r"); pump(1.5)
    known = WORDS[:step]
    ask = (f"Какие кодовые слова я называл раньше в этой переписке? Перечисли их. " if step else "") + f"Новое кодовое слово: {WORDS[step]}. Ничего не запускай, просто ответь коротко, инструментов не вызывай."
    for c in ask + "\r": os.write(fd, c.encode()); time.sleep(0.002)
    t0 = time.time(); pump(2.0)
    while time.time() - t0 < 240:
        pump(0.3)
        if "Waiting for your answer" in text(): os.write(fd, b"\x1b[C\r"); pump(0.5)
        if "Esc stops" not in text() and time.time() - t0 > 4: break
    pump(1.0)
    screen_text = text(); tail = screen_text[screen_text.rfind("ASSISTANT"):] if "ASSISTANT" in screen_text else screen_text[-700:]
    found = [w for w in known if w in tail]
    ok = found == known
    results.append((name, round(time.time() - t0, 1), known, found, ok))
    print(f"--- step {step + 1}: {name}, {time.time() - t0:.1f}s; asked for {known}, named {found}: {'ok' if ok else 'MISSED'}", flush=True)
    print("\n".join(l[:120] for l in tail.splitlines()[:9]), flush=True)
    if "CANNOT" in screen_text or "error" in tail.lower()[:200]: print(screen_text[-900:])
    os.kill(pid, signal.SIGTERM); pump(0.5)
    try: os.waitpid(pid, 0)
    except OSError: pass
    time.sleep(0.5)
print(json.dumps(results, ensure_ascii=False))
# What the test wrote into Claude Code's own folder, for its temp directory only.
folder = os.path.expanduser("~/.claude/projects/") + "".join(c if c.isascii() and c.isalnum() else "-" for c in root)
if os.path.isdir(folder) and "mt-tr-root" in os.path.basename(folder).replace("-", "-") or "mt-tr-root" in folder: shutil.rmtree(folder, ignore_errors=True)
sys.exit(0 if all(r[4] for r in results) else 1)
