#!/usr/bin/env python3
"""A reference for what an agent builds in a target app in step-debug mode (ADR 0013): a small
pipeline with a debug router by the contract. Without MULTITOOL_DEBUG_MODE=1 the debug code is
inert: nothing is sent and no step waits. With it, each step sends flowstate_update and waits for
approve, with no timeout. Env: MULTITOOL_CALLBACK_URL, MULTITOOL_DEBUG_TOKEN, PIPELINE_MAP,
PIPELINE_NODES (comma-separated chart nodes), PIPELINE_PORT_FILE."""
import http.server, json, os, threading, urllib.request, uuid

GATE = os.environ.get("MULTITOOL_DEBUG_MODE") == "1"
TOKEN = os.environ.get("MULTITOOL_DEBUG_TOKEN", "")
state = {"waiting": False, "instance": "pass-1", "node": 0}
go = threading.Semaphore(0)

class Router(http.server.BaseHTTPRequestHandler):
    def answer(self, code, body):
        b = json.dumps(body).encode(); self.send_response(code); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def authorized(self):
        if TOKEN and self.headers.get("authorization") == f"Bearer {TOKEN}": return True
        self.answer(401, {"error": "bad token"}); return False
    def do_GET(self):
        if self.path != "/debug/status": return self.answer(404, {})
        if self.authorized(): self.answer(200, state)
    def do_POST(self):
        if self.path != "/debug/approve": return self.answer(404, {})  # no step back here: no route
        if not self.authorized(): return
        if not state["waiting"]: return self.answer(409, {"error": "nothing is waiting"})
        state["waiting"] = False; self.answer(200, state); go.release()
    def log_message(self, *a): pass

def step(node, data):
    print(f"step {node}", flush=True)
    if not GATE: return
    state.update(node=node, waiting=True)
    body = {"uuid": str(uuid.uuid4()), "type": "flowstate_update", "map": int(os.environ["PIPELINE_MAP"]), "node": node, "instance": state["instance"], "state": "entered", "data": data}
    urllib.request.urlopen(urllib.request.Request(os.environ["MULTITOOL_CALLBACK_URL"], json.dumps(body).encode(), method="POST")).read()
    go.acquire()  # no timeout, no default action (CON-29)

if GATE:
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Router)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    open(os.environ["PIPELINE_PORT_FILE"], "w").write(str(server.server_address[1]))
for i, node in enumerate(int(n) for n in os.environ.get("PIPELINE_NODES", "1,2").split(",")):
    step(node, {"step": i})
print("finished", flush=True)
