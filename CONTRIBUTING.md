# Contributing

Use [issues](https://github.com/vuongvm812/agent-float-term/issues) for reproducible
bugs and focused feature proposals. For vulnerabilities, follow
[SECURITY.md](SECURITY.md) instead of opening a public issue.

## Development Setup

Build with Rust **1.84.1**, edition **2021**. Runtime and PTY tests require
**tmux 3.4+**; tests also require **Expect**, a C compiler (`cc`), **bash**, and
**zsh**, and **Neovim**. Packaging checks also use **Python 3.9+** and **Ruby**. Linux tests need
procps, including an external `/bin/kill`, not just a
shell builtin. Install these dependencies in your chosen development or
disposable test environment. CI installs them on its own runners. Do not change
a developer's real tmux or shell configuration to run tests.

```sh
rustup toolchain install 1.84.1 --profile minimal --component rustfmt --component clippy
cargo +1.84.1 fmt --all -- --check
cargo +1.84.1 clippy --locked --all-targets -- -D warnings
cargo +1.84.1 test --locked --all-targets
cargo +1.84.1 build --locked
export TERM=xterm-256color
expect tests/pty_smoke.exp target/debug/agent-float-term
expect tests/startup_smoke.exp target/debug/agent-float-term
expect tests/autostart_smoke.exp target/debug/agent-float-term
expect tests/theme_smoke.exp target/debug/agent-float-term
cargo +1.84.1 build --locked --release
expect tests/external_smoke.exp target/release/agent-float-term
python3 -B -m unittest discover -s scripts -p 'test_release*.py'
AFT_UNINSTALL_TEST_BINARY=target/debug/agent-float-term python3 -B -m unittest discover -s scripts -p 'test_*local.py'
python3 -B -m unittest discover -s scripts -p 'test_tmux_patch.py'
ruby scripts/test-homebrew-formula.rb target/release/agent-float-term
# In a clean checkout of the source intended for packaging:
bash scripts/verify-cargo-package.sh
```

The binary project's `Cargo.lock` must be tracked and compatible with this pinned
toolchain. Do not drop `--locked` to hide a lockfile problem. All five required smoke
entry points and the lockfile are release prerequisites; CI intentionally fails
rather than skips a missing prerequisite.

Ubuntu 22.04's default tmux 3.2a is too old. Linux CI instead builds official
checksum-pinned tmux 3.4 and 3.7c in separate lanes using
`bash scripts/install-ci-tmux.sh VERSION`. This script is restricted to Linux
GitHub runners and installs only under `RUNNER_TEMP`, exporting through
`GITHUB_PATH`; do not use it to change your host installation. macOS CI uses
Homebrew tmux and records the actual version. See [compatibility](docs/compatibility.md)
for source pins, dependencies, local evidence, and outstanding checks.

## Test Isolation

Tests must use disposable tmux sockets, temporary HOME/XDG directories, and
explicit fixture configuration (`tmux -f /dev/null` for isolated test servers).
They must not attach to or kill an unrelated server, edit real startup files,
or clean up user sessions. Cleanup traps should target only resources the test
created. Keep fixture output synthetic; never record real prompts or secrets.

Release orchestration tests mock Git/GitHub/Cargo/network mutations. Do not run
confirmed `make release` to test the tooling: it publishes releases and commits/
pushes the tap formula. Use the offline tests and `make release DRY_RUN=1`; the
latter is a local plan only, not a remote preflight. See [publishing](docs/publishing.md#make-release).

Likewise, `make install` and `make uninstall` mutate the user-local installation.
Test their orchestration with `test_*local.py` (fake tools and temporary homes),
or use `DRY_RUN=1`. Never run live install/uninstall merely to test the targets.
Uninstall coverage includes busy-build refusal using `lsof` against a temporary
child process; Linux CI installs `lsof` so this check is not skipped.

All five required Expect entry points accept the binary as their first argument.
Each test owns creation and teardown of its isolated environment and must fail
on assertions or timeouts instead of silently skipping missing dependencies.
CI gives each smoke step a separate five-minute timeout and
`TERM=xterm-256color` to catch hangs consistently.

`tests/pty_smoke.exp` runs both the named process fixture and real Neovim from the
original PATH. The real editor uses private configuration, four splits, explicit
navigation mappings, no plugins, and no shada/swap files. Its event log verifies
exact key delivery and retained window/process identity, not just screen text.
Keep coverage for conflicting root bindings/prefixes, rapid main handoff, failed
inspection, and suspended/exited editors without modifying real editor config.

The optional `expect tests/native_pane_probe.exp` requires tmux 3.7c and no app
binary. It intentionally demonstrates a **failed production routing gate** while
verifying native status clicks and shell retention. It is not one of the five
shipping smoke suites. See [the prototype](docs/native-pane-prototype.md).

`tests/mouse_smoke.exp BINARY patched|stock` additionally verifies popup status
clicks, not native panes. It requires the reference tmux 3.7c on PATH and checks the
requested server capability rather than skipping mismatches. The Linux reference
CI lane builds the optional patch in runner temporary storage, runs both mouse
modes, and reruns keyboard/Neovim coverage against the patched server. See
[status-bar mouse switching](docs/status-bar-mouse.md) for the isolated builder;
never replace a developer's tmux or restart a live server as part of these tests.

`tests/autostart_smoke.exp` installs the actual supplied binary under private
HOME/XDG paths and exercises its generated Zsh template. It checks command,
empty-command, and shell-snapshot guards without autostarts or output pollution;
fake Powerlevel10k-style descriptor restoration before silent first-prompt F7
binding in existing tmux; one-shot hook removal; ordinary-shell F7 passthrough;
and dedicated startup outside tmux without nested outer sessions. The prompt
fixture is not a test of the user's actual Powerlevel10k installation.

`tests/theme_smoke.exp` checks actual PTY SGR foreground/background colors, not
just option strings. Keep the raw-popup RGB negative control, terminal-default
content/border positive checks, explicit application RGB preservation, exact
global/parent-style preservation, per-open owned pane/window resets, persistent
identities, and linked-window refusal before mutation. It selects tmux via PATH
and has passed on Linux 3.3a and macOS 3.5a/3.7c. This is not OS opacity or
emulator screenshot coverage; see [theme limits](docs/operations.md#terminal-theme-and-opacity).

`tests/external_smoke.exp` registers a real executable at a temporary Homebrew-style
stable `opt` path, retargets it to a new keg, and removes the old keg while a
watcher runs. It verifies unchanged root/restoration bindings, original watcher
identity, preserved shell state, and safe uninstall. The fixture requires realistic
0775 package directories: `admin` group on macOS, primary group on Linux. Run the
macOS fixture as a user permitted to assign that group; do not silently downgrade
the permission test to 0755. It uses no real Homebrew installation.

Keep installer coverage for Bash's inline interactive/real-TTY guards, execution
string **setness** (including empty `-ic`/`-lic`), Zsh script guards and bounded
deferred retry, SSH/disable/recursion skips, foreign-key preservation, and plain
reinstall template refresh with the recorded shell kind and unselected configs
retained. `update` must remain distinguishable from template refresh.

Inspector/core PTY worker fixtures must preserve the conservative boundary:
at most 64 foreground group members, verified frontend ancestry, helper stdin
from pipe/socket or `/dev/null`, terminal stdout/stderr allowed, and rejection of
extra terminal FDs at 3 or higher. Do not require arbitrary helper argv; retain
frontend-pair recognition and metadata revalidation within the 300 ms budget.

A fake harness can test process detection and popup plumbing, but it does not
establish compatibility with a real AI CLI. Record real tool versions and manual
results separately. WSL2 and authenticated/generating sessions remain untested;
Codex now has temporary-package no-model coverage, not a global installation.

Optional macOS-only installed-harness smoke, after reviewing its isolation and
version guards:

```sh
expect -N -n tests/actual_harness_smoke.exp target/debug/agent-float-term
```

It discovers installed harnesses on the original PATH; optional positional paths
are ordered Claude, OpenCode, Codex, and `-` explicitly skips one. Missing CLIs
are not installed. Reviewed versions are Claude 2.1.263, OpenCode 1.18.29, and
Codex 0.153.4. They run in a no-model sandbox at home/onboarding, without prompts,
authentication, IP networking, or trust approval. Codex supports both the native
executable and the original npm wrapper; pass its absolute path as the fourth
argument after the binary, Claude, and OpenCode paths (use `-` for skipped tools).
A skip or conservative eligibility gap is not a compatibility pass. This is not
a CI dependency or a substitute for authenticated/generating-session acceptance.

Codex validation used a temporary-only official npm package with
`--ignore-scripts` and verified npm integrity, then uninstalled that temporary
package. Do not infer that Codex remains installed, and do not install a global
CLI merely to run this optional test.

For local release acceptance, run all five required smoke entry points against
`target/release/agent-float-term` and again against the packaged executable.
`tests/install_binary.rs` adds a separate actual-CLI installation/update/rollback/
uninstall test to `cargo test --locked --all-targets`; its focused invocation is:

```sh
cargo +1.84.1 test --locked --test install_binary
```

Record its result separately from earlier library/CLI counts. Local archive and
installer tests do not replace download/installation checks of the eventual
published assets. For private-prefix tmux builds, see
[client selection](docs/operations.md#tmux-client-selection); the engine pins its
selected client so login-shell PATH resets do not select a different version.

Client-selection integration coverage is separately runnable with
`cargo +1.84.1 test --locked --test tmux_client`. Preserve matching-client
selection, per-server approved-path reuse, mismatch refusal without mutations,
and rejection of executable replacement during the fingerprint/version-check
bracket. Warm version-cache reuse must remain tied to executable metadata,
server version, and the server-global generation; batching must not drop global
detach-policy or ownership checks.

## Optional Benchmark

After building, select an absolute path to a trusted tmux 3.4+ executable:

```sh
expect tests/benchmark_toggle.exp target/release/agent-float-term /absolute/path/to/tmux 5 5
```

The final arguments mean five cold creations and five warm reopens per creation
(twenty-five warm samples per version). The benchmark uses private HOME/XDG,
socket, clean shell, and a synthetic native harness. Bare timings and separate
exec-only shim invocation counts are distinct observations. It is optional, not
a CI latency threshold or a full-profile/model-startup acceptance test.

Record attachment, prompt frame, hide, post-hide worker cleanup, and cold shell
read/response separately. The cold-read probe now waits for a handoff
acknowledgment before sending `read` input to avoid readline prefetch; do not
compare its revised timings directly with the old cold baseline. No Linux
percentiles are published because the old read probe failed there. Keep observer
overhead, sample size, tmux version, and tested revision with any result.
Latest cold creation uses ten tmux invocations after a global-policy check was
added; the measured optimized revision used nine. See the
[recorded macOS comparison](docs/compatibility.md#f7-benchmark), not a 100 ms
end-to-end guarantee.

## Validation Status

Record focused bugfix runs separately from full-suite totals. The latest macOS
autostart run passed with 31-44 ms command/shell-snapshot checks and zero
autostarts; core PTY worker, inspector, and installer checks also passed. The
latest full Linux Rust run passed 57 tests (50 library + 2 CLI + 1 installer +
4 tmux-client). macOS `cargo test --test tmux_client` passed all four tests; the
expected 59-test full suite still needs a recorded final run. Release/package
reruns remain pending. Live F7 was confirmed before optimization, but latest
deployment and live full-profile latency/theme acceptance remain pending.
Neither live F7 nor `doctor` recognition is model-interaction acceptance.
The demonstrated snapshot attach-blocking fix does not measure or establish
resolution of the reported 2-5 second TUI startup latency.

## Changes

- Keep changes focused, with regression tests for behavior changes.
- Preserve normal, unwrapped `claude`, `codex`, and `opencode` invocations.
- Fail conservatively: ambiguous foreground detection forwards the key.
- Preserve native tmux copy-mode and command-prompt keys; the popup hide binding
  applies to normal input, not every modal key table.
- Preserve detached jobs and orphan sessions; destructive actions are explicit.
- Keep installation preview-first and target only managed integration state.
- Update docs and the changelog when changing the public CLI or config format.
- Exclude `.codegraph/`, local credentials, terminal recordings, and build output
  from contributions and release archives.

In a pull request, explain the problem, the change, commands actually run, and
anything not verified. Do not present planned tests as passing tests. GitHub
Actions references must be pinned to full commit SHAs, with the upstream release
noted in a comment; verify pins against the upstream repository before changing
them. The workflows do not create commits or add author/coauthor trailers.

Contributions are made under the project's [MIT license](LICENSE).
