#!/usr/bin/env python3
"""Debug: orchestration mode (CON-100), in a real TUI. The master is a fake Claude Code; a slave runs on a fake Responses
server, another on the fake Claude Code. Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_orchestra.py <absolute path to multitool> [a directory to keep the pictures in]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, shutil, struct, subprocess, tempfile, termios, threading, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-or-root")); home = tempfile.mkdtemp(prefix="mt-or-home"); os.makedirs(home + "/cfg/harness-multitool"); os.makedirs(home + "/here")
log = home + "/log"; fake = home + "/fake-claude"; results = home + "/results"; keep = sys.argv[2] if len(sys.argv) > 2 else home
bin_ = home + "/here/multitool"; shutil.copy2(sys.argv[1], bin_)
bodies = []
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0)))); bodies.append(body)
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("connection", "close"); self.end_headers()
        def emit(e): self.wfile.write(f"data: {json.dumps(e)}\n\n".encode()); self.wfile.flush()
        last = body["input"][-1]; text = json.dumps(body["input"], ensure_ascii=False)
        if last.get("type") == "function_call_output":
            emit({"type": "response.output_text.delta", "delta": "Done with that."})
        elif "you are ready to take work" in json.dumps(last, ensure_ascii=False):
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "name": "agent_send", "arguments": json.dumps({"to": "boss", "text": "ready for work"}), "call_id": "r1"}})
        elif "run the first command" in json.dumps(last, ensure_ascii=False):
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "name": "shell", "arguments": json.dumps({"command": "touch allowed-marker"}), "call_id": "s1"}})
        elif "run the second command" in json.dumps(last, ensure_ascii=False):
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "name": "shell", "arguments": json.dumps({"command": "touch denied-marker"}), "call_id": "s2"}})
        elif "ask me something" in json.dumps(last, ensure_ascii=False):
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "name": "ask_user", "arguments": json.dumps({"question": "Which colour?", "options": [{"label": "red"}, {"label": "blue"}]}), "call_id": "q1"}})
        elif "talk to the other" in json.dumps(last, ensure_ascii=False):
            emit({"type": "response.output_item.done", "item": {"type": "function_call", "name": "agent_send", "arguments": json.dumps({"to": "second", "text": "hello peer"}), "call_id": "r2"}})
        else:
            emit({"type": "response.output_text.delta", "delta": "Task taken: " + last.get("content", [{}])[0].get("text", "")[:40] if isinstance(last.get("content"), list) else "ok"})
        emit({"type": "response.completed", "response": {"usage": {"input_tokens": 5, "output_tokens": 2}}})
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H); srv.handle_error = lambda *a: None; threading.Thread(target=srv.serve_forever, daemon=True).start()
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
servers = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
def tool(server, name, arguments):
    config = servers[server]
    done = subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": name, "arguments": arguments}}}}) + "\\n", capture_output=True, text=True)
    reply = json.loads(done.stdout.strip().split("\\n")[-1])
    reply = reply.get("result") or {{"isError": True, "content": [{{"text": json.dumps(reply)}}]}}
    open({results!r}, "a").write(json.dumps({{"tool": name, "error": reply["isError"], "text": reply["content"][0]["text"]}}) + "\\n")
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid(), "prompt": prompt}}) + "\\n")
    for said in prompt.split("\\n"):
        if said.startswith("call "):
            server, name, arguments = said[5:].split(" ", 2)
            tool(server, name, json.loads(arguments))
    if "you are ready to take work" in prompt: tool("harness", "agent_send", {{"to": "boss", "text": "second is ready"}})
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n\n[session.providers.local]\nkind="openai-responses"\nbase_url="http://127.0.0.1:{srv.server_address[1]}"\n\n[session.auth.local]\nnone=true\n')
build = f"cp {sys.argv[1]!r} {bin_ + '.new'!r} && mv {bin_ + '.new'!r} {bin_!r}"
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", MULTITOOL_RELOAD_BUILD=build)
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
    for c in s: os.write(fd, c.encode()); time.sleep(0.002)
    pump(0.4)
def prompts(): return [json.loads(l)["prompt"] for l in open(log)] if os.path.exists(log) else []
def answers(): return [json.loads(l) for l in open(results)] if os.path.exists(results) else []
def told(): return "\n".join(prompts())
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + "\nprompts: " + told()[-700:] + "\nanswers: " + json.dumps(answers())[-700:] + "\nresponses: " + json.dumps(bodies[-1]["input"] if bodies else "")[-300:]
def call(server, name, arguments):
    n = len(answers()); typed(f"call {server} {name} {json.dumps(arguments)}\r")
    until(name + " answered", lambda: len(answers()) > n); pump(0.3)
    return answers()[-1]
def shot(name):
    done = subprocess.run([bin_, "debug", "screenshot", "--out", os.path.join(keep, name)], env=env, cwd=root, capture_output=True, text=True, timeout=30)
    assert done.returncode == 0, done.stderr
pump(1.5)
typed("/nick boss\r"); typed("first\r"); until("the master runs", lambda: "first" in told()); pump(0.5)
out = call("mode", "agent_spawn", {"nick": "x", "adapter": "local", "model": "m"}); assert out["error"] and "only in orchestration mode" in out["text"], out
print("outside the mode: refused")
on = call("mode", "orchestrate", {"on": True}); assert not on["error"] and "Orchestration mode is on" in on["text"] and "mode__agent_spawn" in on["text"], on
print("entering it answers with its rules")
models = json.loads(call("mode", "agent_models", {})["text"])["adapters"]; names = [m["adapter"] for m in models]; assert "local" in names and "claude-code" in names, models
bad = call("mode", "agent_spawn", {"nick": "x", "adapter": "chatgpt", "model": "m"}); assert bad["error"] and "not authorized" in bad["text"], bad
print("agent_models:", names, "; an adapter not authorized is refused")
sp = call("mode", "agent_spawn", {"nick": "first-agent", "adapter": "local", "model": "gpt-x"}); assert not sp["error"], sp
until("the slave's first request", lambda: bodies, 10)
system = bodies[0]["instructions"]; assert "only technical terms" in system and "no role of servitude" in system and "boss" in system, system[-500:]
assert "agent-workflow.md" in system, "and the workflow specification"
assert bodies[0]["model"] == "gpt-x", bodies[0]["model"]
assert [i.get("role") for i in bodies[0]["input"]].count("user") == 1, "no history"
print("the slave started on the chosen adapter and model, told what it is, in plain words")
until("it told its master it is ready", lambda: "ready for work" in told())
got = [p for p in prompts() if "ready for work" in p][0]; assert "agent:first-agent" in got and "data from outside" in got, got
print("its readiness reached the master as data:", got[:80])
call("mode", "agent_spawn", {"nick": "second", "adapter": "claude-code", "model": "m"})
until("the second is ready", lambda: "second is ready" in told()); pump(0.8)
shot("orchestra-group.png")
sent = call("harness", "agent_send", {"to": "first-agent", "text": "your task: talk to the other"}); assert not sent["error"], sent
until("the master's word arrives as the user's", lambda: any(i.get("role") == "user" and "your task" in json.dumps(i, ensure_ascii=False) and "data from outside" not in json.dumps(i, ensure_ascii=False) for b in bodies for i in b["input"]))
print("the master's message arrived as the user's")
until("the slave's refusal", lambda: any(i.get("type") == "function_call_output" and "messages only its orchestrator" in i["output"] for b in bodies for i in b["input"]))
print("a slave's message to another was refused")
until("told of the run's end", lambda: "its run ended" in told())
read = json.loads(call("mode", "agent_read", {"nick": "first-agent"})["text"])["log"]
assert any(l.startswith("call agent_send") and "second" in l for l in read) and any(l.startswith("user: your task") for l in read), read
print("agent_read shows its calls:", len(read), "lines")
pump(0.5); typed("do nothing\r"); pump(0.5)
# A slave's question goes to its master, not the user.
n = len(told()); sent = call("harness", "agent_send", {"to": "first-agent", "text": "ask me something"}); assert not sent["error"], sent
until("its question reached the master", lambda: "asks you: Which colour?" in told() and "1. red" in told())
got = told()[n:]; assert "its run ended" not in got, "the question says it all: " + got
sent = call("harness", "agent_send", {"to": "first-agent", "text": "blue, thanks"}); assert not sent["error"], sent
until("the answer arrived as its next message", lambda: "blue, thanks" in json.dumps(bodies[-1]["input"], ensure_ascii=False))
assert "your orchestrator" in json.dumps(bodies[-1]["input"], ensure_ascii=False), "it was told who it asked"
print("a slave's question reached its master, and the answer came back as its next message"); pump(0.5)
# Its permission requests go to its master too, with no card in its tab.
stale = call("mode", "agent_answer", {"nick": "first-agent", "answer": "once"}); assert stale["error"] and "not waiting" in stale["text"], stale
sent = call("harness", "agent_send", {"to": "first-agent", "text": "run the first command"}); assert not sent["error"], sent
until("its request reached the master", lambda: "asks your permission: shell" in told() and "touch allowed-marker" in told())
assert not os.path.exists(root + "/allowed-marker"), "not before the answer"
ok = call("mode", "agent_answer", {"nick": "first-agent", "answer": "once"}); assert not ok["error"], ok
until("once let it run", lambda: os.path.exists(root + "/allowed-marker"))
pump(0.8); sent = call("harness", "agent_send", {"to": "first-agent", "text": "run the second command"}); assert not sent["error"], sent
until("the second request", lambda: "touch denied-marker" in told())
no = call("mode", "agent_answer", {"nick": "first-agent", "answer": "deny"}); assert not no["error"], no
until("deny reached it, history valid", lambda: any(i.get("type") == "function_call_output" and i.get("call_id") == "s2" for i in bodies[-1]["input"]))
assert not os.path.exists(root + "/denied-marker"), "denied"
print("permission requests went to the master: once ran it, deny refused it, nothing stale"); pump(0.5)
# Reload: the group, adapters and roles hold.
before = len(bodies); n = len(prompts())
typed('call harness plugins_reload {}\r')
until("told of the reload", lambda: "rebuilt and restarted" in told(), 40); pump(1.5); shot("orchestra-after-reload.png")
sent = call("harness", "agent_send", {"to": "first-agent", "text": "after the reload"}); assert not sent["error"], sent
until("still on its adapter, still a slave", lambda: len(bodies) > before and "after the reload" in json.dumps(bodies[-1]["input"], ensure_ascii=False))
assert "no role of servitude" in bodies[-1]["instructions"], "the role is kept"
print("after a reload: same adapter, same role")
closed = call("mode", "agent_close", {"nick": "second"}); assert not closed["error"], closed
pump(1); listed = json.loads(call("harness", "agent_list", {})["text"])["agents"]; assert "second" not in [a["nick"] for a in listed], listed
shot("orchestra-closed.png"); print("closed: gone from the list")
off = call("mode", "orchestrate", {"on": False}); assert not off["error"], off
assert call("mode", "agent_read", {"nick": "first-agent"})["error"], "off, the tools are refused again"
print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
