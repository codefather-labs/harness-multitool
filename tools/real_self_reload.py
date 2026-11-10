#!/usr/bin/env python3
"""Debug: a real Claude Code through the harness reloads the harness itself over MCP, with a test build
command: the process is replaced, the same conversation resumes, and the agent is told by itself and
answers, with nothing typed. Spends a little of the owner's plan. Needs pyte and --features claude-code.
Usage: tools/real_self_reload.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, shutil, signal, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; row = os.environ.get("ROW", 'environment="user"\npermissions="skip"\nchrome=false\n')
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-rr-root")); home = tempfile.mkdtemp(prefix="mt-rr-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind="claude-code"\n{row}')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}; env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", MULTITOOL_RELOAD_BUILD="true")
folder = os.path.expanduser("~/.claude/projects/") + "".join(c if c.isascii() and c.isalnum() else "-" for c in root)
COLS, ROWS = 130, 46
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
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
try:
    pump(2)
    for c in "Remember the word ORCHID. Call the tool mcp__harness__plugins_reload now, then end your turn saying ASKED. When the harness tells you it restarted, reply with the word you remembered and RESTARTED-OK.\r": os.write(fd, c.encode()); time.sleep(0.003)
    import glob
    def log():
        files = glob.glob(home + "/data/**/sessions/**/*.jsonl", recursive=True)
        return open(files[0]).read() if files else ""
    t0 = time.time()
    while time.time() - t0 < 60 and "--handoff" not in open(f"/proc/{pid}/cmdline").read(): pump(0.3)
    assert "--handoff" in open(f"/proc/{pid}/cmdline").read(), "replaced: " + text()
    print("the process was replaced"); t0 = time.time()
    def said(): return "".join(json.loads(l).get("text", "") for l in log().splitlines() if json.loads(l)["event"] == "text_delta")
    while time.time() - t0 < 90 and "RESTARTED-OK" not in said(): pump(0.5)
    assert "RESTARTED-OK" in said() and "ORCHID" in said().split("ASKED")[-1], said()[-500:]
    assert '"inbound"' in log() and '"harness"' in log(), "its card in the log"
    print("told by itself, answered with what it remembered:", said()[-60:].replace("\n", " ")); print("ok")
finally:
    os.kill(pid, signal.SIGTERM); pump(0.3)
    if "mt-rr-root" in folder: shutil.rmtree(folder, ignore_errors=True)
