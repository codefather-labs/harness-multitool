#!/usr/bin/env python3
"""Debug: Ctrl+C on real Claude Code through the harness, in a real TUI (CON-86). A long command in the foreground
and a queued message: the stop reaches it through its own stream, the command ends, the card says stopped, the
same process goes on and answers the queued message, the window stays. A second run: two presses cancel the answer
as well. Spends a little of the owner's plan. Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/real_interrupt.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, select, struct, subprocess, tempfile, termios, time

bin_ = sys.argv[1]
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-in-root")); home = tempfile.mkdtemp(prefix="mt-in-home"); os.makedirs(home + "/cfg/harness-multitool")
open(root + "/loop.sh", "w").write("#!/bin/sh\necho $$ > pid\nwhile true; do /usr/bin/env sleep 0.5; done\n")
open(home + "/cfg/harness-multitool/config.toml", "w").write('[session]\nprovider="claude-code"\nmodel="claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind="claude-code"\nenvironment="user"\npermissions="skip"\n')
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}; env.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
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
def events():
    out = []
    for name in glob.glob(home + "/data/**/*.jsonl", recursive=True):
        if "sessions" in name:
            for line in open(name, errors="replace"):
                try: out.append(json.loads(line))
                except ValueError: pass
    return sorted(out, key=lambda e: e.get("seq", 0))
def said(after=0): return "".join(e.get("text", "") for e in events() if e.get("event") == "text_delta" and e.get("seq", 0) >= after)
def stopped_cards(): return [e for e in events() if e.get("event") == "tool_result" and e.get("output") == "Stopped by the user."]
def delegates():
    daemon = json.load(open(glob.glob(home + "/data/**/daemon/address.json", recursive=True)[0]))["pid"]
    lines = subprocess.run(["ps", "-o", "pid=,args=", "--ppid", str(daemon)], capture_output=True, text=True).stdout
    return [int(l.split()[0]) for l in lines.splitlines() if "claude -p" in l or "/claude " in l]
def tool_on():
    try: return os.path.exists("/proc/" + open(root + "/pid").read().strip())
    except (OSError, ValueError): return False
def until(what, check, sec=60):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + json.dumps(events())[-800:]
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
    pump(0.3)
ASK = "Run with the Bash tool in the foreground, exactly this command and nothing else: sh loop.sh\r"
pump(2)
typed(ASK)
until("the command runs", tool_on)
before = delegates()
typed("Say only the word PLUM.\r"); pump(0.5)
start = time.time()
os.write(fd, b"\x03")
until("the card says stopped", lambda: stopped_cards(), 10)
took = time.time() - start
print(f"stopped in {took:.2f} s")
assert took < 3, took
until("the command is gone", lambda: not tool_on(), 15)
until("the queued message is answered", lambda: "PLUM" in said(), 60)
assert delegates() == before, (before, delegates())
print("the command ended, the same process answered the queued message:", before)
os.remove(root + "/pid"); pump(1.5)
mark = events()[-1]["seq"]
typed(ASK)
until("the command runs again", tool_on)
typed("Say only the word PEAR.\r"); pump(0.5)
os.write(fd, b"\x03"); pump(0.2); os.write(fd, b"\x03")
pump(15)
assert "PEAR" not in said(mark), "the second press did not cancel it: " + said(mark)
done, _ = os.waitpid(pid, os.WNOHANG)
assert done == 0, "the window closed"
print("two presses cancelled the answer too; the window is there"); print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.5)
