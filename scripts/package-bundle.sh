#!/usr/bin/env bash
# Packs a built multitool into the bundle the install line downloads (CON-186):
#   scripts/package-bundle.sh <target> <version> <binary> <out dir>
# The bundle is shaped like a checkout, so the bundled install.sh installs it with
# `install.sh --local --no-build`: install.sh, Cargo.toml, assets/claude/,
# target/release/multitool and VERSION. Beside it goes its .sha256.
# A Linux binary that needs a glibc newer than GLIBC_MAX is refused: it would not run on the
# systems the install line promises.
set -euo pipefail
cd "$(dirname "$0")/.."
usage="usage: package-bundle.sh <target> <version> <binary> <out dir>"
target="${1:?$usage}"
version="${2:?$usage}"
binary="${3:?$usage}"
out="${4:?$usage}"
GLIBC_MAX="${GLIBC_MAX:-2.31}"

case "$target" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | aarch64-apple-darwin) ;;
  *) echo "package-bundle: no bundle for $target" >&2; exit 1 ;;
esac
[[ "$version" =~ ^[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}$ ]] || { echo "package-bundle: a bad version: $version" >&2; exit 1; }
[ -x "$binary" ] || { echo "package-bundle: $binary is not an executable" >&2; exit 1; }

if [[ "$target" == *-linux-gnu ]]; then
  # The newest GLIBC_x.y the binary asks for.
  need="$(objdump -T "$binary" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sed 's/GLIBC_//' | sort -t. -k1,1n -k2,2n | tail -n 1)"
  [ -n "$need" ] || { echo "package-bundle: $binary links no glibc symbol: not a $target build?" >&2; exit 1; }
  newest="$(printf '%s\n%s\n' "$need" "$GLIBC_MAX" | sort -t. -k1,1n -k2,2n | tail -n 1)"
  if [ "$newest" != "$GLIBC_MAX" ]; then
    echo "package-bundle: $binary needs glibc $need, newer than $GLIBC_MAX: build it in the release container (scripts/release-local.sh)" >&2
    exit 1
  fi
  echo "package-bundle: $binary needs glibc $need (at most $GLIBC_MAX)"
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/target/release" "$stage/assets"
cp install.sh Cargo.toml "$stage/"
cp -R assets/claude "$stage/assets/claude"
cp "$binary" "$stage/target/release/multitool"
chmod 755 "$stage/target/release/multitool" "$stage/install.sh"
echo "$version" >"$stage/VERSION"
mkdir -p "$out"
name="multitool-$version-$target.tar.gz"
if tar --version 2>/dev/null | grep -q GNU; then owner=(--owner=0 --group=0); else owner=(--uid 0 --gid 0); fi
tar -czf "$out/$name" -C "$stage" "${owner[@]}" .
(cd "$out" && if command -v sha256sum >/dev/null 2>&1; then sha256sum "$name"; else shasum -a 256 "$name"; fi) >"$out/$name.sha256"
echo "package-bundle: $out/$name ($(du -h "$out/$name" | cut -f1))"
