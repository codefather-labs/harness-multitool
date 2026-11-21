#!/usr/bin/env python3
"""Debug, CON-189, the delegate's side: an edited copy of a Claude Code conversation goes on with a form
written fresh from the edited messages. The real TUI in a pty, Claude Code as its delegate played by
a fake `claude` that records its arguments and the transcript it is told to resume.

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
# The fake Claude Code: answers by what it is told; records its arguments and, on --resume, the
# transcript file it is pointed at, as the harness wrote it.
fake = home + "/fake-claude"; record = home + "/resumed"
open(fake, "w").write(f'''#!/bin/sh
IFS= read -r prompt
printf '%s\\n' "$*" >> {home}/calls
id=""; prev=""
for a in "$@"; do [ "$prev" = "--resume" ] && id="$a"; prev="$a"; done
if [ -n "$id" ]; then f=$(find {home} -name "$id.jsonl" -path "*projects*" | head -n 1); echo "== $id $f" >> {record}; [ -n "$f" ] && cat "$f" >> {record}; fi
case "$prompt" in *APPLE*) say=NOTED-APPLE;; *BLUE*) say=NOTED-BLUE;; *) say=ANSWER-AFTER-EDIT;; esac
n=$(wc -l < {home}/calls)
echo '{{"type":"system","subtype":"init","session_id":"sess-'$n'","model":"fake-model"}}'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"'$say'"}}}}}}'
echo '{{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.001,"usage":{{"input_tokens":4,"output_tokens":6}}}}'
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "claude-code"\nmodel = "claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind = "claude-code"\ncontext_window = 200000\nbinary = "{fake}"\n')
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
results["first_exchange"] = w.wait(lambda: "NOTED-APPLE" in w.text(), 30)
w.keys(*"the colour is BLUE", "\r")
results["second_exchange"] = w.wait(lambda: "NOTED-BLUE" in w.text(), 30)
w.pump(2)
logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
assert len(logs) == 1, logs
original = logs[0]; before = sha(original)
w.to_chats()
w.keys("e")
results["editor_open"] = w.wait(lambda: "Edit · the code word is APPLE" in w.text(), 10)
w.keys("\r", "\x1b[B", "\r", "d")
w.show("after d", "delegate-deleted.txt")
w.keys("s")
results["listed"] = w.wait(lambda: "the code word is APPLE (edited)" in w.text(), 20)
w.close()
results["original_unchanged"] = sha(original) == before
copies = glob.glob(home + "/data/harness-multitool/sessions/*/edited-*.jsonl")
results["one_copy"] = len(copies) == 1
if copies:
    w2 = Window("--resume", os.path.basename(copies[0])[:-len(".jsonl")])
    w2.wait(lambda: "Enter send" in w2.text(), 20); w2.pump(1)
    w2.keys(*"what do you know", "\r")
    results["answered"] = w2.wait(lambda: "ANSWER-AFTER-EDIT" in w2.text(), 30)
    w2.show("the copy goes on", "delegate-resumed.txt")
    w2.close()
    import re
    calls = open(home + "/calls").read()
    # The arguments hold newlines (the system prompt): the last --resume is the copy's.
    resumed = (re.findall(r"--resume (\S+)", calls) or [""])[-1]
    print("resumes:", re.findall(r"--resume (\S+)", calls))
    results["resumed_a_new_form"] = resumed != "" and not resumed.startswith("sess-")
    rec = open(record).read() if os.path.exists(record) else ""
    open(proof_dir + "/delegate-form.txt", "w").write(rec)
    part = rec.split("== " + resumed)[-1] if resumed else ""
    results["form_has_blue_not_apple"] = "BLUE" in part and "APPLE" not in part
    print("form resumed:", resumed, "| bytes:", len(part))
print("\nRESULTS:", json.dumps(results))
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
