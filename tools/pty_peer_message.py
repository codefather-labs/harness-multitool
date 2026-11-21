#!/usr/bin/env python3
"""Debug, journal#1528: a message from another Claude Code session (its SendMessage) is drawn in the window.
The real TUI in a pty, Claude Code as its delegate played by a fake `claude` (a Python script) that puts
on its stream, in the middle of its first run, the line Claude Code 2.1.325 puts there for a peer's message
(as recorded in .harness/proofs/1528/probe-busy-stream.jsonl). Checked: the window shows it as a card
from the sender; the log holds it once, as a notice from agent:<sender>; the next run's prompt does not
carry it. Needs pyte and a binary built with --features claude-code.
Usage: tools/pty_peer_message.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, select, signal, struct, tempfile, termios, time
import pyte

root = tempfile.mkdtemp(prefix="mt-peer-root"); home = tempfile.mkdtemp(prefix="mt-peer-home"); os.makedirs(home + "/cfg/harness-multitool")
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-peer-proof")); os.makedirs(proof_dir, exist_ok=True)
COLS, ROWS = 120, 40
peer = {"type": "user", "message": {"role": "user", "content": "<cross-session-message from=\"uds:/run/user/1000/cc-socks/1.sock\" from-name=\"orchestrator-9\" from-mode=\"bypass\">\nPEER-SAYS-HELLO-5150\n</cross-session-message>"},
        "isReplay": True, "isSynthetic": True, "origin": {"kind": "peer", "from": "uds:/run/user/1000/cc-socks/1.sock", "name": "orchestrator-9", "fromMode": "bypass", "body": "PEER-SAYS-HELLO-5150", "verifiedLocal": True}}
json.dump(peer, open(home + "/peer.json", "w"))
fake = home + "/fake-claude"
open(fake, "w").write(f"""#!/usr/bin/env python3
import json, sys, os
home = {home!r}
args = sys.argv[1:]
if args[:1] == ["auth"]:
    print(json.dumps({{"loggedIn": True}})); sys.exit(0)
prompt = sys.stdin.readline()
open(home + "/prompts", "a").write(prompt + "\\n---\\n")
n = int(open(home + "/n").read()) + 1 if os.path.exists(home + "/n") else 1
open(home + "/n", "w").write(str(n))
def out(o): print(json.dumps(o), flush=True)
out({{"type": "system", "subtype": "init", "session_id": f"sess-{{n}}", "model": "fake-model"}})
def text(t): out({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": t}}}}}})
if n == 1:
    text("working. ")
    out(json.load(open(home + "/peer.json")))
text(f"ANSWER-{{n}}")
out({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.001, "usage": {{"input_tokens": 4, "output_tokens": 6}}}})
""")
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "claude-code"\nmodel = "claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind = "claude-code"\ncontext_window = 200000\nbinary = "{fake}"\n')

class Window:
    def __init__(self, *args):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=home + "/claude", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
            os.execv(sys.argv[1], [sys.argv[1], *args])
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
    def wait(self, cond, sec):
        end = time.time() + sec
        while time.time() < end and not cond(): self.pump(0.2)
        return cond()
    def keys(self, *ks):
        for k in ks: os.write(self.fd, k.encode()); time.sleep(0.05)
        self.pump(0.4)
    def show(self, t, save=None):
        shown = "\n".join(l.rstrip() for l in self.screen.display if l.strip())
        print(f"\n===== {t} =====\n" + shown)
        if save: open(proof_dir + "/" + save, "w").write(self.text())
    def to_chats(self):
        self.keys(*"/settings", "\r"); self.pump(2)
        for _ in range(20):
            if "CHATS" in self.text(): break
            self.keys("\x1b[B"); self.pump(0.3)
        self.keys("\x1b[C"); self.pump(1)
    def close(self):
        for _ in range(3): self.keys("\x1b"); self.pump(0.4)
        for c in "/exit\r": os.write(self.fd, c.encode()); time.sleep(0.01)
        self.pump(1)
        for _ in range(50):
            try:
                if os.waitpid(self.pid, os.WNOHANG)[0]: return
            except ChildProcessError: return
            self.pump(0.2)
        print("window did not exit: killed"); os.kill(self.pid, signal.SIGKILL)


results = {}
w = Window("--new")
assert w.wait(lambda: "Enter send" in w.text(), 20), w.text()
w.keys(*"start", "\r")
results["answered"] = w.wait(lambda: "ANSWER-1" in w.text(), 30)
w.pump(1)
results["card_shown"] = "PEER-SAYS-HELLO-5150" in w.text() and "orchestrator-9" in w.text()
w.show("the peer's message in the window", "peer-card.txt")
logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
notices = [json.loads(l) for p in logs for l in open(p) if "PEER-SAYS-HELLO" in l]
print("log lines with it:", notices)
results["logged_once_as_notice"] = len(notices) == 1 and notices[0].get("event") == "notice" and notices[0].get("label") == "agent:orchestrator-9"
w.keys(*"next", "\r")
results["second_answer"] = w.wait(lambda: "ANSWER-2" in w.text(), 30)
prompts = open(home + "/prompts").read().split("---")
results["not_sent_again"] = len(prompts) >= 2 and "PEER-SAYS-HELLO" not in prompts[1]
w.close()
print("\nRESULTS:", json.dumps(results))
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
