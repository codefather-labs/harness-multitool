#!/usr/bin/env python3
"""Debug: the Telegram token and users set in the settings page of a real TUI, with a fake Bot API: saved in
the configuration at mode 0600, the running daemon starts polling at once with no restart, and the token shows
nowhere: not on screen, not in the picture's source, not in the daemon's status. Then two tabs' worth of
/agents works through it. Reads no screen, so it needs no pyte.
Usage: tools/pty_telegram_settings.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, re, select, stat, struct, subprocess, tempfile, termios, threading, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-ts-root")); home = tempfile.mkdtemp(prefix="mt-ts-home"); os.makedirs(home + "/cfg/harness-multitool")
TOKEN = "123456:SETTINGS-SECRET-TOKEN"
calls = []; sent = []; updates = []
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        method = self.path.rsplit("/", 1)[-1]
        calls.append(self.path)
        if method == "getUpdates":
            time.sleep(0.05)
            result = [u for u in updates if u["update_id"] >= body["offset"]]
        else:
            if method == "sendMessage": sent.append(body)
            result = {"message_id": 1}
        out = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Bot); srv.handle_error = lambda *a: None; threading.Thread(target=srv.serve_forever, daemon=True).start()
config = home + "/cfg/harness-multitool/config.toml"
open(config, "w").write(f'[telegram]\nbase_url = "http://127.0.0.1:{srv.server_address[1]}"\n')
bin_ = sys.argv[1]
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
env.pop("MULTITOOL_TELEGRAM_TOKEN", None)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "startup"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
drawn = ""
def pump(sec):
    global drawn
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: raw = os.read(fd, 65536)
            except OSError: return
            drawn += re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", raw.decode("utf8", "replace"))
def until(what, check, sec=15):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + repr(drawn[-600:])
def press(*keys):
    for k in keys: os.write(fd, k.encode()); pump(0.12)
pump(1.5)
press("s"); pump(0.5)
until("settings open", lambda: "Callbacks" in drawn)
assert not calls, "nothing polls with no token"
down = "\x1b[B"
press(*[down] * 7)
until("the page", lambda: "Allowed user ids" in drawn)
press("\t", "\r")
os.write(fd, b"\x1b[200~" + TOKEN.encode() + b"\x1b[201~"); pump(0.3)
press("\r", down, "\r")
for c in "42": os.write(fd, c.encode()); pump(0.02)
press("\r", "\x13"); pump(0.5)
until("it polls at once, with no restart", lambda: any(c.endswith("/getUpdates") or c.endswith("/setMyCommands") for c in calls), 10)
assert all(c.startswith(f"/bot{TOKEN}/") for c in calls), calls[:2]
print("saved and polling, with the running daemon:", sorted({c.rsplit('/', 1)[-1] for c in calls}))
text = open(config).read()
assert TOKEN in text and "allowed_users = [42]" in text.replace("[\n    42,\n]", "[42]"), "saved"
assert stat.S_IMODE(os.stat(config).st_mode) == 0o600, oct(os.stat(config).st_mode)
assert TOKEN not in drawn and "SETTINGS-SECRET" not in drawn, "the token on the screen"
status = subprocess.run([bin_, "daemon", "--status"], env=env, cwd=root, capture_output=True, text=True, timeout=20).stdout
assert '"on":true' in status.replace(" ", "") and TOKEN not in status, status[:300]
print("the file is 0600, and the token is on no screen and in no status")
press("\x1b"); pump(0.3)
updates.append({"update_id": 1, "message": {"message_id": 1, "from": {"id": 42}, "chat": {"id": 7}, "text": "/agents"}})
until("/agents answers", lambda: any("No session" in m["text"] or "Open sessions" in m["text"] for m in sent))
print("/agents answered:", sent[-1]["text"][:40]); print("ok")
os.write(fd, b"\x03"); pump(0.3)
