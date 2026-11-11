#!/usr/bin/env python3
"""Debug: a reload an agent asked for, with the user's next message on its way, in a real TUI in a pty.
The swap waits for that message's run, no delegate outlives the old daemon, the new process does not
refuse the conversation as held by the harness's own child, the agent is told, and quitting afterwards
gives the terminal back as it was. Needs pyte and --features claude-code.
Usage: tools/pty_reload_busy.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, struct, tempfile, termios, time
import pyte

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-rb-root")); home = tempfile.mkdtemp(prefix="mt-rb-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, subprocess, sys, os, time
args = sys.argv[1:]
config = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["harness"]
def note(**what):
    open({log!r}, "a").write(json.dumps(dict(what, pid=os.getpid(), at=time.time())) + "\\n")
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-9", "model": "fake-model"}}), flush=True)
while True:
    line = sys.stdin.readline()
    if not line:
        note(eof=True)
        break
    prompt = json.loads(line)["message"]["content"][0]["text"]
    note(prompt=prompt)
    print(json.dumps({{"type": "user", "isReplay": True, "message": {{"role": "user", "content": [{{"type": "text", "text": prompt}}]}}}}), flush=True)
    if "do reload" in prompt:
        subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": "plugins_reload", "arguments": {{}}}}}}) + "\\n", capture_output=True, text=True)
        time.sleep(2.5)
    if "slow second" in prompt:
        time.sleep(4)
    note(done=prompt)
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard: " + prompt[:40]}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
bin_ = sys.argv[1]; COLS, ROWS = 130, 46
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", MULTITOOL_RELOAD_BUILD="true")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "startup"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def notes(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def until(what, check, sec=30):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ":\n" + text() + "\n" + json.dumps(notes())[-900:]
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
def fakes():
    found = []
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                if fake in open(f"/proc/{p}/cmdline").read() and "R" != "": found.append(int(p))
            except OSError: pass
    return found
def alive(p):
    try: return open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()[0] != "Z"
    except OSError: return False
def swapped(): return "--handoff" in open(f"/proc/{pid}/cmdline").read()

pump(1.5); os.write(fd, b"r"); pump(2)
typed("do reload now\r")
until("the model heard it", lambda: any((n.get("prompt") or "").endswith("do reload now") for n in notes()))
pump(0.5)
typed("slow second\r")
swap_at = None
end = time.time() + 40
while time.time() < end and not swap_at:
    pump(0.02)
    if swapped(): swap_at = time.time()
assert swap_at, "no swap:\n" + text() + "\n" + json.dumps(notes())[-900:]
heard_second = [n for n in notes() if n.get("prompt") == "slow second"]
done_second = [n for n in notes() if n.get("done") == "slow second"]
assert heard_second, "the second message never reached the model before the swap: " + json.dumps(notes())[-900:]
assert done_second and done_second[0]["at"] <= swap_at, f"swapped during a run: swap {swap_at}, " + json.dumps(notes())[-900:]
print(f"the swap waited for the run: {swap_at - done_second[0]['at']:.2f} s after it")
old = {n["pid"] for n in notes()}
pump(1.0)
left = [p for p in old if alive(p)]
assert not left, f"delegates outlive the old daemon: {left}\n" + text()
print("no delegate of the old daemon is left")
until("the agent is told by itself", lambda: any("rebuilt and restarted" in (n.get("prompt") or "") for n in notes()))
told = [n for n in notes() if "rebuilt and restarted" in (n.get("prompt") or "")][0]
assert told["pid"] not in old, told
until("its answer on screen", lambda: "Heard: [from the harness]" in text())
assert "open in Claude Code" not in text(), text()
print("the agent was told and answered; nothing refused")
pump(0.5)
typed("/exit\r")
end = time.time() + 10; gone = False
while time.time() < end and not gone:
    pump(0.05)
    done, _ = os.waitpid(pid, os.WNOHANG)
    gone = done == pid
assert gone, "/exit did not quit:\n" + text()
attrs = termios.tcgetattr(fd)
iflag, oflag, cflag, lflag = attrs[0], attrs[1], attrs[2], attrs[3]
assert lflag & termios.ECHO and lflag & termios.ICANON, f"no echo or line mode after quitting: lflag {lflag:o}"
assert oflag & termios.OPOST and oflag & termios.ONLCR, f"staircase after quitting: oflag {oflag:o}"
print("the terminal is back: echo, line mode, newline translation"); print("ok")
