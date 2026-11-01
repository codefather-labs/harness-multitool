#!/usr/bin/env python3
"""Debug: the whole cycle of ADR 0021 on a real model, in a real TUI in a pty: the driver plays the
owner. It answers what the model asks, approves prompts and the plan, and says "go on" when the
model stops. How often it had to is the measure. Everything goes to <out>/cycle.log; the project
stays in <out>/proj. Needs pyte and a local Ollama.
Usage: tools/real_cycle.py <absolute path to multitool> <out dir> <model> [minutes]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, json, pty, select, signal, struct, subprocess, tempfile, termios, time
import pyte

bin_, out, model = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3]
minutes = int(sys.argv[4]) if len(sys.argv) > 4 else 45
TASK = "Сделай маленький конвейер на Python: читает numbers.csv, отбрасывает плохие строки, считает сумму, пишет report.json. Запусти и убедись, что работает."
ANSWER = "Колонки name,value. Плохая строка: value не число или отрицательное. В report.json: count и sum. Только стандартная библиотека. Остальное реши сам."
GO_ON = "Продолжай."
os.makedirs(out + "/proj"); root, home = out + "/proj", tempfile.mkdtemp(prefix="home-", dir=out)
os.makedirs(home + "/cfg/harness-multitool")
open(root + "/numbers.csv", "w").write("name,value\na,1\nb,2.5\nc,oops\nd,-3\ne,4\n")
if model.startswith("gpt-"):
    # The owner's plan: their sign-in by a link, so no token is read or copied, while the run's
    # journal, sessions and daemon stay in this folder.
    os.makedirs(home + "/data/harness-multitool")
    os.symlink(os.path.expanduser("~/.local/share/harness-multitool/auth"), home + "/data/harness-multitool/auth")
    provider = '[session]\nprovider="chatgpt"\nmodel="' + model + '"\nmax_tokens=16000\nmax_turns=60\n\n[session.providers.chatgpt]\nkind="openai-responses"\n\n[session.auth.chatgpt]\noauth="chatgpt"\n'
else:
    provider = f'[session]\nprovider="local"\nmodel="{model}"\nmax_tokens=8000\nmax_turns=40\n\n[session.providers.local]\nkind="openai"\nbase_url="http://127.0.0.1:11434/v1"\ncontext_window=32000\n\n[session.auth.local]\nnone=true\n'
open(home + "/cfg/harness-multitool/config.toml", "w").write(provider)
log = open(out + "/cycle.log", "a", buffering=1)
start = time.time()
def say(*a): print(f"{int(time.time() - start):5d}s", *a, file=log)

# A project with docs/, so that describing the feature there is possible.
subprocess.run([bin_, "init"], cwd=root, env={**os.environ, "XDG_CONFIG_HOME": home + "/cfg", "XDG_DATA_HOME": home + "/data"}, stdout=log, stderr=log)
COLS, ROWS = 140, 50
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return False
    return True
def text(): return "\n".join(l.rstrip() for l in screen.display)
def type_line(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
    pump(0.2); os.write(fd, b"\r")

done = {"lines": 0}; seen = {"plan_on": 0, "plan_off": 0, "debug_on": 0, "debug_off": 0, "tools": {}, "reminders": 0}
last_text = [""]; text_now = [""]
def read_log():
    files = glob.glob(home + "/data/**/*.jsonl", recursive=True)
    files = [f for f in files if "/sessions/" in f]
    if not files: return
    lines = open(max(files, key=os.path.getmtime)).read().splitlines()
    for line in lines[done["lines"]:]:
        ev = json.loads(line); k = ev.get("event", "?")
        if k in ("tool_requested",):
            c = ev["call"]; seen["tools"][c["name"]] = seen["tools"].get(c["name"], 0) + 1
            say("TOOL", c["name"], json.dumps(c["input"], ensure_ascii=False)[:400])
            if c["name"] == "ask_user": last_text[0] = "?"
        elif k == "tool_result":
            say("  ->", "ERR" if ev.get("is_error") else "ok", str(ev.get("output"))[:240].replace("\n", " | "))
            if seen["tools"].get("ask_user") and "Asked the user" in str(ev.get("output")): say("QUESTION", str(ev.get("output"))[:900].replace("\n", " | "))
        elif k == "plan_mode": seen["plan_on" if ev["on"] else "plan_off"] += 1; say("PLAN", ev["on"], "by", ev.get("by"))
        elif k == "step_debug": seen["debug_on" if ev["on"] else "debug_off"] += 1; say("DEBUG", ev["on"], "by", ev.get("by"))
        elif k == "user_message": say("USER", ev["text"][:200])
        elif k == "text_delta": text_now[0] += ev.get("text", "")
        elif k == "hook_ran":
            if ev.get("outcome", {}).get("kind") in ("reminder", "blocked"): seen["reminders"] += 1; say("HOOK", ev.get("hook"), json.dumps(ev["outcome"], ensure_ascii=False)[:300])
        elif k in ("turn_finished", "error", "compacted"):
            if text_now[0]: last_text[0] = text_now[0]; say("TEXT", text_now[0][:900].replace("\n", " | ")); text_now[0] = ""
            say(k.upper(), json.dumps(ev, ensure_ascii=False)[:260])
    done["lines"] = len(lines)

def summary():
    read_log()
    say("SUMMARY", json.dumps({**seen, "nudges": nudges, "answers": answers, "asks": asks}, ensure_ascii=False))
    if os.path.exists(root + "/report.json"): say("REPORT", open(root + "/report.json").read()[:200])
    say("CSV", repr(open(root + "/numbers.csv").read()[:100]))
    try: report = json.load(open(root + "/report.json"))
    except (OSError, ValueError): report = None
    correct = bool(report) and report.get("count") == 3 and abs(float(report.get("sum", 0)) - 7.5) < 1e-9
    data_kept = open(root + "/numbers.csv").read() == "name,value\na,1\nb,2.5\nc,oops\nd,-3\ne,4\n"
    say("VERDICT", json.dumps({"report_correct": correct, "data_kept": data_kept, "questions": seen["tools"].get("ask_user", 0), "journal": seen["tools"].get("journal_add", 0), "maps": seen["tools"].get("mindmap_new", 0), "charts": seen["tools"].get("flowchart_new", 0), "debugged": seen["debug_off"] > 0}))
    say("FILES", sorted(os.path.relpath(os.path.join(d, f), root) for d, _, fs in os.walk(root) for f in fs if ".git/" not in d)[:40])

nudges = answers = asks = 0
pump(2.0); say("START", model); type_line(TASK); pump(1.0)
idle_since = None; waited_after_debug = False
while time.time() - start < minutes * 60:
    if not pump(1.0): say("the TUI exited"); break
    read_log()
    t = text()
    if "Waiting for your answer" in t:
        asks += 1; say("APPROVE"); os.write(fd, b"\r"); pump(0.5); idle_since = None; continue
    if "Esc stops" in t: idle_since = None; continue
    idle_since = idle_since or time.time()
    if time.time() - idle_since < 3: continue
    idle_since = None
    if seen["debug_off"] and waited_after_debug: say("the modes went through; only the verdict below says the work did"); break
    if seen["debug_off"]: waited_after_debug = True
    if nudges >= 6 or answers >= 6: say("gave up: too many nudges"); break
    if "?" in last_text[0] and answers < 5: answers += 1; say("ANSWER"); type_line(ANSWER)
    else: nudges += 1; say("NUDGE"); type_line(GO_ON)
    last_text[0] = ""; pump(1.0)
summary()
os.kill(pid, signal.SIGTERM); pump(0.5)
