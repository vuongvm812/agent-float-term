# Compatibility And Validation

**Current status: Neovim navigation and optional popup status-mouse patch locally tested.**
The runtime minimum is **3.4**. Each float belongs to one AI invocation and is
reset when it exits. Tmux keyboard shortcuts target the main terminal, except
`Ctrl+h/j/k/l` remain with foreground Neovim inside the float.
Returning to the original AI pane restores a
temporarily hidden float; explicit F7 hide disables restoration until reopened.
Current evidence is separate from the historical results below.

These latest changes have not been published or tested in remote CI. The older
0.2.2 crate is already public. Historical source/archive validation does not
establish acceptance of new runtime changes or a completed remote matrix.

## Current Change Validation

| Change | Evidence | Still Needed |
| --- | --- | --- |
| Configuration, installer, and Rust APIs | 90 macOS ARM64 tests and Clippy passed; current Linux ARM64 test-target cross-check passed; 83 non-root Debian 12 ARM64 tests passed before first-use changes | Current Linux runtime and remote CI acceptance |
| Generated integration layout | Fresh install, Bash/Zsh migration, no-write preview, idempotency, byte/mode preservation, collision/edited-file refusal, and migrated uninstall passed; all four release smoke suites passed | Broader Linux runtime migration coverage |
| JSON-based smoke fixtures | All five current suites passed on macOS tmux 3.7c: core PTY and package upgrade used release builds; startup/autostart/theme used debug. Current core PTY also passed on 3.5a (debug) | Updated archive acceptance, Linux runtime, minimum 3.4 rerun, and latency benchmark |
| Neovim navigation | Real Neovim 0.11.0 with four splits and isolated mappings; all four keys, rapid sequences, root/prefix/prefix2/custom-table main routing, queued handoff, shared navigation prefix, suspend/resume, failed helper, unchanged bindings, and retained state passed | Linux/remote CI runtime, other editor versions, and personal navigation-plugin behavior |
| Native-pane mouse experiment | Isolated tmux 3.7c probe passed actual SGR status session clicks and retained-shell checks; prefix command demonstrably targeted the viewer instead of main | **Not shipping:** main-shortcut parity without user-binding wrappers and multi-client ownership remain blocked; see [prototype](native-pane-prototype.md) |
| Popup status-mouse extension | Pinned tmux 3.7c patch built on macOS ARM64; stock/patched PTY tests passed top/default and bottom/multiline/custom status clicks, observer isolation, preserved shell/restoration, negative events, unchanged bindings, and clean AI exit | Requires a patched **server**, not just an application/client update; Linux CI and live status-plugin deployment pending; see [setup](status-bar-mouse.md) |
| Package-owned executable upgrades | Current first-bind/start and upgrade PTY passed on macOS 3.7c; prior non-root Debian 12 ARM64 3.4 debug upgrade test passed; realistic 0775 package groups, removed old keg, original watcher and shell state, stable root/restore bindings | Linux first-use runtime, real Homebrew upgrade acceptance, and remaining native targets |
| Coordinated publication | 60 offline release-tool tests, local Make dry run, and workflow lint passed; altered assets are refused even when their current checksums/formula agree, unless they match immutable build evidence | Real Actions evidence upload/download and confirmed publication; no live mutation tests were run |
| Cargo distribution | Upload file list inspected; publish dry-run built the unpacked crate and aborted upload as expected; actual Cargo install/uninstall from unpacked crate passed with isolated root and integration-only setup | Clean tagged-source remote gate and installation from published crates.io version |
| Homebrew formula and release helpers | 14 fixture tests, actual Homebrew DSL loading, isolated formula integration test, Ruby syntax, actionlint, and ShellCheck passed | Full audit blocked locally by missing `rubocop-ast`; public formula URLs and real Brew install/test/upgrade/uninstall remain pending |
| tmux 3.4 minimum | Enforced at runtime; core PTY suite passed on macOS 3.4, 3.5a, and 3.7c; CI minimum lane updated and archive hash verified | GitHub CI execution |
| AI-invocation lifetime | Core PTY tests cover same-invocation state, fresh restart, visible/hidden termination, background-job cleanup, and stop/resume | Real authenticated AI exit scenarios; no daemonized-job containment claim |
| Main shortcuts and restoration | Core PTY tests cover root, prefix, prefix2, rapid custom-table sequences, new main windows/sessions, return-to-origin restoration, prompt protection, explicit hide, and client contention | Broader terminal-emulator and arbitrary custom mouse/plugin behavior |

Tests use private sockets, temporary configuration and synthetic CLI fixtures;
they do not change the running OpenCode conversation or install into the live
tmux server. Polling is best-effort: 100 ms liveness checks and approximately
one-second restoration retries, not hard real-time guarantees.

The minimum/reference release tests caught and fixed timing-based paste detection
bypassing rapid custom-table shortcuts on 3.4/3.5a. Only the owned float disables
that heuristic; bracketed paste keeps native handling. Theme coverage now retains
SGR state across checkpoints and handles empty SGR reset and DEC saved rendition,
rather than assuming each captured chunk starts with default colors.

The Linux package-mode tests used `rust:1.84.1-slim` (Debian 12 / aarch64), UID/GID
65534, read-only source/registry mounts, and temporary build output. They include
the external-upgrade debug PTY test, not a rerun of all four other Linux suites
or Linux Clippy. The macOS publish dry run used `--allow-dirty` only to verify this
uncommitted working tree; CI and real publication require a clean checkout.
The current local ARM64 archive is under `target/tmp/release-acceptance/`; its
four-entry layout was inspected and the extracted executable matched the built
binary byte-for-byte before all five packaged smoke suites passed. This is not
a published GitHub asset or acceptance of the other platform archives.

The earlier generated-script layout was applied to the local installation: scripts
now live under the data directory, the exact managed `.zshrc` reference was
updated, and the config directory contains no generated files. Read-only doctor
reported valid defaults and an intact binding; the existing tmux 3.5a server PID
was unchanged. Native-package mode has not been applied to that installation.
This is not an authenticated/model-interaction test.

## Historical Local Evidence

All results in this section predate the JSON/invocation/navigation changes.
"Latest" within these records refers to the preceding latency/theme work, not
acceptance of the new contract. tmux 3.3a results remain historical evidence only;
they do not lower the 3.4 minimum.

### Latest F7 Latency And Theme Work

Live F7 was confirmed before optimization. The latest latency/theme revision is
installed and bound on the user's existing server; visual/full-profile acceptance remains;
the reported roughly one-second F7 delay is not an isolated benchmark result.

| Environment | Scope | Result | Limits |
| --- | --- | --- | --- |
| Linux | Latest full Rust suite | **57 passed: 50 library + 2 CLI + 1 installer + 4 tmux-client integration tests** | Local result, not GitHub CI or native x86_64 archive acceptance |
| macOS | Latest full Rust suite, Clippy, formatting and release build | **59 passed: 52 library + 2 CLI + 1 installer + 4 tmux-client tests** | All four smoke suites also passed against the release binary; latest packaged-archive acceptance is separate |
| Linux tmux 3.3a; macOS tmux 3.5a and 3.7c | `tests/theme_smoke.exp` | Passed on all three versions | Actual PTY SGR colors, not emulator screenshots or OS opacity tests |

Theme smoke checks actual emitted color sequences with positive and negative
controls: a raw popup inherits seeded global RGB styles, while the owned popup's
default content and border use terminal defaults. Explicit application RGB is
preserved. Global popup/window styles and outer overrides remain exactly
unchanged, owned pane/window styles reset on reopening, the shell and parent
identities persist, and foreign-linked windows are refused without style changes.
See [theme behavior and limits](operations.md#terminal-theme-and-opacity).

### F7 Benchmark

`tests/benchmark_toggle.exp` is an optional research benchmark, not a CI latency
threshold. Each tmux version used five cold creations and twenty-five warm
reopens in a private clean shell with a synthetic native fixture, not the user's
full profile or an actual AI startup. Bare runs measure timings; separate
exec-only shim runs count tmux invocations. Attachment is observed by client
listing, prompt-frame by PTY output, hide by client detachment, and cleanup by
worker/error state settling. Polling and observer overhead are included.

Recorded macOS baseline/optimized **p50 milliseconds**:

| tmux | Observation | Baseline | Optimized |
| --- | --- | --- | --- |
| 3.5a | Warm attachment | 209.7 | 113.6 |
| 3.5a | Warm prompt frame | 256.2 | 163.3 |
| 3.5a | Hide | 9.8 | 10.2 |
| 3.5a | Post-hide cleanup settled | 45.0 | 18.1 |
| 3.7c | Warm attachment | 218.2 | 116.6 |
| 3.7c | Warm prompt frame | 263.5 | 162.1 |

The 0.4 ms hide difference is observation noise, not evidence of an improvement
or meaningful regression. Cleanup means post-hide worker cleanup, not orphan
session deletion. Warm open uses **7 tmux invocations, down from 17**; worker
cleanup uses **1, down from 3**. Latest cold creation uses **10**, not the **9**
in the measured optimized revision, because it adds a global-policy check.
Do not label the earlier timing run as a measurement of every final change.

The cold read/response probe now waits for a shell handoff acknowledgment before
sending input to `read`; the old multiline send could be prefetched by readline.
That old probe failed on Linux, so **no Linux percentiles are reported**. The
revised cold-read timing method is not directly comparable to the old baseline.
Five cold samples also cannot establish a reliable tail-latency estimate.

These results do not establish a 100 ms end-to-end guarantee, resolve the user's
roughly one-second live F7 delay, or measure the separate several-second AI CLI
startup delay. The macOS full suite, release smoke tests and installed deployment
are complete. Latest packaged-archive and live visual/latency acceptance remain.

### Earlier F7/Autostart Fix

The reported default tmux 3.5a server lacked F7 because the old automatic shell
template skipped `TMUX`; the previous inspector also refused same-group MCP/LSP
helpers. Human-prompt initialization now quietly binds inside existing tmux
without a nested outer session or foreign root-key overwrite. No user tmux
config was changed, including the pre-existing bad `default-terminal` setting.

| Environment | Scope | Result | Limits |
| --- | --- | --- | --- |
| macOS | `tests/autostart_smoke.exp`, actual installed binary and generated template | Passed; command/shell-snapshot checks took 31-44 ms with zero autostarts; first-prompt binding after fake Powerlevel10k cleanup and dedicated startup passed | Synthetic prompt cleanup, not the user's Powerlevel10k config or TUI startup timing |
| macOS | Core PTY with workers; focused inspector and installer tests | Passed | Earlier focused coverage, not the final latency/theme full-suite count |
| User's tmux 3.5a server | Read-only `doctor` on live Claude and OpenCode invocations with helpers; live F7 | Both recognized; live F7 confirmed before optimization; optimized binary installed and matching client/binding verified | Latest live visual/latency acceptance pending; not model-interaction or generating-session acceptance |

The shell-template fix addresses the demonstrated snapshot attach-blocking
mechanism. The user's reported **2-5 second TUI startup latency has not been
measured**; the 31-44 ms fixture timings do not establish that all startup delays
are solved. `update` alone does not deploy the new shell template: refresh owned
templates with `install --yes`, retaining unselected configs and the recorded
shell kind. See [operations](operations.md#local-update-and-rollback).

Recognition now allows up to 64 foreground group members with verified frontends
and descendant helpers. Helper stdin must be pipe/socket or `/dev/null`; TTY
stdout/stderr are allowed, while terminal FDs at 3 or higher block eligibility.
Helper argv is unnecessary outside frontend-pair recognition. Relevant metadata
is revalidated within a 300 ms inspection budget; this is not an atomic proof of
keyboard ownership. See [recognition limits](operations.md#recognition-limits).

### Earlier Baseline

The following recorded counts and release/archive results predate the latest
fix. They are retained as historical evidence, not final bugfix rerun results.

| Environment | Tools / scope | Result | Limits |
| --- | --- | --- | --- |
| macOS ARM64 | Rust 1.84.1; fmt, Clippy, Rust tests, release build | Passed; **48 tests: 45 library + 2 CLI + 1 installer integration** | Local native build |
| macOS ARM64 | tmux 3.7c; core PTY and startup smoke against `target/release/agent-float-term` | Both passed against the final release engine | Does not validate macOS Intel or every macOS version |
| macOS ARM64 | Native archive packaging; core PTY and startup smoke against `dist/verified/agent-float-term-v0.1.0-aarch64-apple-darwin/agent-float-term` | Final packaging succeeded; both packaged-binary smoke tests passed | Local archive, not a published GitHub asset |
| Debian 12 aarch64, UID 1000 | Rust 1.84.1; complete Rust test suite | Passed; **46 tests: 43 library + 2 CLI + 1 installer integration** | Unprivileged execution with private HOME/cache/artifacts; Clippy and builds also passed in the earlier UID 0 run |
| Debian 12 aarch64, UID 0 | Private-prefix tmux 3.3a; core PTY and startup smoke | Both passed on the first final rerun after client pinning | Not an Ubuntu x86_64 release-artifact test |
| Debian 12 aarch64, UID 0 | Private-prefix tmux 3.7c; core PTY and startup smoke | Both passed on the first final rerun after client pinning, while `/usr/bin/tmux` remained 3.3a | Confirms the client survives login-shell PATH reset; not a Linux x86_64 archive test |
| Debian 12 aarch64 | Default `/tmp` fallback with runtime/temp overrides unset | Passed: private user-owned 0700 directory, generation marker, ordinary shell and shell exit | No reboot-persistence claim |
| macOS ARM64 | Installed Claude Code 2.1.263; tmux 3.7c; release engine | No-model home/onboarding smoke passed; original official paths, zero mappings, same harness/shell PIDs across F7 open/hide/reopen/hide | SIMPLE mode in a sandbox; authenticated/default-mode and generating sessions untested |
| macOS ARM64 | Installed OpenCode 1.18.29; tmux 3.7c; release engine | No-model home/onboarding smoke passed; original official paths, zero mappings, same harness/shell PIDs across F7 open/hide/reopen/hide | Sandboxed startup only; authenticated/provider-connected and generating sessions untested |
| macOS ARM64 | Official Codex 0.153.4 native binary and unmodified npm wrapper | Both launches passed F7 open/hide/reopen/hide with zero mappings and unchanged harness/shell PIDs | Temporary-only package; no authentication, model interaction, or IP networking |
| macOS ARM64 and unprivileged Linux ARM64 | `tests/install_binary.rs` actual-CLI install/update/rollback/uninstall | Passed, including invocation and reinstall through the installed symlink, checksum rejection, activation, rollback and repeated uninstall | Local executable payloads, not published downloads |
| WSL2 | No manual run recorded | Untested | Linux results do not establish WSL2 behavior |
| macOS Intel / Linux x86_64 | Native archive workflows configured | Archives not locally tested; no GitHub run recorded | Local ARM64 results do not establish these targets passed |

Older tmux serializes `C-i` as `Tab`; tmux 3.7c can retain distinct bindings.
The final core PTY runs cover the version-specific representation without
relaxing key-collision ownership checks. `AFT_TMUX_BINARY` pins the selected
client across login-shell PATH changes, preventing an older system client from
silently replacing the private-prefix client. See [operations](operations.md#tmux-client-selection).

Codex was acquired as official `@openai/codex@0.153.4` in a temporary prefix with
`--ignore-scripts` and verified npm integrity. Both the native executable and
unmodified npm-wrapper launch were tested in their original package layout.
The temporary packages were subsequently uninstalled; there is no global Codex
installation. This is actual Codex startup coverage, not authenticated or
generating-session coverage.

The optional `tests/actual_harness_smoke.exp` uses installed, reviewed harness
versions in their original layouts. It adds no `harness_paths` overrides, sends
no prompts, performs no model requests, and approves no trust/onboarding steps.
Its macOS sandbox denies IP networking and isolates HOME/config/state from
personal credentials. This test-specific sandbox does **not** make tmux or the
product a sandbox. The script's skipped-harness result is not a compatibility
pass, and the script itself does not install missing CLIs. It now supports
reviewed Codex 0.153.4 as well as Claude and OpenCode. Authenticated and
generating-session acceptance remains an independent manual task.

The successful Rust installer tests cover local payload/config management and
fixture update/rollback behavior; core PTY runs exercise actual install/uninstall
and owned-key restoration. The additional `tests/install_binary.rs` test exercises
the public CLI with executable payloads, including hash rejection, activation,
rollback, and uninstall. It passed on macOS and unprivileged Linux and is included
in the 48/46 totals. It also caught and now guards macOS executable-symlink
resolution during reinstall. None of these local tests is a download/install
test of a published release.

## Recorded CI Coverage

The configured minimum lane is now tmux 3.4. Local tests do not imply a GitHub
Actions run; historical 3.3a source hashes remain below for reference only.

The reusable CI workflow runs Rust 1.84.1 fmt, Clippy, Rust tests, debug/release
builds, and core PTY, startup, autostart, and theme smoke entry points. Each smoke step
has a five-minute timeout and `TERM=xterm-256color`. It never installs or
authenticates an AI CLI. The optional actual-harness smoke and F7 benchmark are
not CI dependencies; no benchmark latency threshold is enforced.

| Runner | Architecture | tmux selection | Status |
| --- | --- | --- | --- |
| `ubuntu-22.04` | x86_64 | Official 3.4 archive, SHA-256 pinned, built in runner temp | Minimum-version lane configured; no GitHub run recorded |
| `ubuntu-22.04` | x86_64 | Official 3.7c archive, SHA-256 pinned, built in runner temp | Reference lane configured; no GitHub run recorded |
| `macos-14` | ARM64 | Current Homebrew tmux; actual `tmux -V` recorded | Not version-pinned; no GitHub run recorded |
| `macos-15-intel` | x86_64 | Current Homebrew tmux; actual `tmux -V` recorded | Not version-pinned; no GitHub run recorded |

Ubuntu 22.04's packaged tmux 3.2a is below the minimum and is not
used as the test dependency. `scripts/install-ci-tmux.sh` accepts only the two
pinned versions, verifies the archive **before extracting or building it**, and
installs below `RUNNER_TEMP`. It appends the private binary directory to
`GITHUB_PATH`, and the following CI step checks that the selected version is
exact. The script requires a Linux GitHub Actions runner; it does not install
into a host prefix, edit config, or launch a server.

Linux CI installs build-essential (`cc`/make), bison, pkg-config, libevent/ncurses
development headers, curl/CA certificates, Expect, procps (including external
`/bin/kill`), bash, and zsh. macOS uses the runner's C compiler and Homebrew tmux,
Expect, bash, and zsh. All lanes record tool versions; keep those logs with the
tested source commit when recording a final result.

## Source Pins

The following official release assets were queried using the GitHub API and
downloaded over HTTPS to compute their SHA-256.
tmux 3.7c's computed value also matches its GitHub asset `digest`. GitHub exposes
no digest for the 3.3a and 3.4 assets, so those pins are locally computed official
download hash, not a separately published upstream signature.

| Official archive | SHA-256 | GitHub asset ID |
| --- | --- | --- |
| [tmux-3.3a.tar.gz](https://github.com/tmux/tmux/releases/download/3.3a/tmux-3.3a.tar.gz) | `e4fd347843bd0772c4f48d6dde625b0b109b7a380ff15db21e97c11a4dcdf93f` | `67990636` |
| [tmux-3.4.tar.gz](https://github.com/tmux/tmux/releases/download/3.4/tmux-3.4.tar.gz) | `551ab8dea0bf505c0ad6b7bb35ef567cdde0ccb84357df142c254f35a23e19aa` | `151288686` |
| [tmux-3.7c.tar.gz](https://github.com/tmux/tmux/releases/download/3.7c/tmux-3.7c.tar.gz) | `7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf` | `518107178` |

These pins detect archive changes, not compromise of the trusted upstream
release source. Review and verify new pins explicitly; do not bypass hash checks
or substitute an unpinned system tmux when a download/build fails.

## Release Limits

Release packaging remains native macOS ARM64, macOS Intel, and Ubuntu 22.04
Linux x86_64 only. The Linux GNU archive targets a glibc 2.35 baseline; older
glibc and Alpine/musl are not supported by that archive. Debian 12 ARM64 local
source-build evidence does not add an ARM64 release artifact or prove the
x86_64 archive works. WSL2 and other platforms need their own acceptance.

The earlier macOS ARM64 archive was built and its packaged engine passed core
PTY and startup tests locally. The latest changes still need final macOS
full-suite and release/packaged-binary reruns, including all four required smoke
tests. The latest Linux 57-test result above does not replace those checks.
No archive has been published.
GitHub CI, Intel/Linux x86_64 archive
acceptance, WSL2, authenticated/generating harness sessions, and public-release
download/install/update/rollback checks remain open in the
[release checklist](release-checklist.md). Source and `Cargo.lock` tracking also
await an explicit maintainer commit; none has been requested or created.
