#!/usr/bin/env bash
# Test of CON-176: install.sh's wait for working agents before a daemon restart, and its optional
# tool checks, against a stub multitool. Runs the functions out of install.sh itself, between its
# markers, with short timings. Usage: tools/install_wait_test.sh
set -uo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d /tmp/mt-install-test.XXXXXX)
trap 'rm -rf "$tmp"' EXIT
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; fails=$((fails + 1)); }

# The stub: `daemon --status` answers the next busy count of $tmp/busy (one per line, the last kept),
# or exits 1 when $tmp/busy says none.
BIN_DIR="$tmp/bin"; mkdir -p "$BIN_DIR"
cat > "$BIN_DIR/multitool" <<STUB
#!/usr/bin/env bash
first=\$(head -n1 "$tmp/busy")
[ "\$first" = none ] && exit 1
[ "\$(wc -l < "$tmp/busy")" -gt 1 ] && sed -i 1d "$tmp/busy"
echo "{\"conversations\":{\"busy\":\$first,\"windows\":2},\"pid\":1}"
STUB
chmod +x "$BIN_DIR/multitool"

log_info() { echo "[INFO] $1"; }
log_warn() { echo "[WARN] $1"; }
eval "$(sed -n '/^# --- agents before a restart (CON-176) begin ---$/,/^# --- agents before a restart (CON-176) end ---$/p' "$here/install.sh")"
WAIT_STEP=1; ASSUME_YES=true; FORCE=false

printf 'none\n' > "$tmp/busy"
[ "$(busy_agents)" = 0 ] && pass "no daemon: busy 0" || fail "no daemon: busy 0"

printf '0\n' > "$tmp/busy"
WAIT_MAX=3; start=$SECONDS
may_restart && [ $((SECONDS - start)) -lt 1 ] && pass "idle: restart at once" || fail "idle: restart at once"

printf '2\n1\n0\n' > "$tmp/busy"
WAIT_MAX=10; start=$SECONDS
out=$(may_restart); rc=$?
[ "$rc" -eq 0 ] && [ $((SECONDS - start)) -ge 2 ] && grep -q "waiting: 2 agent(s) working" <<<"$out" && pass "busy then idle: waited, then restart ($((SECONDS - start)) s)" || fail "busy then idle: rc=$rc out=$out"

printf '1\n' > "$tmp/busy"
WAIT_MAX=3; start=$SECONDS
may_restart >/dev/null; rc=$?
[ "$rc" -eq 1 ] && [ $((SECONDS - start)) -ge 3 ] && pass "busy throughout: gave up after WAIT_MAX, no restart" || fail "busy throughout: rc=$rc"

FORCE=true; start=$SECONDS
may_restart && [ $((SECONDS - start)) -lt 1 ] && pass "--force: no wait" || fail "--force: no wait"
FORCE=false

RESTART_SKIPPED=false; out=$(restart_skipped; echo "skipped=$RESTART_SKIPPED")
grep -q "multitool daemon restart" <<<"$out" && grep -q "skipped=true" <<<"$out" && pass "skipped: says how to restart later" || fail "skipped: $out"

# The optional tools never fail the install: only zenity (and a uname saying Linux) on PATH.
mkdir -p "$tmp/path"
printf '#!/bin/sh\n' > "$tmp/path/zenity"; printf '#!/bin/sh\necho Linux\n' > "$tmp/path/uname"; chmod +x "$tmp/path/zenity" "$tmp/path/uname"
eval "$(sed -n '/^optional_tools() {$/,/^}$/p' "$here/install.sh")"
has_user_systemd() { return 1; }
log_ok() { echo "[OK] $1"; }
status=0
out=$(PATH="$tmp/path"; optional_tools; echo "status=$status")
grep -q "\[OK\] a file picker" <<<"$out" && grep -q "\[WARN\] no terminal program" <<<"$out" && grep -q "\[WARN\] no systemd-run" <<<"$out" && grep -q "status=0" <<<"$out" && pass "optional tools: warnings only, status untouched" || fail "optional tools: $out"

echo
[ "$fails" -eq 0 ] && echo "ALL PASSED" || { echo "FAILED: $fails"; exit 1; }
