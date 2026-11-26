#!/usr/bin/env python3
"""Debug, CON-203: a room (a group chat of agents) in a real TUI. The master and its two agents are
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
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\nplan_first="off"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\nenvironment="user"\npermissions="skip"\n')
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
typed("/nick boss\r"); pump(0.3); typed("\r") if "/nick boss" in seen[-3000:].decode("utf-8", "replace") else None; typed("first\r"); until("the master runs", lambda: runs()); pump(0.5)
boss = runs()[0]["pid"]
assert not call("mode", "orchestrate", {"on": True})["error"]
pids = {}
for nick in ["ana", "bob"]:
    task = json.loads(call("harness", "journal_add", {"body": f"{nick}'s part of the brainstorm", "category": "todo", "tags": ["t"]})["text"])
    sp = call("mode", "agent_spawn", {"nick": nick, "adapter": "claude-code", "model": "m", "task": task["id"]}); assert not sp["error"], sp
    until(f"{nick} is ready", lambda: any(f"Your task is journal#{task['id']}" in r["prompt"] for r in runs()))
    pids[nick] = [r["pid"] for r in runs() if f"Your task is journal#{task['id']}" in r["prompt"]][0]
    pump(0.5)
print("agents:", pids)

opened = call("mode", "room_open", {"topic": "cache", "members": ["ana", "bob"], "goal": "pick a cache"})
check("the master opens a room", not opened["error"] and "is open with you and ana, bob" in opened["text"], opened["text"])
for nick in ["ana", "bob"]:
    until(f"{nick} is invited", lambda: any("invited to the room 'cache'" in p for p in prompts_of(pids[nick])))
invite = [p for p in prompts_of(pids["ana"]) if "invited to the room" in p][-1]
check("the invitation says how to take part", "room_post" in invite and "Silence is fine" in invite and "pick a cache" in invite, invite[:300])

first = call("harness", "room_post", {"text": "which cache fits a 1 GB working set?"})
check("the master posts", not first["error"] and "Posted #1" in first["text"], first["text"])
for nick in ["ana", "bob"]:
    until(f"{nick} gets #1", lambda: any("room:cache #1 boss" in p and "1 GB working set" in p for p in prompts_of(pids[nick])))
check("every member but the author gets it", not any("room:cache #1 boss" in p for p in prompts_of(boss)))

reply = via("ana", "harness", "room_post", {"text": "LRU, with a scan guard", "reply_to": "boss"})
check("an agent posts a reply to one member", not reply["error"] and "Posted #2" in reply["text"], reply["text"])
until("the master gets it marked for it", lambda: any("room:cache #2 ana → you" in p for p in prompts_of(boss)))
until("bob gets it too, marked for the master", lambda: any("room:cache #2 ana → boss" in p for p in prompts_of(pids["bob"])))
check("a reply is seen by all and marked for its addressee", True)

by_number = via("bob", "harness", "room_post", {"text": "scans would evict it", "reply_to": "#2"})
check("a reply by message number", not by_number["error"])
until("ana gets the reply to her message", lambda: any("room:cache #3 bob → you" in p for p in prompts_of(pids["ana"])))

# The room's tab (R): its feed, and the user's word to every member.
until("the room's tab is in the bar", lambda: "cache (R)" in text(), 10)
check("the room is a tab of the group marked (R)", "cache (R)" in text())
os.write(fd, b"\x1b4"); pump(1.0)  # Alt+4: boss, ana, bob, cache
open(os.path.join(keep, "room-tab.txt"), "w").write(text())
check("the room's tab shows the feed with authors and addressees", "#1 boss" in text() and "#2 ana → boss" in text() and "#3 bob → ana" in text(), text()[-800:])
typed("keep it simple\r")
for nick in ["ana", "bob"]:
    until(f"{nick} gets the user's word", lambda: any("[room cache #4, the user] keep it simple" in p for p in prompts_of(pids[nick])))
until("the master gets it too", lambda: any("[room cache #4, the user] keep it simple" in p for p in prompts_of(boss)))
pump(0.5)
check("the user's message reaches every agent as the user's", "#4 you" in text(), text()[-600:])
os.write(fd, b"\x1b1"); pump(1.0)  # back to the master's tab
read = json.loads(call("harness", "room_read", {})["text"])
check("room_read gives the room so far", [m["n"] for m in read["messages"]] == [1, 2, 3, 4] and read["room"]["members"] == ["boss", "ana", "bob"], json.dumps(read)[:300])

no_room = via("bob", "mode", "room_open", {"topic": "mine", "members": ["ana"]})
check("an agent of an orchestra cannot open a room", no_room["error"], no_room["text"])

closed = call("mode", "room_close", {"summary": "LRU with a scan guard; bob's scan concern kept as a test."})
check("the master closes it with a summary entry", not closed["error"] and "its summary is journal#" in closed["text"], closed["text"])
entry = int(closed["text"].split("journal#")[1].split(".")[0])
got = json.loads(call("harness", "journal_get", {"id": entry})["text"])
check("the summary entry names the room and its members", "Summary of the room 'cache'" in got["body"] and "ana, bob" in got["body"] and "scan guard" in got["body"], got["body"][:200])
for nick in ["ana", "bob"]:
    until(f"{nick} is told it closed", lambda: any("is closed; its summary is journal#" in p for p in prompts_of(pids[nick])))
late = via("ana", "harness", "room_post", {"text": "one more thing"})
check("a closed room takes no post", late["error"], late["text"])

logs = os.listdir(home + "/data/harness-multitool/rooms")
lines = open(os.path.join(home + "/data/harness-multitool/rooms", logs[0])).read().splitlines()
os.write(fd, b"\x1b4"); pump(1.0)
check("the room's tab says it closed", "ROOM CLOSED" in text(), text()[-400:])
open(os.path.join(keep, "room-tab-closed.txt"), "w").write(text())
check("the room's log holds what was said", len(logs) == 1 and len(lines) == 6, f"{logs} {len(lines)}")
open(os.path.join(keep, "room-log.jsonl"), "w").write("\n".join(lines) + "\n")
open(os.path.join(keep, "prompts.jsonl"), "w").write("\n".join(json.dumps(r, ensure_ascii=False) for r in runs()) + "\n")

try: os.kill(pid, 15)
except Exception: pass
time.sleep(0.5)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], 15)
except Exception as e: print("daemon stop:", e)
time.sleep(0.5)
shutil.rmtree(root, ignore_errors=True); shutil.rmtree(home, ignore_errors=True)
print("\nRESULTS:", json.dumps(results_ok))
print("ALL PASSED" if all(results_ok.values()) else "FAILED")
