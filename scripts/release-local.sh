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
#   scripts/release-local.sh --no-github  publish on the site only: no tag, no GitHub Release
#
# A release is made only from main as GitHub has it (HEAD = origin/main). After the site has the
# bundle, the commit is tagged v<version> and a GitHub Release of that tag holds the bundle and its
# checksum (CON-193). A rerun on the same commit keeps the tag and replaces the assets.
#
# Settings: DEPLOY_HOST (harnessmt-deploy@95.179.242.74), RELEASE_MEMORY (6g),
# RELEASE_GH_CONFIG_DIR (gh's folder for the repository's account, ~/.config/gh-labs).
set -euo pipefail
cd "$(dirname "$0")/.."
RELEASE_IMAGE="${RELEASE_IMAGE:-rust:1-bullseye}"
DEPLOY_HOST="${DEPLOY_HOST:-harnessmt-deploy@95.179.242.74}"
RELEASE_MEMORY="${RELEASE_MEMORY:-6g}"
TARGET=x86_64-unknown-linux-gnu
dirty=false
upload=true
github=true
for a in "$@"; do
  case "$a" in
    --dirty) dirty=true ;;
    --no-upload) upload=false ;;
    --no-github) github=false ;;
    *) echo "release-local: unknown option $a" >&2; exit 1 ;;
  esac
done
die() { echo "release-local: $*" >&2; exit 1; }

[ "$(uname -m)" = x86_64 ] || die "this script builds $TARGET on an x86_64 machine"
command -v docker >/dev/null || die "docker is needed"
if [ "$dirty" != true ]; then
  [ "$(git rev-parse --abbrev-ref HEAD)" = main ] || die "not on main (--dirty for a test release)"
  [ -z "$(git status --porcelain --untracked-files=no -- crates Cargo.toml Cargo.lock install.sh assets)" ] || die "uncommitted changes (--dirty for a test release)"
  # Only what GitHub has is released (CON-193).
  git fetch -q origin main || die "cannot reach origin to check that main is pushed"
  [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "HEAD $(git rev-parse --short HEAD) is not origin/main $(git rev-parse --short origin/main): push main first"
else
  github=false
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

# The tag and the GitHub Release of this version (CON-193).
if [ "$upload" = true ] && [ "$github" = true ]; then
  tag="v$version"
  head="$(git rev-parse HEAD)"
  remote="$(git ls-remote --tags origin "refs/tags/$tag^{}" | cut -f1)"
  [ -n "$remote" ] || remote="$(git ls-remote --tags origin "refs/tags/$tag" | cut -f1)"
  if [ -z "$remote" ]; then
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null || git tag -a "$tag" -m "multitool $version" HEAD
    git push -q origin "refs/tags/$tag"
    echo "release-local: tagged $tag"
  elif [ "$remote" != "$head" ]; then
    die "the tag $tag on origin points at $remote, not at HEAD $head"
  else
    echo "release-local: $tag is already at HEAD"
  fi
  export GH_CONFIG_DIR="${RELEASE_GH_CONFIG_DIR:-$HOME/.config/gh-labs}"
  repo="$(git config remote.origin.url | sed -E 's#^(git@|ssh://git@|https://)github\.com[:/]##; s#\.git$##')"
  assets=("$out"/multitool-"$version"-"$TARGET".tar.gz "$out"/multitool-"$version"-"$TARGET".tar.gz.sha256)
  if gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
    gh release upload "$tag" "${assets[@]}" -R "$repo" --clobber
    echo "release-local: the assets of $tag replaced"
  else
    previous="$(git tag -l 'v*' --sort=-creatordate | grep -vx "$tag" | sed -n 1p || true)"
    notes="$(mktemp)"
    {
      echo "multitool $version, built from $(git rev-parse --short HEAD) of main."
      echo
      echo "Install:"
      echo
      echo '```'
      echo "curl --proto '=https' --tlsv1.2 -sSf https://harness-multitool.dev/install.sh | sh"
      echo '```'
      echo
      if [ -n "$previous" ]; then echo "Changes since $previous:"; else echo "Changes:"; fi
      echo
      # --max-count, not head: with pipefail a closed pipe would end the script (SIGPIPE).
      git log --no-merges --max-count=200 --format='- %s (%h)' ${previous:+"$previous"..}HEAD
    } >"$notes"
    pre=()
    case "$version" in *-*) pre=(--prerelease) ;; esac
    gh release create "$tag" "${assets[@]}" -R "$repo" --title "multitool $version" --notes-file "$notes" "${pre[@]}" --verify-tag
    rm -f "$notes"
    echo "release-local: GitHub Release $tag made"
  fi
fi
