# Changelog

Notable user-facing changes are recorded here. An entry under Unreleased is not
evidence that the implementation or compatibility testing is complete.

## Unreleased

### Status-Bar Mouse Switching

- Add `make install` to build/install the managed application and a separate
  persistent patched tmux build. `DRY_RUN=1` is a no-write preview. The target
  prints activation instructions without starting or restarting any server.
- Add a pinned, opt-in tmux 3.7c patch: `display-popup -M` dismisses the popup and
  continues the original exposed-status left click through native main bindings.
  Retain shell state and return-to-origin restoration; do not wrap user bindings.
- Detect the running server's capability before using the flag and report support
  in `doctor`. Stock tmux remains supported with its existing mouse limitation.
- Add an isolated builder and stock/patched PTY regressions. No automatic tmux
  installation, server restart, package replacement, or live-session migration.

### Neovim Navigation

- Let `Ctrl+h/j/k/l` reach foreground Neovim inside the float without changing
  user tmux bindings or editor mappings. Keep main routing outside Neovim and
  for dedicated tmux shortcuts; uncertain process inspection keeps keys local.
- Add real-Neovim PTY coverage for split navigation, rapid input, main shortcut
  handoff, suspend/resume, failed inspection, and retained editor state.
- Add an isolated tmux 3.7c native-pane mouse/lifecycle probe, not a runtime
  backend. Status clicking works in the probe, but prefix commands target the
  viewer rather than main. Stock tmux popup mouse behavior remains unchanged.

### Coordinated Publication And Homebrew First Use

- Add `make release` with an exact confirmation prompt and a local plan-only
  `DRY_RUN=1` mode. Coordinate verified GitHub archives, a formula-only tap
  commit/push, and local Cargo publication from clean tagged source. Never create
  or move source tags, overwrite release assets, or republish an existing crate.
- Verify completed stages on retry, including crate source provenance. Reuse one
  active release run, reject ambiguous runs and unsafe tap downgrades/edits, and
  retain existing environment-protected OIDC publication as a manual alternative.
- Homebrew now needs no separate application install command: first `bind` or
  interactive `start` registers its verified stable opt path. No package install
  hook writes to user homes; no binary copy or startup-file edits are implicit.
  Help/version/doctor stay read-only, and conflicting installation modes/paths
  require explicit migration. Existing v0.2.2 assets are unchanged.

### Homebrew And Cargo Release Preparation

- Add `install --external-binary PATH` for native package-manager ownership.
  Register integrations without copying or modifying the supplied executable;
  retain stable Homebrew/Cargo paths across upgrades and old-keg removal.
- Manifest format 3 records external registration; existing managed formats 1/2
  remain supported. External update/rollback require the package manager;
  uninstall removes only owned integrations and preserves the package binary.
- Prepare a checksum-derived custom-tap formula and manually dispatched crates.io
  verification/Trusted Publishing workflow. Stable GitHub drafts include the
  generated formula; no tap commits or stable publication happen automatically.
- Add crate-content verification, package-path permissions/ownership regressions,
  and isolated upgrade coverage for unchanged bindings and running watchers.
- Separate Homebrew, Cargo, release archive, and Git source instructions. Cargo
  still compiles locally; publication of all distribution channels remains pending.

### Generated Integration Layout

- Store generated shell/tmux scripts under the XDG data directory instead of the
  config directory. `config.json` remains optional user configuration.
- Plain reinstall migrates intact legacy scripts and exact previously managed
  startup references transactionally. Preserve unrelated startup bytes, modes,
  and user settings; refuse edited, missing, or unowned migration targets.
- Manifest format 2 records the managed layout. Management commands retain support
  for formats 1 and 2; binary rollback does not reverse the filesystem migration.

### JSON Configuration And Runtime Contract

- Configuration is JSON-only at
  `${XDG_CONFIG_HOME:-$HOME/.config}/agent-float-term/config.json`. The public
  field is `shortcut`, default F7; width and height default to 80 and accept
  integer percentages from 10 to 100 inclusive. Unknown fields are rejected.
- Advanced `shell` and `harness_paths`, executable/security validation, private
  directories, and XDG behavior remain. TOML and unused transitive dependencies
  are removed. There is no TOML fallback or automatic file migration: an old file
  is preserved; if JSON is absent, an actionable error explains conversion and
  the `key` to `shortcut` rename. Existing JSON is authoritative.
- Converted every existing Expect config-format fixture, including the F7
  benchmark, invalid-field test, and custom-shortcut test, to JSON. Seven focused
  config unit tests pass locally; smoke/benchmark runtime reruns remain separate.
- Shortened README; detailed installation, configuration, commands, safety, and
  operational guidance live under `docs/`.
- tmux minimum is now 3.4; floats belong to verified individual AI invocations
  and reset on AI exit, including while hidden. A lightweight watcher checks
  process identity every 100 ms without treating suspension as exit.
  Main-terminal shortcuts control the main terminal;
  navigating back to the original AI pane restores a temporarily hidden float.
  Explicit F7 hide disables restoration until reopened.
- Updated PTY scenarios cover main root/prefix/prefix2/custom-table shortcuts,
  session/window restoration, prompt protection, visible/hidden AI termination,
  background jobs, fresh invocations, and explicit-hide suppression. The new
  minimum CI archive is SHA-256-pinned. See [current validation](docs/compatibility.md#current-change-validation)
  for tested platforms and remaining release/deployment limits.
- Disable timing-based paste guessing only inside the float so rapid keyboard
  shortcuts cannot bypass routing on older supported tmux versions. Preserve
  bracketed paste and the main terminal's options. Correct the theme test decoder
  for empty SGR reset and saved rendition across capture boundaries.

### F7 Latency And Terminal Theme

The following sections record earlier work, before the JSON/runtime contract.

- Batched runtime metadata and guarded mutations reduce warm-open tmux
  invocations from 17 to 7, and post-hide worker cleanup from 3 to 1. Latest cold
  creation uses 10, including a global-policy check added after the measured
  nine-invocation revision. Orphan cleanup behavior is unchanged.
- Cached client-version verification is tied to the executable metadata
  fingerprint, selected path, server version, and server-global generation.
  Binding checks the fingerprint before and after version verification; replaced
  binaries require rechecking. Private per-server binding records retain an
  optional approved client path for matching-client reuse across PATH changes.
  A tmux 3.5a live server needs a matching client, not simply a newer one.
- Read-only `doctor` reports the selected client path/version and last popup
  failure cause. No client download or live-server restart is automatic.
- Popup content and border explicitly use `fg=terminal,bg=terminal`, not tmux
  `default`, which can inherit an opaque RGB style. Owned window/pane styles reset
  to `none,fg=terminal,bg=terminal` on every open; global and parent styles remain
  unchanged. Foreign-linked windows are refused before style mutation.
- The inner attach client uses `-T RGB`, preserving explicit application colors.
  Transparency remains emulator policy: no OS opacity changes or see-through
  compositing of the underlying AI pane. Explicit application backgrounds remain
  colored, and custom `pane-colours` palette overrides can affect the result.
- Added actual-SGR positive/negative theme regression coverage, verified on
  Linux tmux 3.3a and macOS 3.5a/3.7c. CI runs `tests/theme_smoke.exp` after
  autostart, with its own five-minute timeout and `TERM=xterm-256color`.
- Optional `tests/benchmark_toggle.exp` uses five cold and twenty-five warm
  samples per version; it is not a CI threshold. macOS 3.5a warm attachment p50
  improved 209.7 to 113.6 ms and prompt-frame p50 256.2 to 163.3 ms; 3.7c improved
  218.2 to 116.6 ms and 263.5 to 162.1 ms respectively. On 3.5a hide was 9.8 to
  10.2 ms (observation noise), and post-hide cleanup settled in 45.0 to 18.1 ms.
  These are private clean-shell measurements, not the user's full-profile
  roughly one-second F7 delay or a 100 ms end-to-end guarantee.
- The benchmark's cold-read probe now acknowledges shell handoff before sending
  input, avoiding readline prefetch. No Linux percentiles are reported because
  the old probe failed there; revised cold-read timing is not directly baseline
  comparable. See [benchmark scope](docs/compatibility.md#f7-benchmark).
- Latest Linux Rust suite: 57 passed (50 library, 2 CLI, 1 installer, 4 tmux-client).
  macOS full suite: 59 passed, including four `tmux_client` tests. All four
  release smoke suites passed. Live F7 was confirmed before optimization; the
  optimized revision is installed, with live visual/latency acceptance remaining.
  The separate reported 2-5 second AI startup
  delay remains unresolved and unmeasured; this is not an AI startup fix.

### F7 And Autostart Fix

- Fixed automatic initialization inside existing tmux: the reported default
  tmux 3.5a server had no F7 binding because the old shell template skipped
  `TMUX`. Human shells now call quiet `start`, which initializes/binds that
  server without nesting an outer session or overwriting a foreign root key.
- Foreground inspection now permits up to 64 group members: verified frontends
  and descendant MCP/LSP or other helpers with pipe/socket or `/dev/null` stdin.
  TTY stdout/stderr are allowed; any extra terminal FD at 3 or higher blocks
  eligibility. Helper argv is not required outside frontend-pair recognition;
  identity, executable, group, and relevant descriptor metadata are revalidated
  within a 300 ms inspection budget.
- Generated Bash integration initializes inline only in interactive real-TTY
  shells. Zsh defers to a one-shot tail `precmd` hook after earlier Powerlevel10k
  descriptor restoration, allowing one bounded retry. Execution-string setness
  guards skip `-ic`/`-lic`, including empty commands, and script/snapshot execution;
  SSH, non-TTY, disable, and recursion guards remain in place.
- This fixes the demonstrated shell-snapshot attach-blocking mechanism, not a
  measured resolution of every startup delay. The reported 2-5 second TUI startup
  latency remains unmeasured.
- `update` remains binary-only. `install --yes` refreshes owned generated
  templates, retaining unselected configs and the recorded shell kind. No user
  tmux config was changed; the pre-existing bad `default-terminal` was left intact.
- Added `tests/autostart_smoke.exp` to CI with its own five-minute timeout and
  `TERM=xterm-256color`, consistent with the other smoke steps.

### Focused Bugfix Validation

- macOS `tests/autostart_smoke.exp` passed using the actual installed binary and
  a fake Powerlevel10k-style descriptor cleanup, not the user's prompt config.
  Command/shell-snapshot checks took 31-44 ms with zero autostarts.
- Core PTY coverage with workers, focused inspector tests, and focused installer
  tests passed. These earlier focused results are separate from the latest
  latency/theme validation and historical totals below.
- Read-only `doctor` recognizes the user's current live Claude and OpenCode
  invocations with helpers. Live F7 was subsequently confirmed before the latest
  optimization; this is not model-interaction or generating-session acceptance.

### 0.1.0 Release Candidate Scope

- Harness Floating Terminal (`agent-float-term`): an F7 floating shell for
  ordinary, unwrapped Claude Code, Codex, and OpenCode invocations in tmux.
- Per-parent-pane shells with a single viewer, persistent detached jobs and cwd,
  conservative foreground detection, and explicit orphan cleanup.
- Preview-first installation; existing-server binding and an optional dedicated
  tmux environment that starts a normal shell.
- Initially XDG TOML configuration for key, popup dimensions, shell, and harness
  paths; superseded by the JSON-only change above.
- Diagnostics, session inspection, explicit local SHA-256-verified updates,
  rollback, and managed uninstall.
- MIT licensing, contribution/security guidance, pinned Linux/macOS CI, and
  manual native release packaging with archive checksums.
- Separate Linux minimum tmux 3.3a and reference 3.7c CI lanes with verified
  official archive hashes; core PTY, dedicated startup, autostart, and theme smoke steps.

### Earlier Local Validation

These results predate the latest F7/autostart fix; they do not establish that its
final full-suite or packaged-binary reruns are complete.

- macOS ARM64 fmt, Clippy, 48 Rust tests (45 library + 2 CLI + 1 integration), release build,
  and native archive packaging passed. Core PTY and startup smoke passed against
  both the final release engine and the packaged engine with tmux 3.7c.
- Claude Code 2.1.263 SIMPLE and OpenCode 1.18.29 no-model smoke passed using
  official paths, zero mappings, and unchanged PIDs with the release engine.
- Codex 0.153.4 native and unmodified npm-wrapper launches passed no-model F7
  open/hide/reopen/hide checks. The integrity-verified official npm package was
  installed only temporarily with scripts disabled, then removed; no global
  Codex installation remains.
- Final Debian 12 aarch64 UID 0 Rust 1.84.1 checks passed: 45 tests (43 library +
  2 CLI), Clippy, and build. Both core PTY and startup passed with private-prefix
  tmux 3.3a and 3.7c after `AFT_TMUX_BINARY` pinned the client across login PATH
  resets. Default `/tmp` startup ownership, generation, and shell exit also passed.
- Unprivileged Debian 12 aarch64 execution passed all 46 Rust tests, including
  the actual-CLI installer integration test. Installation, installed-symlink
  reinvocation, updates, rollback and uninstall passed on macOS and Linux.
  See [compatibility](docs/compatibility.md).

### Validation Pending

- Final macOS full-suite and release/packaged-binary reruns for the latest changes,
  including all four required smoke tests.
- GitHub Rust/core PTY/startup/autostart/theme matrix evidence and Intel/Linux x86_64
  archive acceptance; earlier local ARM64 runs are not remote CI runs.
- Latest deployment and live F7 latency/theme acceptance in the reported
  Claude/OpenCode invocations; separate measurement of the unresolved reported
  2-5 second TUI startup latency.
- Authenticated/generating-session acceptance for all three harnesses.
- WSL2 verification; Linux ARM64 remains source-build-only despite local evidence.
- Eventual public-release download/install/update/rollback/uninstall acceptance. Local macOS archive
  packaging and smoke checks are complete; no release is published.

No published 0.1.0 release or completed validation matrix is asserted here.
