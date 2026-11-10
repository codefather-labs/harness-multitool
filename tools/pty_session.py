#!/usr/bin/env python3
"""Debug: drives the real TUI in a pty against a fake Anthropic endpoint and prints the screen
after each step, plus the idle CPU. Needs pyte. Usage: tools/pty_session.py $PWD/target/debug/multitool"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import os, pty, select, sys, time, json, threading, http.server, tempfile, fcntl, termios, struct, resource
import pyte
def sse(events): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
def turn(text, tool=None):
    ev=[{"type":"message_start","message":{"usage":{"input_tokens":1200,"output_tokens":1}}},
        {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}]
    for w in text.split(" "):
        ev.append({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":w+" "}})
    ev.append({"type":"content_block_stop","index":0})
    if tool:
        ev+= [{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":tool[0],"name":tool[1],"input":{}}},
              {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":json.dumps(tool[2])}},
              {"type":"content_block_stop","index":1}]
    ev.append({"type":"message_delta","delta":{"stop_reason":"tool_use" if tool else "end_turn"},"usage":{"output_tokens":80}})
    ev.append({"type":"message_stop"})
    return sse(ev)
root=tempfile.mkdtemp(prefix="mt-root"); open(root+"/a.txt","w").write("hello world\n")
turns=[turn("Let me change the file.",("t1","edit",{"path":"a.txt","old_text":"world","new_text":"harness"})),
       turn("Now I check it.",("t2","shell",{"command":"cat a.txt"})),
       turn("The file says hello harness.")]
served=[]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n=int(self.headers.get("content-length",0)); self.rfile.read(n); served.append(self.path)
        b=turns.pop(0) if turns else turn("no more")
        self.send_response(200); self.send_header("content-type","text/event-stream"); self.send_header("content-length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
srv=http.server.ThreadingHTTPServer(("127.0.0.1",0),H); threading.Thread(target=srv.serve_forever,daemon=True).start()
cfg=tempfile.mkdtemp(prefix="mt-cfg"); os.makedirs(cfg+"/harness-multitool")
open(cfg+"/harness-multitool/config.toml","w").write(f'[session]\nprovider="anthropic"\nmodel="claude-opus-5-5"\n[session.providers.anthropic]\nbase_url="http://127.0.0.1:{srv.server_port}"\n')
COLS,ROWS=110,36
bin_=sys.argv[1]
pid,fd=pty.fork()
if pid==0:
    os.chdir(root)
    os.environ.update(XDG_CONFIG_HOME=cfg,MULTITOOL_HOME=cfg+"/mt-home",XDG_DATA_HOME=cfg+"/data",ANTHROPIC_API_KEY="test-key-local-fake",TERM="xterm-256color",MULTITOOL_GRAPHICS="off",COLORTERM="truecolor")
    os.execv(bin_,[bin_,"preview","welcome"])
fcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack("HHHH",ROWS,COLS,0,0))
screen=pyte.Screen(COLS,ROWS); stream=pyte.ByteStream(screen)
def pump(sec):
    end=time.time()+sec
    while time.time()<end:
        r,_,_=select.select([fd],[],[],0.05)
        if r:
            try: d=os.read(fd,65536)
            except OSError: return
            stream.feed(d)
def text(): return "\n".join(l.rstrip() for l in screen.display).rstrip()
def show(t): print(f"\n===== {t} =====\n{text()}")
def typ(s):
    for c in s: os.write(fd,c.encode()); time.sleep(0.005)
pump(1.0); show("started")
typ("change it\r"); pump(1.0); show("after prompt: permission prompt for edit")
assert "edit" in text()
os.write(fd,b"1"); pump(1.0); show("edit allowed: permission prompt for shell")
os.write(fd,b"1"); pump(1.5); show("shell allowed: finished")
print("\nfile on disk:", repr(open(root+"/a.txt").read()), "requests:", served)
# idle CPU over 3 seconds
def cpu():
    f=open(f"/proc/{pid}/stat").read().split(); return (int(f[13])+int(f[14]))/os.sysconf("SC_CLK_TCK")
c0=cpu(); pump(3.0); print("idle cpu seconds over 3s:", round(cpu()-c0,3))
typ("/exit\r"); pump(0.5)
print("exit:", os.waitpid(pid,os.WNOHANG))

# Part two: a new process, same folder: the startup page lists the session, opening it continues.
turns[:] = [turn("Continuing the old session.")]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root)
    os.environ.update(XDG_CONFIG_HOME=cfg,MULTITOOL_HOME=cfg+"/mt-home", XDG_DATA_HOME=cfg + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", COLORTERM="truecolor")
    os.execv(bin_, [bin_, "preview", "startup"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen.reset()
pump(1.0); show("restart: the startup page lists the session")
assert "change it" in text()
os.write(fd, b"r"); pump(1.0); show("resumed: the old transcript is back")
assert "The file says hello harness" in text()
typ("go on\r"); pump(1.0); show("continued")
assert "Continuing the old session" in text()
typ("/exit\r"); pump(0.5)
print("second exit:", os.waitpid(pid, os.WNOHANG))
