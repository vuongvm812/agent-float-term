# Experimental Native Pane Viewer

**Not shipping. This is an isolated tmux 3.7c feasibility probe, not an application backend.**
The implemented dismiss-on-click approach instead uses the optional
[popup status-mouse patch](status-bar-mouse.md); this experiment remains disabled.
The production requirement is that all tmux shortcuts continue to target the main
workspace while the shell viewer is open, without wrapping or replacing user
bindings. Native floating panes do not satisfy that requirement: the outer tmux
intercepts its prefix and pane-targeted commands naturally target the active
floating viewer, not the main pane. Passing this probe means that this limitation
was observed, not that the production gate passed.

## Run

```sh
expect tests/native_pane_probe.exp
```

No arguments, application binary, compilation, Cargo tests, or CI integration are
needed. Dependencies are Expect, Bash, `mktemp`, and real `tmux 3.7c` on `PATH`.
Other version strings are rejected rather than silently testing another API.
The probe uses five-second polling windows with a 45-second overall polling
deadline; run it with an external limit of at most 60 seconds as an additional
guard against a hung subprocess.

The script follows the existing Expect smoke-test isolation pattern: a fresh
`mktemp` directory with mode 0700, cleared inherited environment except `PATH`,
private HOME/XDG/temp/socket directories, explicit `-S` and `-f /dev/null` on
every server command, and Bash `--noprofile --norc`. No live server, user home,
configuration, or bindings are modified. Normal completion, assertion failures,
and INT/TERM/HUP clean up only the created server and directory. As with other
process cleanup traps, SIGKILL cannot be handled.

## Presentation Model

Two disposable main sessions (`alpha`, `beta`) and a private `retained` session
live on the fixture server. The retained pane owns the interactive Bash shell.
An attached 120x40 outer client displays a native floating pane whose command is
an explicitly nested tmux attachment to the retained session:

```text
new-pane -t MAIN -x 60 -y 16 -X 20 -Y 5 -P -F '#{pane_id}' \
  env -u TMUX tmux -S PRIVATE_SOCKET -f /dev/null attach-session -t RETAINED
```

Lowercase `-x`/`-y` specify width/height; uppercase `-X`/`-Y` specify position.
There is no reliance on moving an existing tiled shell into a float. `new-pane`
creates the viewer; `kill-pane` destroys only the viewer. Reopening creates a
different native pane and nested client around the same retained shell. Hiding
is simulated explicitly by the fixture, not implemented as an application
shortcut or automatic lifecycle policy.

The installed 3.7c man page's `new-pane` synopsis is incomplete for these flags.
The implementation is in tmux's `cmd-split-window.c` (`cmd_new_pane_entry` and
`cmd_split_window_exec`), with native floating layout separate from tiled layout.
The runtime assertions, not an assumption based on the version string or the
command name, verify `pane_floating_flag=1`, active state, 60x16 content geometry,
origin `(20,5)`, and that the 120x39 underlying main pane was not split.

## Checks And Attribution

- All shell input, mouse input, and the prefix demonstration use the attached
  outer client's PTY. No `send-keys` or CLI-selected prefix target stands in for
  actual input. CLI commands are limited to fixture setup, lifecycle control,
  inspection, and a clearly separated final return for shell-state inspection.
- A top status line has a session range for `beta` and a separate `left` range
  labelled BACK. SGR left-button press/release at terminal column 3, row 1 uses
  the untouched default `MouseDown1Status` session-range binding to switch the
  main client from alpha to beta while the alpha float is active.
- The first viewer is destroyed and the retained session becomes unattached.
  Its shell pane and PID remain live. A different viewer is created in beta;
  shell-generated snapshots prove the same `$$`, unexported variable, and cwd.
  Captured viewer output also confirms that the retained shell renders through
  both native viewers.
- A fixture-only `MouseDown1StatusLeft` binding records mouse client, range, and
  status-line index, then switches naturally to alpha. SGR press/release at
  column 10, row 1 triggers it exactly once while the beta float is active.
  It records the outer client, `left`, and status index `0`, not the nested
  retained-session client.
- An outer-client-filtered `client-session-changed` hook counts session
  transitions. Each click adds exactly one, including a 350 ms post-click
  duplicate-detection window; the custom binding also has its own invocation
  counter. The untouched default binding is not separately invocation-counted;
  its exactly-once assertion is for the observed session transition within this
  bounded window. Nested attach/detach events are excluded. The final CLI return
  to beta happens after these two mouse assertions and is not claimed as a
  mouse-generated switch.
- Actual default prefix `C-b` followed by F12 runs a fixture-only prefix binding
  recording `#{pane_id}|#{session_name}|#{client_name}`. It must record the beta
  floating viewer and the outer client, distinct from both the beta main pane
  and the retained shell pane. This is outer prefix interception with active
  float targeting, **not** evidence that the prefix reached the nested shell.
- A fresh shell-generated snapshot after both clicks and the prefix test proves
  shell PID/state/cwd unchanged. Both main shell PIDs also remain unchanged.
  The default session-range binding and the entire post-setup fixture binding
  table must remain byte-for-byte unchanged throughout the scenario.

Only the empty-config fixture server receives the two instrumentation bindings
and counting hook. There is no production binding installation, forwarding,
replacement, wrapper, or attempted prefix repair.

## Local Result

`expect tests/native_pane_probe.exp` completed with exit status 0 on the installed
Homebrew tmux 3.7c. Both viewers reported `floating=1 active=1 size=60x16
origin=20,5`. The outer-client transition count was 1 after the session-range
click and 2 after the custom click; the custom binding invocation count was 1.
The prefix record identified the second floating viewer in beta and the outer
client, not the main pane or retained shell. PID, unexported state, cwd, and
binding-table checks passed. The final output explicitly says **production gate
FAIL**, **multi-client visibility UNSUPPORTED**, and **NOT SHIPPING**.

## Production Gates

| Requirement | Probe outcome |
| --- | --- |
| Real native float, attached PTY, main status mouse access | Asserted by probe |
| Retained shell across viewer destruction/recreation | Asserted by probe |
| Session-range and custom status clicks switch main client exactly once | Asserted by probe |
| All tmux shortcuts target main workspace without binding wrappers | **FAIL: native prefix command targets active float** |
| Per-client native float visibility and ownership | **UNSUPPORTED, not solved or tested** |
| Shipping application backend | **Blocked; no application changes** |

Native panes belong to windows, not to an independently isolated per-client
overlay. The single outer client plus its nested viewer does not establish
correctness for two independent outer clients viewing the same window. Visibility,
focus, and lifecycle ownership in that case remain unsupported.

The test also does not establish arbitrary key-table, prefix2, modal/copy-mode,
terminal, resize, zoom, crash-recovery, or real harness behavior. One naturally
misdirected prefix command is enough to fail the all-shortcuts production gate.
Do not enable or ship a native backend unless **all** tmux shortcuts retain main
workspace semantics without mutating user bindings and multi-client behavior is
resolved separately.
