#!/usr/bin/env bash
# ============================================================================
# harness-multitool installer (Linux and macOS)
# ============================================================================
# Builds the harness from source, puts it on your PATH, runs its daemon as a user
# service, and sets up Claude Code's rules and hook for it (CON-162). No releases
# exist yet, so a Rust toolchain is needed.
#
#   ./install.sh --local                  from this checkout
#   curl -fsSL <raw url>/install.sh | bash  from a temporary clone
#
# Speech to text is not installed here: it is chosen and downloaded in the
# harness's own settings, section Speech to text.
# ============================================================================
set -euo pipefail

REPO_URL="https://github.com/codefather-labs/harness-multitool.git"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/harness-multitool"
BIN_DIR="$DATA_HOME/bin"
LINK_DIR="$HOME/.local/bin"
# The one line this installer may add to a shell's profile, and how it finds it again.
PATH_MARK="# harness-multitool: multitool on PATH"

CLAUDE_DIR="$HOME/.claude"
SERVICE="multitool-daemon"
SYSTEMD_UNIT="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$SERVICE.service"
LAUNCH_AGENT="dev.multitool.daemon"
LAUNCH_PLIST="$HOME/Library/LaunchAgents/$LAUNCH_AGENT.plist"
# What the agent setup put in place, one path a line, for --uninstall to take back.
MANIFEST="$DATA_HOME/installed-claude-files"
HOOK_NAME="harness-selfcheck-reminder.sh"

ASSUME_YES=false
LOCAL_MODE=false
WITH_CLAUDE_CODE=true
UNINSTALL=false
WITH_SERVICE=true
WITH_AGENT_SETUP=true
WITH_LINGER=true
BUILD=true
FORCE=false
# How the installer waits for working agents before a daemon restart (CON-176), in seconds.
WAIT_STEP="${MULTITOOL_INSTALL_WAIT_STEP:-5}"
WAIT_MAX="${MULTITOOL_INSTALL_WAIT_MAX:-900}"
RESTART_SKIPPED=false

if [ -t 1 ]; then BOLD=$'\033[1m'; RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BLUE=$'\033[34m'; NC=$'\033[0m'; else BOLD=""; RED=""; GREEN=""; YELLOW=""; BLUE=""; NC=""; fi
log_info()  { echo "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo "${GREEN}  [OK]${NC} $1"; }
log_warn()  { echo "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo "${RED}[ERROR]${NC} $1" >&2; }

print_help() {
  cat <<HELP
harness-multitool installer

USAGE:
  ./install.sh [OPTIONS]

OPTIONS:
  --yes              Skip confirmation prompts
  --local            Build this checkout instead of cloning
  --no-claude-code   Without Claude Code as an adapter
  --no-service       Do not run the daemon as a user service
  --no-linger        Do not enable linger (Linux): the service then stops when you log out
  --no-agent-setup   Do not install Claude Code's rules and hook
  --no-build         With --local: install the build already in target/release
  --force            Restart the daemon even while agents work (their running commands end);
                     otherwise it asks, or with --yes waits up to 15 minutes for them
  --uninstall        Remove the binary, its link, the service and the agent setup;
                     keep your data and the backups
  --help             Show this help

WHAT GETS INSTALLED:
  $BIN_DIR/multitool
  $LINK_DIR/multitool   a link to it
  if $LINK_DIR is not on your PATH, and you agree: one line in your shell's
  profile that puts it there (zsh ~/.zshrc, bash ~/.bashrc or on macOS
  ~/.bash_profile, fish ~/.config/fish/conf.d, other shells ~/.profile)
  the daemon as a user service: $SYSTEMD_UNIT (Linux),
  or $LAUNCH_PLIST (macOS); with neither, it is started in the background
  $CLAUDE_DIR/rules/project-journal.md, cognitive-self-check.md and
  $CLAUDE_DIR/hooks/$HOOK_NAME, its UserPromptSubmit entry merged into
  $CLAUDE_DIR/settings.json; a file there that differs is backed up first
HELP
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) ASSUME_YES=true ;;
    --local) LOCAL_MODE=true ;;
    --no-claude-code) WITH_CLAUDE_CODE=false ;;
    --no-service) WITH_SERVICE=false ;;
    --no-linger) WITH_LINGER=false ;;
    --no-agent-setup) WITH_AGENT_SETUP=false ;;
    --no-build) BUILD=false ;;
    --force) FORCE=true ;;
    --uninstall) UNINSTALL=true ;;
    --help|-h) print_help; exit 0 ;;
    *) log_error "unknown option: $1"; print_help >&2; exit 2 ;;
  esac
  shift
done

# A prompt reads the terminal, so a piped install can still ask; with neither it is a no.
confirm() {
  [ "$ASSUME_YES" = true ] && return 0
  local answer=""
  if [ -r /dev/tty ]; then
    printf '%s [y/N] ' "$1" > /dev/tty
    read -r answer < /dev/tty || true
  fi
  case "$answer" in y|Y|yes) return 0 ;; *) return 1 ;; esac
}

# The profile file of the user's shell, by OS and shell.
profile_file() {
  case "$(basename "${SHELL:-sh}")" in
    zsh) echo "${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash) if [ "$(uname -s)" = Darwin ]; then echo "$HOME/.bash_profile"; else echo "$HOME/.bashrc"; fi ;;
    fish) echo "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/harness-multitool.fish" ;;
    *) echo "$HOME/.profile" ;;
  esac
}

path_line() {
  case "$1" in
    *.fish) echo "fish_add_path -g \"$LINK_DIR\" $PATH_MARK" ;;
    *) echo "export PATH=\"$LINK_DIR:\$PATH\" $PATH_MARK" ;;
  esac
}

# Puts LINK_DIR on PATH for new shells, once, if it is not there: asked first.
ensure_on_path() {
  case ":$PATH:" in *":$LINK_DIR:"*) return 0 ;; esac
  local file line
  file="$(profile_file)"
  line="$(path_line "$file")"
  if [ -f "$file" ] && grep -qF "$PATH_MARK" "$file"; then
    log_info "$file already puts $LINK_DIR on PATH: open a new terminal to use multitool"
    return 0
  fi
  log_warn "$LINK_DIR is not on your PATH"
  if confirm "Add this line to $file?
  $line
"; then
    mkdir -p "$(dirname "$file")"
    printf '\n%s\n' "$line" >> "$file"
    log_ok "added to $file: open a new terminal to use multitool"
  else
    log_warn "not added. To use multitool by its name, add this to your shell's profile:"
    log_warn "  $line"
  fi
}

# Takes back the line ensure_on_path added, and nothing else.
remove_from_path() {
  local file
  for file in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile" "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/harness-multitool.fish"; do
    [ -f "$file" ] && grep -qF "$PATH_MARK" "$file" || continue
    grep -vF "$PATH_MARK" "$file" > "$file.harness-multitool.tmp" && cat "$file.harness-multitool.tmp" > "$file" && rm -f "$file.harness-multitool.tmp"
    log_ok "removed the PATH line from $file"
  done
}

# ---------------------------------------------------------------------------
# The daemon as a user service (CON-162)
# ---------------------------------------------------------------------------
has_user_systemd() {
  [ "$(uname -s)" = Linux ] && command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1
}

# --- agents before a restart (CON-176) begin ---
# How many conversations the running daemon has working now; 0 when none answers.
busy_agents() {
  local out n
  out=$("$BIN_DIR/multitool" daemon --status 2>/dev/null) || { echo 0; return 0; }
  n=$(printf '%s' "$out" | sed -n 's/.*"busy":\([0-9][0-9]*\).*/\1/p')
  echo "${n:-0}"
}

# Whether the daemon may be restarted now: a restart ends every working agent's running command
# (journal#1285). Asked, the answer decides (default no); with --yes, or with no terminal to ask
# on, it waits for them to be idle, up to WAIT_MAX seconds; --force does not wait.
may_restart() {
  [ "$FORCE" = true ] && return 0
  local n waited=0
  n=$(busy_agents)
  [ "$n" -eq 0 ] && return 0
  if [ "$ASSUME_YES" != true ] && [ -r /dev/tty ]; then
    local answer=""
    printf '%s agent(s) are working: restarting the daemon now ends their running commands. Restart now? [y/N] ' "$n" > /dev/tty
    read -r answer < /dev/tty || true
    case "$answer" in y|Y|yes) return 0 ;; *) return 1 ;; esac
  fi
  while [ "$n" -gt 0 ]; do
    if [ "$waited" -ge "$WAIT_MAX" ]; then return 1; fi
    if [ $((waited % 30)) -eq 0 ]; then log_info "waiting: $n agent(s) working, the daemon restarts once they are idle (--force not to wait)"; fi
    sleep "$WAIT_STEP"
    waited=$((waited + WAIT_STEP))
    n=$(busy_agents)
  done
  return 0
}

restart_skipped() {
  RESTART_SKIPPED=true
  log_warn "agents are still working: the daemon is left running its old build; run multitool daemon restart once they are idle"
}
# --- agents before a restart (CON-176) end ---

# A daemon not run by the service makes way for the service's: one daemon a machine.
# macOS: the LaunchAgent takes over from a daemon that runs now (launchd starts it at bootstrap).
stop_running_daemon() {
  if "$BIN_DIR/multitool" daemon --status >/dev/null 2>&1; then
    log_info "stopping the daemon that runs now, so the LaunchAgent runs the only one"
    "$BIN_DIR/multitool" daemon stop >/dev/null 2>&1 || true
  fi
}

# The daemon that runs now makes way for the service's in one step: `daemon restart` stops it and
# restarts the service right after, so no window has a gap to start one of its own (journal#1193).
# It ends every run of that daemon, this installer's own caller's too when an agent runs it: an
# agent runs this from a transient unit (systemd-run --user --unit=...), which outlives its run.
move_to_service() {
  may_restart || { restart_skipped; return 0; }
  log_info "the daemon now runs as the service $SERVICE"
  "$BIN_DIR/multitool" daemon restart >/dev/null 2>&1 || systemctl --user start "$SERVICE" >/dev/null 2>&1 || { log_error "the service would not start: see systemctl --user status $SERVICE"; return 1; }
}

install_service() {
  if has_user_systemd; then
    mkdir -p "$(dirname "$SYSTEMD_UNIT")"
    # PATH as it is now: the daemon starts claude and codex, and a service's own PATH is bare.
    cat > "$SYSTEMD_UNIT" <<UNIT
[Unit]
Description=harness-multitool daemon
After=network-online.target

[Service]
ExecStart=$BIN_DIR/multitool daemon
ExecStop=$BIN_DIR/multitool daemon stop --pid \$MAINPID
Restart=on-failure
RestartSec=3
Environment=PATH=$PATH

[Install]
WantedBy=default.target
UNIT
    systemctl --user daemon-reload
    systemctl --user enable "$SERVICE" >/dev/null 2>&1 || { log_error "systemctl --user enable $SERVICE failed: see systemctl --user status $SERVICE"; return 1; }
    move_to_service || return 1
    [ "$RESTART_SKIPPED" = true ] || log_ok "the daemon runs as the user service $SERVICE"
    # Linger keeps the user's services running after logout and starts them at boot (CON-162).
    if [ "$WITH_LINGER" = true ] && [ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null)" != yes ]; then
      if loginctl enable-linger "$(id -un)" 2>/dev/null; then
        log_ok "linger is on: the daemon runs after you log out and starts at boot"
      else
        log_warn "could not enable linger: run loginctl enable-linger $(id -un), or the daemon stops when you log out"
      fi
    fi
  elif [ "$(uname -s)" = Darwin ]; then
    may_restart || { restart_skipped; return 0; }
    stop_running_daemon
    mkdir -p "$(dirname "$LAUNCH_PLIST")"
    cat > "$LAUNCH_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LAUNCH_AGENT</string>
  <key>ProgramArguments</key><array><string>$BIN_DIR/multitool</string><string>daemon</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>$PATH</string></dict>
</dict>
</plist>
PLIST
    launchctl bootout "gui/$(id -u)/$LAUNCH_AGENT" >/dev/null 2>&1 || true
    launchctl bootstrap "gui/$(id -u)" "$LAUNCH_PLIST" && log_ok "the daemon runs as the LaunchAgent $LAUNCH_AGENT"
  else
    log_warn "no user service manager here: the daemon is started in the background, and does not come back after a reboot"
    "$BIN_DIR/multitool" daemon --status >/dev/null 2>&1 || (setsid "$BIN_DIR/multitool" daemon >/dev/null 2>&1 < /dev/null &)
  fi
}

uninstall_service() {
  if [ -f "$SYSTEMD_UNIT" ]; then
    systemctl --user disable --now "$SERVICE" >/dev/null 2>&1 || true
    rm -f "$SYSTEMD_UNIT"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
    log_ok "removed the user service $SERVICE"
  fi
  if [ -f "$LAUNCH_PLIST" ]; then
    launchctl bootout "gui/$(id -u)/$LAUNCH_AGENT" >/dev/null 2>&1 || true
    rm -f "$LAUNCH_PLIST"
    log_ok "removed the LaunchAgent $LAUNCH_AGENT"
  fi
}

# ---------------------------------------------------------------------------
# Claude Code's agent setup (CON-162): the harness's rules and self-check hook
# ---------------------------------------------------------------------------
# A file of the user's that differs is kept beside it before it is replaced.
put_file() {
  local from="$1" to="$2"
  mkdir -p "$(dirname "$to")"
  if [ -f "$to" ] && ! cmp -s "$from" "$to"; then
    cp -p "$to" "$to.before-multitool-$(date +%Y%m%d%H%M%S)"
    log_info "backed up $to"
  fi
  cmp -s "$from" "$to" 2>/dev/null || cp "$from" "$to"
  grep -qxF "$to" "$MANIFEST" 2>/dev/null || echo "$to" >> "$MANIFEST"
  log_ok "$to"
}

# Adds the hook's UserPromptSubmit entry to settings.json, or takes it out: the rest is left as it is.
settings_hook() {
  local action="$1" settings="$CLAUDE_DIR/settings.json" hook="$CLAUDE_DIR/hooks/$HOOK_NAME"
  if ! command -v python3 >/dev/null 2>&1; then
    [ "$action" = add ] && log_warn "no python3 to edit $settings: add a UserPromptSubmit hook running $hook yourself" >&2
    return 0
  fi
  [ "$action" = remove ] && [ ! -f "$settings" ] && return 0
  python3 - "$action" "$settings" "$hook" <<'PY'
import json, os, shutil, sys, time
action, path, hook = sys.argv[1:]
settings = json.load(open(path)) if os.path.exists(path) else {}
groups = settings.setdefault("hooks", {}).setdefault("UserPromptSubmit", [])
present = any(h.get("command") == hook for g in groups for h in g.get("hooks", []))
if action == "add" and not present:
    groups.append({"hooks": [{"type": "command", "command": hook}]})
elif action == "remove" and present:
    for g in groups:
        g["hooks"] = [h for h in g.get("hooks", []) if h.get("command") != hook]
    settings["hooks"]["UserPromptSubmit"] = [g for g in groups if g["hooks"]]
    if not settings["hooks"]["UserPromptSubmit"]:
        del settings["hooks"]["UserPromptSubmit"]
else:
    sys.exit(0)
if os.path.exists(path):
    shutil.copy2(path, path + ".before-multitool-" + time.strftime("%Y%m%d%H%M%S"))
tmp = path + ".tmp"
open(tmp, "w").write(json.dumps(settings, indent=2) + "\n")
os.replace(tmp, path)
print("changed")
PY
}

agent_setup() {
  local src="$1/assets/claude"
  [ -d "$src" ] || { log_warn "no assets/claude in the source: Claude Code's agent setup skipped"; return 0; }
  mkdir -p "$DATA_HOME"
  put_file "$src/rules/project-journal.md" "$CLAUDE_DIR/rules/project-journal.md"
  put_file "$src/rules/cognitive-self-check.md" "$CLAUDE_DIR/rules/cognitive-self-check.md"
  put_file "$src/hooks/$HOOK_NAME" "$CLAUDE_DIR/hooks/$HOOK_NAME"
  chmod 755 "$CLAUDE_DIR/hooks/$HOOK_NAME"
  if [ "$(settings_hook add)" = changed ]; then log_ok "$CLAUDE_DIR/settings.json: the self-check hook added"; else log_ok "$CLAUDE_DIR/settings.json: the self-check hook is there"; fi
}

agent_unsetup() {
  if [ "$(settings_hook remove)" = changed ]; then log_ok "took the self-check hook out of $CLAUDE_DIR/settings.json"; fi
  [ -f "$MANIFEST" ] || return 0
  local file
  while IFS= read -r file; do
    if [ -n "$file" ] && [ -f "$file" ]; then rm -f "$file"; log_ok "removed $file"; fi
  done < "$MANIFEST"
  rm -f "$MANIFEST"
  log_info "kept the backups beside them (*.before-multitool-*): restore one by renaming it"
}

uninstall() {
  uninstall_service
  agent_unsetup
  local link="$LINK_DIR/multitool"
  if [ -L "$link" ] && [ "$(readlink "$link")" = "$BIN_DIR/multitool" ]; then
    rm -f "$link"
    log_ok "removed the link $link"
  fi
  if [ -e "$BIN_DIR/multitool" ]; then
    rm -f "$BIN_DIR/multitool"
    log_ok "removed $BIN_DIR/multitool"
  else
    log_info "nothing installed at $BIN_DIR/multitool"
  fi
  remove_from_path
  log_info "kept your data, delete it yourself if you want it gone:"
  log_info "  $DATA_HOME  (sessions, journals, speech models)"
  log_info "  ${MULTITOOL_HOME:-$HOME/.harness}  (profiles: settings and sign-ins; may hold a bot token)"
}

# Named with the exact command, and installed only on your word: a script that quietly runs sudo
# is a script nobody should pipe from the internet.
ensure_build_deps() {
  local missing=()
  command -v cargo >/dev/null 2>&1 || { log_error "no Rust toolchain: install it from https://rustup.rs and run this again"; exit 1; }
  command -v git >/dev/null 2>&1 || missing+=("git")
  command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || missing+=("a C compiler")
  [ ${#missing[@]} -eq 0 ] && return 0

  local sudo_="" cmd=""
  [ "$(id -u)" -ne 0 ] && sudo_="sudo "
  if command -v apt-get >/dev/null 2>&1; then cmd="${sudo_}apt-get update && ${sudo_}apt-get install -y build-essential git pkg-config"
  elif command -v dnf >/dev/null 2>&1; then cmd="${sudo_}dnf install -y gcc gcc-c++ git pkgconf-pkg-config"
  elif command -v pacman >/dev/null 2>&1; then cmd="${sudo_}pacman -S --needed --noconfirm base-devel git"
  elif command -v brew >/dev/null 2>&1; then cmd="brew install git"
  fi
  log_warn "needed to build, and missing: ${missing[*]}"
  if [ -z "$cmd" ]; then
    log_error "no package manager known here: install those yourself and run this again"
    exit 1
  fi
  log_info "this would run: $cmd"
  if confirm "Run it?"; then
    bash -c "$cmd"
  else
    log_error "not installed. Install them yourself and run this again"
    exit 1
  fi
}

source_dir() {
  if [ "$LOCAL_MODE" = true ]; then
    local here
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    [ -f "$here/Cargo.toml" ] || { log_error "--local: no Cargo.toml beside this script"; exit 1; }
    echo "$here"
    return
  fi
  CLONE_DIR="$(mktemp -d)"
  log_info "cloning $REPO_URL" >&2
  git clone --depth 1 -q "$REPO_URL" "$CLONE_DIR"
  echo "$CLONE_DIR"
}

CLONE_DIR=""
cleanup() { [ -n "$CLONE_DIR" ] && rm -rf "$CLONE_DIR"; return 0; }
trap cleanup EXIT

if [ "$UNINSTALL" = true ]; then
  uninstall
  exit 0
fi

if [ "$BUILD" = true ]; then ensure_build_deps; fi
src="$(source_dir)"

features=()
[ "$WITH_CLAUDE_CODE" = true ] && features+=("multitool/claude-code")
args=(build --release -p multitool)
if [ ${#features[@]} -gt 0 ]; then
  args+=(--features "$(IFS=,; echo "${features[*]}")")
fi

if [ "$BUILD" = true ]; then
  log_info "building: cargo ${args[*]}"
  (cd "$src" && cargo "${args[@]}")
else
  [ "$LOCAL_MODE" = true ] && [ -x "$src/target/release/multitool" ] || { log_error "--no-build needs --local and a build in target/release"; exit 1; }
  log_info "not building: installing $src/target/release/multitool as it is"
fi

# Beside the old one and moved over it, so a running harness keeps its file.
mkdir -p "$BIN_DIR" "$LINK_DIR"
cp "$src/target/release/multitool" "$BIN_DIR/multitool.new"
chmod 755 "$BIN_DIR/multitool.new"
mv "$BIN_DIR/multitool.new" "$BIN_DIR/multitool"
log_ok "installed $BIN_DIR/multitool"

if [ -e "$LINK_DIR/multitool" ] && [ ! -L "$LINK_DIR/multitool" ]; then
  log_warn "$LINK_DIR/multitool is a file of yours, left alone: run $BIN_DIR/multitool directly"
else
  ln -sf "$BIN_DIR/multitool" "$LINK_DIR/multitool"
  log_ok "linked $LINK_DIR/multitool"
fi

status=0
if "$BIN_DIR/multitool" --version >/dev/null; then
  log_ok "it runs: $("$BIN_DIR/multitool" --version)"
else
  log_error "the installed binary does not run"
  status=1
fi
ensure_on_path
if [ "$WITH_SERVICE" = true ]; then install_service || status=1; fi
if [ "$WITH_AGENT_SETUP" = true ]; then agent_setup "$src"; fi

# What works, before saying it does (CON-162).
echo ""
log_info "checking the install"
check() { if eval "$2" >/dev/null 2>&1; then log_ok "$1"; else log_error "$1: no"; status=1; fi; }
# What some features use, said and never counted as a failure (CON-176).
optional_tools() {
  [ "$(uname -s)" = Linux ] || return 0
  if command -v zenity >/dev/null 2>&1 || command -v kdialog >/dev/null 2>&1; then log_ok "a file picker for Settings > Chats' Load"; else log_warn "no zenity or kdialog: Settings > Chats' Load lists the archives in the window instead of opening a file picker"; fi
  if command -v xdg-terminal-exec >/dev/null 2>&1 || command -v ptyxis >/dev/null 2>&1 || command -v gnome-terminal >/dev/null 2>&1 || command -v x-terminal-emulator >/dev/null 2>&1; then log_ok "a terminal for Telegram's /open and /new"; else log_warn "no terminal program found for Telegram's /open and /new: name one in Settings > Telegram ([telegram] terminal)"; fi
  if has_user_systemd && command -v systemd-run >/dev/null 2>&1; then log_ok "systemd-run --user, for a scope per agent"; else log_warn "no systemd-run --user: agents run without scopes of their own"; fi
}
check "multitool runs" "\"$BIN_DIR/multitool\" --version"
if [ "$WITH_SERVICE" = true ]; then
  for _ in $(seq 1 30); do "$BIN_DIR/multitool" daemon --status >/dev/null 2>&1 && break; sleep 0.5; done
  check "the daemon answers" "\"$BIN_DIR/multitool\" daemon --status"
  if [ -f "$SYSTEMD_UNIT" ] && [ "$RESTART_SKIPPED" != true ]; then check "the service $SERVICE is active" "systemctl --user is-active $SERVICE"; fi
  if [ -f "$SYSTEMD_UNIT" ] && [ "$WITH_LINGER" = true ]; then check "linger is on" "[ \"\$(loginctl show-user \"\$(id -un)\" -p Linger --value)\" = yes ]"; fi
fi
optional_tools

if [ "$WITH_AGENT_SETUP" = true ] && [ -d "$src/assets/claude" ]; then
  check "the rules are in $CLAUDE_DIR/rules" "[ -f \"$CLAUDE_DIR/rules/project-journal.md\" ] && [ -f \"$CLAUDE_DIR/rules/cognitive-self-check.md\" ]"
  check "the self-check hook runs" "echo '{}' | \"$CLAUDE_DIR/hooks/$HOOK_NAME\""
  check "settings.json calls it" "grep -qF \"$CLAUDE_DIR/hooks/$HOOK_NAME\" \"$CLAUDE_DIR/settings.json\""
fi

echo ""
if [ "$status" -eq 0 ]; then echo "${BOLD}harness-multitool installed.${NC}"; else echo "${BOLD}harness-multitool installed, with the problems above.${NC}"; fi
echo ""
echo "  multitool                     open it; s opens the settings"
echo "  Settings, Provider            choose an adapter and sign in"
echo "  Settings, Telegram            a bot token, to reach your sessions from a chat"
echo "  Settings, Speech to text      choose and download an engine"
echo "  Settings, Chats               dump a conversation, load one from a .zip (multitool chat)"
echo "  ./install.sh --uninstall      remove it, keeping your data"
echo ""
exit "$status"
