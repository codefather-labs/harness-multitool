"""Imported first by the debug scripts: when a script ends, even on a failed assertion, it stops the
daemons it started and removes its temp directories. Only daemons whose address file is under a temp directory this script made,
and only if that process still is a multitool."""
import atexit, glob, json, os, shutil, signal, tempfile, time

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

def _stop():
    for made in _made:
        for path in glob.glob(made + "/**/daemon/address.json", recursive=True):
            try:
                pid = json.load(open(path))["pid"]
                if _is_multitool(pid):
                    os.kill(pid, signal.SIGTERM)
            except (OSError, ValueError, KeyError):
                pass
    # What it made goes too, once what it stopped has let go.
    time.sleep(0.2)
    for made in _made:
        shutil.rmtree(made, ignore_errors=True)

atexit.register(_stop)
