#!/usr/bin/env python3
"""Debug, CON-171, CON-172 and CON-173: the Settings' Chats tab, driven by mouse clicks. The real TUI
in a pty with a fake model and a temporary home. One conversation is made, its log padded to about
200 MB so a dump and a load take a while. In the Chats tab:
- a click on the conversation shows [ Dump ] under it, and a click on that dumps it into
  multitool_dumps/ with a progress toast over the Settings (the archive named with the time);
- [ Load… ] opens the file picker, a fake zenity first on PATH that records its arguments and
  answers the archive: it loads under new ids with its toast (CON-175), and a second load of it is
  another conversation; a cancelled picker says 'No archive chosen.';
- a second window whose PATH has no picker falls back to the in-window list of archives.
Needs pyte. Usage: tools/pty_chats_tab.py <absolute path to multitool>"""
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
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-chats-proof")); os.makedirs(proof_dir, exist_ok=True)
# The fake picker: records its arguments, answers what $home/pick holds, or cancels (exit 1) on CANCEL.
fakebin = home + "/fakebin"; os.makedirs(fakebin); emptybin = home + "/emptybin"; os.makedirs(emptybin)
open(fakebin + "/zenity", "w").write(f'#!/bin/sh\nprintf "%s\\n" "$@" > {home}/picker-args\nanswer=$(/bin/cat {home}/pick)\n[ "$answer" = CANCEL ] && exit 1\nprintf "%s\\n" "$answer"\n')
os.chmod(fakebin + "/zenity", 0o755)

class Window:
    def __init__(self, path_dirs):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=home + "/claude", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", PATH=path_dirs, DISPLAY=":99")
            os.environ.pop("WAYLAND_DISPLAY", None)
            os.execv(sys.argv[1], [sys.argv[1], "--new"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 140, 0, 0))
        self.screen = pyte.Screen(140, 44); self.stream = pyte.ByteStream(self.screen)
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
    def show(self, t): print(f"\n===== {t} =====\n" + "\n".join(l.rstrip() for l in self.screen.display if l.strip()))
    def find(self, label, also=""):
        """Where `label` is on the screen, 1-based (column, row), on a line that also holds `also`."""
        for y, line in enumerate(self.screen.display):
            x = line.find(label)
            if x >= 0 and also in line: return x + 1, y + 1
        return None
    def click(self, label, also=""):
        at = self.find(label, also)
        if at is None: return False
        x, y = at; self.keys(f"\x1b[<0;{x + 1};{y}M", f"\x1b[<0;{x + 1};{y}m"); self.pump(0.5)
        return True
    def to_chats(self):
        self.keys(*"/settings", "\r"); self.pump(2)
        for _ in range(20):
            if "CHATS" in self.text(): break
            self.keys("\x1b[B"); self.pump(0.3)
        self.keys("\x1b[C"); self.pump(1)
    def close(self):
        for _ in range(3): self.keys("\x1b"); self.pump(0.4)  # out of a list, the page, the Settings
        for c in "/exit\r": os.write(self.fd, c.encode()); time.sleep(0.01)
        self.pump(1)
        for _ in range(50):
            try:
                if os.waitpid(self.pid, os.WNOHANG)[0]: return
            except ChildProcessError: return
            self.pump(0.2)
        print("window did not exit: killed"); os.kill(self.pid, signal.SIGKILL)

results = {}
w = Window(fakebin + ":" + os.environ["PATH"])
assert w.wait(lambda: "Enter send" in w.text(), 20), w.text()
w.keys(*"hello chats", "\r")
results["conversation_made"] = w.wait(lambda: "ANSWER-ONE" in w.text(), 20)
w.pump(2)
# CON-172: about 200 MB of incompressible lines in the conversation's log, so a dump takes a while.
logs = glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl")
assert len(logs) == 1, logs
with open(logs[0], "a") as f:
    for _ in range(1600):
        f.write(json.dumps({"pad": os.urandom(64 * 1024).hex()[: 128 * 1024]}) + "\n")
print("log padded to", os.path.getsize(logs[0]) // 1_000_000, "MB")
w.to_chats()
w.show("the Chats tab")
results["listed_by_adapter"] = "hello chats" in w.text() and "anthropic" in w.text()
results["load_button_always_there"] = "[ Load… ]" in w.text() and "[ Dump ]" not in w.text()
def progress_seen(word):
    """Waits for the toast with `word` and a share, and keeps the screen it was seen on."""
    seen = w.wait(lambda: word in w.text() and re.search(r"MB · ([1-9]|[1-9][0-9])%", w.text()) is not None, 60)
    if seen: w.show(f"progress: {word}"); open(proof_dir + f"/{word.lower()}-toast.txt", "w").write(w.text())
    return seen
# CON-173: a click on the conversation shows its Dump button; a click on that dumps it.
w.click("hello chats", also=" MB")
results["row_click_shows_dump"] = w.wait(lambda: "[ Dump ]" in w.text(), 5)
w.show("after a click on the conversation")
w.click("[ Dump ]")
results["dump_progress_over_settings"] = progress_seen("Dumping") and "CHATS" in w.text()
results["dumped"] = w.wait(lambda: "Dumped hello chats to" in w.text(), 120)
results["dump_toast_gone"] = "Dumping" not in w.text()
zips = glob.glob(root + "/multitool_dumps/*.zip")
results["zip_in_multitool_dumps"] = len(zips) == 1 and not glob.glob(root + "/*.zip")
results["zip_named_with_its_time"] = bool(zips) and re.fullmatch(r"hello-chats-[0-9A-Za-z]{1,8}-\d{4}-\d\d-\d\d-\d\d-\d\d-\d\d\.zip", os.path.basename(zips[0])) is not None
print("archive:", [os.path.basename(z) for z in zips])
# Load: the picker answers the archive. CON-175: a load always has new ids, so it loads at once.
open(home + "/pick", "w").write(zips[0] if zips else "/nonexistent.zip")
w.click("[ Load… ]")
results["load_progress_over_settings"] = progress_seen("Loading")
results["loaded_with_new_ids"] = w.wait(lambda: "Loaded hello chats as loaded-" in w.text(), 120)
results["load_toast_gone"] = "Loading" not in w.text()
args = open(home + "/picker-args").read().split("\n") if os.path.exists(home + "/picker-args") else []
print("picker arguments:", args)
results["picker_opened_on_multitool_dumps"] = "--file-selection" in args and f"--filename={root}/multitool_dumps/" in args and "--file-filter=Chat archives | *.zip" in args
results["no_copy_box"] = "as a copy" not in w.text()
w.show("after Load")
# The same archive again: another conversation.
w.click("[ Load… ]")
results["second_load_another_copy"] = w.wait(lambda: w.text().count("hello chats") >= 3 and "Loaded hello chats as loaded-" in w.text(), 120)
w.show("after a second Load")
# A cancelled picker.
open(home + "/pick", "w").write("CANCEL")
w.click("[ Load… ]")
results["cancel_said"] = w.wait(lambda: "No archive chosen." in w.text(), 20)
open(proof_dir + "/chats-buttons.txt", "w").write(w.text())
w.close()

# A window with no picker on its PATH: the in-window list instead.
w2 = Window(emptybin)
w2.wait(lambda: "Enter send" in w2.text(), 20); w2.pump(1)
w2.to_chats()
w2.click("[ Load… ]")
results["no_picker_said"] = w2.wait(lambda: "no file picker found" in w2.text(), 20)
results["fallback_list"] = "Load a conversation into this project" in w2.text() and ("multitool_dumps/" + os.path.basename(zips[0]) if zips else "?") in w2.text()
w2.show("no picker: the list")
open(proof_dir + "/no-picker.txt", "w").write(w2.text())
w2.close()

print("\nRESULTS:", json.dumps(results))
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
