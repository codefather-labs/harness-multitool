#!/usr/bin/env python3
"""Debug: the hot reload in a real TUI in a pty, with a fake model endpoint and the real daemon.
Needs pyte. Usage: tools/pty_reload.py <absolute path to multitool>"""
import fcntl, http.server, json, os, pty, re, select, signal, struct, sys, tempfile, termios, threading, time, urllib.request, urllib.error
import pyte
def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text):
    return sse([{"type":"message_start","message":{"usage":{"input_tokens":500,"output_tokens":1}}},{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}},{"type":"content_block_stop","index":0},{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}},{"type":"message_stop"}])
bodies=[]; hold=[0.0]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        bodies.append(json.loads(self.rfile.read(int(self.headers.get("content-length",0)))))
        time.sleep(hold[0])
        b=turn(f"Answer number {len(bodies)}.")
        self.send_response(200); self.send_header("content-type","text/event-stream"); self.send_header("content-length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
srv=http.server.ThreadingHTTPServer(("127.0.0.1",0),H); threading.Thread(target=srv.serve_forever,daemon=True).start()
root=tempfile.mkdtemp(prefix="mt-rl-root"); home=tempfile.mkdtemp(prefix="mt-rl-home"); os.makedirs(home+"/cfg/harness-multitool")
open(home+"/cfg/harness-multitool/config.toml","w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{srv.server_port}"\n')
COLS,ROWS=110,40; bin_=sys.argv[1]
def start(build=None):
    pid,fd=pty.fork()
    if pid==0:
        os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home+"/cfg",XDG_DATA_HOME=home+"/data",ANTHROPIC_API_KEY="test-key-local-fake",TERM="xterm-256color",MULTITOOL_GRAPHICS="off",COLORTERM="truecolor")
        if build is not None: os.environ["MULTITOOL_RELOAD_BUILD"]=build
        os.execv(bin_,[bin_,"preview","welcome"])
    fcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack("HHHH",ROWS,COLS,0,0))
    return pid,fd
screen=pyte.Screen(COLS,ROWS); stream=pyte.ByteStream(screen); fd=None
def pump(sec):
    end=time.time()+sec
    while time.time()<end:
        r,_,_=select.select([fd],[],[],0.05)
        if r:
            try: stream.feed(os.read(fd,65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def show(t): print(f"\n===== {t} =====\n{text()}")
def typ(s):
    for c in s: os.write(fd,c.encode()); time.sleep(0.005)
def post(url,obj):
    try:
        with urllib.request.urlopen(urllib.request.Request(url,json.dumps(obj).encode(),method="POST")) as r: return r.status,json.load(r)
    except urllib.error.HTTPError as e: return e.code,json.load(e)
def daemon_pid(): return json.load(open(home+"/data/harness-multitool/daemon/address.json"))
def wait_for(needle,sec):
    end=time.time()+sec
    while time.time()<end:
        pump(0.3)
        if needle in text(): return True
    return False

# 1. A failing build changes nothing.
pid,fd=start("echo the compiler says no >&2; exit 1")
pump(1.5); typ("first question\r"); pump(1.5)
typ("/reload\r"); assert wait_for("Nothing was swapped",8); show("a failing build")
assert "the compiler says no" in text() and "Answer number 1" in text()
os.kill(pid,signal.SIGTERM); os.waitpid(pid,0); os.close(fd)
os.kill(daemon_pid()["pid"],signal.SIGTERM); time.sleep(0.5)

# 2. The real build and the swap, during a slow turn: it waits for the turn to end.
screen.reset(); bodies.clear()
pid,fd=start()
pump(1.5); typ("second question\r"); pump(1.5)
url=re.search(r"http://127\.0\.0\.1:\d+/callback/[0-9a-f-]+",bodies[0]["system"]).group(0); before=daemon_pid()
print("url:",url,"daemon pid",before["pid"])
hold[0]=6.0; typ("a slow one\r"); pump(0.6)
typ("/reload\r"); pump(0.3); typ("half a thought")
show("the reload asked during a slow turn")
assert "generating" in text()
t0=time.time()
os_pid=pid
assert wait_for("Answer number 2",20), text()
hold[0]=0.0
print("the slow turn ended after",round(time.time()-t0,1),"s")
assert wait_for("half a thought",120), text()
pump(2.0); show("after the swap")
print("same pid:",os.path.exists(f"/proc/{pid}"),"cmdline:",open(f"/proc/{pid}/cmdline").read().replace("\0"," ")[-70:])
assert "--handoff" in open(f"/proc/{pid}/cmdline").read(), "the process was replaced in place"
assert "second question" in text() or "a slow one" in text()
assert "Answer number 2" in text()
after=daemon_pid(); print("daemon pid",after["pid"],"port kept:",after["port"]==before["port"])
assert after["pid"]!=before["pid"] and after["port"]==before["port"]
assert not os.path.exists(home+"/data/harness-multitool/handoff.json"), "the hand-off is used up"

# 3. Sessions go on: the old history, and the old callback URL.
for _ in range(20): os.write(fd,b"\x7f"); time.sleep(0.003)
n=len(bodies); typ("third question\r"); assert wait_for("Answer number 3",10), text()
msgs=bodies[-1]["messages"]; print("the next request carries",len(msgs),"messages; first:",json.dumps(msgs[0])[:80])
assert len(msgs)==5 and "second question" in json.dumps(msgs[0]) and url in bodies[-1]["system"]
print("the old URL:",post(url,{"uuid":"after-swap","type":"notification","label":"ci","text":"still here"}))
assert wait_for("Answer number 4",10), text()
show("a callback to the old URL, after the swap")
typ("/exit\r"); pump(0.5); print("exit:",os.waitpid(pid,os.WNOHANG))
os.kill(daemon_pid()["pid"],signal.SIGTERM); print("stopped the test's daemon")
