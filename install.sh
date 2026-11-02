#!/usr/bin/env bash
# ============================================================================
# harness-multitool installer (Linux and macOS)
# ============================================================================
# Builds the harness from source and puts it on your PATH. No releases exist yet,
# so a Rust toolchain is needed.
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

ASSUME_YES=false
LOCAL_MODE=false
WITH_CLAUDE_CODE=true
UNINSTALL=false

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
  --uninstall        Remove the binary and its link, and keep your data
  --help             Show this help

WHAT GETS INSTALLED:
  $BIN_DIR/multitool
  $LINK_DIR/multitool   a link to it
HELP
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) ASSUME_YES=true ;;
    --local) LOCAL_MODE=true ;;
    --no-claude-code) WITH_CLAUDE_CODE=false ;;
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

uninstall() {
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
  log_info "kept your data, delete it yourself if you want it gone:"
  log_info "  $DATA_HOME  (sessions, journals, speech models)"
  log_info "  ${XDG_CONFIG_HOME:-$HOME/.config}/harness-multitool  (settings, may hold a bot token)"
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

ensure_build_deps
src="$(source_dir)"

features=()
[ "$WITH_CLAUDE_CODE" = true ] && features+=("multitool/claude-code")
args=(build --release -p multitool)
if [ ${#features[@]} -gt 0 ]; then
  args+=(--features "$(IFS=,; echo "${features[*]}")")
fi

log_info "building: cargo ${args[*]}"
(cd "$src" && cargo "${args[@]}")

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
case ":$PATH:" in
  *":$LINK_DIR:"*) ;;
  *) log_warn "$LINK_DIR is not on your PATH. Add this to your shell's profile:"
     log_warn "  export PATH=\"$LINK_DIR:\$PATH\"" ;;
esac

echo ""
echo "${BOLD}harness-multitool installed.${NC}"
echo ""
echo "  multitool                     open it; s opens the settings"
echo "  Settings, Provider            choose an adapter and sign in"
echo "  Settings, Telegram            a bot token, to reach your sessions from a chat"
echo "  Settings, Speech to text      choose and download an engine"
echo "  ./install.sh --uninstall      remove it, keeping your data"
echo ""
exit "$status"
