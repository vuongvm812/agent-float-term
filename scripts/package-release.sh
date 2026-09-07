#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  printf '%s\n' 'Usage: bash scripts/package-release.sh TAG TARGET [OUTPUT_DIRECTORY]' >&2
  exit 2
fi

tag=$1
target=$2
output=${3:-dist}
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]] || {
  printf '%s\n' 'Expected a version tag beginning with v.' >&2
  exit 2
}
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu) ;;
  *) printf 'Unsupported release target: %s\n' "$target" >&2; exit 2 ;;
esac

host=$(rustc +1.84.1 -vV | sed -n 's/^host: //p')
if [[ "$host" != "$target" ]]; then
  printf 'Native builds only: host %s does not match target %s\n' "$host" "$target" >&2
  exit 1
fi

version=$(cargo +1.84.1 metadata --locked --no-deps --format-version 1 |
  python3 -c 'import json, sys; data = json.load(sys.stdin); print(next(p["version"] for p in data["packages"] if p["name"] == "agent-float-term"))')
if [[ "$tag" != "v$version" ]]; then
  printf 'Tag %s does not match package version %s\n' "$tag" "$version" >&2
  exit 1
fi

# Pin the output path and native target even if the caller has Cargo overrides.
CARGO_TARGET_DIR=target cargo +1.84.1 build --locked --release --target "$target"
name="agent-float-term-$tag-$target"
mkdir -p "$output"
if [[ -e "$output/$name" || -e "$output/$name.tar.gz" ]]; then
  printf 'Refusing to overwrite existing package: %s\n' "$name" >&2
  exit 1
fi
mkdir "$output/$name"
install -m 755 "target/$target/release/agent-float-term" "$output/$name/agent-float-term"
install -m 644 LICENSE README.md "$output/$name/"
tar -czf "$output/$name.tar.gz" -C "$output" "$name"
printf 'Created %s/%s.tar.gz\n' "$output" "$name"
