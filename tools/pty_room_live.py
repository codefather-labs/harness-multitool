#!/usr/bin/env python3
"""Live, CON-203: a room with real agents. The orchestrator is the scripted fake Claude Code of pty_room.py; its two agents run on
Ollama (MODEL, gemma4:e2b by default). It opens a room on a question, and the agents talk in it on their own; then it closes it.
The original pty_room.py header follows.

Debug, CON-203: a room (a group chat of agents) in a real TUI. The master and its two agents are
fake Claude Code programs: each logs every prompt it gets, and runs the lines 'call <server> <tool>
<json>' it finds in one. The master opens a room with ana and bob; every post reaches every member
but its author; a reply to one member reaches all, marked for its addressee; an agent cannot open a
room; the close writes a summary entry and tells the members. Needs --features claude-code; reads no
screen. Usage: tools/pty_room.py <absolute path to multitool> [a directory for the proofs]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, select, shutil, struct, tempfile, termios, time
import pyte

root = os.path.realpath(tempfile.mkdtemp(prefix="mt-room-root")); home = tempfile.mkdtemp(prefix="mt-room-home"); os.makedirs(home + "/cfg/harness-multitool")
log = home + "/log"; fake = home + "/fake-claude"; results = home + "/results"; keep = sys.argv[2] if len(sys.argv) > 2 else home
os.makedirs(keep, exist_ok=True)
open(fake, "w").write(f'''#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
servers = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "sess-" + str(os.getpid()), "model": "fake-model"}}), flush=True)
def tool(server, name, arguments):
    config = servers[server]
    print(json.dumps({{"type": "assistant", "message": {{"id": "m" + name, "role": "assistant", "content": [{{"type": "tool_use", "id": "t" + name, "name": "mcp__" + server + "__" + name, "input": arguments}}]}}}}), flush=True)
    done = subprocess.run([config["command"]] + config["args"], input=json.dumps({{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {{"name": name, "arguments": arguments}}}}) + "\\n", capture_output=True, text=True)
    reply = json.loads(done.stdout.strip().split("\\n")[-1])
    reply = reply.get("result") or {{"isError": True, "content": [{{"text": json.dumps(reply)}}]}}
    open({results!r}, "a").write(json.dumps({{"pid": os.getpid(), "tool": name, "error": reply["isError"], "text": reply["content"][0]["text"]}}) + "\\n")
for line in sys.stdin:
    prompt = json.loads(line)["message"]["content"][0]["text"]
    open({log!r}, "a").write(json.dumps({{"pid": os.getpid(), "prompt": prompt}}) + "\\n")
    for said in prompt.split("\\n"):
        said = said.strip()
        if said.startswith("call "):
            server, name, arguments = said[5:].split(" ", 2)
            tool(server, name, json.loads(arguments))
    if "Your task is journal#" in prompt: tool("harness", "agent_send", {{"to": "boss", "text": "ready"}})
    print(json.dumps({{"type": "result", "is_error": False, "stop_reason": "end_turn", "total_cost_usd": 0.0, "usage": {{}}}}), flush=True)
''')
os.chmod(fake, 0o755)
MODEL = os.environ.get("MODEL", "gemma4:e2b")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\nenvironment="user"\npermissions="skip"\n\n[session.providers.ollama]\nkind="ollama"\nbase_url="http://127.0.0.1:11434"\ncontext_window=32768\n\n[session.auth.ollama]\nnone=true\n')
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(sys.argv[1], [sys.argv[1], "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
seen = bytearray()
screen = pyte.Screen(130, 46); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try:
                got = os.read(fd, 65536); seen.extend(got); stream.feed(got)
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def typed(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.002)
    pump(0.4)
def runs(): return [json.loads(l) for l in open(log)] if os.path.exists(log) else []
def answers(): return [json.loads(l) for l in open(results)] if os.path.exists(results) else []
def prompts_of(who): return [r["prompt"] for r in runs() if r["pid"] == who]
def until(what, check, sec=25):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    if not check(): open(os.path.join(keep, "screen-raw.txt"), "wb").write(bytes(seen[-20000:]))
    assert check(), what + "\nlast prompts: " + json.dumps([r["prompt"][:160] for r in runs()[-6:]], ensure_ascii=False) + "\nanswers: " + json.dumps(answers()[-4:], ensure_ascii=False)
def call(server, name, arguments):
    n = len(answers()); typed(f"call {server} {name} {json.dumps(arguments)}\r")
    until(name + " answered", lambda: len(answers()) > n); pump(0.3)
    return answers()[-1]
def via(nick, server, name, arguments):
    """Has the agent `nick` call a tool: the master's agent_send reaches it as the user's words."""
    n = len(answers()); call("harness", "agent_send", {"to": nick, "text": f"call {server} {name} {json.dumps(arguments)}"})
    until(f"{nick}'s {name} answered", lambda: any(a["tool"] == name for a in answers()[n:])); pump(0.3)
    return [a for a in answers()[n:] if a["tool"] == name][-1]
results_ok = {}
def check(name, ok, detail=""):
    results_ok[name] = bool(ok); print(("ok   " if ok else "FAIL ") + name + (": " + detail if detail and not ok else ""))
end = time.time() + 30
while time.time() < end and "Enter send" not in seen[-20000:].decode("utf-8", "replace"): pump(0.3)
typed("/nick boss\r"); pump(0.3); typed("first\r"); until("the master runs", lambda: runs()); pump(0.5)
assert not call("mode", "orchestrate", {"on": True})["error"]
for nick in ["ana", "bob"]:
    task = json.loads(call("harness", "journal_add", {"body": f"{nick}: take part in the room your orchestrator opens about a cache for this project. Wait for its invitation; then say your view there with room_post.", "category": "todo", "tags": ["room"]})["text"])
    sp = call("mode", "agent_spawn", {"nick": nick, "adapter": "ollama", "model": MODEL, "task": task["id"]}); assert not sp["error"], sp
    pump(2)
rooms_dir = home + "/data/harness-multitool/rooms"
def room_lines(): return [json.loads(l) for f in (os.listdir(rooms_dir) if os.path.isdir(rooms_dir) else []) for l in open(os.path.join(rooms_dir, f))]
def posts_by(nick): return [l["message"] for l in room_lines() if "message" in l and l["message"]["author_nick"] == nick]
pump(30)
opened = call("mode", "room_open", {"topic": "cache", "members": ["ana", "bob"], "goal": "agree on a cache policy for a service whose hot set is 1 GB and which sometimes scans everything"})
check("the room opens", not opened["error"], opened["text"])
first = call("harness", "room_post", {"text": "Question for both: LRU, LFU or something else for a 1 GB hot set with occasional full scans? Say why in two sentences, and answer each other where you disagree."})
check("the question is posted", not first["error"], first["text"])
end = time.time() + int(os.environ.get("WAIT", "600"))
while time.time() < end and not (posts_by("ana") and posts_by("bob") and len(room_lines()) >= 5): pump(2)
pump(60)
for nick in ["ana", "bob"]:
    check(f"{nick} posts in the room by itself", posts_by(nick), json.dumps(room_lines())[-400:])
replies = [l["message"] for l in room_lines() if "message" in l and l["message"].get("to_nick")]
print("addressed replies:", len(replies))
os.write(fd, b"\x1b4"); pump(1.5)
open(os.path.join(keep, "room-tab-live.txt"), "w").write(text())
os.write(fd, b"\x1b1"); pump(1.5)  # back to the orchestrator's tab before its next call
shutil.copytree(home + "/data/harness-multitool/sessions", os.path.join(keep, "sessions"), dirs_exist_ok=True)
closed = call("mode", "room_close", {"summary": "Live room on " + MODEL + ": " + "; ".join(f"{m['author_nick']}: {m['text'][:160]}" for l in room_lines() if 'message' in l for m in [l['message']] if m['author_nick'] != 'boss')})
check("the master closes it with a summary entry", not closed["error"] and "journal#" in closed["text"], closed["text"])
open(os.path.join(keep, "room-log-live.jsonl"), "w").write("\n".join(json.dumps(l, ensure_ascii=False) for l in room_lines()) + "\n")
try: os.kill(pid, 15)
except Exception: pass
time.sleep(0.5)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], 15)
except Exception as e: print("daemon stop:", e)
time.sleep(3)
shutil.rmtree(root, ignore_errors=True); shutil.rmtree(home, ignore_errors=True)
print("\nRESULTS:", json.dumps(results_ok))
print("ALL PASSED" if all(results_ok.values()) else "FAILED")
