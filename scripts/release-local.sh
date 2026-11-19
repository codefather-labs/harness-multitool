#!/usr/bin/env bash
# Publishes a Linux x86_64 multitool for the install line from this machine (CON-186).
#
# TEMPORARY: it stands in for .github/workflows/release.yml while GitHub Actions cannot run on
# this account. It builds in the same container image as the workflow (RELEASE_IMAGE, glibc
# 2.31), so the binary runs on older systems than this machine's, packs it with
# scripts/package-bundle.sh and sends it to the site's deploy command.
#
#   scripts/release-local.sh            version <Cargo version>+<short commit>, from a clean main
#   scripts/release-local.sh --dirty    allow uncommitted changes (a test release)
#   scripts/release-local.sh --no-upload  build and pack only, into target/bundles
#
# Settings: DEPLOY_HOST (harnessmt-deploy@95.179.242.74), RELEASE_MEMORY (6g).
set -euo pipefail
cd "$(dirname "$0")/.."
RELEASE_IMAGE="${RELEASE_IMAGE:-rust:1-bullseye}"
DEPLOY_HOST="${DEPLOY_HOST:-harnessmt-deploy@95.179.242.74}"
RELEASE_MEMORY="${RELEASE_MEMORY:-6g}"
TARGET=x86_64-unknown-linux-gnu
dirty=false
upload=true
for a in "$@"; do
  case "$a" in
    --dirty) dirty=true ;;
    --no-upload) upload=false ;;
    *) echo "release-local: unknown option $a" >&2; exit 1 ;;
  esac
done
die() { echo "release-local: $*" >&2; exit 1; }

[ "$(uname -m)" = x86_64 ] || die "this script builds $TARGET on an x86_64 machine"
command -v docker >/dev/null || die "docker is needed"
if [ "$dirty" != true ]; then
  [ "$(git rev-parse --abbrev-ref HEAD)" = main ] || die "not on main (--dirty for a test release)"
  [ -z "$(git status --porcelain --untracked-files=no -- crates Cargo.toml Cargo.lock install.sh assets)" ] || die "uncommitted changes (--dirty for a test release)"
fi
cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
version="$cargo_version+$(git rev-parse --short HEAD)"
[ "$dirty" = true ] && [ -n "$(git status --porcelain --untracked-files=no -- crates Cargo.toml Cargo.lock install.sh assets)" ] && version="$version.dirty"
echo "release-local: multitool $version for $TARGET in $RELEASE_IMAGE"

# The build, in the container: the workspace read-only, its own target dir and cargo cache.
mkdir -p target/release-container
docker run --rm --memory "$RELEASE_MEMORY" --memory-swap "$RELEASE_MEMORY" \
  -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/out -e OWNER="$(id -u):$(id -g)" \
  -v "$PWD:/src:ro" -v "$PWD/target/release-container:/out" -v multitool-release-cargo:/cargo \
  -w /src "$RELEASE_IMAGE" \
  sh -c 'export PATH=/usr/local/cargo/bin:$PATH; cargo --version; cargo build --release --locked -p multitool -j 4; s=$?; chown -R "$OWNER" /out; exit $s'

out=target/bundles
rm -rf "$out"
scripts/package-bundle.sh "$TARGET" "$version" target/release-container/release/multitool "$out"
if [ "$upload" = true ]; then
  tar -czf - -C "$out" . | ssh -o BatchMode=yes "$DEPLOY_HOST" bundle "$version"
  curl -fsS https://harness-multitool.dev/downloads/latest
fi
