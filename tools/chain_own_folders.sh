#!/bin/sh
# journal#1025: a chain of multitool processes keeps the user's own .claude.json.
# Usage: tools/chain_own_folders.sh <absolute path to multitool>
BIN="$1"; T=$(mktemp -d /tmp/mt-1025-XXXX); mkdir -p "$T/bin" "$T/cfg" "$T/data"
cat > "$T/bin/claude" <<'F'
#!/bin/sh
env | grep -E '^(CLAUDE_CONFIG_DIR|MULTITOOL_USERS_CLAUDE_JSON|MULTITOOL_OWN_[A-Z_]*)=' | sort > "$DUMP"
F
chmod +x "$T/bin/claude"
echo '{"mcpServers":{"probe":{"command":"true"}}}' > "$T/.claude.json"
base="HOME=$T XDG_CONFIG_HOME=$T/cfg XDG_DATA_HOME=$T/data PATH=$T/bin:/usr/bin:/bin"
env -i $base "$BIN" profile add work >/dev/null 2>&1
run() { env -i $base DUMP="$1" "$@" >/dev/null 2>&1; }
# 1. Started by the user, no CLAUDE_CONFIG_DIR of their own.
env -i $base DUMP=$T/one "$BIN" --profile work login claude-code >/dev/null 2>&1
echo "first:"; cat $T/one
# 2. A child started with what the first one set in its environment (as a reloaded window is).
P=$(grep '^CLAUDE_CONFIG_DIR=' $T/one | cut -d= -f2-)
M=$(grep '^MULTITOOL_OWN_CLAUDE_CONFIG_DIR=' $T/one | cut -d= -f2-)
env -i $base DUMP=$T/two CLAUDE_CONFIG_DIR="$P" MULTITOOL_OWN_CLAUDE_CONFIG_DIR="$M" "$BIN" --profile work login claude-code >/dev/null 2>&1
echo "child:"; cat $T/two
# 3. A chain from before the markers: the profile's folder inherited, no marker.
env -i $base DUMP=$T/three CLAUDE_CONFIG_DIR="$P" "$BIN" --profile work login claude-code >/dev/null 2>&1
echo "old chain:"; cat $T/three
# 4. A user with a CLAUDE_CONFIG_DIR of their own, and a child of that run.
mkdir -p "$T/mine"; echo '{}' > "$T/mine/.claude.json"
env -i $base DUMP=$T/four CLAUDE_CONFIG_DIR="$T/mine" "$BIN" --profile work login claude-code >/dev/null 2>&1
M4=$(grep '^MULTITOOL_OWN_CLAUDE_CONFIG_DIR=' $T/four | cut -d= -f2-)
env -i $base DUMP=$T/five CLAUDE_CONFIG_DIR="$P" MULTITOOL_OWN_CLAUDE_CONFIG_DIR="$M4" "$BIN" --profile work login claude-code >/dev/null 2>&1
echo "own folder, and its child:"; cat $T/four $T/five | grep USERS
own=$(grep -l "^MULTITOOL_USERS_CLAUDE_JSON=$T/mine/.claude.json$" $T/four $T/five 2>/dev/null | wc -l); echo "own folder kept: $own of 2"
echo "want MULTITOOL_USERS_CLAUDE_JSON=$T/.claude.json in all three"
n=$(grep -l "^MULTITOOL_USERS_CLAUDE_JSON=$T/.claude.json$" $T/one $T/two $T/three 2>/dev/null | wc -l); echo "passed $n of 3"
rm -rf "$T"; [ "$n" = 3 ] && [ "$own" = 2 ]
