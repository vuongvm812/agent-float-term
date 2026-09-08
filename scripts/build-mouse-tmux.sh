#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf '%s\n' \
    'Usage: bash scripts/build-mouse-tmux.sh NEW_BUILD_DIR [LOCAL_ARCHIVE]' \
    'Build tmux 3.7c with opt-in display-popup -M status mouse support.' \
    'The parent must exist and NEW_BUILD_DIR must not exist (created mode 0700).' \
    'Requires a C compiler, make, pkg-config, libevent, ncurses, tar and patch.' \
    'Uses a pinned official archive; curl is needed only without LOCAL_ARCHIVE.' \
    'Keeps sources and build.log; never installs, runs tmux, or changes live config.' \
    'On success, stdout contains only the executable path for explicit later use.'
}

die() {
  printf 'Error: %s\n' "$*" >&2
  exit 1
}

if [[ $# -eq 1 && ( $1 == --help || $1 == -h ) ]]; then
  usage
  exit 0
fi
if [[ $# -lt 1 || $# -gt 2 || -z $1 ]]; then
  usage >&2
  exit 2
fi

version=3.7c
sha256=7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf
build=$1
[[ $build == /* ]] || build="$PWD/$build"
while [[ $build != / && $build == */ ]]; do
  build=${build%/}
done
parent=$(dirname -- "$build")
[[ -d $parent ]] || die "Parent directory does not exist: $parent"
parent=$(cd -- "$parent" && pwd -P)
build="$parent/$(basename -- "$build")"
[[ ! -e $build && ! -L $build ]] || die "Destination already exists: $build"

local_archive=${2:-}
if [[ $# -eq 2 ]]; then
  [[ -f $local_archive && -r $local_archive ]] || die "Archive is not a readable file: $local_archive"
  [[ $local_archive == /* ]] || local_archive="$PWD/$local_archive"
fi

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
patch_file="$script_dir/../patches/tmux-$version-status-mouse.patch"
[[ -f $patch_file && -r $patch_file ]] || die "Patch is not a readable file: $patch_file"
for tool in tar patch make; do
  command -v "$tool" >/dev/null 2>&1 || die "Required tool not found: $tool"
done
if command -v shasum >/dev/null 2>&1; then
  hash_command=(shasum -a 256)
elif command -v sha256sum >/dev/null 2>&1; then
  hash_command=(sha256sum)
else
  die 'Either shasum or sha256sum is required.'
fi
if [[ -z $local_archive ]]; then
  command -v curl >/dev/null 2>&1 || die 'Required tool not found: curl'
fi

umask 077
mkdir -m 0700 -- "$build"
printf 'Building patched tmux; full log: %s/build.log\n' "$build" >&2
report_failure() {
  local status=$?
  printf 'Build failed (exit %s); retained files and log: %s\n' "$status" "$build" >&2
  # Preserve quiet successful builds, but expose the useful end of failures in CI.
  tail -n 80 "$build/build.log" >&2 || true
  exit "$status"
}
trap report_failure ERR
(
  archive="$build/tmux-$version.tar.gz"
  if [[ -n $local_archive ]]; then
    cp -- "$local_archive" "$archive"
  else
    curl --disable --fail --location --silent --show-error \
      --connect-timeout 15 --max-time 45 --retry 1 --retry-delay 1 --retry-max-time 45 \
      --output "$archive" \
      "https://github.com/tmux/tmux/releases/download/$version/tmux-$version.tar.gz"
  fi
  actual_sha256=$("${hash_command[@]}" < "$archive")
  actual_sha256=${actual_sha256%% *}
  [[ $actual_sha256 == "$sha256" ]] || die "SHA-256 mismatch for $archive"
  printf 'Verified tmux %s archive SHA-256: %s\n' "$version" "$sha256"

  tar -xzf "$archive" -C "$build"
  cd -- "$build/tmux-$version"
  patch --batch --forward --fuzz=0 -p1 < "$patch_file"

  # Caller paths take priority; Homebrew is queried, never asked to install.
  if [[ $(uname -s) == Darwin ]] && command -v brew >/dev/null 2>&1; then
    for dependency in libevent ncurses; do
      if prefix=$(brew --prefix "$dependency" 2>/dev/null); then
        if [[ -d $prefix/lib/pkgconfig ]]; then
          export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:+$PKG_CONFIG_PATH:}$prefix/lib/pkgconfig"
        fi
      fi
    done
  fi

  ./configure --prefix="$build/install"
  make -j2
  [[ -x $build/tmux-$version/tmux ]] || die 'Build did not produce a tmux executable.'
) > "$build/build.log" 2>&1
printf '%s\n' "$build/tmux-$version/tmux"
