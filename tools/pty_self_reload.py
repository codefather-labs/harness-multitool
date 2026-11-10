#!/usr/bin/env python3
"""Debug: an agent reloads the harness itself, in a real TUI in a pty, with a fake delegate that calls
plugins_reload through the MCP server it was given. A failed build comes back to it as its next message.
A good one swaps the process, and the agent gets a message by itself, with nothing typed. Needs pyte and
--features claude-code.
Usage: tools/pty_self_reload.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, signal, struct, tempfile, termios, time
import pyte

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-sr-root")); home = tempfile.mkdtemp(prefix="mt-sr-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/prompts"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, subprocess, sys, os
args = sys.argv[1:]
config = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["harness"]
resume = args[args.index("--resume") + 1] if "--resume" in args else None
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-9", "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid(), "resume": resume, "prompt": prompt}}) + "\\n")
    if "do reload" in prompt:
        mcp = subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": "plugins_reload", "arguments": {{}}}}}}) + "\\n", capture_output=True, text=True)
        open({log!r}, "a").write(json.dumps({{"tool": mcp.stdout[:300], "err": mcp.stderr[:200]}}) + "\\n")
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard: " + prompt[:40]}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
bin_ = sys.argv[1]; COLS, ROWS = 130, 46
def start(build, page="welcome"):
    env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", MULTITOOL_RELOAD_BUILD=build)
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", page])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return pid, fd
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def prompts(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def until(what, check, sec=25):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.1)
    assert check(), what + ": " + text() + "\n" + json.dumps(prompts())[-600:]
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
pid, fd = start("echo the-build-broke >&2; exit 3"); pump(1.5)
typed("do reload please\r")
until("a failed build comes back", lambda: any("failed" in p.get("prompt", "") and "the-build-broke" in p.get("prompt", "") for p in prompts()))
assert any(p.get("tool", "").find("Building") >= 0 for p in prompts()), prompts()
assert "--handoff" not in open(f"/proc/{pid}/cmdline").read(), "nothing swapped"
print("a failed build came back to the agent, nothing swapped")
os.kill(pid, signal.SIGTERM); pump(0.5); os.waitpid(pid, 0); time.sleep(0.5)
os.remove(log)
screen.reset(); pid, fd = start("true", "startup"); pump(1.5); os.write(fd, b"r"); pump(10)
typed("do reload again\r")
until("the process is replaced", lambda: "--handoff" in open(f"/proc/{pid}/cmdline").read())
until("the agent is told by itself", lambda: any("rebuilt and restarted" in p.get("prompt", "") for p in prompts()))
told = [p for p in prompts() if "rebuilt and restarted" in p.get("prompt", "")][0]; first = prompts()[0]
assert told["pid"] != first["pid"] and told["resume"] == "sess-9", (told, first)
assert "[from the harness]" in told["prompt"], told
until("its card", lambda: "HARNESS" in text() and "Heard: [from the harness]" in text())
print("the process was replaced and the agent told by itself:", told["prompt"][:70].replace("\n", " ")); print("ok")
os.kill(pid, signal.SIGTERM); pump(0.3)
