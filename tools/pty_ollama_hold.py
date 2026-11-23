#!/usr/bin/env python3
"""Debug, CON-190 point 11 (journal#1686): an Ollama model is held by the sessions that use it and
unloaded when the last one ends, not by time. The real TUI with a daemon of its own, on this
machine's Ollama and a small model:
  1. tab 1 and tab 2 each send a message: the model is loaded;
  2. both stay open past Ollama's default 5 idle minutes (IDLE seconds): still loaded;
  3. tab 2 closed (Ctrl+W): still loaded, tab 1 holds it;
  4. tab 1 closed: unloaded;
  5. a new tab sends a message, the daemon is killed with SIGKILL: still loaded (nobody let go);
     a new daemon starts: it unloads what the dead one held.
Needs pyte and Ollama with MODEL pulled. Usage: tools/pty_ollama_hold.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, shutil, signal, struct, subprocess, tempfile, termios, time, urllib.request
import pyte

B = sys.argv[1]
MODEL = os.environ.get("MODEL", "gemma4:e2b")
IDLE = int(os.environ.get("IDLE", "330"))
OLLAMA = "http://127.0.0.1:11434"
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-hold-proof")); os.makedirs(proof_dir, exist_ok=True)
log = open(os.path.join(proof_dir, "steps.txt"), "w")
def say(line):
    line = time.strftime("%H:%M:%S ") + line; print(line); log.write(line + "\n"); log.flush()

def loaded():
    with urllib.request.urlopen(OLLAMA + "/api/ps") as r:
        return [(m["name"], m["expires_at"]) for m in json.load(r)["models"] if m["name"] == MODEL]

root = tempfile.mkdtemp(prefix="mt-hold-root"); home = tempfile.mkdtemp(prefix="mt-hold-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "ollama"\nmodel = "{MODEL}"\n\n[session.providers.ollama]\nkind = "ollama"\nbase_url = "{OLLAMA}"\ncontext_window = 32768\n\n[session.auth.ollama]\nnone = true\n\n[behaviour]\non_exit = "stop"\n')
ENV = dict(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
data = home + "/data/harness-multitool"

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(ENV); os.execv(B, [B, "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
screen = pyte.Screen(120, 40); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def typed(t):
    for c in t: os.write(fd, c.encode()); time.sleep(0.003)
def until(cond, sec):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(0.5)
    return cond()
def daemon_pid():
    try: return json.load(open(data + "/daemon/address.json"))["pid"]
    except Exception: return None
def answered(n):
    # The daemon's sessions write their logs; a finished turn is a TurnFinished line in one of them.
    count = 0
    for dirpath, _, files in os.walk(data + "/sessions"):
        for f in files:
            if f.endswith(".jsonl"):
                count += sum(1 for line in open(os.path.join(dirpath, f)) if '"turn_finished"' in line)
    return count >= n

results = {}
until(lambda: "Enter send" in text(), 30)
say(f"before: {loaded()}")
typed("Say hi in two words."); os.write(fd, b"\r")
results["tab1_answered"] = until(lambda: answered(1), 240); say(f"tab 1 answered: {results['tab1_answered']}, {loaded()}")
os.write(fd, b"\x14"); pump(2)  # Ctrl+T
until(lambda: "Enter send" in text(), 20)
typed("Say bye in two words."); os.write(fd, b"\r")
results["tab2_answered"] = until(lambda: answered(2), 240); say(f"tab 2 answered: {results['tab2_answered']}, {loaded()}")
results["loaded_with_two"] = bool(loaded())
say(f"idle {IDLE} s with both tabs open")
pump(IDLE)
results["still_loaded_after_idle"] = bool(loaded()); say(f"after idle: {loaded()}")
os.write(fd, b"\x17"); pump(4)  # Ctrl+W: tab 2
results["loaded_after_one_closed"] = bool(loaded()); say(f"tab 2 closed: {loaded()}")
open(os.path.join(proof_dir, "screen-one-tab.txt"), "w").write(text())
os.write(fd, b"\x17")  # tab 1
results["unloaded_after_last_closed"] = until(lambda: not loaded(), 20); say(f"tab 1 closed: {loaded()}")
open(os.path.join(proof_dir, "screen-closed.txt"), "w").write(text())

# A daemon that dies holding the model.
os.write(fd, b"\x14"); pump(2)
until(lambda: "Enter send" in text(), 20)
typed("Say ok."); os.write(fd, b"\r")
until(lambda: answered(3), 240); say(f"tab 3 answered, {loaded()}")
dp = daemon_pid(); say(f"SIGKILL daemon {dp}")
assert dp, "no daemon address"
os.kill(dp, signal.SIGKILL); time.sleep(1)
results["loaded_after_daemon_killed"] = bool(loaded()); say(f"after SIGKILL: {loaded()}")
ledger = open(data + "/ollama-held.json").read() if os.path.exists(data + "/ollama-held.json") else "(none)"
say(f"ledger left: {ledger}")
try: os.kill(pid, signal.SIGTERM)
except Exception: pass
time.sleep(1)
d = subprocess.Popen([B, "daemon"], env={**os.environ, **ENV}, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
results["unloaded_by_next_daemon"] = until(lambda: not loaded(), 30); say(f"next daemon started: {loaded()}")
d.terminate()
try: out = d.communicate(timeout=10)[0]
except Exception: d.kill(); out = d.communicate()[0]
say("next daemon said: " + " | ".join(l for l in out.splitlines() if "unloaded" in l or "daemon on" in l))
subprocess.run(["ollama", "stop", MODEL], capture_output=True)
shutil.rmtree(root, ignore_errors=True); shutil.rmtree(home, ignore_errors=True)
say("RESULTS: " + json.dumps(results))
print("ALL PASSED" if all(results.values()) else "FAILED")
