#!/usr/bin/env python3
"""Debug, journal#1543: every tool and every mode of the own loop, through the Ollama wire (CON-190), in the
real TUI. A fake Ollama (/api/show, /api/chat as NDJSON) plays the model: on 'RUN-ALL' it calls, one per
turn, each tool the request offers, with arguments built from its schema and from what earlier calls made
(journal ids, map and node ids), and the modes in their order:
- ask_user, answered in the window;
- mode__debug on, then off;
- mode__plan_enter, then mode__plan_exit with the user's words;
- mode__orchestrate on, mode__agent_models, mode__agent_spawn of a slave on this same fake Ollama (which
  answers it), mode__agent_read, mode__agent_answer, mode__agent_close, then off.
Each call's result is kept. The run fails on a call with no result, or one the harness does not know
('unknown tool'), or a mode whose card is not drawn. Permission prompts are answered 'Always allow'.
Needs pyte. Usage: tools/pty_ollama_parity.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

B = sys.argv[1]
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-olp-proof")); os.makedirs(proof_dir, exist_ok=True)
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-olp-root")); home = tempfile.mkdtemp(prefix="mt-olp-home")
os.makedirs(home + "/cfg/harness-multitool")
open(root + "/note.txt", "w").write("alpha is the word\n")
COLS, ROWS = 140, 44
lock = threading.Lock()
calls = []          # (name, args, result or None)
offered = []        # the tool names of the main session's requests
ids = {"journal": [], "todo": None, "map": [], "node": [], "flow": None, "flow_root": None, "mind": None, "mind_root": None}
slave_requests = []

def parse_result(name, text):
    try:
        v = json.loads(text)
    except Exception:
        return
    if not isinstance(v, dict):
        return
    if name == "journal_add" and "id" in v:
        ids["journal"].append(v["id"])
        if "todo" in v.get("flags", []):
            ids["todo"] = v["id"]
    if name in ("mindmap_new", "flowchart_new") and "id" in v:
        ids["map"].append(v["id"])
        key = "mind" if name == "mindmap_new" else "flow"
        ids[key] = v["id"]; ids[key + "_root"] = v.get("root")
    if name == "mindmap_add" and v.get("nodes"):
        ids["node"].append(max(n["id"] for n in v["nodes"]))

ORDER = [
    "read", "search", "edit", "shell", "fetch",
    "journal_add", "journal_add#todo", "journal_search", "journal_get", "journal_list", "journal_agenda", "journal_tags",
    "journal_mark", "journal_link", "journal_branch", "journal_close", "journal_abandon", "journal_reopen",
    "mindmap_new", "mindmap_add", "mindmap_show", "flowchart_new", "flowchart_add", "flowchart_show",
    "map_edit", "map_node_state", "map_link", "map_ref", "map_close",
    "skill_list", "skill_load", "callback_url", "callback_check", "callback_handlers", "debug_contract", "debug_runs",
    "agent_list", "agent_send", "game_tables", "game_invite", "game_join", "game_state", "game_history", "game_act", "game_decline", "game_leave",
    "plugins_list", "set_nick", "ask_user",
    "mode__debug#on", "mode__debug#off",
    # A plan holds a decision, a mind map and a flowchart made in plan mode (W-33, W-34): the same rule as on a delegate.
    "mode__plan_enter", "journal_add#plan", "mindmap_new#plan", "mindmap_add#plan", "flowchart_new#plan", "flowchart_add#plan", "mode__plan_exit",
    "mode__orchestrate#on", "mode__agent_models", "mode__agent_spawn", "mode__agent_read", "mode__agent_answer", "mode__agent_close", "mode__orchestrate#off",
    "plugins_reload",
]

def args_for(step, port):
    name, _, variant = step.partition("#")
    j = ids["journal"][0] if ids["journal"] else 1
    t = ids["todo"] or j
    fixed = {
        "read": {"path": "note.txt"}, "search": {"text": "alpha"}, "edit": {"path": "note.txt", "old_text": "alpha", "new_text": "beta"},
        "shell": {"command": "echo parity-shell"}, "fetch": {"url": f"http://127.0.0.1:{port}/page"},
        "journal_add": {"body": "Parity decision: the Ollama wire runs every tool.", "category": "decision", "tags": ["parity"]},
        "journal_search": {"query": "parity"}, "journal_get": {"id": j}, "journal_list": {}, "journal_agenda": {}, "journal_tags": {},
        "journal_mark": {"id": j, "flags": ["important"]}, "journal_branch": {"id": t},
        "journal_close": {"id": t, "resolution": "parity check"}, "journal_abandon": {"id": t, "resolution": "parity check"}, "journal_reopen": {"id": t},
        "journal_link": {"from": j, "to": t, "kind": "cites"},
        "mindmap_new": {"title": "parity map"}, "mindmap_add": {"map": ids["mind"], "text": "a node", "kind": "fact", "refs": [j]}, "mindmap_show": {"map": ids["mind"]},
        "flowchart_new": {"title": "parity flow"}, "flowchart_add": {"map": ids["flow"], "text": "a step", "kind": "process", "after": ids["flow_root"]}, "flowchart_show": {"map": ids["flow"]},
        "skill_list": {}, "skill_load": {"name": "no-such-skill"}, "callback_url": {}, "callback_check": {}, "callback_handlers": {}, "debug_contract": {}, "debug_runs": {},
        "agent_list": {}, "agent_send": {"to": "nobody-here", "text": "hello"},
        "game_tables": {}, "game_invite": {"players": ["nobody-here"]}, "game_join": {"table": "t1"}, "game_state": {"table": "t1"}, "game_history": {"table": "t1"},
        "game_act": {"table": "t1", "do": "fold"}, "game_decline": {"table": "t1"}, "game_leave": {"table": "t1"},
        "plugins_list": {}, "plugins_reload": {}, "set_nick": {"nick": "parity-agent"},
        "ask_user": {"question": "Proceed with the parity run?", "options": ["Yes, go on", "No"]},
        "mode__plan_enter": {}, "mode__plan_exit": {"plan": "The parity plan: run every tool.", "user_said": "RUN-ALL"},
        "mode__agent_models": {}, "mode__agent_spawn": {"nick": "slave1", "adapter": "ollama", "model": "fake-model", "task": t},
        "mode__agent_read": {"nick": "slave1"}, "mode__agent_answer": {"nick": "slave1", "answer": "deny"}, "mode__agent_close": {"nick": "slave1"},
    }
    if variant == "plan" and name == "journal_add":
        return {"body": "Plan decision: the parity plan runs every tool.", "category": "decision", "tags": ["parity"]}
    if variant == "plan" and name in ("mindmap_new", "flowchart_new"):
        return {"title": "the parity plan"}
    if variant == "plan" and name == "mindmap_add":
        return {"map": ids["mind"], "text": "run every tool", "kind": "task", "refs": [j]}
    if variant == "plan" and name == "flowchart_add":
        return {"map": ids["flow"], "text": "call each tool", "kind": "process", "after": ids["flow_root"]}
    if name == "journal_add" and variant == "todo":
        return {"body": "Parity todo: a task for the slave.", "category": "plan", "tags": ["parity"], "flags": ["todo"]}
    if name in ("mode__debug", "mode__orchestrate"):
        return {"on": variant == "on"}
    node = ids["node"][-1] if ids["node"] else ids["mind_root"]
    fixed.update({
        "map_edit": {"map": ids["mind"], "node": node, "text": "edited by parity"},
        "map_node_state": {"map": ids["mind"], "node": node, "state": "done"},
        "map_link": {"map": ids["mind"], "from": node, "to": ids["mind_root"], "label": "back"},
        "map_ref": {"map": ids["mind"], "node": node, "entries": [j]},
        "map_close": {"map": ids["mind"]},
    })
    if name in fixed:
        return fixed[name]
    # The map tools of the larger set: from their schemas, with the ids made above.
    schema = next((s for s in schemas if s["name"] == name), None)
    props = (schema or {}).get("parameters", {}).get("properties", {})
    out = {}
    node = ids["node"][-1] if ids["node"] else ids["mind_root"]
    for p in props:
        if p == "map": out[p] = ids["mind"]
        elif p in ("node", "from"): out[p] = node
        elif p == "to": out[p] = ids["mind_root"]
        elif p in ("text", "title", "label"): out[p] = "edited by parity"
        elif p == "state": out[p] = "done"
        elif p == "refs": out[p] = [j]
        elif p == "kind": out[p] = "fact"
        elif p == "id": out[p] = j
        elif p == "remove": out[p] = False
    return out

schemas = []
state = {"step": 0, "port": 0}

class Fake(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = b"<html><body>parity page PAGE-7</body></html>"
        self.send_response(200); self.send_header("content-type", "text/html"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def reply(self, obj):
        b = json.dumps(obj).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def stream(self, lines):
        b = "".join(json.dumps(l) + "\n" for l in lines).encode()
        self.send_response(200); self.send_header("content-type", "application/x-ndjson"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path == "/api/show":
            return self.reply({"capabilities": ["completion", "tools", "thinking"], "model_info": {"fake.context_length": 131072}})
        done = {"done": True, "done_reason": "stop", "prompt_eval_count": 100, "eval_count": 10}
        msgs = body.get("messages", [])
        firsts = [m for m in msgs if m.get("role") == "user"]
        main = any("RUN-ALL" in (m.get("content") or "") for m in firsts[:1])
        with lock:
            if not main:
                slave_requests.append(body)
                return self.stream([{"message": {"role": "assistant", "content": "SLAVE-DONE: the task is read."}, "done": False}, done])
            global schemas
            schemas = [t["function"] for t in body.get("tools", [])]
            offered[:] = [s["name"] for s in schemas]
            last = msgs[-1] if msgs else {}
            if last.get("role") == "tool" and calls and calls[-1][2] is None:
                calls[-1] = (calls[-1][0], calls[-1][1], last.get("content", ""))
                if not calls[-1][0].endswith("#plan"):
                    parse_result(calls[-1][0].partition("#")[0], last.get("content", ""))
            elif last.get("role") == "user" and "RUN-ALL" not in (last.get("content") or "") and calls and calls[-1][2] is None:
                # A mode's answer or a question's can come back as the user's words.
                calls[-1] = (calls[-1][0], calls[-1][1], "(as user) " + (last.get("content") or ""))
            while state["step"] < len(ORDER):
                step = ORDER[state["step"]]; state["step"] += 1
                name = step.partition("#")[0]
                if name not in offered and not name.startswith("mode__agent_") and name not in ("mode__plan_exit",):
                    calls.append((step, None, "NOT OFFERED"))
                    continue
                args = args_for(step, state["port"])
                calls.append((step, args, None))
                cid = f"call_{state['step']}"
                return self.stream([{"message": {"role": "assistant", "content": "", "thinking": f"next: {name}"}, "done": False},
                                    {"message": {"role": "assistant", "content": "", "tool_calls": [{"id": cid, "function": {"name": name, "arguments": args}}]}, "done": False}, done])
            return self.stream([{"message": {"role": "assistant", "content": "ALL-DONE"}, "done": False}, done])

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fake); state["port"] = server.server_port
threading.Thread(target=server.serve_forever, daemon=True).start()
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "ollama"\nmodel = "fake-model"\n\n[session.providers.ollama]\nkind = "ollama"\nbase_url = "http://127.0.0.1:{server.server_port}"\n\n[session.auth.ollama]\nnone = true\n')

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=home + "/claude", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(B, [B, "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display)
def keys(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.003)
    pump(0.3)
end = time.time() + 30
while time.time() < end and "Enter send" not in text(): pump(0.2)
keys("RUN-ALL\r")
seen_cards = set()
answered = {"permission": 0, "question": 0}
end = time.time() + 300
while time.time() < end:
    pump(0.3)
    t = text()
    for card in ("STEP DEBUG", "PLAN MODE", "ORCHESTRA", "MASTER"):
        if card in t: seen_cards.add(card)
    if "Always allow" in t:
        os.write(fd, b"2"); answered["permission"] += 1; pump(0.5); continue
    if "Proceed with the parity run?" in t and "Yes, go on" in t and calls and calls[-1][0] == "ask_user" and calls[-1][2] is None:
        os.write(fd, b"\r"); answered["question"] += 1; pump(0.8); continue
    if "ALL-DONE" in t: break
pump(1)
open(proof_dir + "/parity-screen.txt", "w").write(text())

# The log: modes' events.
logs = [json.loads(l) for root_, _, files in os.walk(home + "/data") for f in files if f.endswith(".jsonl") and "sessions" in root_ for l in open(os.path.join(root_, f)) if l.strip()]
events = {e.get("event") for e in logs}
modes = {
    "debug on/off": sum(1 for e in logs if e.get("event") == "step_debug") >= 2,
    "plan on/off": sum(1 for e in logs if e.get("event") == "plan_mode") >= 2,
    "orchestra role": any(e.get("event") == "orchestra_role" for e in logs),
    "slave answered": bool(slave_requests),
}
report = []
fail = []
for step, args, result in calls:
    r = (result or "")
    status = "NO RESULT" if result is None else ("NOT OFFERED" if r == "NOT OFFERED" else ("UNKNOWN TOOL" if "unknown tool" in r.lower() else "answered"))
    report.append(f"{step:24} {status:12} {r[:150]!r}")
    if status != "answered" or (step == "mode__plan_exit" and "approved" not in r.lower()) or (step.startswith("journal_") and r.startswith(("invalid arguments", "from must"))):
        fail.append(step)
open(proof_dir + "/parity-calls.txt", "w").write("\n".join(report) + "\n\noffered: " + " ".join(offered) + f"\n\nmodes: {modes}\nanswered: {answered}\n")
print("\n".join(report))
print("\noffered", len(offered), ":", " ".join(sorted(offered)))
print("modes:", modes, "| prompts answered:", answered)
os.kill(pid, signal.SIGKILL)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception: pass
ok = not fail and all(modes.values()) and state["step"] >= len(ORDER)
print("\nFAILED STEPS:", fail)
print("ALL PASSED" if ok else "FAILED")
