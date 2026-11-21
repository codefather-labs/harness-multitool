#!/usr/bin/env python3
"""Debug, journal#1543 point 3: a conversation moves between Ollama (MODEL) and the ChatGPT plan (the owner's
profile PROFILE, default by default, signed in to ChatGPT), both ways, in the real TUI, carrying words and
tool calls. The window runs on the real profile with its own data folder, so its own daemon; the profile's
profile.toml is put back as it was at the end. On Ollama: a code word, and a file read with a tool. Moved in
Settings to the plan: it answers both from the history and reads another file. Moved back: Ollama answers all
three.
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
            os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
            # The user's own Claude Code sign-in (~/.claude): the row runs in the user's environment.
            # CLAUDE_DIR names a signed-in Claude Code directory; else the user's own ~/.claude.
            if os.environ.get("CLAUDE_DIR"): os.environ["CLAUDE_CONFIG_DIR"] = os.environ["CLAUDE_DIR"]
            else: os.environ.pop("CLAUDE_CONFIG_DIR", None)
            os.execv(B, [B, "--profile", PROFILE, *args])
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


PROFILE = os.environ.get("PROFILE", "default")
PROFILE_TOML = os.path.expanduser(f"~/.harness/profiles/{PROFILE}/profile.toml")
saved = open(PROFILE_TOML).read()
results = {}
root, home = home_for("gpt", "")
open(root + "/colour.txt", "w").write("The colour of the day is TEAL.\n")
open(root + "/animal.txt", "w").write("The animal of the day is an OWL.\n")
try:
    w = Window(root, home, "--new")
    assert w.wait(lambda: "Enter send" in w.text(), 60), w.text()
    def ask(text, sec):
        w.say(text)
        w.wait(lambda: not w.idle(), 30)
        end = time.time() + sec * SLOW
        while time.time() < end:
            w.pump(0.5)
            if "Always allow" in w.text():
                os.write(w.fd, b"2"); w.pump(0.5); continue
            if w.idle():
                w.pump(2)
                if w.idle() and "Always allow" not in w.text(): break
        return w.text()
    def after(marker):
        t = w.text()
        return t.split(marker)[-1] if marker in t else ""
    move_to(w, "Ollama")
    results["on_ollama"] = "ADAPTER  ollama" in w.status()
    ask("Remember this code word: ZEBRA-42. Then read the file colour.txt with your read tool and tell me the colour.", 300)
    results["ollama_read_and_answered"] = "TEAL" in after("Remember this code word").upper()
    w.show("on Ollama", "gpt-1-ollama.txt")
    move_to(w, "gpt-subscription")
    print("moved:", w.status().strip())
    results["moved_to_plan"] = "ADAPTER  chatgpt" in w.status() or "gpt-subscription" in w.status()
    ask("Without using any tool: what code word did I give you, and what colour was in colour.txt? Answer in one short line.", 180)
    a = after("Without using any tool: what code word")
    results["plan_knows_the_word"] = "ZEBRA-42" in a
    results["plan_knows_the_tool_result"] = "TEAL" in a.upper()
    ask("Now read the file animal.txt with the read tool and tell me the animal.", 180)
    results["plan_read_and_answered"] = "OWL" in after("Now read the file animal.txt").upper()
    w.show("on the ChatGPT plan", "gpt-2-plan.txt")
    move_to(w, "Ollama")
    print("moved back:", w.status().strip())
    results["moved_back_to_ollama"] = "ADAPTER  ollama" in w.status()
    ask("Without using any tool: list the code word, the colour and the animal from this conversation, in one short line.", 300)
    a = after("list the code word, the colour and the animal").upper()
    results["ollama_knows_all_three"] = all(x in a for x in ("ZEBRA-42", "TEAL", "OWL"))
    w.show("back on Ollama", "gpt-3-ollama.txt")
    results["no_unknown_tool"] = "no tool named" not in w.text()
    w.close()
finally:
    open(PROFILE_TOML, "w").write(saved)
    print("profile.toml put back:", open(PROFILE_TOML).read() == saved)
print("\nRESULTS:", json.dumps(results))
print("ALL PASSED" if all(results.values()) else "FAILED")
