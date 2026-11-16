#!/usr/bin/env python3
"""Debug, CON-171 and CON-172: the Settings' Chats tab. The real TUI in a pty with a fake model and a temporary home: one
conversation is made, the Chats tab lists it by its adapter, d dumps it into a .zip in the project's folder,
l lists that archive, Enter on it is refused (already on this machine), n and Enter load a copy, which the
list then shows. CON-172: the log is padded to about 200 MB, so the dump's and the load's progress
toasts are seen over the Settings with their share; the archive's name carries the time of the dump
(journal#1281). Needs pyte. Usage: tools/pty_chats_tab.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import re, fcntl, glob, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}])
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        b = turn("ANSWER-ONE")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-chats-root"); home = tempfile.mkdtemp(prefix="mt-chats-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=home + "/claude", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(sys.argv[1], [sys.argv[1], "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 140, 0, 0))
screen = pyte.Screen(140, 44); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def wait(cond, sec):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(0.2)
    return cond()
def keys(*ks):
    for k in ks: os.write(fd, k.encode()); time.sleep(0.05)
def show(t): print(f"\n===== {t} =====\n" + "\n".join(l.rstrip() for l in screen.display if l.strip()))
results = {}
assert wait(lambda: "Enter send" in text(), 20), text()
keys(*"hello chats", "\r")
results["conversation_made"] = wait(lambda: "ANSWER-ONE" in text(), 20)
pump(2)
# CON-172: about 200 MB of incompressible lines in the conversation's log, so a dump takes a while.
logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
assert len(logs) == 1, logs
with open(logs[0], "a") as f:
    for _ in range(1600):
        f.write(json.dumps({"pad": os.urandom(64 * 1024).hex()[: 128 * 1024]}) + "\n")
print("log padded to", os.path.getsize(logs[0]) // 1_000_000, "MB")
keys(*"/settings", "\r"); pump(2)
# To the Chats section: it comes after Journal.
for _ in range(20):
    if "CHATS" in text(): break
    keys("\x1b[B"); pump(0.3)
keys("\x1b[C"); pump(1)
show("the Chats tab")
results["listed_by_adapter"] = "hello chats" in text() and "anthropic" in text()
results["listed_size_is_the_log"] = " MB" in text() and any(f"{n} MB" in text() or f"{n}." in text() for n in range(200, 230))
def progress_seen(word):
    """Waits for the toast with `word` and a share, and keeps the screen it was seen on."""
    seen = wait(lambda: word in text() and re.search(r"MB · ([1-9]|[1-9][0-9])%", text()) is not None, 60)
    if seen: show(f"progress: {word}"); open(proof_dir + f"/{word.lower()}-toast.txt", "w").write(text())
    return seen
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-chats-proof")); os.makedirs(proof_dir, exist_ok=True)
keys("d")
results["dump_progress_over_settings"] = progress_seen("Dumping") and "CHATS" in text()
results["dumped"] = wait(lambda: "Dumped hello chats to" in text(), 120)
results["dump_toast_gone"] = "Dumping" not in text()
zips = glob.glob(root + "/*.zip")
results["zip_in_the_project_folder"] = len(zips) == 1
import re
results["zip_named_with_its_time"] = bool(zips) and re.fullmatch(r"hello-chats-[0-9A-Za-z]{1,8}-\d{4}-\d\d-\d\d-\d\d-\d\d-\d\d\.zip", os.path.basename(zips[0])) is not None
print("archive:", [os.path.basename(z) for z in zips])
show("after d")
keys("l"); pump(1)
results["archive_offered"] = wait(lambda: os.path.basename(zips[0]) in text() if zips else False, 5)
keys("\r")
results["refused_already_here"] = wait(lambda: "is already on this" in text() and "machine: load it with --new-id" in text(), 60)  # wrapped at a word (journal#1278)
show("after Enter: refused")
keys("l"); pump(0.5); keys("n"); pump(0.3); keys("\r")
results["load_progress_over_settings"] = progress_seen("Loading")
results["copy_loaded"] = wait(lambda: "Loaded hello chats as loaded-" in text(), 120)
results["load_toast_gone"] = "Loading" not in text()
show("after n and Enter: a copy")
results["copy_listed"] = text().count("hello chats") >= 2
print("\nRESULTS:", json.dumps(results))
keys("\x1b"); pump(0.5)
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.01)
pump(1)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
