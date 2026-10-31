#!/usr/bin/env bash
# Run harness-multitool from this checkout with its environment set.
# The future installer sets the same variables globally; until then this script does it.
#
#   ./run.sh                          # preview, starting at the splash
#   ./run.sh --graphics sixel         # force a picture protocol: auto|sixel|kitty|iterm2|off
#   ./run.sh --kitty                  # open a kitty window and run there (pictures on)
#   ./run.sh preview main-session     # any multitool arguments after the options
#   MULTITOOL_GRAPHICS=off ./run.sh   # variables already set win over the defaults below
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

kitty=0
passed=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --graphics) MULTITOOL_GRAPHICS="$2"; passed+=("$1" "$2"); shift 2 ;;
    --debug)    PROFILE=debug; passed+=("$1"); shift ;;
    --kitty)    kitty=1; shift ;;
    *) break ;;
  esac
done

# --kitty: kitty draws pictures (the logo, icons, mind maps); most VTE terminals do not.
# Build here, where errors are visible, then open a kitty window running this script.
if [[ $kitty -eq 1 && -z "${KITTY_WINDOW_ID:-}" ]]; then
  if ! command -v kitty >/dev/null; then
    echo "kitty is not installed (Ubuntu: sudo apt install kitty)" >&2
    exit 1
  fi
  cargo build --quiet --release --manifest-path "$here/Cargo.toml"
  kitty --directory "$here" --title harness-multitool "$here/run.sh" "${passed[@]}" "$@" >/dev/null 2>&1 &
  disown
  echo "opened in a kitty window"
  exit 0
fi

# --- environment ---------------------------------------------------------------------------
# How pictures (logo, icons, big text) are drawn. auto asks graphics-capable terminals.
export MULTITOOL_GRAPHICS="${MULTITOOL_GRAPHICS:-auto}"
# 24-bit colour: the themes are RGB.
export COLORTERM="${COLORTERM:-truecolor}"
# ---------------------------------------------------------------------------------------------

profile="${PROFILE:-release}"
bin="$here/target/$profile/multitool"
if [[ "$profile" == release ]]; then
  cargo build --quiet --release --manifest-path "$here/Cargo.toml"
else
  cargo build --quiet --manifest-path "$here/Cargo.toml"
fi

if [[ $# -eq 0 ]]; then
  set -- preview splash
fi
exec "$bin" "$@"
