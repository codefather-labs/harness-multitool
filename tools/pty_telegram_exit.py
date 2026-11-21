#!/usr/bin/env python3
"""Debug, CON-192: a multitool window closed from Telegram's 🛠 Service → Exit, in the real TUI, with the real
daemon carrying the Telegram channel on a fake Bot API and a fake model.
1. '⛔ Закрыть сразу': the window's process exits, and the chat is told '✅ Окно закрыто'.
2. '⏳ Дать закончить работу' while a slow run is on: the window quits at once, the run ends in the
   daemon (its answer lands in the session's log), and the chat is told.
Needs pyte. Usage: tools/pty_telegram_exit.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

B = sys.argv[1]
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-tgx-proof")); os.makedirs(proof_dir, exist_ok=True)

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
        last = json.loads(body)["messages"][-1]
        slow = "slow" in json.dumps(last)
        if slow: time.sleep(8)
        b = turn("SLOW-DONE" if slow else "HELLO-BACK")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

TOKEN = "123456:PTYSECRET"
updates = []; sent = []; lock = threading.Lock()
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path.endswith("/getUpdates"):
            time.sleep(0.2)
            with lock: reply = {"ok": True, "result": [u for u in updates if u["update_id"] >= body.get("offset", 0)]}
        else:
            with lock: sent.append(body)
            reply = {"ok": True, "result": {"message_id": 100 + len(sent)}}
        b = json.dumps(reply).encode(); self.send_response(200); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
bot = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Bot); threading.Thread(target=bot.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-tgx-root"); home = tempfile.mkdtemp(prefix="mt-tgx-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[telegram]\nbase_url="http://127.0.0.1:{bot.server_port}"\nallowed_users=[42]\n\n[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
ENV = dict(MULTITOOL_TELEGRAM_TOKEN=TOKEN, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")

class Window:
    def __init__(self):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.update(ENV); os.execv(B, [B, "--new"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.screen = pyte.Screen(120, 40); self.stream = pyte.ByteStream(self.screen)
    def pump(self, sec):
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.stream.feed(os.read(self.fd, 65536))
                except OSError: return
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display)
    def say(self, t):
        for c in t: os.write(self.fd, c.encode()); time.sleep(0.003)
        os.write(self.fd, b"\r"); self.pump(0.5)
    def gone(self):
        try: return os.waitpid(self.pid, os.WNOHANG)[0] != 0
        except ChildProcessError: return True

n = [10]
def push(update):
    with lock:
        n[0] += 1; update["update_id"] = n[0]; updates.append(update)
def message(text): push({"message": {"message_id": n[0], "from": {"id": 42}, "chat": {"id": -100}, "text": text}})
def tap(data): push({"callback_query": {"id": f"q{n[0]}", "from": {"id": 42}, "data": data, "message": {"message_id": 100, "chat": {"id": -100}}}})
def said():
    with lock: return [s.get("text", "") for s in sent]
def until(what, cond, sec=30):
    end = time.time() + sec
    while time.time() < end and not cond(): time.sleep(0.2)
    ok = cond(); print(("ok   " if ok else "FAIL ") + what); return ok

results = {}
# 1. Close at once.
w = Window()
w.pump(1)
end = time.time() + 30
while time.time() < end and "Enter send" not in w.text(): w.pump(0.3)
w.say("hello"); end = time.time() + 20
while time.time() < end and "HELLO-BACK" not in w.text(): w.pump(0.3)
message("🛠 Service")
results["menu_has_exit"] = until("the menu has Exit", lambda: any("svc:exit" in json.dumps(s.get("reply_markup", {})) for s in sent))
tap("svc:exit")
results["windows_listed"] = until("the windows listed", lambda: any("Какое окно закрыть" in t for t in said()))
tap("x:0")
results["asked_how"] = until("asked how", lambda: any(t.startswith("Закрыть окно:") for t in said()))
t0 = time.time(); tap("xm:0:stop")
end = time.time() + 20
while time.time() < end and not w.gone(): w.pump(0.2)
results["window_process_gone"] = w.gone(); print(("ok   " if w.gone() else "FAIL ") + "the window's process exited in %.1fs" % (time.time() - t0))
results["closed_said"] = until("'✅ Окно закрыто' said", lambda: any(t.startswith("✅ Окно закрыто") for t in said()), 40)

# 2. Let it finish, while a slow run is on.
w2 = Window()
end = time.time() + 30
while time.time() < end and "Enter send" not in w2.text(): w2.pump(0.3)
w2.say("this one is slow"); w2.pump(1.5)
before = len(said())
tap("svc:exit")
until("listed again", lambda: sum("Какое окно закрыть" in t for t in said()) >= 2)
tap("x:0")
until("asked again", lambda: sum(t.startswith("Закрыть окно:") for t in said()) >= 2)
t0 = time.time(); tap("xm:0:finish-work")
end = time.time() + 20
while time.time() < end and not w2.gone(): w2.pump(0.2)
quit_in = time.time() - t0
results["window_quits_at_once"] = w2.gone() and quit_in < 6; print(("ok   " if results["window_quits_at_once"] else "FAIL ") + "the window quit in %.1fs, the run still on" % quit_in)
def finished():
    for p in glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl"):
        if "SLOW-DONE" in open(p).read(): return True
    return False
results["run_finished_in_daemon"] = until("the run ended in the daemon (SLOW-DONE in the log)", finished, 40)
results["closed_said_again"] = until("'✅ Окно закрыто' said again", lambda: sum(t.startswith("✅ Окно закрыто") for t in said()) >= 2, 40)
open(proof_dir + "/bot-said.txt", "w").write("\n---\n".join(said()))
print("\nthe bot said:", [t for t in said() if t][-8:])
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("\nRESULTS:", json.dumps(results))
print("ALL PASSED" if all(results.values()) else "FAILED")
