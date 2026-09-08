# Operations

The [README](../README.md) gives the short installation and configuration guide.
Check `agent-float-term --help` and each command's `--help` on the installed
version. See [compatibility](compatibility.md) for local validation and open gaps;
do not infer release readiness or a completed GitHub CI run from this document.
Invocation lifetime and main-terminal navigation have isolated PTY coverage on
tmux 3.4, 3.5a, and 3.7c; see the current validation table for remaining limits.

## Commands

| Command | Purpose |
| --- | --- |
| `install [--tmux-config PATH] [--shell-config PATH --shell-kind bash\|zsh] [--yes]` | Preview installation; apply with `--yes`. Each startup file requires its own opt-in flag. |
| `bind [--socket PATH] [--replace-key]` | Integrate with an existing server; foreign-key replacement requires opt-in. |
| `start` | Open a dedicated normal shell, or initialize/bind the existing server when inside tmux. |
| `doctor [--socket PATH]` | Read-only dependency, JSON config, and selected-server diagnosis. |
| `sessions [--socket PATH]` | Inspect owned floats, including retained orphans. |
| `cleanup [--socket PATH] [--session NAME] [--yes]` | Preview or explicitly remove eligible owned orphans. |
| `update --from PATH --sha256 HASH` | Binary-only local SHA-256-verified update, not a template refresh. |
| `rollback` | Restore the retained previous binary, if available. |
| `uninstall [--yes]` | Preview or explicitly remove managed installation state. |

See [JSON configuration](configuration.md) for `shortcut`, dimensions, and
advanced executable paths. Use the same socket consistently across commands.

## Managed Installation

Run `install` directly from the extracted release binary or from
`./target/release/agent-float-term`, review the preview, then repeat with `--yes`.
The installer creates a digest-addressed payload and an owned symlink at
`$HOME/.local/bin/agent-float-term`. Do not prepopulate that destination with a
regular binary: unowned regular destinations are refused, not overwritten.
See the [installation examples](installation.md).

Without `--tmux-config` or `--shell-config`, a fresh installation modifies no startup
files. Each flag independently selects a user file for integration.
`--shell-config PATH --shell-kind bash|zsh` can opt into dedicated auto startup
without `--tmux-config`. Reinstalling retains previously selected integrations;
omitting their flags does not remove them or authorize edits to unselected user
configs. A plain `install --yes` refreshes owned `integration.sh` and
`integration.tmux` templates under the application's data directory, using the
recorded shell kind when no shell file is selected. Edited or unowned templates
are refused, not overwritten. Select whichever tmux config you actually use,
such as `~/.tmux.conf` or `~/.config/tmux/tmux.conf`, rather than adding a second one.

When migrating an existing layout, plain `install --yes` also repoints previously
managed startup blocks to `${XDG_DATA_HOME:-$HOME/.local/share}/agent-float-term/`.
Only the exact blocks recorded in the manifest are replaced; surrounding bytes
and file permissions are preserved. Intact legacy scripts are removed from the
config directory in the same recoverable transaction. Edited/missing scripts or
blocks and occupied new destinations abort migration without overwriting them.
The read-only preview lists these migration targets. `config.json` is untouched.

## Existing Or Dedicated tmux

Run your AI command inside a parent tmux pane. Use `bind` from that server, or
pass `--socket /absolute/path/to/tmux.sock` to select an existing server. Keep
`doctor`, `sessions`, and `cleanup` pointed at the same socket.

```sh
agent-float-term bind --socket /absolute/path/to/tmux.sock
agent-float-term doctor --socket /absolute/path/to/tmux.sock
```

If the configured key is already bound, inspect that binding before explicitly
using `bind --replace-key`. Replacement sacrifices that key's previous tmux
behavior. Do not blindly replace your whole tmux configuration to resolve a
conflict.

Do not edit or rebind the same key concurrently with `bind` or `uninstall`.
tmux has no atomic compare-and-swap operation for key tables. The tool journals
the expected owned binding before writing it and never adopts an observed
replacement as its own, but this cannot make external concurrent edits atomic.
Apply key changes serially and inspect conflicts rather than repeatedly forcing
replacement.

After rebuilding from source or changing the engine's executable path, rerun
`bind` on the intended server to refresh the helper command. An exact previously
owned binding remains eligible for refresh without treating it as a foreign key.
During an interrupted refresh, the journal retains both the prior owned helper
and the intended new binding so a retry or uninstall can recover the original
user binding. Do not delete pending ownership records or adopt an observed user
replacement to force recovery; unrelated changes remain unowned.

Existing servers with global `exit-unattached on` or `destroy-unattached on` are
rejected because they can destroy detached jobs. Review and change those options
explicitly only if appropriate, or use a dedicated server. Owned floating
sessions set `destroy-unattached off` and force local `detach-on-destroy on`;
this keeps a destroyed float's viewer from attaching to unrelated work. The
tool does not rewrite these global options in your existing server.

The optional `agent-float-term start` command opens a dedicated tmux environment
with a normal shell, where you run `claude`, `codex`, or `opencode` yourself.
No alias, wrapper command, harness prefix, or replacement AI executable is needed.
The dedicated server starts with `-f /dev/null`: it does **not** load your private
personal tmux configuration, including its plugins and startup commands.
Normal shell startup still applies; this is not an environment clone or a
general shell sandbox.

Opt-in shell integration runs only for an interactive human shell with real
terminal stdin and stdout. Bash initializes inline. Zsh appends a one-shot hook
to the tail of `precmd_functions`, so earlier Powerlevel10k instant-prompt cleanup
can restore descriptors before initialization. If terminal descriptors are still
redirected, it allows one bounded retry at the next prompt, then removes itself;
it does not poll or retry forever.

The guards check **setness**, not just nonempty values, of
`BASH_EXECUTION_STRING`, `ZSH_EXECUTION_STRING`, and `ZSH_SCRIPT`. Thus shell
`-ic`/`-lic` command invocations, including empty commands, and script/snapshot
execution do not start tmux or register a deferred hook. Noninteractive/non-TTY
shells, SSH (`SSH_CONNECTION`, `SSH_CLIENT`, or `SSH_TTY`), nonempty `AFT_DISABLE`,
`AFT_STARTING`, or `_AFT_AUTO_STARTED`, and a missing/nonexecutable installed
binary are also skipped. Set `AFT_DISABLE=1` before starting a shell to disable
automatic startup.

Existing tmux (`TMUX`) is **not** a skip condition. At a human prompt, the template
calls `start` with `AFT_STARTING=1 AFT_QUIET=1`; inside tmux this only
initializes/binds the existing server. It never attaches a nested outer session
or overwrites a foreign root-key binding. Outside tmux it starts/attaches the
owned dedicated environment. Startup is attempted at most once per shell, and
failure continues the shell. These automatic guards do not prohibit explicit
commands. Existing tmux config files and global terminal options are not rewritten.

`doctor` is read-only: it does not create application state, bind keys, or change
the selected server. It reports the selected tmux client path/version and, when
present, the last popup failure cause. Use it before requesting a mutation.

## tmux Client Selection

`AFT_TMUX_BINARY` is the internal absolute-path pin for the tmux client used by
runtime helpers, dedicated sessions, and owned floats. The engine normally
resolves tmux from PATH and carries that client path through its integration.
You do not need to set this variable for normal installation. The pin prevents
a login shell's PATH reset from replacing a private-prefix client with a
different `/usr/bin/tmux`; this was validated with private tmux 3.7c while the
system client remained 3.3a.

The client must match the **running server's version**, not merely meet the
**3.4 minimum**. A newer PATH client is not a substitute for the
matching private 3.5a client when the live server is 3.5a: mismatched terminal
file-descriptor passing can fail even when metadata commands work. `bind` stores
the approved client path in the private per-server binding record (`binary`, an
optional field for older records). If PATH later selects a mismatched client,
the recorded client can be reused only after its version matches the server.
No client is downloaded and no live server is restarted automatically.

F7 caches version verification using the recorded executable metadata fingerprint,
client path, server version, and server-global generation. `bind` verifies that
the fingerprint is unchanged across the version check before recording it;
replaced binaries or older records require a fresh check. Global generation and
detach-policy checks remain in the batched runtime path. This avoids a `tmux -V`
spawn on every warm keypress without trusting a stale version string alone.

If you maintain multiple tmux versions, compare the selected client's `-V` with
the running server's version. Point diagnostics at the same socket and, if
needed, explicitly select a trusted matching client:

```sh
/absolute/path/to/tmux -V
/absolute/path/to/tmux -S /absolute/path/to/tmux.sock display-message -p '#{version}'
AFT_TMUX_BINARY=/absolute/path/to/tmux \
  agent-float-term doctor --socket /absolute/path/to/tmux.sock
```

Use an absolute executable path for an advanced override, not a version string
or shell command. An unavailable pin fails explicitly rather than silently
switching clients. Correct a stale override and rebind with the intended client;
do not kill a server with live jobs to fix a client/server version mismatch.

Without `XDG_RUNTIME_DIR`, dedicated startup uses the system temporary directory,
including `/tmp` when runtime/temp overrides are unset. Its per-user directory
must be private (0700) and user-owned, with the server generation marker checked
before reuse. The default `/tmp` path and ordinary shell exit passed local Linux
validation; a shared temporary parent is not permission to adopt a foreign server.

## Recognition Limits

Recognition is conservative, best-effort foreground inspection, not proof of
native TUI keyboard ownership. It checks a known interactive CLI grammar and
supported executable layouts rather than accepting every process named after
an AI tool. An explicit `harness_paths` mapping can identify an executable but
does not bypass mode, argument, foreground, or ancestry checks.

| Invocation shape | Recognition boundary |
| --- | --- |
| Ordinary Claude, Codex, or OpenCode interactive launch | Eligible only when identity and foreground checks also succeed. |
| Claude `--continue` / `--resume`, Codex `resume` / `fork`, OpenCode `attach` | Recognized grammar with supported arguments; parsing support is not authenticated-session validation. |
| Claude `-p` / `--print`, Codex `exec` / `review`, OpenCode `run` / `serve` / `web` | Noninteractive or alternate modes are not popup-eligible. |
| Authentication, administration, help/version, MCP/server commands | Known non-TUI modes are rejected. |
| Unknown flags, missing option values, `--` argument delimiter, unsupported argument forms | Forward the key; do not guess the selected mode. |
| Node eval/loaders/flags before the entry point, or a relative Node entry point | Unsupported; recognized Node launches require the expected absolute entry point and argument shape. |
| Rewritten argv/process titles, unknown wrappers, intervening SSH/editor processes, nested PTYs, stopped or ambiguous jobs | Forward the key when original identity or supported foreground ancestry cannot be established. |

A new harness release can add a flag that this version does not recognize.
Report the harness version and a minimal synthetic invocation shape, not real
prompts or secrets. Do not weaken foreground checks just to make an unsupported
mode open a popup.

The foreground group may contain up to **64 total members**, including the
verified frontend (or supported wrapper/native frontend pair) and descendant
helpers. Same-group MCP/LSP and other workers need not have recognizable helper
argv: their identity, executable metadata, ancestry, and descriptors are checked
instead. Argument recognition is required for the frontend and, where applicable,
its frontend pair, not arbitrary helpers.

Each helper must have pipe/socket or `/dev/null` stdin. TTY stdout and stderr
(FDs 1 and 2) are allowed for logging, but an extra terminal FD at **3 or higher**
blocks eligibility even with nonterminal stdin. Unrelated group members, unsafe
descriptors, and unsupported ancestry remain ineligible. Process, executable,
group, and relevant descriptor metadata are revalidated within a **300 ms
inspection budget**. This is best-effort race detection, not an atomic guarantee
of keyboard ownership or a measurement of TUI startup latency.

## Lifetime And Ownership

A float belongs to one
eligible AI invocation in its original parent pane, not to that pane forever.
Hiding detaches the viewer; the shell PID, local state, cwd, and running jobs
persist only within that invocation. When the AI exits, its float resets
promptly, including its shell and terminal jobs, without waiting for another F7 press.
A later AI invocation in the same pane starts a fresh float. Do not use the float
for work that must outlive the AI invocation.

There is only one viewer; another client must not steal an attached viewer or
create a duplicate float for the same invocation. A small detached watcher checks
PID/start-time identity every 100 ms; stopped processes remain alive, and unknown
metadata never authorizes deletion. Normal cleanup is prompt, not a hard real-time
deadline. Daemonized jobs that escape the terminal are not process containment.

The first float uses the parent pane's current directory. Later opens keep the
float's own directory. Its environment comes from normal shell/tmux startup,
not a snapshot of the parent program. If you activated a virtual environment or
exported variables after starting tmux, you may need to do so in the float too.

Main-terminal tmux keyboard shortcuts control the main terminal while
the float is open, not an accidental inner tmux session. Navigating away
temporarily hides the float. Returning to the original AI pane automatically
restores it, provided the same invocation is still active and the float was not
explicitly hidden. F7 hide disables automatic restoration until the user opens
the float again. Native main-client key replay preserves the user's binding
commands and modal tables. The popup yields during shortcut dispatch; prompts
and choosers take focus until their native interaction ends. Return-to-origin
restoration is checked approximately once per second, not on every redraw.

Private input tables disable inner tmux keyboard commands. Main binding keys are
refreshed on each opening; changes made while the popup is open take effect on
the next opening. Mouse events stay with the float rather than being replayed
against unrelated main-pane coordinates. Explicit `tmux` commands typed into the
floating shell are still ordinary shell commands, not sandboxed.

Restoration reserves an unused tmux `User` key with no terminal byte sequence.
It enters native client input handling, so a prompt or unrelated popup consumes
the probe without being replaced. A changed restore binding is never executed
or removed as if it were still owned. Key tables and restore bindings are removed
on verified cleanup/uninstall; global user shortcut definitions are not rewritten.

F7 hides an owned float during normal popup input, including inside `nvim`.
tmux command prompts and copy mode retain their native mode keys: exit the mode
first, then press F7 to hide. This is not a universal binding over all tmux key
tables. In the outer pane, `nvim` and other non-AI programs receive F7 normally.
Process-tree ambiguity also forwards it, even if that means a popup does not open.

Legacy or otherwise retained orphan sessions can still be inspected with
`sessions` and removed only through explicit owned-orphan cleanup. They are not
a promise that a completed AI invocation retains its jobs under the new
contract. Neither detachment nor retained orphans provide persistence across
reboot, tmux server termination, or shell exit.

## Terminal Theme And Opacity

Each popup uses `display-popup -s 'fg=terminal,bg=terminal'` and
`-S 'fg=terminal,bg=terminal'` for its content and border. The explicit `terminal`
color sentinel is essential: tmux's `default` can still resolve to an inherited,
opaque RGB style. No user-specific color hex values are needed.

On every open, both `window-style` and `window-active-style` are set locally at
the owned window and pane scopes to `none,fg=terminal,bg=terminal`. This clears
inherited attributes/colors, including overrides added while hidden, without
replacing the persistent shell. Global `popup-style`, `popup-border-style`,
window styles, and parent-pane overrides are unchanged. A window linked into
another session is refused before style mutation, rather than changing shared
styles. The inner attach client uses `-T RGB` to preserve explicit application
truecolor output.

Terminal-default background allows the emulator to apply its configured theme
and transparency policy. The tool does not change terminal-emulator or OS
opacity, nor composite the underlying AI pane through the popup. An application
that paints an explicit background still paints that color. Custom tmux
`pane-colours` palette overrides can affect colors and are not reset by this fix.
The reported theme mismatch involved an existing global popup background; it
does not require rewriting that global style or copying a particular theme's RGB.

## F7 Latency

The optimized runtime batches tmux metadata and mutation commands. The verified
warm path uses seven tmux invocations instead of seventeen; post-hide worker
cleanup uses one instead of three. Latest cold creation uses ten, including the
added global-policy check; the earlier measured optimized revision used nine.
These counts are invocations, not the number of commands within each batch.
Worker cleanup here is not the destructive orphan `cleanup` CLI command.

See the [benchmark record](compatibility.md#f7-benchmark) for measured attachment,
prompt-frame, hide, and cleanup timings. These private clean-shell measurements
do not include the user's full shell profile or establish resolution of the
reported roughly one-second live F7 latency. There is no 100 ms end-to-end
guarantee. Deployment and live checks of the latest optimization remain pending;
live F7 was confirmed before optimization. The separate several-second AI CLI
startup delay remains unresolved and unmeasured, beyond the known shell-template
command/snapshot attach guard already fixed.

## Explicit Cleanup

```sh
agent-float-term sessions
agent-float-term cleanup
agent-float-term cleanup --session OWNED_ORPHAN_SESSION
# Only after reviewing the preview and confirming jobs can be terminated:
agent-float-term cleanup --session OWNED_ORPHAN_SESSION --yes
```

Replace `OWNED_ORPHAN_SESSION` with a name reported by `sessions`. Cleanup must
reject live parent-owned sessions and foreign sessions, even with `--yes` or an
explicit name. A bare `cleanup --yes` requests deletion of eligible owned
orphans, not arbitrary tmux sessions. It is destructive to those orphan shells
and their jobs. Do not use `tmux kill-server` as a cleanup shortcut.

## Local Update And Rollback

Download a release archive yourself and verify its hash against the published
`SHA256SUMS`. Extract only after verification, then calculate the **binary** hash:

```sh
# macOS (Linux: sha256sum /absolute/path/to/new/agent-float-term)
shasum -a 256 /absolute/path/to/new/agent-float-term
agent-float-term update --from /absolute/path/to/new/agent-float-term \
  --sha256 REPLACE_WITH_THE_64_HEX_CHARACTER_BINARY_HASH
agent-float-term --version
agent-float-term doctor
```

The archive hash is not the binary hash. A mismatched digest must prevent the
update. This command does not fetch a URL, poll for updates, or authorize an
unrelated config migration. Ensure the candidate matches your OS/architecture.
Review the release notes before replacing the installed binary.

`update` is **binary-only**: it does not refresh generated shell/tmux templates.
To apply the autostart fix to an existing installation, use the updated binary
to preview and refresh the owned templates, then bind the running server:

```sh
agent-float-term install
# After reviewing the preview; no config flags are needed to retain integration:
agent-float-term install --yes
# Inside the intended existing tmux server:
agent-float-term bind
agent-float-term doctor
```

Unselected user configs and their recorded integration blocks are retained;
the recorded shell kind is used. The refreshed shell template takes effect in
new shells. Manual `bind` applies to the running server without restarting it,
editing its config, or requiring the current shell to rerun startup scripts.

If a retained previous binary is available:

```sh
agent-float-term rollback
agent-float-term --version
agent-float-term doctor
```

The data-directory layout uses installation manifest format 2; the current
installer also reads format 1 to migrate existing installations. Binary rollback
does not reverse a layout migration. An older binary can still run its runtime
commands, but its installer/update/uninstall commands may reject format 2. Use
the newer release binary directly for installation management in that case.

Rollback is not a tmux-session snapshot, a job recovery mechanism, or a general
configuration backup. Neither command guarantees recovery after a reboot.

## Uninstall

```sh
agent-float-term uninstall
# Apply only after reviewing exactly what will be removed:
agent-float-term uninstall --yes
```

Managed uninstall is not permission to terminate arbitrary tmux sessions or
remove user-authored config. Before uninstalling, inspect owned sessions with
`sessions` and use explicit orphan cleanup only when safe. Follow the uninstall
preview/output rather than deleting broad directories.
Both `config.json` and legacy `config.toml` are user files to preserve; see
[migration](configuration.md#migration-and-safety). The final installer output
must name both formats rather than implying only TOML is preserved.

Applied uninstall calls the main command's `unbind_all` hook for recorded tmux
servers. It restores prior key state only where the current binding still
exactly matches the installed, owned binding; it does not overwrite a binding
you subsequently changed. If a recorded server is unavailable, it cannot be
restored at that time. A remaining runtime binding falls back to forwarding the
key when the installed executable is missing. This fallback is not a claim that
an unavailable server was successfully unbound. Do not change the same key
concurrently with uninstall; the ownership journal is not an atomic tmux key-table
lock. See the [validation matrix](compatibility.md) for tested scope and remaining
publication checks. Do not restart servers with live jobs just to clear bindings.

## Troubleshooting

| Symptom | Checks |
| --- | --- |
| Installation refuses an unowned regular destination | Do not copy the binary into `~/.local/bin/agent-float-term` before installation. Inspect and relocate any existing file yourself if appropriate, then run the extracted/source binary's installer directly. |
| F7 never reaches the application | Check the terminal/OS key mapping, Fn mode, and tmux binding conflicts. |
| F7 reaches the AI CLI rather than opening a float | Run `doctor`; confirm foreground eligibility, executable identity, and the selected server. Ambiguity intentionally forwards. |
| An older shell integration left existing tmux without F7 | Refresh owned templates with the updated binary's `install --yes`, then run `bind` in the intended server. Older templates skipped `TMUX`; a binary-only `update` does not replace them. |
| F7 takes roughly a second | Check the installed revision, owned binding, matching tmux client, and `doctor`'s last popup failure. The latest optimization has isolated clean-shell evidence only; measure full-profile latency separately from AI CLI startup. |
| Popup background differs from the terminal or stays opaque | Use the terminal-default-style revision and rebind. Check emulator opacity policy, explicit application backgrounds, and custom `pane-colours`; no global theme rewrite or OS opacity change is required. |
| TUI startup pauses for a few seconds | Refresh old shell templates to prevent command/snapshot shells from attaching tmux. The demonstrated attach-blocking mechanism is fixed; the reported 2-5 second TUI latency itself is not measured, and other causes may remain. |
| A new harness flag or alternate mode does not open a float | Check recognition limits. A supported executable path alone does not make every CLI grammar eligible. |
| A private tmux works until the popup's login shell starts | Check `AFT_TMUX_BINARY` and the client/server versions; refresh integration with the correct absolute client instead of relying on login PATH. |
| Custom AI installation is not detected | Add an absolute executable path in the JSON `harness_paths` array; use the real supported harness name. See [configuration](configuration.md#advanced-paths). |
| Config fails after upgrading from TOML | Create `config.json`, convert the structure to JSON, and rename `key` to `shortcut`. The old file is unchanged and never loaded. `{}` explicitly selects defaults. |
| F7 hides an editor inside the float | Expected during normal popup input. Use another editor key there. |
| F7 does not hide during a tmux command prompt or copy mode | Native mode keys take priority. Exit the mode, then press F7. |
| Existing server integration rejects detach settings | Review global `exit-unattached` and `destroy-unattached`; either being `on` is unsafe for this integration. Use a dedicated server if you cannot change them. |
| Dedicated startup does not load personal tmux plugins/config | Intentional: the dedicated server uses `-f /dev/null`. Use existing-server integration to retain your chosen tmux setup. |
| A custom function key is rejected | Function keys are F1-F12 only. F13-F24 are not supported. |
| The float has a different environment | Expected: it is a new shell, not an environment clone. Check shell startup and tmux environment. |
| The float has an older cwd | Expected after first creation: its own cwd persists. Change directory inside it. |
| A second client cannot open the same float | Check for an existing viewer; a float is single-viewer. |
| Retained orphan sessions remain | Inspect before explicit cleanup. Historical orphan behavior does not verify the new invocation-exit reset contract. |
| Float resets after the AI exits | Shell and terminal jobs belong to that invocation. This is not persistent background-job storage. |
| Shell/jobs disappear after reboot or server exit | Those lifetimes are not supported persistence boundaries. |
| Download fails with 404 | The selected version may not be published yet; use source builds or inspect the releases page. |

For an existing server, inspect the root key and relevant options without editing
the user's configuration. Run these from a shell in that server:

```sh
tmux display-message -p 'server=#{version} socket=#{socket_path} configs=#{config_files}'
tmux list-keys -T root F7
tmux show-options -g default-terminal
tmux show-options -g exit-unattached
tmux show-options -g destroy-unattached
agent-float-term doctor
# Explicit runtime binding, after inspecting any conflict:
agent-float-term bind
```

A missing F7 binding is distinct from an ineligible foreground invocation. To
inspect an AI invocation without replacing it with `doctor`, run diagnostics from
another shell with the target pane ID and socket, substituting your actual values:

```sh
TMUX_PANE=%3 agent-float-term doctor --socket /absolute/path/to/tmux.sock
```

The reported default tmux 3.5a server had no F7 binding because the old automatic
script skipped existing tmux; the old inspector also refused same-group helpers.
Read-only diagnostics recognize the user's Claude and OpenCode invocations with
helpers, and live F7 was confirmed before optimization. Deployment and live
acceptance of the latest latency/theme changes remain pending. No user tmux
config was changed; the pre-existing bad `default-terminal`
setting was left intact. Review terminal settings separately rather than having
this tool rewrite them or restarting a server with live jobs.

When reporting a bug, include OS/architecture, `tmux -V`, the binary version,
terminal, harness version, whether the server is existing/dedicated, and minimal
redacted diagnostic output. Never upload raw prompt history or tokens.

## Privacy And Development

This tool includes no telemetry or prompt logging. The float is an ordinary
shell with your permissions; **tmux is not a sandbox**. AI tools, shell history,
tmux scrollback, plugins, and terminal recording may independently retain
sensitive content. Do not include prompts, tokens, or private output in reports.

See [Contributing](../CONTRIBUTING.md), the [release checklist](release-checklist.md),
and [Changelog](../CHANGELOG.md). CI configuration is not proof of a passing
GitHub run. Historical local tests and benchmarks are recorded separately in
the [compatibility matrix](compatibility.md); no end-to-end latency guarantee is
asserted. MIT licensed; copyright (c) 2026 vuongvm812. See [LICENSE](../LICENSE).
