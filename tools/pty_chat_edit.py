#!/usr/bin/env python3
"""Debug, CON-189: editing a conversation from Settings > Chats, in the real TUI in a pty with a fake
model and a temporary home.

A conversation of two exchanges is made: 'the code word is APPLE' and 'the colour is BLUE'. In
Settings > Chats, e opens the editor on it. The first message and its answer are marked with Enter
and deleted with d, and s saves. Checked:
- the list shows '<name> (edited)', and the save says the original is kept;
- the original log's sha256 is the same before and after;
- the copy's log holds no APPLE and keeps BLUE;
- the copy, opened with --resume and sent one message, sends the model a history with BLUE and no
  APPLE: the agent no longer knows the deleted message.
Needs pyte. Usage: tools/pty_chat_edit.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, hashlib, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
bodies = []
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
        bodies.append(body)
        last = json.loads(body)["messages"][-1]
        said = json.dumps(last)
        answer = "NOTED-APPLE" if "APPLE" in said else "NOTED-BLUE" if "BLUE" in said else "ANSWER-AFTER-EDIT"
        b = turn(answer)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-edit-root"); home = tempfile.mkdtemp(prefix="mt-edit-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-edit-proof")); os.makedirs(proof_dir, exist_ok=True)
COLS, ROWS = int(os.environ.get("COLS", 120)), int(os.environ.get("ROWS", 40))

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

def sha(path): return hashlib.sha256(open(path, "rb").read()).hexdigest()
def user_texts(path):
    out = []
    for line in open(path):
        e = json.loads(line)
        if e.get("event") == "user_message": out.append(e["text"])
    return out

results = {}
w = Window("--new")
assert w.wait(lambda: "Enter send" in w.text(), 20), w.text()
w.keys(*"the code word is APPLE", "\r")
results["first_exchange"] = w.wait(lambda: "NOTED-APPLE" in w.text(), 20)
w.keys(*"the colour is BLUE", "\r")
results["second_exchange"] = w.wait(lambda: "NOTED-BLUE" in w.text(), 20)
w.pump(2)
logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
assert len(logs) == 1, logs
original = logs[0]
before = sha(original)
print("original:", os.path.basename(original), before)

w.to_chats()
w.show("the Chats tab")
def page():
    """The Settings page's column, its rows joined: the text between the second and third bars."""
    rows = [l.split("│") for l in w.screen.display]
    return " ".join(" ".join(r[2].split()) for r in rows if len(r) > 3)
results["edit_in_help"] = "e edit" in page()
w.keys("e")
results["editor_open"] = w.wait(lambda: "Edit · the code word is APPLE" in w.text(), 10)
w.show("the editor", "editor.txt")
results["drawn_as_chat"] = "the code word is APPLE" in w.text() and "NOTED-APPLE" in w.text() and "NOTED-BLUE" in w.text()
# The first message and its answer: marked, then deleted.
w.keys("\r", "\x1b[B", "\r")
results["two_marked"] = "2 marked" in w.text()
w.show("two cards marked", "marked.txt")
w.keys("d")
results["deleted_from_view"] = "2 deleted" in w.text() and "NOTED-APPLE" not in w.text() and "NOTED-BLUE" in w.text()
w.show("after d", "deleted.txt")
w.keys("s")
results["saved"] = w.wait(lambda: "(edited)\" as edited-" in page() and "cards left out. The original is kept" in page(), 20)
results["listed"] = w.wait(lambda: "the code word is APPLE (edited)" in w.text(), 10)
w.show("back in Chats after s", "saved.txt")
w.close()

after = sha(original)
results["original_unchanged"] = before == after
copies = [p for p in glob.glob(home + "/data/harness-multitool/sessions/*/edited-*.jsonl")]
results["one_copy"] = len(copies) == 1
copy = copies[0] if copies else None
said = user_texts(copy) if copy else []
results["copy_without_apple"] = bool(copy) and not any("APPLE" in t for t in said) and any("BLUE" in t for t in said) and "NOTED-APPLE" not in open(copy).read()
results["copy_has_no_form_link"] = bool(copy) and '"form_synced"' not in open(copy).read() and '"backend_session"' not in open(copy).read()
print("copy:", os.path.basename(copy) if copy else None, "user messages:", said)

# The copy goes on: the model is sent BLUE, and no APPLE.
if copy:
    n = len(bodies)
    w2 = Window("--resume", os.path.basename(copy)[:-len(".jsonl")])
    w2.wait(lambda: "Enter send" in w2.text(), 20); w2.pump(1)
    w2.show("the copy, resumed", "resumed.txt")
    results["resumed_shows_edited_card"] = "EDITED" in w2.text()
    w2.keys(*"what do you know", "\r")
    results["answered"] = w2.wait(lambda: "ANSWER-AFTER-EDIT" in w2.text(), 20)
    sent = bodies[n:]
    results["model_sent_blue_not_apple"] = bool(sent) and "BLUE" in sent[-1] and "APPLE" not in sent[-1]
    open(proof_dir + "/request-after-edit.json", "w").write(sent[-1] if sent else "")
    w2.close()

open(proof_dir + "/sha256.txt", "w").write(f"original before {before}\noriginal after  {after}\n")
print("\nRESULTS:", json.dumps(results))
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
