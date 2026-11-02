#!/usr/bin/env python3
"""Debug: a reload whose build really replaces the binary, as cargo does when the code changed: a new
file at the same path, so the running process's own path reads as deleted. The window swaps to the new
binary and the agent is told. Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_reload_replaced.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, shutil, struct, tempfile, termios, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-rr-root")); home = tempfile.mkdtemp(prefix="mt-rr-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"; here = home + "/bin"; os.makedirs(here)
bin_ = here + "/multitool"; shutil.copy2(sys.argv[1], bin_)
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, signal, subprocess, sys, os
def asked(*_):
    open({log!r} + ".signals", "a").write("term\\n")
    sys.exit(0)
signal.signal(signal.SIGTERM, asked)
args = sys.argv[1:]
config = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["harness"]
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-9", "model": "fake-model"}}), flush=True)
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid(), "prompt": prompt}}) + "\\n")
    if "do reload" in prompt:
        subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": "plugins_reload", "arguments": {{}}}}}}) + "\\n", capture_output=True, text=True)
    print(json.dumps({{"type": "stream_event", "event": {{"type": "content_block_delta", "delta": {{"type": "text_delta", "text": "Heard"}}}}}}), flush=True)
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
# A new file at the same path, as the linker leaves it.
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
            try: seen = (seen + os.read(fd, 65536))[-4000:]
            except OSError: return
def prompts(): return [json.loads(l)["prompt"] for l in open(log)] if os.path.exists(log) else []
def exited():
    done, status = os.waitpid(pid, os.WNOHANG)
    return status if done == pid else None
pump(1.5)
for c in "do reload now\r": os.write(fd, c.encode()); time.sleep(0.005)
told = False; end = time.time() + 40
while time.time() < end and not told:
    pump(0.1)
    status = exited()
    assert status is None, f"the window exited ({status}) instead of swapping: " + seen[-400:].decode("utf8", "replace")
    told = any("rebuilt and restarted" in p for p in prompts())
assert told, "the agent was not told: " + json.dumps(prompts())
assert "--handoff" in open(f"/proc/{pid}/cmdline").read(), "not swapped"
assert os.stat(f"/proc/{pid}/exe").st_ino == os.stat(bin_).st_ino, "it runs the old binary"
assert os.path.exists(log + ".signals"), "the old delegate was killed without being asked to end"
children = open(f"/proc/{pid}/task/{pid}/children").read().split()
defunct = [c for c in children if os.path.exists(f"/proc/{c}/stat") and open(f"/proc/{c}/stat").read().rsplit(")", 1)[1].split()[0] == "Z"]
assert not defunct, f"defunct children under the window: {defunct}"
print("swapped to the new file, and the agent was told; the old delegate was asked to end, nothing defunct"); print("ok")
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.005)
pump(0.5)
