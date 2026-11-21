#!/usr/bin/env python3
"""Debug, journal#1543 point 2: a real model on Ollama (MODEL, gemma4:e2b by default) asked, in the real TUI,
to use each family of the harness's tools and each mode, as a user would ask it. A second window is another
agent, for agent_send. After each ask, the session's log says which tools the model called and what each
answered. Permission prompts are answered 'Always allow', and a plan's approval card 'Approve'.

What the model leaves uncalled is a model's limit, reported as such; a call the harness answers with
'unknown tool' or no result is a fault. SLOW multiplies every wait, for a big model.
Needs pyte. Usage: tools/pty_ollama_live.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, glob, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

B = sys.argv[1]
MODEL = os.environ.get("MODEL", "gemma4:e2b")
SLOW = int(os.environ.get("SLOW", "1"))
ONLY = os.environ.get("ONLY", "")
proof_dir = os.environ.get("PROOF_DIR", tempfile.mkdtemp(prefix="mt-oll-live-proof")); os.makedirs(proof_dir, exist_ok=True)
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-oll-live-root")); home = tempfile.mkdtemp(prefix="mt-oll-live-home")
os.makedirs(home + "/cfg/harness-multitool")
open(root + "/note.txt", "w").write("alpha is the word, and the code is LIME-31.\n")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session]\nprovider = "ollama"\nmodel = "{MODEL}"\n\n[session.providers.ollama]\nkind = "ollama"\nbase_url = "http://127.0.0.1:11434"\n\n[session.auth.ollama]\nnone = true\n')
COLS, ROWS = 140, 44

class Page(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = b"<html><body><h1>Status</h1><p>The deploy code is MANGO-88.</p></body></html>"
        self.send_response(200); self.send_header("content-type", "text/html"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
page = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Page); threading.Thread(target=page.serve_forever, daemon=True).start()

class Window:
    def __init__(self, *args):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", CLAUDE_CONFIG_DIR=home + "/claude", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
            os.execv(B, [B, *args])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.screen = pyte.Screen(COLS, ROWS); self.stream = pyte.ByteStream(self.screen)
        self.approvals = 0
        self.questions_answered = {}
    def pump(self, sec):
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.stream.feed(os.read(self.fd, 65536))
                except OSError: return
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display)
    def status(self): return next((l for l in self.screen.display if "ADAPTER" in l), "")
    def busy(self): return "Esc stops" in self.status()
    def keys(self, s):
        for c in s: os.write(self.fd, c.encode()); time.sleep(0.003)
        self.pump(0.4)
    def answer_prompts(self):
        t = self.text()
        # A question of the agent's (ask_user), still unanswered: its first option.
        entries = log()
        asked = [e for e in entries if e.get("event") == "tool_requested" and e["call"]["name"] == "ask_user"]
        if asked:
            cid = asked[-1]["call"]["id"]
            answered = any(e.get("event") == "tool_result" and e.get("id") == cid for e in entries) or any(e.get("event") == "user_message" and e.get("seq", 0) > asked[-1].get("seq", 0) for e in entries)
            if not answered and self.questions_answered.get(cid, 0) < 3:
                self.questions_answered[cid] = self.questions_answered.get(cid, 0) + 1
                os.write(self.fd, b"\r"); self.pump(1.0); return True
        if "Always allow" in t:
            os.write(self.fd, b"2"); self.approvals += 1; self.pump(0.6); return True
        if "Approve the plan" in t or ("Approve" in t and "plan" in t.lower() and "Change" in t):
            os.write(self.fd, b"\r"); self.approvals += 1; self.pump(0.8); return True
        return False
    def ask(self, text, sec=240):
        self.keys(text + "\r")
        end = time.time() + sec * SLOW
        started = False
        while time.time() < end:
            self.pump(0.5)
            if self.answer_prompts(): continue
            if self.busy(): started = True
            elif started:
                self.pump(1.5)
                if not self.busy() and not self.answer_prompts(): return True
        return False

def log():
    out = []
    for p in glob.glob(home + "/data/harness-multitool/sessions/*/*.jsonl"):
        out += [json.loads(l) for l in open(p) if l.strip()]
    return out

def calls_since(mark):
    """The tool calls after the log's first `mark` entries, with what each answered."""
    entries = log()[mark:]
    asked = {e["call"]["id"]: e["call"] for e in entries if e.get("event") == "tool_requested"}
    done = {e["id"]: e for e in entries if e.get("event") == "tool_result"}
    return [(c["name"], c.get("input"), done.get(i, {}).get("output"), done.get(i, {}).get("is_error")) for i, c in asked.items()]

ASKS = [
    ("files", ["read", "search"], "Read the file note.txt with the read tool, then use the search tool to find the word alpha in this project. Tell me the code written in note.txt."),
    ("shell+edit", ["shell", "edit"], "Use the shell tool to run `ls -la`. Then use the edit tool to replace the word alpha with gamma in note.txt."),
    ("fetch", ["fetch"], f"Use the fetch tool to get http://127.0.0.1:{page.server_port}/status and tell me the deploy code on that page."),
    ("journal write", ["journal_add"], "Use journal_add twice: first a decision (category decision, tags [\"ollama\"]) saying 'We run local models on Ollama'; then a todo (category plan, tags [\"docs\"], flags [\"todo\"]) saying 'Write the README'."),
    ("journal read", ["journal_search", "journal_get", "journal_agenda", "journal_list", "journal_tags"], "Use journal_search for Ollama, then journal_get on the entry it finds, then journal_agenda, then journal_list, then journal_tags. Tell me what is open."),
    ("journal links", ["journal_mark", "journal_link", "journal_branch"], "Use journal_mark to flag the decision entry important, journal_link to link the decision to the todo, and journal_branch on the todo."),
    ("maps", ["mindmap_new", "mindmap_add", "flowchart_new", "flowchart_add", "map_edit", "map_node_state", "map_close"], "Make a mind map with mindmap_new titled 'local models', and add one node with mindmap_add. Then make a flowchart with flowchart_new titled 'run a model' and add one process step with flowchart_add. Then use map_edit to rename your mind map node, map_node_state to mark it done, and map_close to close the mind map."),
    ("harness", ["skill_list", "callback_url"], "Call skill_list, then callback_url, and tell me the callback URL."),
    ("harness 2", ["set_nick", "agent_list"], "Call set_nick with the nick local-gemma, then call agent_list and tell me who is online."),
    ("harness 3", ["plugins_list"], "Call plugins_list and tell me which hooks are installed."),
    ("debug mode", ["mode__debug"], "Turn step-debug mode on with mode__debug (on true), then turn it off again with mode__debug (on false)."),
    ("plan mode", ["mode__plan_enter", "mode__plan_exit"], "Enter plan mode with mode__plan_enter. Plan adding a README: record a journal decision with journal_add, make a mind map with mindmap_new and one node, and a flowchart with flowchart_new and one step. Then call mode__plan_exit with your plan."),
    ("a2a", ["agent_send"], "Use agent_send to send the agent named peer the message: hello from the Ollama agent."),
    ("orchestrate", ["mode__orchestrate", "mode__agent_spawn", "mode__agent_read", "mode__agent_close"], "Enter orchestration mode with mode__orchestrate (on true). Use mode__agent_spawn to start an agent with nick helper, adapter ollama, model MODEL, on the open todo entry about the README as its task. Then use mode__agent_read on helper, then mode__agent_close on helper, then mode__orchestrate (on false).".replace("MODEL", MODEL)),
]

results = []
w = Window("--new")
end = time.time() + 30
while time.time() < end and "Enter send" not in w.text(): w.pump(0.3)
peer = None
for name, expected, prompt in ASKS:
    if ONLY and name not in ONLY.split(","): continue
    if name == "a2a" and peer is None:
        peer = Window("--new")
        e2 = time.time() + 30
        while time.time() < e2 and "Enter send" not in peer.text(): peer.pump(0.3)
        peer.keys("/nick peer\r"); peer.pump(2)
    mark = len(log())
    finished = w.ask(prompt)
    got = calls_since(mark)
    names = [g[0] for g in got]
    missing = [e for e in expected if e not in names]
    faults = [g for g in got if g[2] is None or "unknown tool" in str(g[2]).lower()]
    errors = [(g[0], str(g[2])[:120]) for g in got if g[3]]
    results.append({"ask": name, "finished": finished, "called": names, "missing": missing, "faults": [f[0] for f in faults], "errors": errors})
    print(f"\n== {name}: called {names}\n   missing {missing} | harness faults {[f[0] for f in faults]} | tool errors {errors}")
    open(proof_dir + f"/screen-{name.replace(' ', '_')}.txt", "w").write(w.text())
if peer is not None:
    peer.pump(3)
    a2a_seen = "hello from the Ollama agent" in peer.text() or any("hello from the Ollama agent" in json.dumps(e) for e in log())
    print("\npeer received the message:", a2a_seen)
    results.append({"ask": "a2a received", "ok": a2a_seen})
    open(proof_dir + "/screen-peer.txt", "w").write(peer.text())
json.dump(results, open(proof_dir + "/live-results.json", "w"), indent=1)
for x in (w, peer):
    if x: os.kill(x.pid, signal.SIGKILL)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception: pass
faults = [r for r in results if r.get("faults")]
print("\nHARNESS FAULTS:", faults)
print("MODEL MISSES:", [(r["ask"], r["missing"]) for r in results if r.get("missing")])
