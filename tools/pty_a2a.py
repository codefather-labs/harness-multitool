#!/usr/bin/env python3
"""Debug: two agents talking through the daemon (CON-99), in a real TUI with fake delegates, one to a tab. One lists,
sends, is answered; what cannot be sent is refused with why; the sender's name is the harness's to give. Reads no
screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_a2a.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, struct, tempfile, termios, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-a2-root")); home = tempfile.mkdtemp(prefix="mt-a2-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"; results = home + "/results"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
config = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["harness"]
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
def tool(name, arguments):
    done = subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": name, "arguments": arguments}}}}) + "\\n", capture_output=True, text=True)
    reply = json.loads(done.stdout.strip().split("\\n")[-1])["result"]
    open({results!r}, "a").write(json.dumps({{"tool": name, "error": reply["isError"], "text": reply["content"][0]["text"]}}) + "\\n")
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid(), "prompt": prompt}}) + "\\n")
    for said in prompt.split("\\n"):
        if said.startswith("call "):
            name, arguments = said[5:].split(" ", 1)
            tool(name, json.loads(arguments))
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(sys.argv[1], [sys.argv[1], "preview", "welcome"])
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
def prompts(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def answers(): return [json.loads(l) for l in open(results)] if os.path.exists(results) else []
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + json.dumps(prompts())[-500:] + " " + json.dumps(answers())[-500:]
def last(): return answers()[-1]
def call(name, arguments):
    n = len(answers()); typed("call " + name + " " + json.dumps(arguments) + "\r")
    until(name + " answered", lambda: len(answers()) > n)
    return last()
pump(1.5)
typed("/nick orchid\r"); typed("first in one\r"); until("one runs", lambda: any("first in one" in p["prompt"] for p in prompts())); pump(0.5)
typed("/tab two\r"); pump(0.5)
typed("/nick lotus\r"); typed("first in two\r"); until("two runs", lambda: any("first in two" in p["prompt"] for p in prompts())); pump(0.8)
one = next(p["pid"] for p in prompts() if "first in one" in p["prompt"]); two = next(p["pid"] for p in prompts() if "first in two" in p["prompt"])
listed = call("agent_list", {}); agents = json.loads(listed["text"])["agents"]
assert sorted(a["nick"] for a in agents) == ["lotus", "orchid"], agents
assert all(a["adapter"] == "claude-code" and a["folder"] == root for a in agents), agents
print("agent_list:", [(a["nick"], a["adapter"], a["running"]) for a in agents])
# Two, in this tab, sends to one: it reaches the other process as data with the sender's name.
sent = call("agent_send", {"to": "ORCHID", "text": "from lotus, hello"}); assert not sent["error"], sent
until("it reached the other", lambda: any(p["pid"] == one and "from lotus, hello" in p["prompt"] for p in prompts()))
got = next(p["prompt"] for p in prompts() if p["pid"] == one and "from lotus, hello" in p["prompt"])
assert "lotus" in got and got != "from lotus, hello", "marked, with the sender's name: " + got
assert not any(p["pid"] == two and "from lotus, hello" in p["prompt"] for p in prompts() if not p["prompt"].startswith("call")), "not to itself"
print("delivered, marked:", got[:110].replace("\n", " "))
pump(0.5)
# An answer the other way: the first tab, told by Alt+1.
os.write(fd, b"\x1b1"); pump(0.4)
answer = call("agent_send", {"to": "lotus", "text": "from orchid, back"}); assert not answer["error"], answer
until("the answer came back", lambda: any(p["pid"] == two and "from orchid, back" in p["prompt"] for p in prompts()))
print("answered the other way"); pump(0.5)
for bad, why in [({"to": "nobody", "text": "x"}, "no open conversation"), ({"to": "orchid", "text": "x"}, "that is you"), ({"to": "lotus", "text": "  "}, "text is required")]:
    refused = call("agent_send", bad); assert refused["error"] and why in refused["text"], refused; pump(0.2)
print("refused with why: unknown, itself, empty")
n = 0
while True:
    reply = call("agent_send", {"to": "lotus", "text": f"flood {n}"}); n += 1
    if reply["error"]: break
    assert n < 70, "no limit"
    pump(0.05)
assert "the limit" in reply["text"], reply
print(f"the limit stopped it after {n - 1} more, 60 in all"); print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
