#!/usr/bin/env python3
"""Debug: the settings' Profiles tab in a real window (CON-125, CON-121). Opens the settings by a click
on their button, goes to Profiles, makes a profile with Add, refuses to delete the one in use, deletes
another, moves the window onto a profile with Use, and takes screenshots on the way. Reads files and
pictures, not the screen, so it needs no pyte.
Usage: tools/pty_profiles_tab.py <absolute path to multitool> [a directory to keep the pictures in]"""
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import pty_cleanup  # noqa: F401
import fcntl, pty, select, shutil, struct, subprocess, tempfile, termios, time

bin_ = sys.argv[1]; keep = sys.argv[2] if len(sys.argv) > 2 else None; COLS, ROWS = 120, 34
root = os.path.realpath(tempfile.mkdtemp(prefix="mt-pt-root")); home = tempfile.mkdtemp(prefix="mt-pt-home"); os.makedirs(home + "/cfg/harness-multitool")
mt = home + "/mt-home"
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
env.update(XDG_CONFIG_HOME=home + "/cfg", MULTITOOL_HOME=mt, XDG_DATA_HOME=home + "/data", TERM="xterm-256color", MULTITOOL_GRAPHICS="off", CLAUDE_CONFIG_DIR=home + "/dot-claude")
for n in ["gpt", "max"]:
    assert subprocess.run([bin_, "profile", "add", n], env=env, cwd=root, capture_output=True).returncode == 0
# max runs a fake Claude Code, so its Sign in button can be pressed: it writes down how it was run.
calls, fake = home + "/auth-calls", home + "/fake-claude"
open(fake, "w").write(f"""#!/usr/bin/env python3
import json, os, sys
d = os.environ.get("CLAUDE_CONFIG_DIR")
open({calls!r}, "a").write(json.dumps({{"argv": sys.argv[1:], "dir": d}}) + "\\n")
mark = os.path.join(d or "/nonexistent", "signed-in")
if sys.argv[1:3] == ["auth", "login"]: open(mark, "w").write("yes"); print("fake sign-in done")
if sys.argv[1:3] == ["auth", "status"]: print(json.dumps({{"loggedIn": os.path.exists(mark)}}))
""")
os.chmod(fake, 0o755)
open(mt + "/profiles/max/profile.toml", "w").write(f'[session]\nprovider="claude-code"\nmodel="m"\n\n[session.providers.claude-code]\nkind="claude-code"\nbinary="{fake}"\n')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root); os.environ.clear(); os.environ.update(env); os.execv(bin_, [bin_, "preview", "welcome"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
def press(*ks):
    for k in ks: os.write(fd, k.encode()); pump(0.15)
shots = []
def shot(name):
    out = home + "/" + name
    p = subprocess.Popen([bin_, "debug", "screenshot", "--out", out], env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    start = time.time()
    while p.poll() is None and time.time() - start < 20: pump(0.05)
    _, err = p.communicate(timeout=1)
    assert p.returncode == 0 and open(out, "rb").read()[:8] == b"\x89PNG\r\n\x1a\n", err
    shots.append(out)
    if keep: shutil.copy(out, os.path.join(keep, name))
def profiles(): return sorted(os.listdir(mt + "/profiles"))
def until(what, check, sec=10):
    end = time.time() + sec
    while time.time() < end and not check(): pump(0.1)
    assert check(), what
DOWN, UP, LEFT, RIGHT = "\x1b[B", "\x1b[A", "\x1b[D", "\x1b[C"
pump(2)
press("\x1b[<0;112;33M", "\x1b[<0;112;33m"); pump(0.8)          # the Settings button
press(*[DOWN] * 9); press("\t"); pump(0.5)                      # Profiles, then into the page
shot("profiles-tab.png")
press("a", "w", "o", "r", "k", "\r")
until("Add made work", lambda: "work" in profiles())
assert os.path.isdir(mt + "/profiles/work/claude") and os.readlink(mt + "/profiles/work/claude/projects") == home + "/dot-claude/projects"
print("Add made a clean profile:", profiles())
# journal#799: Add moves the window onto the new profile and opens its Provider page, not signed in.
shot("added-provider.png")
press(LEFT, *[DOWN] * 9, RIGHT); pump(0.6)                     # back to Profiles: Left, down to it, Right
shot("profiles-added.png")
press("d"); pump(0.5)                                           # on default: refused
assert "default" in profiles()
press(DOWN, "d", "y")                                           # gpt: asked, then deleted
until("Delete removed gpt", lambda: "gpt" not in profiles())
print("Delete refused default, removed gpt:", profiles())
press(UP, UP, DOWN, "e"); pump(1.5)                             # max: Edit is Use and the Provider page
shot("profiles-edit-provider.png")
# journal#794: the sign-in is the Provider page's. The cursor is on max's adapter (claude-code); its
# Login button comes after the six adapter rows.
press(DOWN, DOWN, "\r"); pump(2)
logins = [l for l in open(calls).read().splitlines() if '"login"' in l] if os.path.exists(calls) else []
assert logins and mt + "/profiles/max/claude" in logins[0], logins
press("\r"); pump(1.5)                                          # Enter: back to the harness
shot("provider-signed-in.png")
print("Edit opened max's Provider page; its Login handed the terminal to claude auth login in max's directory and came back")
press("\t", *[DOWN] * 9, "\t"); pump(0.8)                      # back to Profiles
shot("profiles-used.png")
# journal#791: after Use the Provider page is max's (claude-code, its own Claude Code signed in), and a
# choice saved there is written to max's file, not to default's.
default_before = open(mt + "/profiles/default/profile.toml").read()
press("\t", *[UP] * 9); pump(0.6)                              # the sections pane, up to Provider
shot("provider-after-use.png")
press("\t", UP, UP, UP, UP, " "); pump(0.4)                    # into the page, from claude-code up to Anthropic API
shot("provider-chosen.png")
press("\x13"); pump(1.5)                                       # Ctrl+S
shot("provider-saved.png")
until("max's file has Anthropic API", lambda: 'provider = "anthropic"' in open(mt + "/profiles/max/profile.toml").read())
assert open(mt + "/profiles/default/profile.toml").read() == default_before, "default's file changed"
print("after Use the Provider page saved to max's file, default's untouched")
press("\x1b"); pump(0.5)
shot("status-after-use.png")
saved = subprocess.run([bin_, "profile", "list"], env=env, cwd=root, capture_output=True, text=True).stdout
print(saved.rstrip())
assert any(l.split()[0] == "max" and "Claude Code: signed in" in l for l in saved.splitlines()), saved
if keep:
    for s in shots: shutil.copy(s, os.path.join(keep, os.path.basename(s)))
print("ok")
os.write(fd, b"\x03"); pump(0.3); os.write(fd, b"\x03"); pump(0.3)
