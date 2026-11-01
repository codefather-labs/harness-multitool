#!/usr/bin/env bash
# Run harness-multitool from this checkout with its environment set.
# The future installer sets the same variables globally; until then this script does it.
#
#   ./run.sh                          # preview, starting at the splash
#   ./run.sh --graphics sixel         # force a picture protocol: auto|sixel|kitty|iterm2|off
#   ./run.sh --claude-code            # with Claude Code as a delegate agent (ADR 0020)
#   ./run.sh preview main-session     # any multitool arguments after the options
#   MULTITOOL_GRAPHICS=off ./run.sh   # variables already set win over the defaults below
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --graphics) MULTITOOL_GRAPHICS="$2"; shift 2 ;;
    --debug)    PROFILE=debug; shift ;;
    --claude-code) FEATURES="--features multitool/claude-code"; shift ;;
    *) break ;;
  esac
done

# --- environment ---------------------------------------------------------------------------
# How pictures (logo, icons, big text) are drawn. auto asks graphics-capable terminals.
export MULTITOOL_GRAPHICS="${MULTITOOL_GRAPHICS:-auto}"
# 24-bit colour: the themes are RGB.
export COLORTERM="${COLORTERM:-truecolor}"
# ---------------------------------------------------------------------------------------------

profile="${PROFILE:-release}"
bin="$here/target/$profile/multitool"
if [[ "$profile" == release ]]; then
  cargo build --release ${FEATURES:-} --manifest-path "$here/Cargo.toml"
else
  cargo build ${FEATURES:-} --manifest-path "$here/Cargo.toml"
fi

if [[ $# -eq 0 ]]; then
  set -- preview splash
fi
exec "$bin" "$@"
