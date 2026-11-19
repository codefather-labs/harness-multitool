#!/usr/bin/env bash
# Run harness-multitool from this checkout with its environment set.
# The future installer sets the same variables globally; until then this script does it.
#
#   ./run.sh                          # preview, starting at the splash
#   ./run.sh --graphics sixel         # force a picture protocol: auto|sixel|kitty|iterm2|off
#   ./run.sh --no-claude-code         # without Claude Code as a delegate agent (ADR 0020)
#   ./run.sh preview main-session     # any multitool arguments after the options
#   ./run.sh --under "<launcher> ..."  # inside a launcher that starts `claude` from the PATH and
#                                     # gives it an identity and messages: the harness takes its place
#   MULTITOOL_GRAPHICS=off ./run.sh   # variables already set win over the defaults below
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --graphics) MULTITOOL_GRAPHICS="$2"; shift 2 ;;
    --debug)    PROFILE=debug; shift ;;
    --claude-code) shift ;;
    --no-claude-code) FEATURES="--no-default-features"; shift ;;
    --under)    UNDER="$2"; shift 2 ;;
    *) break ;;
  esac
done

# --- environment ---------------------------------------------------------------------------
# How pictures (logo, icons, big text) are drawn. auto asks graphics-capable terminals.
export MULTITOOL_GRAPHICS="${MULTITOOL_GRAPHICS:-auto}"
# 24-bit colour: the themes are RGB.
export COLORTERM="${COLORTERM:-truecolor}"
# ---------------------------------------------------------------------------------------------

# Claude Code as an adapter is in the default build (ADR 0020); --no-claude-code leaves it out.
FEATURES="${FEATURES-}"
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
if [[ -n "${UNDER:-}" ]]; then
  # The launcher starts `claude` from the PATH: here that is this harness, with the PATH as it was,
  # so that a delegate finds the real one. Whatever the launcher gives it travels on to its sessions.
  shim="$(mktemp -d)"
  trap 'rm -rf "$shim"' EXIT
  args="$(printf '%q ' "$@")"
  printf '#!/usr/bin/env bash\nexport PATH=%q\nexec %q %s\n' "$PATH" "$bin" "$args" > "$shim/claude"
  chmod +x "$shim/claude"
  PATH="$shim:$PATH" $UNDER
  exit $?
fi
exec "$bin" "$@"
