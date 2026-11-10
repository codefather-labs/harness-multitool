#!/usr/bin/env bash
# Debug: install.sh puts multitool on PATH by shell (journal#780, journal#781). For each of zsh, bash,
# fish and sh, in a temporary HOME whose PATH lacks ~/.local/bin: install with --yes, look for exactly
# one marked line in that shell's profile, install again and find still one, run multitool from a new
# shell that reads the profile, then uninstall and find the line, the link and the binary gone.
# The build is this checkout's (install.sh --local); Rust is found through CARGO_HOME and RUSTUP_HOME.
# Usage: tools/install_check.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"; rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
clean_path="$(echo "$PATH" | tr ':' '\n' | grep -v '/.local/bin' | paste -sd:)"
mark="harness-multitool: multitool on PATH"
failed=0
check() { if eval "$2"; then echo "  ok: $1"; else echo "  FAILED: $1"; failed=1; fi; }
marks() { grep -rl --exclude="*.log" "$mark" "$1" 2>/dev/null | wc -l; }
lines() { local n=0 f; for f in $(grep -rl --exclude="*.log" "$mark" "$1" 2>/dev/null); do n=$((n + $(grep -c "$mark" "$f"))); done; echo "$n"; }
for shell in zsh bash fish sh; do
  exe="$(command -v "$shell")" || { echo "== $shell: not installed here, skipped"; continue; }
  t="$(mktemp -d)"
  run_env=(env -i HOME="$t" PATH="$clean_path" SHELL="$exe" CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" TERM=dumb)
  echo "== SHELL=$exe"
  "${run_env[@]}" bash "$here/install.sh" --local --yes > "$t/first.log" 2>&1
  check "installed" '[ -x "$t/.local/share/harness-multitool/bin/multitool" ] && [ -L "$t/.local/bin/multitool" ]'
  check "one marked line" '[ "$(lines "$t")" = 1 ]'
  echo "  in: $(grep -rl "$mark" "$t" 2>/dev/null | sed "s|$t|~|")"
  "${run_env[@]}" bash "$here/install.sh" --local --yes > "$t/second.log" 2>&1
  check "a second run adds none" '[ "$(lines "$t")" = 1 ] && grep -q "already puts" "$t/second.log"'
  case "$shell" in
    zsh) found="$(env -i HOME="$t" PATH="$clean_path" zsh -c "source $t/.zshrc; multitool --version" 2>&1)" ;;
    bash) found="$(env -i HOME="$t" PATH="$clean_path" bash -c "source $t/.bashrc; multitool --version" 2>&1)" ;;
    fish) found="$(env -i HOME="$t" PATH="$clean_path" fish -c "multitool --version" 2>&1)" ;;
    *) found="$(env -i HOME="$t" PATH="$clean_path" sh -c ". $t/.profile; multitool --version" 2>&1)" ;;
  esac
  check "runs by its name from a new $shell: $found" '[[ "$found" == multitool* ]]'
  "${run_env[@]}" bash "$here/install.sh" --uninstall > "$t/uninstall.log" 2>&1
  check "uninstall removed the line, link and binary" '[ "$(marks "$t")" = 0 ] && [ ! -e "$t/.local/bin/multitool" ] && [ ! -e "$t/.local/share/harness-multitool/bin/multitool" ]'
  rm -rf -- "$t"
done
# With no terminal and no --yes nothing is added, and the line is told.
t="$(mktemp -d)"
env -i HOME="$t" PATH="$clean_path" SHELL=/bin/bash CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" TERM=dumb setsid --wait bash "$here/install.sh" --local < /dev/null > "$t/asked.log" 2>&1
echo "== no terminal, no --yes"
check "nothing added, the line told" '[ "$(marks "$t")" = 0 ] && grep -q "To use multitool by its name" "$t/asked.log"'
rm -rf -- "$t"
[ "$failed" = 0 ] && echo "ok" || { echo "FAILED"; exit 1; }
