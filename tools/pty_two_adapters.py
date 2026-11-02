#!/usr/bin/env python3
"""Debug: two conversations in one window on two adapters (CON-94), with a fake Claude Code and a fake Responses
server. Tab one stays on the delegate while tab two is moved to the other in the settings; each message reaches its
own; a run in tab one leaves tab two free to change; after a real reload with a new binary each is still on its own.
Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_two_adapters.py <absolute path to multitool> [a directory to keep the pictures in]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, shutil, struct, subprocess, tempfile, termios, threading, time, tomllib

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-ta-root")); home = tempfile.mkdtemp(prefix="mt-ta-home"); os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/here")
log = home + "/delegate"; fake = home + "/fake-claude"; keep = sys.argv[2] if len(sys.argv) > 2 else home
bin_ = home + "/here/multitool"; shutil.copy2(sys.argv[1], bin_)
bodies = []
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length", 0))).decode())
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        emit({"type": "response.output_text.delta", "delta": f"Responses {len(bodies)}."})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); srv.handle_error = lambda *a: None; threading.Thread(target=srv.serve_forever, daemon=True).start()
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, subprocess, sys, time
args = sys.argv[1:]
config = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["harness"]
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-one", "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(prompt + "\\n")
    if "slow" in prompt: time.sleep(5)
    if "compact yourself" in prompt:
        print(json.dumps({{"type": "system", "subtype": "compact_boundary", "session_id": "sess-one"}}), flush=True)
    if "be named" in prompt:
        subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": "set_nick", "arguments": {{"nick": "orchid"}}}}}}) + "\\n", capture_output=True, text=True)
    if "do reload" in prompt:
        subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": "plugins_reload", "arguments": {{}}}}}}) + "\\n", capture_output=True, text=True)
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Delegate"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
config = home + "/cfg/harness-multitool/config.toml"
open(config, "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.local]\nnone=true\n')
build = f"cp {sys.argv[1]!r} {bin_ + '.new'!r} && mv {bin_ + '.new'!r} {bin_!r}"
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", MULTITOOL_RELOAD_BUILD=build)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
seen = b""
def pump(sec):
    global seen
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: seen = (seen + os.read(fd, 65536))[-300000:]
            except OSError: return
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.004)
    pump(0.5)
def keys(*names):
    for k in names: os.write(fd, k.encode()); pump(0.2)
def delegate(): return open(log).read() if os.path.exists(log) else ""
def responses(): return "\n".join(bodies)
def saved(): return tomllib.load(open(config, "rb"))["session"]
def stored(): return json.load(open(home + "/data/harness-multitool/daemon/conversations.json"))
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + f"\ndelegate: {delegate()[-300:]}\nresponses: {responses()[-300:]}"
def shot(name):
    out = os.path.join(keep, name)
    done = subprocess.run([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
down = "\x1b[B"
pump(1.5)
typed("to the delegate\r")
until("tab one on the delegate", lambda: "to the delegate" in delegate())
pump(0.5)
typed("/tab two\r"); pump(0.5)
typed("/settings\r"); pump(0.5)
keys("\t", down, down, "\r"); keys("\x13"); pump(1)
assert saved()["provider"] == "local", saved()["provider"]
keys("\x1b"); pump(0.3)
typed("to responses\r")
until("tab two on the other adapter", lambda: "to responses" in responses())
assert "to responses" not in delegate(), "it went to the delegate"
print("tab two moved in the settings: its message went to its own adapter")
keys("\x1b1"); pump(0.3)
typed("the delegate again\r")
until("tab one stayed", lambda: "the delegate again" in delegate())
assert "the delegate again" not in responses(), "tab one moved with tab two"
assert sorted(c["provider"] for c in stored().values()) == ["claude-code", "local"], stored()
print("tab one stayed on the delegate, whatever the default says now:", saved()["provider"])
shot("two-adapters-one.png")
keys("\x1b2"); pump(0.3); shot("two-adapters-two.png")
# A run in tab one leaves tab two's adapter free to change.
keys("\x1b1"); pump(0.3); typed("slow one\r"); until("the run began", lambda: "slow one" in delegate())
keys("\x1b2"); pump(0.3); typed("/settings\r"); pump(0.4)
keys("\t", down, down, down, down, "\r"); keys("\x13"); pump(1)
assert saved()["provider"] == "claude-code", "tab two's settings were refused during tab one's run: " + saved()["provider"]
print("a run in tab one did not hold up tab two's settings:", saved()["provider"])
keys("\x1b"); pump(5)
# Back on the fake for what follows, by the settings again.
typed("/settings\r"); pump(0.4); keys("\t", down, down, "\r", down, down, down, down, down, "\r", "b", "\r"); shot("two-adapters-back.png"); keys("\x13"); pump(1); keys("\x1b"); pump(0.3)
assert saved()["provider"] == "local", saved()["provider"]
keys("\x1b1"); pump(0.3); seen = b""; typed("be named\r")
until("the agent's own tool named the tab as /nick does", lambda: b"orchid" in seen)
pump(0.3); shot("two-adapters-named.png")
print("the agent's own set_nick named the tab")
spec = home + "/data/harness-multitool/spec/agent-workflow.md"
assert spec in delegate().split("\n")[0], "the delegate's first message names the specification: " + delegate()[:200]
assert spec not in delegate().split("the delegate again")[1], "for one turn"
n = delegate().count(spec); typed("compact yourself\r"); until("it compacted", lambda: "compact yourself" in delegate()); pump(0.5)
typed("after compacting\r"); until("the next message", lambda: "after compacting" in delegate())
assert delegate().count(spec) == n + 1, "after its compaction the delegate is told again: " + delegate()[-400:]
print("the delegate was given the path at its start and after its compaction")
typed("do reload now\r")
until("the agent was told of the reload", lambda: "rebuilt and restarted" in delegate(), 40)
print("reloaded, a new binary at the same path")
pump(1)
shot("two-adapters-named-after-reload.png")
typed("the delegate after\r")
until("tab one still on the delegate", lambda: "the delegate after" in delegate())
assert spec in delegate().split("do reload now")[1], "after the reload, a resume, it is told again: " + delegate()[-400:]
assert "the delegate after" not in responses()
keys("\x1b2"); pump(0.5); typed("responses after\r")
until("tab two still on the other", lambda: "responses after" in responses())
assert "responses after" not in delegate()
print("after the reload each is on its own"); print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
