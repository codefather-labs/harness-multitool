#!/usr/bin/env python3
"""A fake MCP stdio server for the tests of harness-tools-mcp (CON-201).
Tools: echo (says its text back), fail (an error result), sleep (never answers), die (exits),
roots (asks the client roots/list and says what it got). The listing comes in two pages.
FAKE_STARTS, when set, is a file that gets one line per start."""
import json, os, sys, time
if os.environ.get("FAKE_STARTS"):
    open(os.environ["FAKE_STARTS"], "a").write("start\n")
TOOLS = [
    {"name": "echo", "description": "Says the text back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
    {"name": "fail", "description": "Always fails", "inputSchema": {"type": "object"}},
    {"name": "sleep", "description": "Never answers", "inputSchema": {"type": "object"}},
    {"name": "die", "description": "Exits", "inputSchema": {"type": "object"}},
    {"name": "roots", "description": "Asks for the roots", "inputSchema": {"type": "object"}},
]
def send(m): sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    m = json.loads(line)
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}}})
    elif method == "tools/list":
        page = 1 if m.get("params", {}).get("cursor") == "p2" else 0
        result = {"tools": TOOLS[:2]} if page == 0 else {"tools": TOOLS[2:]}
        if page == 0: result["nextCursor"] = "p2"
        send({"jsonrpc": "2.0", "id": mid, "result": result})
    elif method == "tools/call":
        name, args = m["params"]["name"], m["params"].get("arguments", {})
        sys.stderr.write("call %s\n" % name); sys.stderr.flush()
        if name == "echo":
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "echo: " + args.get("text", "")}, {"type": "image", "data": "AA==", "mimeType": "image/png"}]}})
        elif name == "fail":
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "it failed"}], "isError": True}})
        elif name == "sleep":
            pass
        elif name == "die":
            sys.exit(3)
        elif name == "roots":
            send({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info", "data": "x"}})
            send({"jsonrpc": "2.0", "id": "r1", "method": "roots/list"})
            answer = json.loads(sys.stdin.readline())
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": json.dumps(answer["result"]["roots"])}]}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": "no tool " + name}})
    elif mid is not None and method:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "unknown"}})
