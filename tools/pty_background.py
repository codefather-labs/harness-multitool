#!/usr/bin/env python3
"""Debug: Claude Code starts a turn by itself, between our messages, as it does when a background task
ends: the harness reads it at once, with nothing typed, its next message runs to its own result, and a
background task reaches the log. Reads the session's log, not the screen, so it needs no pyte. Needs
--features claude-code.
Usage: tools/pty_background.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, re, select, struct, tempfile, termios, time

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-bg-root")); home = tempfile.mkdtemp(prefix="mt-bg-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, sys, threading, time
out_lock = threading.Lock()
def say(**what):
    with out_lock: print(json.dumps(what), flush=True)
def text(words): say(type="stream_event", event=dict(type="content_block_delta", delta=dict(type="text_delta", text=words)))
def result(): say(type="result", is_error=False, stop_reason="end_turn", total_cost_usd=0.0, usage=dict(input_tokens=999999, output_tokens=5))
def by_itself():
    time.sleep(2)
    say(type="system", subtype="task_notification", task_id="t1", status="completed", summary="Background command finished")
    say(type="system", subtype="background_tasks_changed", tasks=[])
    time.sleep(0.5)
    say(type="system", subtype="init", session_id="sess-9", model="fake-model")
    text("I noticed by myself")
    time.sleep(1)
    say(type="assistant", message=dict(content=[], usage=dict(input_tokens=42, output_tokens=1)))
    result()
say(type="system", subtype="init", session_id="sess-9", model="fake-model")
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    say(type="user", isReplay=True, message=dict(role="user", content=[dict(type="text", text=prompt)]))
    open({log!r}, "a").write(prompt + "\\n")
    if "start task" in prompt:
        say(type="system", subtype="background_tasks_changed", tasks=[dict(task_id="t1", description="a long job", task_type="local_bash")])
        threading.Thread(target=by_itself, daemon=True).start()
    text("Heard: " + prompt)
    result()
''')
os.chmod(fake, 0o755)
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
bin_ = sys.argv[1]
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
drawn = ""
def pump(sec):
    global drawn
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: raw = os.read(fd, 65536)
            except OSError: return
            # What was written to the screen, with the styling and the moves taken out.
            drawn = (drawn + re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", raw.decode("utf8", "replace")))[-200000:]
def events():
    out = []
    for name in glob.glob(home + "/data/**/*", recursive=True):
        if os.path.isfile(name) and name.endswith(".jsonl"):
            for line in open(name, errors="replace"):
                try: out.append(json.loads(line))
                except ValueError: pass
    return sorted(out, key=lambda e: e.get("seq", 0))
def until(what, check, sec=20):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + json.dumps(events())[-1500:]
def said(): return "".join(e.get("text", "") for e in events() if e.get("event") == "text_delta")
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.005)
pump(1.5)
typed("start task\r")
until("the first answer", lambda: "start task" in said())
until("the task is in the log", lambda: any("a long job" in json.dumps(e) for e in events()))
until("the panel on the screen", lambda: "in background" in drawn and "a long job" in drawn[drawn.index("in background"):])
print("the task reached the log and the panel:", drawn[drawn.index("in background") - 4:][:40].replace("\n", " "))
until("the turn it began by itself, with nothing typed", lambda: "I noticed by myself" in said())
print("the turn it began by itself was read")
until("the task's end", lambda: any("Background command finished" in json.dumps(e) for e in events()))
pump(1.5)
typed("second message\r")
until("the next message's answer", lambda: "Heard: second message" in said())
assert said().index("I noticed by myself") < said().index("Heard: second message"), said()
usage = [e["usage"] for e in events() if e.get("event") == "turn_finished"]
assert any(u["input"] == 42 for u in usage), ("the last call's own prompt, not the result's sum", usage)
print("the next message ran after it, to its own result; tokens are the last call's"); print("ok")
typed("/exit\r"); pump(0.5)
