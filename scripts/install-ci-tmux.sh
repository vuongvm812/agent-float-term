#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  printf '%s\n' 'Usage: bash scripts/install-ci-tmux.sh 3.3a|3.7c' >&2
  exit 2
fi
version=$1
# Official release archives downloaded and hashed; see docs/compatibility.md.
case "$version" in
  3.3a) sha256=e4fd347843bd0772c4f48d6dde625b0b109b7a380ff15db21e97c11a4dcdf93f ;;
  3.7c) sha256=7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf ;;
  *) printf 'Unsupported CI tmux version: %s\n' "$version" >&2; exit 2 ;;
esac

if [[ ${GITHUB_ACTIONS:-} != true || ${RUNNER_OS:-} != Linux ||
      ${RUNNER_TEMP:-} != /* || ! -d ${RUNNER_TEMP:-} ||
      ! -f ${GITHUB_PATH:-} || ! -w ${GITHUB_PATH:-} ]]; then
  printf '%s\n' 'This installer requires a Linux GitHub Actions runner and its temporary directories.' >&2
  exit 1
fi

# Never install into a host prefix or start a server. The runner owns all cleanup.
work=$(mktemp -d "$RUNNER_TEMP/aft-tmux-$version.XXXXXXXX")
prefix="$work/install"
archive="$work/tmux-$version.tar.gz"
curl --fail --location --silent --show-error --retry 3 \
  --output "$archive" "https://github.com/tmux/tmux/releases/download/$version/tmux-$version.tar.gz"
printf '%s  %s\n' "$sha256" "$archive" | sha256sum --check --strict
tar -xzf "$archive" -C "$work"
(
  cd "$work/tmux-$version"
  ./configure --prefix="$prefix"
  make -j2
  make install
)
test "$("$prefix/bin/tmux" -V)" = "tmux $version"
printf '%s\n' "$prefix/bin" >> "$GITHUB_PATH"
printf 'Installed tmux %s in runner temporary prefix %s\n' "$version" "$prefix"
