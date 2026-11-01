#!/usr/bin/env python3
"""Debug: real TUI in a pty, a fake model endpoint, and real callbacks over HTTP to the real daemon.
Prints the screen after each. Needs pyte. Usage: tools/pty_callbacks.py <absolute path to multitool>"""
import fcntl, http.server, json, os, pty, re, select, signal, struct, subprocess, sys, tempfile, termios, threading, time, urllib.request, urllib.error
import pyte
def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text, tool=None):
    ev=[{"type":"message_start","message":{"usage":{"input_tokens":500,"output_tokens":1}}},{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}},{"type":"content_block_stop","index":0}]
    if tool: ev+=[{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":tool[0],"name":tool[1],"input":{}}},{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":json.dumps(tool[2])}},{"type":"content_block_stop","index":1}]
    ev+=[{"type":"message_delta","delta":{"stop_reason":"tool_use" if tool else "end_turn"},"usage":{"output_tokens":20}},{"type":"message_stop"}]
    return sse(ev)
turns=[turn("Drawing the chart.",("t1","flowchart_new",{"title":"deploy","direction":"down"})),turn("Adding a step.",("t2","flowchart_add",{"map":1,"text":"build","kind":"process","after":1})),turn("Ready."),turn("I saw the build failure.")]
bodies=[]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length",0))).decode())
        b=turns.pop(0) if turns else turn("no more")
        self.send_response(200); self.send_header("content-type","text/event-stream"); self.send_header("content-length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
srv=http.server.ThreadingHTTPServer(("127.0.0.1",0),H); threading.Thread(target=srv.serve_forever,daemon=True).start()
root=tempfile.mkdtemp(prefix="mt-cb-root"); home=tempfile.mkdtemp(prefix="mt-cb-home"); os.makedirs(root+"/.harness"); open(root+"/.harness/conditions.toml","w").write('[[condition]]\nname = "pipeline keeps failing"\non = "iteration_failed"\nat_least = 3\npolicies = [".harness/policies/pipeline.md"]\n'); os.makedirs(home+"/cfg/harness-multitool")
open(home+"/cfg/harness-multitool/config.toml","w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{srv.server_port}"\n')
COLS,ROWS=110,44; bin_=sys.argv[1]
pid,fd=pty.fork()
if pid==0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home+"/cfg",XDG_DATA_HOME=home+"/data",ANTHROPIC_API_KEY="test-key-local-fake",TERM="xterm-256color",MULTITOOL_GRAPHICS="off",COLORTERM="truecolor")
    os.execv(bin_,[bin_,"preview","welcome"])
fcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack("HHHH",ROWS,COLS,0,0))
screen=pyte.Screen(COLS,ROWS); stream=pyte.ByteStream(screen)
def pump(sec):
    end=time.time()+sec
    while time.time()<end:
        r,_,_=select.select([fd],[],[],0.05)
        if r:
            try: stream.feed(os.read(fd,65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def show(t): print(f"\n===== {t} =====\n{text()}")
def post(url,obj):
    req=urllib.request.Request(url,json.dumps(obj).encode(),method="POST")
    try:
        with urllib.request.urlopen(req) as r: return r.status,json.load(r)
    except urllib.error.HTTPError as e: return e.code,json.load(e)
pump(1.5)
for c in "draw it\r": os.write(fd,c.encode()); time.sleep(0.005)
pump(2.0); show("the agent drew a chart")
system=json.loads(bodies[0]).get("system","")
url=re.search(r"http://127\.0\.0\.1:\d+/callback/[0-9a-f-]+",system).group(0); print("callback url from the system prompt:",url)
print("flow:",post(url,{"uuid":"f1","type":"flowstate_update","map":1,"node":2,"instance":"req-1","state":"entered"})); pump(0.8); show("flowstate_update: node n2 active")
print("journal:",post(url,{"uuid":"j1","type":"journal","text":"The nightly deploy finished.","category":"update"})); pump(0.8); show("journal callback")
print("unknown:",post(url,{"uuid":"x1","type":"deploy_done"}))
print("notification:",post(url,{"uuid":"n1","type":"notification","label":"ci","text":"build 42 failed"})); pump(2.0); show("notification: card and the agent's answer")
last=json.loads(bodies[-1])["messages"]; print("the model got:",json.dumps(last[-1])[:200])
print("duplicate:",post(url,{"uuid":"n1","type":"notification","label":"ci","text":"build 42 failed"})); n=len(bodies); pump(1.0); print("requests after the duplicate:",len(bodies)-n)
turns.append(turn("Reading the policy, then the log."))
for i in range(50): post(url,{"uuid":f"b{i}","type":"iteration_failed","iteration":i})
n=len(bodies); pump(2.0); show("a burst of 50 signals: one message")
print("requests after the burst:",len(bodies)-n,"| the model got:",json.dumps(json.loads(bodies[-1])["messages"][-1])[:330])
print("log outcomes:",[json.loads(l)["outcome"] for l in open(home+"/data/harness-multitool/daemon/triggers.jsonl")])
for c in "/exit\r": os.write(fd,c.encode()); time.sleep(0.005)
pump(0.5); print("exit:",os.waitpid(pid,os.WNOHANG))
daemon=json.load(open(home+"/data/harness-multitool/daemon/address.json"))["pid"]; os.kill(daemon,signal.SIGTERM); print("stopped the test's daemon, pid",daemon)
