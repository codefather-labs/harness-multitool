"""Imported first by the debug scripts: when a script ends, even on a failed assertion, it stops the
daemons it started and removes its temp directories. A daemon is the script's when it is a multitool
whose data (XDG_DATA_HOME or MULTITOOL_HOME) lies in a temp directory this script made. A script's
own shutil.rmtree of such a directory stops them first too (journal#1098)."""
import atexit, glob, json, os, shutil, signal, tempfile, time

# A script run from inside a harness session inherits what that session's multitool noted of its
# own user (journal#1025): the user a script plays is its own, so those notes go.
for _name in ("MULTITOOL_OWN_CLAUDE_CONFIG_DIR", "MULTITOOL_OWN_CODEX_HOME"):
    os.environ.pop(_name, None)

_made = []
_mkdtemp = tempfile.mkdtemp

def _tracking_mkdtemp(*args, **kwargs):
    made = _mkdtemp(*args, **kwargs)
    _made.append(made)
    return made

tempfile.mkdtemp = _tracking_mkdtemp

def _is_multitool(pid):
    try:
        return b"multitool" in open(f"/proc/{pid}/cmdline", "rb").read()
    except OSError:
        return os.name != "posix" or not os.path.isdir("/proc")

def _inside(path, folders):
    path = os.path.realpath(path)
    return any(path == f or path.startswith(f + os.sep) for f in folders)

def _daemons_in(folders):
    """The multitool processes whose data lies in one of `folders`, found through /proc: a script
    removes its own folders, address files with them, before this module's exit hook runs
    (journal#1098), so the address files cannot be what finds them."""
    folders = [os.path.realpath(f) for f in folders]
    found = set()
    for entry in os.listdir("/proc") if os.path.isdir("/proc") else []:
        if not entry.isdigit() or int(entry) == os.getpid() or not _is_multitool(entry):
            continue
        try:
            env = open(f"/proc/{entry}/environ", "rb").read().split(b"\0")
        except OSError:
            continue
        for var in env:
            name, _, value = var.partition(b"=")
            if name in (b"XDG_DATA_HOME", b"MULTITOOL_HOME") and _inside(value.decode(errors="replace"), folders):
                found.add(int(entry))
    # Where /proc tells nothing (another OS), the address files that are still there.
    for f in folders:
        for path in glob.glob(f + "/**/daemon/address.json", recursive=True):
            try:
                pid = json.load(open(path))["pid"]
                if _is_multitool(pid):
                    found.add(pid)
            except (OSError, ValueError, KeyError):
                pass
    return found

def _stop_daemons(folders):
    pids = _daemons_in(folders)
    for pid in pids:
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    # A daemon ends its delegates before it goes: up to 3 s for them (CON-128).
    until = time.time() + 5
    while pids and time.time() < until:
        pids = {pid for pid in pids if os.path.exists(f"/proc/{pid}") and _is_multitool(pid)}
        time.sleep(0.1)

_rmtree = shutil.rmtree

def _stopping_rmtree(path, *args, **kwargs):
    # A folder of this script's goes only once the daemons living in it have stopped: alive, they
    # write into it as it is removed, and outlive it (journal#1095).
    if isinstance(path, (str, os.PathLike)) and _inside(os.fspath(path), [os.path.realpath(m) for m in _made]):
        _stop_daemons([os.fspath(path)])
    return _rmtree(path, *args, **kwargs)

shutil.rmtree = _stopping_rmtree

def _stop():
    _stop_daemons(_made)
    for made in _made:
        _rmtree(made, ignore_errors=True)

atexit.register(_stop)
