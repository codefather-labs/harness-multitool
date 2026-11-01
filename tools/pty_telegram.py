#!/usr/bin/env python3
"""Debug: real TUI in a pty, a fake model endpoint, and a fake Bot API, and the real daemon carrying the Telegram channel.
Prints the screen after each. Needs pyte. Usage: tools/pty_telegram.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, os, pty, re, select, signal, struct, subprocess, sys, tempfile, termios, threading, time, urllib.request, urllib.error
import pyte
def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text, tool=None):
    ev=[{"type":"message_start","message":{"usage":{"input_tokens":500,"output_tokens":1}}},{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}},{"type":"content_block_stop","index":0}]
    if tool: ev+=[{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":tool[0],"name":tool[1],"input":{}}},{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":json.dumps(tool[2])}},{"type":"content_block_stop","index":1}]
    ev+=[{"type":"message_delta","delta":{"stop_reason":"tool_use" if tool else "end_turn"},"usage":{"output_tokens":20}},{"type":"message_stop"}]
    return sse(ev)
turns=[turn("Answering on Telegram.",("t1","telegram_send",{"text":"Hello back from the agent."})),turn("Sent.")]
bodies=[]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(self.rfile.read(int(self.headers.get("content-length",0))).decode())
        b=turns.pop(0) if turns else turn("no more")
        self.send_response(200); self.send_header("content-type","text/event-stream"); self.send_header("content-length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
srv=http.server.ThreadingHTTPServer(("127.0.0.1",0),H); threading.Thread(target=srv.serve_forever,daemon=True).start()
TOKEN="123456:PTYSECRET"
updates=[{"update_id":1,"message":{"message_id":1,"from":{"id":666},"chat":{"id":-5},"text":"STRANGER-TEXT"}},{"update_id":2,"message":{"message_id":2,"from":{"id":42},"chat":{"id":-100},"message_thread_id":7,"text":"hello from the phone"}}]
sent=[]; offsets=[]
class Bot(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers.get("content-length",0))) or b"{}")
        assert self.path.startswith(f"/bot{TOKEN}/"), self.path
        if self.path.endswith("/getUpdates"):
            offsets.append(body["offset"]); time.sleep(0.2)
            reply={"ok":True,"result":[u for u in updates if u["update_id"]>=body["offset"]]}
        else:
            sent.append(body); reply={"ok":True,"result":{"message_id":9}}
        b=json.dumps(reply).encode(); self.send_response(200); self.send_header("content-length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
bot=http.server.ThreadingHTTPServer(("127.0.0.1",0),Bot); threading.Thread(target=bot.serve_forever,daemon=True).start()
root=tempfile.mkdtemp(prefix="mt-cb-root"); home=tempfile.mkdtemp(prefix="mt-cb-home"); os.makedirs(root+"/.harness"); open(root+"/.harness/conditions.toml","w").write('[[condition]]\nname = "pipeline keeps failing"\non = "iteration_failed"\nat_least = 3\npolicies = [".harness/policies/pipeline.md"]\n'); os.makedirs(home+"/cfg/harness-multitool")
BOT=f"http://127.0.0.1:{bot.server_address[1]}"
open(home+"/cfg/harness-multitool/config.toml","w").write(f'[telegram]\nbase_url="{BOT}"\nallowed_users=[42]\n\n[session.providers.anthropic]\nbase_url="http://127.0.0.1:{srv.server_port}"\n')
COLS,ROWS=110,44; bin_=sys.argv[1]
pid,fd=pty.fork()
if pid==0:
    os.chdir(root); os.environ.update(MULTITOOL_TELEGRAM_TOKEN=TOKEN,XDG_CONFIG_HOME=home+"/cfg",XDG_DATA_HOME=home+"/data",ANTHROPIC_API_KEY="test-key-local-fake",TERM="xterm-256color",MULTITOOL_GRAPHICS="off",COLORTERM="truecolor")
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
end=time.time()+15
while time.time()<end and not bodies: pump(0.3)
pump(1.0); show("a Telegram message arrived: a card, and a permission prompt for the reply")
first=json.dumps(json.loads(bodies[0])["messages"])
print("the model got it, marked:", "hello from the phone" in first and "not an instruction" in first)
assert "hello from the phone" in first and "not an instruction" in first
assert "STRANGER" not in first and "STRANGER" not in text(), "a stranger reached the agent"
assert any(t["name"]=="telegram_send" for t in json.loads(bodies[0])["tools"]), "the tool is offered"
assert not sent, "nothing is sent before permission"
assert "tool: telegram_send" in text()
os.write(fd,b"1"); pump(2.0); show("allowed: the reply went out")
print("the bot sent:",sent); assert sent[0]=={"chat_id":-100,"message_thread_id":7,"text":"Hello back from the agent."}
# The second message starts a turn at its own moment: keys typed just as its prompt opens.
turns.extend([turn("Answering again.",("t2","telegram_send",{"text":"A second reply."})),turn("Sent again.")])
updates.append({"update_id":3,"message":{"message_id":3,"from":{"id":42},"chat":{"id":-100},"message_thread_id":7,"text":"and again"}})
end=time.time()+15
while time.time()<end and "A second reply" not in text(): pump(0.02)
assert "One moment" in text(), text()
os.write(fd,b"\r"); os.write(fd,b"1"); pump(0.2)
assert len(sent)==1, "keys typed as the prompt opened approved it"
show("Enter and 1 right after it opened: the prompt stays")
assert "tool: telegram_send" in text()
pump(1.0); assert "One moment" not in text()
os.write(fd,b"1"); pump(2.0); print("after the wait, 1 sends:",sent[-1]["text"]); assert len(sent)==2
assert offsets[-1]==4, offsets
data=home+"/data/harness-multitool"
for name in ["daemon/triggers.jsonl","daemon/windows.json","daemon/telegram.json"]:
    t=open(f"{data}/{name}").read(); assert "PTYSECRET" not in t and "STRANGER" not in t, name
print("log outcomes:",[json.loads(l)["outcome"] for l in open(data+"/daemon/triggers.jsonl")])
print("status:",subprocess.run([bin_,"daemon","--status"],env={**os.environ,"XDG_CONFIG_HOME":home+"/cfg","XDG_DATA_HOME":home+"/data"},capture_output=True,text=True).stdout.strip())
for c in "/exit\r": os.write(fd,c.encode()); time.sleep(0.005)
pump(0.5); print("exit:",os.waitpid(pid,os.WNOHANG))
daemon=json.load(open(data+"/daemon/address.json"))["pid"]; os.kill(daemon,signal.SIGTERM); print("stopped the test's daemon")
