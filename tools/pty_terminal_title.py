#!/usr/bin/env python3
"""Debug, CON-170: a window names its terminal tab after its agent. The real TUI in a pty with a fake model
whose turn calls set_nick; the terminal title (OSC 0, which pyte keeps as screen.title) follows it, and
/nick typed in the window changes it again. CON-174: the model holds its second answer 3 s; while
it holds, the title shows the moon's phases before the name, and after it, none. Needs pyte.
Usage: tools/pty_terminal_title.py <absolute path to multitool>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, http.server, json, pty, select, signal, struct, tempfile, termios, threading, time
import pyte

def sse(ev): return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev).encode()
def turn(text, tool=None):
    ev = [{"type": "message_start", "message": {"usage": {"input_tokens": 50, "output_tokens": 1}}}, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}, {"type": "content_block_stop", "index": 0}]
    if tool:
        ev += [{"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": tool[0], "name": tool[1], "input": {}}}, {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": json.dumps(tool[2])}}, {"type": "content_block_stop", "index": 1}]
    ev += [{"type": "message_delta", "delta": {"stop_reason": "tool_use" if tool else "end_turn"}, "usage": {"output_tokens": 5}}, {"type": "message_stop"}]
    return sse(ev)
turns = [turn("Naming myself.", ("n1", "set_nick", {"nick": "alpha-nick"})), turn("Named.")]
answered = []
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        answered.append(1)
        if len(answered) == 2: time.sleep(3)  # CON-174: the window is busy meanwhile
        b = turns.pop(0) if turns else turn("more")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
model = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model); threading.Thread(target=model.serve_forever, daemon=True).start()

root = tempfile.mkdtemp(prefix="mt-title-root"); home = tempfile.mkdtemp(prefix="mt-title-home"); os.makedirs(home + "/cfg/harness-multitool")
open(home + "/cfg/harness-multitool/config.toml", "w").write(f'[session.providers.anthropic]\nbase_url="http://127.0.0.1:{model.server_port}"\n')
folder = os.path.basename(root)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=home + "/mt-home", XDG_DATA_HOME=home + "/data", ANTHROPIC_API_KEY="test-key-local-fake", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")
    os.execv(sys.argv[1], [sys.argv[1], "--new"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
screen = pyte.Screen(120, 40); stream = pyte.ByteStream(screen)
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: stream.feed(os.read(fd, 65536))
            except OSError: return
def wait(cond, sec):
    end = time.time() + sec
    while time.time() < end and not cond(): pump(0.2)
    return cond()
def type_(s):
    for c in s: os.write(fd, c.encode()); time.sleep(0.01)
results, seen = {}, []
results["titled_at_start"] = wait(lambda: screen.title.endswith(f"· {folder}"), 20); seen.append(screen.title)
type_("name yourself\r")
# CON-174: while the answer is held, the title's moon turns.
moons = set(); end = time.time() + 6
while time.time() < end:
    pump(0.05)
    head = screen.title.split(" ", 1)[0]
    if head in "🌑🌒🌓🌔🌕🌖🌗🌘" and head: moons.add(head)
print("moon phases seen:", "".join(sorted(moons)))
results["moon_turns_while_busy"] = len(moons) >= 3
results["title_follows_set_nick"] = wait(lambda: screen.title == f"alpha-nick · {folder}", 20); seen.append(screen.title)
results["no_moon_when_idle"] = not any(m in screen.title for m in "🌑🌒🌓🌔🌕🌖🌗🌘")
if not results["title_follows_set_nick"]:
    print("===== screen while waiting for set_nick =====\n" + "\n".join(l.rstrip() for l in screen.display if l.strip()))
type_("/nick beta-nick\r")
results["title_follows_slash_nick"] = wait(lambda: screen.title == f"beta-nick · {folder}", 20); seen.append(screen.title)
print("titles seen:", seen)
print("RESULTS:", json.dumps(results))
for c in "/exit\r": os.write(fd, c.encode()); time.sleep(0.01)
pump(1)
try: os.kill(json.load(open(home + "/data/harness-multitool/daemon/address.json"))["pid"], signal.SIGTERM)
except Exception as e: print("daemon stop:", e)
print("ALL PASSED" if all(results.values()) else "FAILED")
