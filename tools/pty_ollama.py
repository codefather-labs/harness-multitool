#!/usr/bin/env python3
"""Debug, CON-190: the Ollama adapter in the real TUI, against the real Ollama on this machine (its usual
address) and a model with reasoning (MODEL, gemma4:e2b by default). A temporary home for each window.

1. Settings > Provider lists Ollama's models from /api/tags with what they can do.
2. A message: its reasoning is drawn, the answer comes, and the status line's TOKENS window is the
   one /api/ps says Ollama loaded.
3. Esc in the middle of a long answer stops it, and the next message is answered.
4. A small window (8192) compacts: a COMPACTED card, and the conversation goes on.
5. A conversation moves: begun on Claude Code (a fake `claude` that answers NOTED), resumed on
   Ollama, which answers from it; then back on Claude Code, whose fresh form holds Ollama's turn.
Needs pyte and a binary built with --features claude-code.
Usage: tools/pty_ollama.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, re, select, signal, struct, tempfile, termios, time, urllib.request
import pyte

MODEL = os.environ.get("MODEL", "gemma4:e2b")
# A big model on this machine answers in minutes: every wait is this many times longer.
SLOW = int(os.environ.get("SLOW", "1"))
OLLAMA = "http://127.0.0.1:11434"
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-ollama-proof")); os.makedirs(proof_dir, exist_ok=True)
COLS, ROWS = 120, 40
B = sys.argv[1]

def home_for(name, config):
    root = tempfile.mkdtemp(prefix=f"mt-oll-{name}-root"); home = tempfile.mkdtemp(prefix=f"mt-oll-{name}-home")
    os.makedirs(home + "/cfg/harness-multitool")
    open(home + "/cfg/harness-multitool/config.toml", "w").write(config)
    return root, home

def config(provider="ollama", model=MODEL, window=None, fake=None):
    c = f'[session]\nprovider = "{provider}"\nmodel = "{model}"\n\n[session.providers.ollama]\nkind = "ollama"\nbase_url = "{OLLAMA}"\n'
    if window: c += f"context_window = {window}\n"
    c += '\n[session.auth.ollama]\nnone = true\n'
    if fake: c += f'\n[session.providers.claude-code]\nkind = "claude-code"\ncontext_window = 200000\nbinary = "{fake}"\n'
    return c

class Window:
    def __init__(self, root, home, *args):
        self.home = home
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=home + "/claude", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
            os.execv(B, [B, *args])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.screen = pyte.Screen(COLS, ROWS); self.stream = pyte.ByteStream(self.screen)
    def pump(self, sec):
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.stream.feed(os.read(self.fd, 65536))
                except OSError: return
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display).rstrip()
    def status(self): return next((l for l in self.screen.display if "ADAPTER" in l), "")
    def wait(self, cond, sec):
        end = time.time() + sec
        while time.time() < end and not cond(): self.pump(0.2)
        return cond()
    def keys(self, *ks):
        for k in ks: os.write(self.fd, k.encode()); time.sleep(0.03)
        self.pump(0.3)
    def say(self, text):
        for c in text: os.write(self.fd, c.encode()); time.sleep(0.003)
        os.write(self.fd, b"\r"); self.pump(0.5)
    def idle(self): return "Esc stops" not in self.status()
    def show(self, t, save=None):
        print(f"\n===== {t} =====\n" + "\n".join(l.rstrip() for l in self.screen.display if l.strip()))
        if save: open(proof_dir + "/" + save, "w").write(self.text())
    def log(self):
        logs = glob.glob(self.home + "/data/harness-multitool/sessions/*/*.jsonl")
        return [json.loads(l) for p in logs for l in open(p) if l.strip()]
    def close(self):
        for _ in range(3): self.keys("\x1b"); self.pump(0.3)
        for c in "/exit\r": os.write(self.fd, c.encode()); time.sleep(0.01)
        self.pump(1)
        for _ in range(50):
            try:
                if os.waitpid(self.pid, os.WNOHANG)[0]: break
            except ChildProcessError: break
            self.pump(0.2)
        else:
            os.kill(self.pid, signal.SIGKILL)
        try: os.kill(json.load(open(self.home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
        except Exception: pass

def loaded_window():
    ps = json.load(urllib.request.urlopen(OLLAMA + "/api/ps", timeout=5))
    return {m["name"]: m.get("context_length") for m in ps["models"]}

results = {}
ONLY = [x for x in os.environ.get("ONLY", "main,compact,move").split(",") if x]

# 1-3: Settings, reasoning, the window, a stop.
def main():
    root, home = home_for("main", config())
    w = Window(root, home, "--new")
    assert w.wait(lambda: "Enter send" in w.text(), 30), w.text()
    w.keys(*"/settings", "\r"); w.pump(1)
    w.keys("\t"); w.pump(0.5)
    for _ in range(8):
        if "Ollama" in w.text(): break
        w.pump(0.5)
    results["settings_row"] = "( ● )  Ollama" in w.text()
    # Onto the Ollama row, where its models are listed.
    for _ in range(6):
        if re.search(r"▌.*Ollama|> *Ollama", w.text()): break
        w.keys("\x1b[B")
    w.wait(lambda: MODEL in w.text() and "thinking" in w.text(), 20)
    w.show("Settings > Provider, the Ollama row", "settings-ollama.txt")
    results["models_listed_with_capabilities"] = MODEL in w.text() and "tools, thinking" in w.text()
    w.keys("\x1b"); w.keys("\x1b"); w.pump(0.5)

    w.say("In two sentences: why is the sky blue?")
    results["answered"] = w.wait(lambda: w.idle() and "ASSISTANT" in w.text(), 180 * SLOW)
    w.pump(1)
    w.show("a message on Ollama", "answer.txt")
    log = w.log()
    results["reasoning_logged"] = any(e.get("event") == "thinking_delta" for e in log)
    results["reasoning_drawn"] = "Thought for" in w.text()
    m = re.search(r"TOKENS\s+[\d.,k]+\s*/\s*([\d.,]+k)", w.status())
    loaded = loaded_window().get(MODEL)
    print("status:", w.status().strip(), "| /api/ps:", loaded)
    results["window_matches_api_ps"] = bool(m) and loaded == 65536 and m.group(1) in ("65k", "66k", "65.5k", "65.536k")
    open(proof_dir + "/api-ps.txt", "w").write(json.dumps(loaded_window()) + "\n" + w.status() + "\n")

    w.say("Write a very long, detailed essay, at least 1500 words, about the history of lighthouses.")
    w.wait(lambda: not w.idle(), 30)
    w.wait(lambda: "ASSISTANT" in w.text() and len(w.text()) > 0, 60)
    time.sleep(4); w.pump(0.5)
    t0 = time.time(); os.write(w.fd, b"\x1b")
    results["esc_stops"] = w.wait(w.idle, 10)
    print("stopped in %.1fs" % (time.time() - t0))
    w.show("after Esc", "stopped.txt")
    w.say("Reply with the single word PINEAPPLE.")
    results["goes_on_after_stop"] = w.wait(lambda: w.idle() and "PINEAPPLE" in w.text().upper().split("REPLY WITH")[-1], 180 * SLOW)
    w.close()


# 4: a small window compacts.
def compact():
    root, home = home_for("compact", config(window=8192))
    w = Window(root, home, "--new")
    assert w.wait(lambda: "Enter send" in w.text(), 30), w.text()
    for i, topic in enumerate(["rivers", "mountains", "deserts", "forests"]):
        w.say(f"Write about 350 words about {topic}. No tools.")
        w.wait(lambda: w.idle() and w.text().count("ASSISTANT") >= 1, 240 * SLOW); w.pump(1)
        if any(e.get("event") in ("compacted", "context_summary") for e in w.log()): break
    log = w.log()
    results["compacted"] = any(e.get("event") in ("compacted", "context_summary") for e in log)
    w.say("In one sentence, which topics have we covered?")
    results["goes_on_after_compaction"] = w.wait(lambda: w.idle(), 240 * SLOW)
    w.show("after compaction", "compacted.txt")
    w.close()


# 5: a conversation moves Claude Code -> Ollama -> Claude Code.
def move():
    mv_home = tempfile.mkdtemp(prefix="mt-oll-move-fake")
    fake = mv_home + "/fake-claude"
    open(fake, "w").write(f"""#!/usr/bin/env python3
    import json, sys, os, re
    home = {mv_home!r}
    args = sys.argv[1:]
    if args[:1] == ["auth"]:
        print(json.dumps({{"loggedIn": True}})); sys.exit(0)
    prompt = sys.stdin.readline()
    open(home + "/calls", "a").write(" ".join(args) + "\\n")
    if "--resume" in args:
        rid = args[args.index("--resume") + 1]
        for p in __import__("glob").glob("/tmp/mt-oll-move*/**/" + rid + ".jsonl", recursive=True):
            open(home + "/resumed", "a").write("== " + rid + "\\n" + open(p).read())
    n = int(open(home + "/n").read()) + 1 if os.path.exists(home + "/n") else 1
    open(home + "/n", "w").write(str(n))
    def out(o): print(json.dumps(o), flush=True)
    out({{"type": "system", "subtype": "init", "session_id": f"sess-{{n}}", "model": "fake"}})
    out({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "NOTED-" + str(n)}}}}}})
    out({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.001, "usage": {{"input_tokens": 4, "output_tokens": 6}}}})
    """.replace("\n    ", "\n"))
    os.chmod(fake, 0o755)
    root, home = home_for("move", config(provider="claude-code", model="claude-haiku-5-5", fake=fake))
    w = Window(root, home, "--new")
    assert w.wait(lambda: "Enter send" in w.text(), 30), w.text()
    w.say("Remember this: the code word is ZEBRA-42.")
    results["began_on_claude_code"] = w.wait(lambda: "NOTED-1" in w.text(), 30)
    w.close()
    session = [os.path.basename(p)[:-6] for p in glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")][0]
    def switch(home, provider, model):
        """The window reads its profile, made from config.toml at the first start: the switch goes there."""
        prof = home + "/mt-home/profiles/default/profile.toml"
        text = open(prof).read()
        text = re.sub(r'(?m)^provider = ".*"$', f'provider = "{provider}"', text, count=1)
        text = re.sub(r'(?m)^model = ".*"$', f'model = "{model}"', text, count=1)
        if "[session.providers.ollama]" not in text:
            text += f'\n[session.providers.ollama]\nkind = "ollama"\nbase_url = "{OLLAMA}"\n\n[session.auth.ollama]\nnone = true\n'
        open(prof, "w").write(text)
    def move_to(w, row, model=None):
        """Settings > Provider in the open tab: the row chosen and saved moves this conversation (CON-134)."""
        w.keys(*"/settings", "\r"); w.pump(1); w.keys("\t"); w.pump(0.5)
        for _ in range(10):
            line = next((l for l in w.screen.display if row in l and "(" in l), "")
            if "▌" in line or "›" in line or ">" in line.split(row)[0][-6:]: break
            w.keys("\x1b[A")
        # Up to the top, then down to the row: the cursor is on the row's line.
        for _ in range(8): w.keys("\x1b[A")
        rows = [l for l in w.screen.display if re.search(r"\(\s*●?\s*\)\s+\S", l)]
        names = [re.sub(r".*\)\s+", "", l).split("  ")[0].strip(" │") for l in rows]
        for _ in range(names.index(row) if row in names else 0): w.keys("\x1b[B")
        w.keys(" "); w.pump(2)
        w.show(f"Settings: {row} chosen", f"move-{row}.txt")
        w.keys("\x13"); w.pump(3)
        w.keys("\x1b"); w.pump(1)
        w.wait(lambda: f"ADAPTER  {row}" in w.status() or f"ADAPTER  {row.lower()}" in w.status(), 60)
    w = Window(root, home, "--resume", session)
    w.wait(lambda: "Enter send" in w.text(), 30); w.pump(1)
    move_to(w, "Ollama")
    print("after the move:", w.status().strip())
    w.say("What is the code word I gave you? Answer with the code word only.")
    results["ollama_answers_from_it"] = w.wait(lambda: w.idle() and "ZEBRA-42" in w.text().split("What is the code word")[-1], 240 * SLOW)
    w.show("resumed on Ollama", "moved-to-ollama.txt")
    w.close()
    # And back on Claude Code.
    w = Window(root, home, "--resume", session)
    w.wait(lambda: "Enter send" in w.text(), 30); w.pump(1)
    move_to(w, "claude-code")
    print("after the move back:", w.status().strip())
    w.say("thanks")
    w.wait(lambda: "NOTED-2" in w.text(), 30)
    w.close()
    calls = open(mv_home + "/calls").read() if os.path.exists(mv_home + "/calls") else ""
    resumed = open(mv_home + "/resumed").read() if os.path.exists(mv_home + "/resumed") else ""
    last = (re.findall(r"--resume (\S+)", calls) or [""])[-1]
    part = resumed.split("== " + last)[-1] if last else ""
    results["back_on_claude_code_with_ollamas_turn"] = bool(last) and "ZEBRA-42" in part and "What is the code word" in part
    open(proof_dir + "/moved-back-form.txt", "w").write(part)
    print("resumed form:", last, len(part), "bytes")


for part in ONLY:
    globals()[part]()

print("\nRESULTS:", json.dumps(results))
print("ALL PASSED" if all(results.values()) else "FAILED")
