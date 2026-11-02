#!/usr/bin/env python3
"""Debug: a real Claude Code through the harness uses the harness's own tools over MCP: it writes a
journal entry and a map into the harness's journal for the project, with nothing of any launcher in its
environment. Spends a little of the owner's plan. Needs pyte and --features claude-code.
Usage: tools/real_mcp.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, shutil, signal, struct, subprocess, tempfile, termios, time
import pyte

bin_ = sys.argv[1]; row = os.environ.get("ROW", 'environment="user"\npermissions="skip"\nchrome=false\n')
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-mc-root")); home = tempfile.mkdtemp(prefix="mt-mc-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="claude-haiku-5-5"\n\n[session.providers.claude-code]\nkind="claude-code"\n{row}')
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
try:
    pump(2)
    for c in "Using the tool mcp__harness__journal_add, add an entry with body MCP-WORD-77, category finding, tags [mcp]. Then using mcp__harness__mindmap_new make a map titled MCP-MAP-77. Then reply DONE. If you cannot, say what is missing.\r": os.write(fd, c.encode()); time.sleep(0.003)
    t0 = time.time(); pump(4)
    while time.time() - t0 < 120 and "Esc stops" in text(): pump(0.3)
    pump(1.5)
    import glob
    for line in open(glob.glob(home + "/data/**/sessions/**/*.jsonl", recursive=True)[0]):
        e = json.loads(line)
        if e["event"] in ("text_delta", "model_tool_call", "tool_result", "error"): print(e["event"], str(e.get("text") or e.get("call") or e.get("output") or e.get("message"))[:300].replace("\n", " "))
    out = subprocess.run([bin_, "journal", "export"], cwd=root, env=env, capture_output=True, text=True).stdout
    print("in the journal:", "MCP-WORD-77" in out, "map:", "MCP-MAP-77" in out)
    assert "MCP-WORD-77" in out and "MCP-MAP-77" in out, out[:500]
    print("ok")
finally:
    os.kill(pid, signal.SIGTERM); pump(0.3)
    if "mt-mc-root" in folder: shutil.rmtree(folder, ignore_errors=True)
