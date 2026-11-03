#!/usr/bin/env python3
"""Debug: the map node limit (CON-114) in a real window and over MCP. The settings' Memory page sets `map_max_nodes` in
a real TUI: the page shows the saved 200, the typed 5 is saved in the configuration at once, and the window says a running
session keeps its limit. Then `multitool mcp` started with that configuration refuses mindmap_add on the limit, naming the
count and the limit, and takes the adds under it. Reads no screen, so it needs no pyte. Needs --features claude-code.
Usage: tools/pty_map_limit.py <absolute path to a release multitool built with --features claude-code>"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup
import fcntl, json, pty, re, select, struct, subprocess, tempfile, termios, time

LIMIT = 5
bin_ = sys.argv[1]
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-ml-root")); home = tempfile.mkdtemp(prefix="mt-ml-home")
os.makedirs(home + "/cfg/harness-multitool")
config = home + "/cfg/harness-multitool/config.toml"
open(config, "w").write("[session]\nmap_max_nodes = 200\n")
env = dict(os.environ, XDG_CONFIG_HOME=home + "/cfg", XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off")

pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "startup"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 130, 0, 0))
drawn = ""
def pump(sec):
    global drawn
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: raw = os.read(fd, 65536)
            except OSError: return
            drawn += re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", raw.decode("utf8", "replace"))
def until(what, check, sec=15):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.05)
    assert check(), what + ": " + repr(drawn[-800:])
def press(*keys):
    for k in keys: os.write(fd, k.encode()); pump(0.12)
def saved_limit():
    for line in open(config).read().splitlines():
        if line.strip().startswith("map_max_nodes"):
            return int(line.split("=")[1])
    return None

pump(1.5)
press("s"); pump(0.5)
until("settings open", lambda: "Callbacks" in drawn)
down = "\x1b[B"
press(*[down] * 3)
until("the Memory page", lambda: "Map node limit" in drawn)
until("the saved 200 shows", lambda: "Map node limit" in drawn and "200" in drawn[drawn.rfind("Map node limit"):])
print("the Memory page shows the saved limit: 200")
assert saved_limit() == 200, saved_limit()

press("\t", "\r")                          # to the page, Enter edits: what is typed starts as 200
for _ in range(3): press("\x7f")           # Backspace clears it
for c in str(LIMIT): os.write(fd, c.encode()); pump(0.05)
press("\x13")                              # Ctrl+S
until("the save is said", lambda: "keeps its limit until it restarts" in drawn, 10)
end = time.time() + 10
while time.time() < end and saved_limit() != LIMIT: pump(0.1)
assert saved_limit() == LIMIT, f"config says {saved_limit()}"
print(f"typed {LIMIT} and saved; config.toml now holds:"); print(open(config).read().rstrip())
print("the window says a running session keeps its limit until it restarts")

# The MCP server is a separate process, started now with the saved configuration (CON-114).
def call(i, name, arguments):
    return json.dumps({"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
init = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})
notify = json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"})
messages = [init, notify, call(2, "mindmap_new", {"title": "limit check"})]
adds = LIMIT  # the root holds one: the first LIMIT-1 adds pass, the last one finds the map at the limit
messages += [call(3 + n, "mindmap_add", {"map": 1, "text": f"node {n}", "kind": "note"}) for n in range(adds)]
run = subprocess.run([bin_, "mcp", "--root", root], input="\n".join(messages) + "\n", env=env, cwd=root, capture_output=True, text=True, timeout=60)
assert run.returncode == 0, run.stderr[-800:]
replies = {r["id"]: r for r in (json.loads(l) for l in run.stdout.splitlines() if l.strip())}
map_id = json.loads(replies[2]["result"]["content"][0]["text"])["id"]
print("mindmap_new over mcp: map", map_id)
results = []
for n in range(adds):
    r = replies[3 + n]["result"]
    results.append((n + 1, r["isError"], r["content"][0]["text"]))
    print(f"mindmap_add {n + 1}: isError={r['isError']}  {r['content'][0]['text'][:150]}")
# Map 1 is the first map of this store; the id the server gave is the one the adds used.
assert map_id == 1, map_id
for n, is_error, text in results[:-1]:
    assert not is_error, f"add {n} under the limit was refused: {text}"
last_n, last_error, last_text = results[-1]
assert last_error, f"the add at the limit passed: {last_text}"
assert f"holds {LIMIT} nodes" in last_text and f"limit is {LIMIT}" in last_text, last_text
assert "nothing was added" in last_text and "the user's to change" in last_text, last_text
print(f"ok: adds under the limit pass; the add at {LIMIT} is refused naming {LIMIT}")

# CON-118: the second row saves the page of linked entries, and search answers neighbours by it, over MCP.
press("\x1b[B", "\r")                      # the row below, Enter edits: what is typed starts as 100
for _ in range(3): press("\x7f")
press("2", "\x13")
end = time.time() + 10
while time.time() < end and "search_max_linked = 2" not in open(config).read(): pump(0.1)
assert "search_max_linked = 2" in open(config).read(), open(config).read()
print("typed 2 in the second row and saved it")
adds = [call(100, "journal_add", {"body": "Graph root entry.", "category": "finding", "tags": ["g"]})]
adds += [call(101 + n, "journal_add", {"body": f"Graph leaf {n}. Cites journal#1 and says more.", "category": "finding", "tags": ["g"]}) for n in range(3)]
search = lambda i, offset: call(i, "journal_search", {"query": "root", "offset": offset})
run = subprocess.run([bin_, "mcp", "--root", root], input="\n".join([init, notify] + adds + [search(110, 0), search(111, 2)]) + "\n", env=env, cwd=root, capture_output=True, text=True, timeout=60)
replies = {r["id"]: r for r in (json.loads(l) for l in run.stdout.splitlines() if l.strip())}
first = json.loads(replies[110]["result"]["content"][0]["text"]); second = json.loads(replies[111]["result"]["content"][0]["text"])
print("page 1:", first["linked"], first["linked_total"], first["linked_left"], first.get("next_offset"))
print("page 2:", second["linked"], second["linked_total"], second["linked_left"], second.get("next_offset"))
assert first["entries"][0]["body"] == "Graph root entry.", first
assert [n["id"] for n in first["linked"]] == [2, 3] and first["linked_total"] == 3 and first["linked_left"] == 1 and first["next_offset"] == 2, first
assert set(first["linked"][0]) == {"id", "title", "via"} and first["linked"][0]["title"] == "Graph leaf 0" and first["linked"][0]["via"] == "#2 cites #1", first
assert [n["id"] for n in second["linked"]] == [4] and "next_offset" not in second, second
print("ok: neighbours as id, title and link, a page of the saved size, the rest by next_offset")

os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
print("ok")
