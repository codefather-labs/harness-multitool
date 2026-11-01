#!/usr/bin/env python3
"""Debug: step-debug end to end with no model: a real daemon, the reference pipeline, the contract
check, approvals one by one, and the run in the project's SQLite.
Usage: tools/debug_live.py <absolute path to multitool>"""
import json, os, signal, subprocess, sys, tempfile, time, urllib.error, urllib.request

bin_ = sys.argv[1]; here = os.path.dirname(os.path.abspath(__file__))
home = tempfile.mkdtemp(prefix="mt-dbg-home"); root = tempfile.mkdtemp(prefix="mt-dbg-root")
env = {**os.environ, "XDG_CONFIG_HOME": home + "/cfg", "XDG_DATA_HOME": home + "/data"}
daemon = subprocess.Popen([bin_, "daemon"], env=env, stdout=subprocess.DEVNULL)
def mt(*args): return subprocess.run([bin_, *args], env=env, cwd=root, capture_output=True, text=True)
try:
    for _ in range(50):
        path = home + "/data/harness-multitool/daemon/address.json"
        if os.path.exists(path): break
        time.sleep(0.1)
    time.sleep(0.3)
    a = json.load(open(path)); base = f"http://127.0.0.1:{a['port']}"
    def call(p, body=None, token=a["admin"], method=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + p if p.startswith("/") else p, data, method=method or ("POST" if data else "GET"))
        if token: req.add_header("authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(req) as r: return r.status, json.load(r)
        except urllib.error.HTTPError as e: return e.code, json.load(e)
    _, reg = call("/register", {"root": root}); window = reg["window"]
    def memory(c): return call("/memory", {"root": root, "owner": "t", "call": c})[1]["result"]
    chart = memory({"op": "map_new", "kind": "flowchart", "title": "pipeline", "direction": None}); nodes = []
    for kind, text in [("process", "fetch"), ("process", "transform"), ("end", "done")]:
        data = memory({"op": "map_add", "map": chart["id"], "node": {"text": text, "kind": kind, "parent": None, "after": nodes[-1] if nodes else None, "label": "", "refs": []}})
        nodes.append([n["id"] for n in data["nodes"] if n["text"] == text][0])
    pipe_env = {**env, "PIPELINE_MAP": str(chart["id"]), "PIPELINE_NODES": ",".join(map(str, nodes)), "MULTITOOL_CALLBACK_URL": reg["url"]}
    example = [sys.executable, "-I", here + "/debug_pipeline_example.py"]

    off = subprocess.run(example, env={k: v for k, v in pipe_env.items()}, capture_output=True, text=True, timeout=10)
    print("gate off:", off.stdout.split()); assert off.stdout.strip().endswith("finished")
    assert call(f"/events?window={window}&after=0")[1]["events"] == [], "the gate is off: nothing is sent"

    assert call("/window/step-debug", {"window": window, "on": True})[0] == 200
    port_file = home + "/port"; token = "local-test-token"
    pipe = subprocess.Popen(example, env={**pipe_env, "MULTITOOL_DEBUG_MODE": "1", "MULTITOOL_DEBUG_TOKEN": token, "PIPELINE_PORT_FILE": port_file}, stdout=subprocess.PIPE, text=True)
    try:
        for _ in range(50):
            if os.path.exists(port_file) and open(port_file).read(): break
            time.sleep(0.1)
        router = f"http://127.0.0.1:{open(port_file).read()}"
        checked = subprocess.run([bin_, "debug", "check", router], env={**env, "MULTITOOL_DEBUG_TOKEN": token}, capture_output=True, text=True)
        print(checked.stdout.strip()); assert checked.returncode == 0, checked.stderr
        assert call(router + "/debug/status", token=None)[0] == 401
        seen = 0
        for i, node in enumerate(nodes):
            for _ in range(50):
                st = call(router + "/debug/status", token=token)[1]
                if st["waiting"] and st["node"] == node: break
                time.sleep(0.1)
            assert st["waiting"] and st["node"] == node, st
            time.sleep(1.5)
            st2 = call(router + "/debug/status", token=token)[1]; assert st2["waiting"] and st2["node"] == node, "it must wait with nothing happening"
            events = call(f"/events?window={window}&after={seen}")[1]["events"]
            assert events and events[-1]["event"]["node"] == node, events
            seen = events[-1]["seq"]
            print(f"waiting on node {node}; the chart's path:", events[-1]["event"]["path"], "run:", events[-1]["event"].get("run"))
            if i == 0: assert not os.path.exists(root + "/.harness/debug/runs.sqlite"), "nothing saved mid-cycle"
            assert call(router + "/debug/approve", {}, token=token)[0] == 200
        assert pipe.wait(timeout=10) == 0
    finally:
        pipe.kill()
    print("pipeline:", pipe.stdout.read().split())
    runs = mt("debug", "runs"); print(runs.stdout.strip()); assert runs.returncode == 0 and "pipeline" not in runs.stderr
    assert os.path.exists(root + "/.harness/debug/runs.sqlite") and f"steps {len(nodes)}" in runs.stdout, runs.stdout
    print("ok")
finally:
    daemon.send_signal(signal.SIGTERM); daemon.wait(timeout=10)
