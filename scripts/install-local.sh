#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 0 || ${DRY_RUN:-0} != 0 && ${DRY_RUN:-0} != 1 ]]; then
  printf '%s\n' 'Usage: make install [DRY_RUN=1]' >&2
  exit 2
fi
if [[ ${DRY_RUN:-0} == 1 ]]; then
  printf '%s\n' \
    'Plan only: build the Rust release and preview the managed application install.' \
    'Build pinned, patched tmux in a new private directory under the XDG data root.' \
    'Apply the application install only after the tmux build succeeds.' \
    'Print a separate-server launch command; do not launch, restart, or replace any server.'
  exit 0
fi

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "$repo"
data_home=${XDG_DATA_HOME:-${HOME:?HOME must be set}/.local/share}
if [[ $data_home != /* ]]; then
  printf '%s\n' 'XDG_DATA_HOME must be absolute.' >&2
  exit 1
fi

cargo +1.84.1 build --locked --release
app="$repo/target/release/agent-float-term"
# Refuse incompatible package registrations before building tmux. The installer's
# apply phase verifies owned files under its own lock before changing them.
"$app" install

umask 077
compat="$data_home/agent-float-term/compat"
mkdir -p -- "$compat"
work=$(mktemp -d "$compat/tmux-status-mouse.XXXXXXXX")
# Never overwrite a tmux image: a previous build may still own a live server.
patched=$(bash "$repo/scripts/build-mouse-tmux.sh" "$work/build")
"$app" install --yes

printf '\nPatched tmux: %s\n' "$patched"
printf '%s\n' \
  'Installation complete. Existing tmux servers and sessions were not changed.' \
  'From a terminal outside tmux, start a separate server (use an unused server name):'
printf 'AFT_TMUX_BINARY=%q %q -L aft-mouse new-session -s main\n' "$patched" "$patched"
printf '%s\n' \
  'Inside that new server, enable mouse in your tmux settings, then run:' \
  'agent-float-term bind' \
  'agent-float-term doctor' \
  'Doctor should report: Status-bar mouse: supported.' \
  'Existing sessions remain on the old server; they are not migrated automatically.'
