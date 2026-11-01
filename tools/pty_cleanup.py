"""Imported first by the debug scripts: when a script ends, even on a failed assertion, it stops the
daemons it started. Only daemons whose address file is under a temp directory this script made,
and only if that process still is a multitool."""
import atexit, glob, json, os, signal, tempfile

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

atexit.register(_stop)
