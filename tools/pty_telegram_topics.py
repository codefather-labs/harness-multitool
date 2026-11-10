#!/usr/bin/env python3
"""Debug: the owner's case in a real TUI with a fake Bot API: two tabs, each a fake delegate with its nick, each
attached to its own forum topic with /switch. A message in a topic reaches that tab's agent, its telegram_send
comes back in that topic, a rename keeps the binding, and a daemon restart keeps nicks and bindings. Reads no
screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_telegram_topics.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, struct, tempfile, termios, threading, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-tt-root")); home = tempfile.mkdtemp(prefix="mt-tt-home"); os.makedirs(home + "/cfg/harness-multitool")
TOKEN = "123456:TOPICS"; log = home + "/log"; fake = home + "/fake-claude"
updates = []; sent = []; next_id = [1]
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        method = self.path.rsplit("/", 1)[-1]
        if method == "getUpdates":
            time.sleep(0.05)
            result = [u for u in list(updates) if u["update_id"] >= body["offset"]]
        else:
            if method == "sendMessage": sent.append(body)
            result = {"message_id": 1}
        out = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Bot); srv.handle_error = lambda *a: None; threading.Thread(target=srv.serve_forever, daemon=True).start()
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
config = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["harness"]
tab = args[args.index("--mcp-config") + 1]
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid(), "prompt": prompt}}) + "\\n")
    if "please answer" in prompt:
        word = prompt.split("please answer")[-1].strip().split()[0]
        subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": "telegram_send", "arguments": {{"text": "answer " + word}}}}}}) + "\\n", capture_output=True, text=True)
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[telegram]\ntoken = "{TOKEN}"\nallowed_users = [42]\nbase_url = "http://127.0.0.1:{srv.server_address[1]}"\n\n[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
bin_ = sys.argv[1]
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
env.pop("MULTITOOL_TELEGRAM_TOKEN", None)
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
def prompts(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.004)
    pump(0.4)
def tg(topic, text):
    updates.append({"update_id": next_id[0], "message": {"message_id": next_id[0], "from": {"id": 42}, "chat": {"id": -100}, "message_thread_id": topic, "text": text}}); next_id[0] += 1
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + json.dumps(sent)[-700:] + " " + json.dumps(prompts())[-500:]
def texts(topic): return [m["text"] for m in sent if m.get("message_thread_id") == topic]
pump(1.5)
typed("/nick orchid\r"); typed("first in one\r")
until("the first tab runs", lambda: any("first in one" in p["prompt"] for p in prompts()))
pump(0.5)
typed("/tab two\r"); pump(0.5)
typed("/nick lotus\r"); typed("first in two\r")
until("the second tab runs", lambda: any("first in two" in p["prompt"] for p in prompts()))
pump(1)
one = next(p["pid"] for p in prompts() if "first in one" in p["prompt"]); two = next(p["pid"] for p in prompts() if "first in two" in p["prompt"])
assert one != two, "two tabs, two processes"
tg(10, "/agents")
until("the list has both nicks", lambda: any("orchid" in t and "lotus" in t for t in texts(10)))
print("/agents:", [t for t in texts(10) if "orchid" in t][0].replace("\n", " | "))
tg(10, "/switch orchid"); tg(11, "/switch lotus")
until("both bound", lambda: any("orchid" in t and "now goes" in t for t in texts(10)) and any("lotus" in t and "now goes" in t for t in texts(11)))
tg(10, "please answer ONE"); tg(11, "please answer TWO")
until("each topic's message at its own agent", lambda: any("answer ONE" in p["prompt"] and p["pid"] == one for p in prompts()) and any("answer TWO" in p["prompt"] and p["pid"] == two for p in prompts()))
until("each answer in its own topic", lambda: "answer ONE" in texts(10) and "answer TWO" in texts(11))
assert "answer TWO" not in texts(10) and "answer ONE" not in texts(11), sent
print("each topic reached its own agent and got its own answer")
tg(10, "/nick iris")
until("renamed", lambda: any("goes by iris" in t for t in texts(10)))
pump(0.5)
tg(10, "please answer THREE")
until("a rename keeps the binding", lambda: "answer THREE" in texts(10))
tg(11, "/agents")
until("the new nick is listed", lambda: any("iris" in t and "lotus (current)" in t for t in texts(11)))
print("a rename kept the binding"); print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
