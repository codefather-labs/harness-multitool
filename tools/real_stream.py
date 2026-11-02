#!/usr/bin/env python3
"""Debug: Claude Code as one live process, for real, through the harness (journal: delegate stream). A
long tool is on; a message sent meanwhile reaches the model in that turn. The process stays the same
across messages, and goes when the window closes. Spends a little of the owner's plan. Needs pyte and
--features claude-code.
Usage: tools/real_stream.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, select, shutil, signal, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-st-root")); home = tempfile.mkdtemp(prefix="mt-st-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="claude-code"\nmodel="claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind="claude-code"\nenvironment="user"\npermissions="skip"\nchrome=false\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}; env.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
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
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
def children():
    daemon = json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"]
    out = subprocess.run(["ps", "-o", "pid=,args=", "--ppid", str(daemon)], capture_output=True, text=True).stdout
    return [int(l.split()[0]) for l in out.splitlines() if "stream-json" in l]
def until(what, check, sec=90):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.2)
    assert check(), what + ": " + text()
def idle(): return "Esc stops" not in text()
try:
    pump(2); typed("Run in bash exactly: sleep 20 ; echo one . Then run in bash exactly: echo two . Then reply with the word FINISHED.\r")
    until("the long tool is on", lambda: "sleep 20" in text()); first = children(); assert len(first) == 1, first; pump(1)
    typed("Also say the word EXTRA-WORD when you finish.\r"); pump(1); print("a message sent during the tool")
    until("the turn ended", lambda: "FINISHED" in text() and idle(), 80); pump(1)
    assert "EXTRA-WORD" in text().split("FINISHED")[-1] or "EXTRA-WORD" in text()[text().rfind("ASSISTANT"):], "it reached the model in that turn: " + text()
    print("it reached the model in that turn; requests in the log:", sum(1 for l in open(glob.glob(home + "/data/**/sessions/**/*.jsonl", recursive=True)[0]) if '"user_message"' in l))
    assert children() == first, "the same process"
    typed("Say the word SECOND-ANSWER.\r"); until("second answer", lambda: "SECOND-ANSWER" in text()[text().rfind("ASSISTANT"):] and idle(), 60)
    assert children() == first, "the same process after a second message"; print("one process across messages:", first)
    os.kill(pid, signal.SIGTERM); pump(0.5)
    try: os.waitpid(pid, 0)
    except OSError: pass
    end = time.time() + 45
    while time.time() < end and children(): time.sleep(0.5)
    print("after the window went:", children()); assert not children(), "the process goes with the window"
    print("ok")
finally:
    if "mt-st-root" in folder: shutil.rmtree(folder, ignore_errors=True)
