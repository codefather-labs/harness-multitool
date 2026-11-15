#!/usr/bin/env python3
"""Debug, CON-163 and journal#1188 live: the real TUI in a pty, the real daemon carrying the Telegram
channel, a fake model endpoint and a fake Bot API. A plan's card shows in the window and its buttons go to
Telegram. Run 1: the plan is approved by a press on Telegram, and the window's card goes with the toast
'✅ План одобрен в Telegram'. Run 2: a second plan is approved in the window, and the Telegram message is
edited to '✅ Одобрено в терминале' with no buttons. Prints the screens. Needs pyte.
Usage: tools/pty_plan_toast.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text, tool=None):
    ev = [{"type": "message_start", "message": {"usage": {"input_tokens": 500, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}]
    if tool:
        ev += [{"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": tool[0], "name": tool[1], "input": {}}}, {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": json.dumps(tool[2])}}, {"type": "content_block_stop", "index": 1}]
    ev += [{"type": "message_delta", "delta": {"stop_reason": "tool_use" if tool else "end_turn"}, "usage": {"output_tokens": 20}}, {"type": "message_stop"}]
    return sse(ev)

def plan_turns(n, plan):
    return [
        turn("Planning.", (f"p{n}a", "mode__plan_enter", {})),
        turn("Noting.", (f"p{n}b", "journal_add", {"body": plan, "category": "decision", "tags": ["plan"]})),
        turn("", (f"p{n}m", "mindmap_new", {"title": f"plan {n}"})),
        turn("", (f"p{n}c", "mindmap_add", {"map": 2 * n - 1, "text": "the work", "refs": [1]})),
        turn("", (f"p{n}f", "flowchart_new", {"title": f"plan {n} logic"})),
        turn("", (f"p{n}d", "flowchart_add", {"map": 2 * n, "text": "a branch", "kind": "process"})),
        turn("Asking.", (f"p{n}e", "mode__plan_exit", {"plan": plan})),
        turn(f"Building plan {n} now."),
    ]

turns = plan_turns(1, "Plan one: change a.txt.")
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        b = turns.pop(0) if turns else turn("no more")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

TOKEN = "123456:PTYSECRET"
updates = [{"update_id": 1, "message": {"message_id": 1, "from": {"id": 42}, "chat": {"id": -100}, "text": "plan the change"}}]
sent = []
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path.endswith("/getUpdates"):
            time.sleep(0.2)
            reply = {"ok": True, "result": [u for u in updates if u["update_id"] >= body["offset"]]}
        else:
            sent.append((self.path.rsplit("/", 1)[1], body)); reply = {"ok": True, "result": {"message_id": 50 + len(sent)}}
        b = json.dumps(reply).encode(); self.send_response(200); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
    def handle(self):
        try: super().handle()
        except BrokenPipeError: pass
bot = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Bot); threading.Thread(target=bot.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-toast-root"); home = tempfile.mkdtemp(prefix="mt-toast-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[telegram]\nbase_url="http://127.0.0.1:{bot.server_address[1]}"\nallowed_users=[42]\n\n[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
COLS, ROWS = 120, 44; bin_ = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(MULTITOOL_TELEGRAM_TOKEN=TOKEN, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", COLORTERM="truecolor")
    os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def show(t): print(f"\n===== {t} =====\n{text()}")
def wait(cond, sec, step=0.2):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(step)
    return cond()
def plan_message():
    for kind, body in sent:
        if kind == "sendMessage" and "reply_markup" in body:
            yield body
CARD = "leave plan mode"
results = {}

# Run 1: the Telegram message starts the run; the plan asks in both places; a press on Telegram decides.
if not wait(lambda: CARD in text() and any(True for _ in plan_message()), 40):
    show("run 1: no card or no plan"); print("Telegram calls:", [(k, str(b)[:300]) for k, b in sent]); sys.exit(1)
show("run 1: the plan's card in the window, its buttons sent to Telegram")
msg = list(plan_message())[-1]
buttons = [b["callback_data"] for row in msg["reply_markup"]["inline_keyboard"] for b in row]
print("Telegram plan buttons:", buttons)
yes = next(b for b in buttons if b.startswith("plan:y:"))
updates.append({"update_id": 2, "callback_query": {"id": "q1", "from": {"id": 42}, "message": {"message_id": 51, "chat": {"id": -100}}, "data": yes}})
results["run1_toast"] = wait(lambda: "План одобрен в Telegram" in text(), 20, 0.05)
show("run 1: after the press on Telegram")
results["run1_card_gone"] = CARD not in text()
edits = [b for k, b in sent if k == "editMessageText"]
results["run1_telegram_edited"] = any("Одобрено в Telegram" in e.get("text", "") for e in edits)
results["run1_agent_went_on"] = wait(lambda: "Building plan 1 now." in text(), 20)

# Run 2: a second plan, approved in the window; the Telegram message follows.
pump(4.0)  # the toast goes by itself
turns.extend(plan_turns(2, "Plan two: change b.txt."))
for c in "plan again\r": os.write(fd, c.encode()); time.sleep(0.01)
before = len(list(plan_message()))
if not wait(lambda: CARD in text() and len(list(plan_message())) > before, 40):
    show("run 2: no card or no plan"); print("Telegram calls:", [(k, str(b)[:300]) for k, b in sent]); sys.exit(1)
show("run 2: the second plan's card")
# The card's lock against keys meant for the composer (CON-54) passes first.
wait(lambda: "One moment" not in text(), 10, 0.1); pump(0.3)
os.write(fd, b"1")
results["run2_telegram_edited"] = wait(lambda: any("Одобрено в терминале" in b.get("text", "") for k, b in sent if k == "editMessageText"), 20)
last_edit = [b for k, b in sent if k == "editMessageText"][-1]
results["run2_no_buttons"] = last_edit.get("reply_markup", {}).get("inline_keyboard", None) == []
results["run2_agent_went_on"] = wait(lambda: "Building plan 2 now." in text(), 20)
show("run 2: after the approval in the window")
print("\nTelegram calls:", [(k, b.get("text", "")[:60], b.get("reply_markup")) for k, b in sent])
print("\nRESULTS:", json.dumps(results, ensure_ascii=False))
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5)
data = home + "/data/harness-multitool"
try:
    os.kill(json.load(open(data + "/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e:
    print("daemon stop:", e)
assert all(results.values()), results
print("ALL PASSED")
