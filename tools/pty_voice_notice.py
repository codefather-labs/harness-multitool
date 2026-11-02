#!/usr/bin/env python3
"""Debug: a voice note with no speech engine set up, in a real TUI with a fake Bot API: the user is told in the chat
and in the conversation's log, and the agent is not woken nor told, then or with its next message. Then the settings'
Speech to text page is drawn, with the dropdown open, into pictures. Reads no screen, so it needs no pyte. Needs
--features claude-code.
Usage: tools/pty_voice_notice.py <absolute path to multitool> [a directory to keep the pictures in]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, struct, subprocess, tempfile, termios, threading, time

NOTICE = "Voice Transcript error. Missing speech to text backend. Choose any one inside of harness settings"
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-vn-root")); home = tempfile.mkdtemp(prefix="mt-vn-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"; keep = sys.argv[2] if len(sys.argv) > 2 else home
updates = []; sent = []; fetched = []
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        method = self.path.rsplit("/", 1)[-1]
        if method == "getUpdates":
            time.sleep(0.05)
            result = [u for u in list(updates) if u["update_id"] >= body["offset"]]
        else:
            if method == "sendMessage": sent.append(body)
            if method == "getFile": fetched.append(body)
            result = {"message_id": 1}
        out = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Bot); srv.handle_error = lambda *a: None; threading.Thread(target=srv.serve_forever, daemon=True).start()
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, sys
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-1", "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    open({log!r}, "a").write(line)
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[telegram]\ntoken = "1:VOICE"\nallowed_users = [42]\nbase_url = "http://127.0.0.1:{srv.server_address[1]}"\n\n[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
bin_ = sys.argv[1]
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
env.pop("MULTITOOL_TELEGRAM_TOKEN", None)
os.makedirs(home + "/bin"); open(home + "/bin/pw-record", "w").write("#!/bin/sh\nwhile :; do head -c 3200 /dev/urandom; sleep 0.1; done\n"); os.chmod(home + "/bin/pw-record", 0o755)
env["PATH"] = home + "/bin:" + env["PATH"]
# The page, with a stand-in for the runtime, a model and a microphone: what it draws is for a person to look at.
data = home + "/data/harness-multitool/speech"; os.makedirs(data + "/runtime/bin"); os.makedirs(data + "/models/tiny")
for name in ("tiny-encoder.int8.onnx", "tiny-decoder.int8.onnx", "tiny-tokens.txt"): open(data + "/models/tiny/" + name, "w").close()
open(data + "/runtime/bin/sherpa-onnx-offline", "w").write("#!/bin/sh\necho 'loading'\necho '{\"text\": \" привет, это проверка распознавания \"}'\n")
os.chmod(data + "/runtime/bin/sherpa-onnx-offline", 0o755)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.004)
    pump(0.4)
def keys(*names):
    for k in names: os.write(fd, k.encode()); pump(0.15)
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + json.dumps(sent)[-500:] + " " + told()[-500:]
def told(): return open(log).read() if os.path.exists(log) else ""
def in_log():
    found = []
    for folder, _, files in os.walk(home + "/data"):
        for name in files:
            try: text = open(os.path.join(folder, name), errors="replace").read()
            except OSError: continue
            found += [l for l in text.split("\n") if '"notice"' in l and NOTICE in l]
    return found
def shot(name):
    out = os.path.join(keep, name)
    done = subprocess.run([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
    print("picture:", out)
pump(1.5)
typed("first message\r")
until("the agent runs", lambda: "first message" in told())
pump(1)
before = told()
updates.append({"update_id": 1, "message": {"message_id": 1, "from": {"id": 42}, "chat": {"id": 7}, "voice": {"file_id": "F", "duration": 2}}})
until("told in the chat", lambda: any(m["text"] == NOTICE for m in sent))
until("in the conversation's log", lambda: in_log())
pump(1.5)
assert told() == before, "the agent was woken: " + told()[len(before):]
assert not fetched, "a note was fetched for nothing"
print("the user is told in the chat and the log; the agent was not woken, nothing fetched")
shot("voice-notice.png")
typed("second message\r")
until("the next message runs", lambda: "second message" in told())
assert NOTICE not in told(), "the agent was told with its next message"
print("the next message reached the agent without it")
pump(1)
down = "\x1b[B"
typed("/settings\r"); pump(0.5)
keys(*[down] * 8); keys("\t", "\r"); shot("speech-engine-dropdown.png")
keys(down, "\r", down, "\r"); shot("speech-model-dropdown.png")
keys("\x1b"); keys(down, down, "\r"); pump(1.2); shot("speech-listening.png")
keys("\r"); pump(1.5); shot("speech-heard.png")
print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
