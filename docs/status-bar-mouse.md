# Status-Bar Mouse Switching

The dismiss-on-click implementation requires the **optional patched tmux 3.7c
server** built below, plus the updated application. It is not provided by stock
tmux or the existing Homebrew formula. No live server is upgraded automatically.

## Behavior

- With `mouse on`, an unmodified left-button press on an exposed main status-bar
  row closes the application's popup and continues through the original main
  client's mouse binding. Session/window targets and custom status ranges retain
  their native coordinates and context; there is no synthetic key replay.
- The floating shell, cwd, variables, and jobs remain alive. Switching back to the
  original AI pane restores the same float while that AI invocation is alive.
- Explicit F7 hide still disables automatic restoration. AI exit still terminates
  its float and jobs. The float never follows the user into another session.
- A click that does not navigate away may allow the float to restore over the
  original AI pane. Native prompts/menus keep focus until they finish.
- Only status rows outside the popup qualify. A full-height popup covering the
  status bar does not expose a clickable main bar. Inside clicks, border/menu
  interaction, drags, wheel events, modifier clicks, and mouse-off behavior are not
  changed by this extension. It does not enable mouse mode or rewrite bindings.

## Build And Test

For a fresh or existing **managed source/archive installation**, run from the
repository:

```sh
make install
# Or preview without building or writing anything:
make install DRY_RUN=1
```

This builds the application, previews its install to catch registration conflicts,
builds patched tmux, and then applies the managed install (including owned
integration refresh). Registered Homebrew/Cargo installations are refused rather
than silently converted. Patched tmux builds live in separate private directories
under `${XDG_DATA_HOME:-$HOME/.local/share}/agent-float-term/compat/`. Previous
builds are retained because a server may still use them. A failed tmux build does
not apply the application install; the build log remains available for diagnosis.

The command **does not start or restart a server**. It prints the exact launch
command for a separate patched server, to run outside tmux, followed by activation
and doctor instructions. Existing sessions are never migrated automatically. If
already using a compatible patched server, an application update does not require
creating another server.

Requires a C compiler, make, pkg-config, libevent, ncurses, and patch. The existing
development test prerequisites also apply. The builder uses the SHA-256-pinned
official tmux 3.7c source archive; it does not run Homebrew/package installation.

To build and test the components separately without installing the application:

```sh
cargo +1.84.1 build --locked --release
# This destination must not already exist; its parent must exist.
patched_tmux=$(bash scripts/build-mouse-tmux.sh "$PWD/target/tmux-status-mouse")
PATH="$(dirname "$patched_tmux"):$PATH" \
  expect tests/mouse_smoke.exp target/release/agent-float-term patched
PATH="$(dirname "$patched_tmux"):$PATH" \
  expect tests/pty_smoke.exp target/release/agent-float-term
```

An optional second builder argument supplies a local archive, checked against the
same pinned hash. Sources, executable, and `build.log` stay in the new private
directory. The builder neither installs tmux nor launches a server. The tests
create and tear down only isolated servers, homes, and synthetic AI fixtures.

## Server Requirement

The application queries the **running server's** `#{aft_popup_status_mouse}`
format. Value `1` advertises this patch's `display-popup -M` contract, so the app
opts its own popups in. Popups from other tools remain unchanged unless they
explicitly pass `-M`. This is a local extension, not an upstream tmux API.

`agent-float-term doctor` reports `Status-bar mouse: supported` or `unavailable`.
On a stock server, the application does not pass the unsupported flag; the
existing hide-then-click workflow remains available. Stock tmux 3.4+ remains
supported for the rest of the application.

**Replacing a tmux executable or changing PATH does not upgrade a running server.**
Deployment requires starting a separate server with the patched executable or
planning a later migration after existing jobs are safely finished. Use the
matching client for that server. Do not restart/kill a live server merely to try
this feature: its sessions and terminal jobs would be lost. No live migration,
package replacement, global binding change, or startup-file edit is part of the
builder or test workflow.

## Validation

Local macOS ARM64 tests pass against patched and stock tmux 3.7c. The mouse suite
checks a default top session range, a custom second-row bottom range, press/release
exactly-once switching, original-client attribution, observer-client isolation,
shell retention/restoration, explicit hiding, AI-exit cleanup, and unchanged user
tables. Unopted-in raw popups and negative mouse cases remain modal. The application
reports no popup error for the intentional mouse dismissal.

CI includes stock/patched checks in the Linux 3.7c reference lane. That remote
execution and deployment with the user's actual status-bar plugins remain pending.
The earlier [native-pane experiment](native-pane-prototype.md) is still disabled;
this solution retains client-local popups and existing keyboard routing instead.
