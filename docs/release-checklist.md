# Release Checklist

**Current status: refactor locally tested; publication checks remain pending.**
The minimum is now tmux **3.4**. Current macOS core PTY tests pass on 3.4, 3.5a,
and 3.7c. Current results are recorded separately from the historical work below.
Earlier macOS ARM64 release/packaged-engine and Debian 12 aarch64 reruns passed.
The earlier Linux Rust suite passed 57 tests; macOS passed 59 tests and all four
release-binary smoke suites. Latest packaged-archive acceptance remains pending. Theme smoke passed
on Linux tmux 3.3a and macOS 3.5a/3.7c. Claude, OpenCode, and Codex have earlier
no-model smoke coverage. See the
[compatibility matrix](compatibility.md) for exact versions, counts, and limits.
No commit, remote GitHub CI run, or release publication has been performed.

## Package Release Preparation

See [publishing](publishing.md) for the Homebrew tap handoff, initial crates.io
API-token bootstrap, subsequent OIDC workflow, and exact approval steps. The
following evidence is newer than the historical counts below. This work does
not deploy the native-package mode to the user's live installation.

- [x] Implement integration-only registration with stable package paths, retained
  external reinstall mode, and package-preserving uninstall. Keep managed formats
  1/2 and add format 3 for external registrations.
- [x] macOS ARM64: 85 Rust tests and Clippy passed. Non-root Debian 12 ARM64:
  83 Rust tests passed using Rust 1.84.1, UID/GID 65534, in a disposable container.
- [x] Debug external-upgrade PTY acceptance on macOS tmux 3.7c and Linux tmux 3.4:
  realistic 0775 package directories, old-keg removal, unchanged root/restore
  commands, original watcher, shell-state preservation, cleanup, and managed
  runtime with alternate XDG config/data roots.
- [x] Rerun all five current release-binary smoke suites on macOS tmux 3.7c.
  Verify the packaged crate via publish dry-run and actual isolated Cargo
  install/integration/uninstall. Local `--allow-dirty` verifies working-tree
  changes only; it is not clean tagged-source or registry installation acceptance.
- [x] Package a fresh macOS ARM64 archive, inspect its four-entry layout, compare
  the extracted binary byte-for-byte, and run all five smoke suites against it.
  Local output is `target/tmp/release-acceptance/`, not a published release.
- [x] Prepare version/tag validation, checksum/layout-derived stable formula,
  isolated formula test, and crate content/build verification. Fourteen helper
  tests, real Homebrew DSL loading, actionlint, ShellCheck, and Ruby syntax passed.
- [x] Prepare dispatch-only crates.io workflow with dry-run default, exact publish
  confirmation, protected-environment hook, and pinned short-lived OIDC auth.
- [ ] Run clean remote CI including the new external-upgrade release smoke gate.
- [ ] Complete real Homebrew audit/install/test/upgrade/uninstall on all supported
  platforms. Local strict audit is blocked by missing `rubocop-ast`; no host
  dependency was installed to bypass that blocker. DSL/shim checks are not audit.
- [ ] Publish the reviewed stable archives and formula; hand off the formula to
  `vuongvm812/homebrew-tap/Formula/agent-float-term.rb` and publish the tap change.
- [ ] Complete initial crates.io publish manually, verify Cargo installation from
  the registry, and configure both the `crates-io` required-reviewer environment
  and matching crates.io Trusted Publisher for subsequent versions.
- [ ] Only after channel publication/acceptance, replace the pending-availability
  notices in README, installation guide, publishing guide, and tap README.

## Current Change Acceptance

- [x] JSON-only configuration: seven focused config unit tests passed with Rust
  1.84.1; defaults, `shortcut`, dimension boundaries, advanced paths, legacy-file
  preservation, security checks, and XDG semantics covered.
- [x] Remove TOML dependency and unused lockfile packages; convert existing
  Expect config-format fixtures to JSON. Six Tcl fixture expressions round-trip
  through JSON parsing, including quote/backslash paths and custom shortcuts.
  This is not a smoke/benchmark pass.
- [x] Enforce tmux 3.4, update the CI minimum lane/archive pin, and run the core
  PTY suite on private 3.4, 3.5a, and 3.7c servers on macOS.
- [x] Verify reset when the owning AI invocation exits, both visible
  and hidden; next invocation in the same pane must create a fresh float.
- [x] Verify main-terminal shortcuts act on the main terminal, temporary hiding
  during navigation, and automatic restoration only on return to the original
  still-active AI pane. F7 hide must disable restoration until reopened.
- [x] Cover command prompts, rapid custom-table input, multiple clients, process
  replacement, visible/hidden exit, terminal jobs and stop/resume in PTY tests.
- [x] Update installer preservation wording; run 75 macOS Rust tests and the
  four JSON-based smoke suites. Linux ARM64 test-target cross-check passes.
- [x] Migrate generated scripts into the data directory and update exact managed
  references. The local migration passed doctor without restarting tmux.
- [ ] Rerun current Linux runtime tests, refresh toggle benchmarks, and accept
  packaged archives before publication.

## Historical Latency/Theme Evidence

These checked items predate the JSON and runtime-contract changes.

- [x] Latest Linux Rust suite: 57 passed (50 library + 2 CLI + 1 installer +
  4 tmux-client). This is not native x86_64 release-artifact or GitHub CI evidence.
- [x] macOS focused `cargo test --test tmux_client`: four passed, including
  executable replacement during the fingerprint/version-check bracket.
- [x] Latest macOS full suite: 59 passed, plus Clippy, formatting, release build,
  core/startup/autostart/theme release smoke tests. Optimized binary installed and
  bound to the matching client without changing user shell/global tmux theme files.
- [x] Actual-SGR theme smoke passed on Linux tmux 3.3a and macOS 3.5a/3.7c:
  positive/negative color controls, global/parent preservation, per-open owned
  style reset, explicit application RGB, and linked-window refusal.
- [x] Isolated macOS F7 benchmark recorded five cold and twenty-five warm samples
  per version. See [measurements and limits](compatibility.md#f7-benchmark);
  no Linux percentiles or end-to-end latency guarantee are asserted.
- [x] Live F7 confirmed before optimization; this does not accept the latest
  deployment or establish resolution of the user's roughly one-second delay.

## Earlier Local Evidence

The counts and release/archive checks below are the earlier baseline, not
updated full-suite results for the latest bugfix.

- [x] Rust 1.84.1 / edition 2021 local builds; macOS fmt, Clippy, and 48 tests
  (45 library + 2 CLI + 1 integration) passed. This does not mark the lockfile as tracked.
- [x] Final macOS ARM64 release build and native archive packaging succeeded.
- [x] Both `pty_smoke.exp` and `startup_smoke.exp` passed with tmux 3.7c against
  `target/release/agent-float-term` and the packaged
  `dist/verified/agent-float-term-v0.1.0-aarch64-apple-darwin/agent-float-term`.
- [x] Final Debian 12 aarch64 UID 0 Rust 1.84.1 run passed 45 tests
  (43 library + 2 CLI), Clippy, and build.
- [x] Unprivileged Debian 12 aarch64 UID 1000 rerun passed all 46 tests, including
  actual-CLI install, installed-symlink reinstall, update, rollback and uninstall.
- [x] Final Linux core PTY and startup reruns both passed on the first pass with
  private-prefix tmux **3.3a** and **3.7c**, including version-specific `C-i`/`Tab`
  handling and the client pin across login-shell PATH reset.
- [x] Default `/tmp` startup with runtime/temp overrides unset passed private
  0700 ownership, generation-marker, ordinary-shell, and shell-exit checks.
- [x] No-model release-engine smoke passed for installed Claude Code 2.1.263
  SIMPLE and OpenCode 1.18.29: original paths, zero mappings, unchanged PIDs.
- [x] No-model Codex 0.153.4 native and unmodified npm-wrapper launches passed
  F7 open/hide/reopen/hide with zero mappings and unchanged PIDs. Official npm
  integrity was verified, acquisition was temporary-only with scripts disabled,
  and temporary packages were subsequently uninstalled.
- [x] Core PTY coverage passed for foreground scope, normal-input F7 hide,
  detached shell/job/cwd persistence, rename/exit, per-pane isolation, single
  viewer, orphan preservation, explicit cleanup, and incompatible-policy refusal.
- [x] Startup smoke passed ordinary shells, terminal guard, literal cwd, owned
  server reuse without client takeover, and no nested outer session.
- [x] Existing Rust installer tests passed preview/consent, independent config
  opt-ins, unowned destination refusal, retained integrations, bash/zsh guards,
  fixture updates/rollback, and safe transaction/ownership handling.
- [x] Core PTY actual install/uninstall passed exact owned-key restoration,
  preservation of later user bindings and live shells, and missing-executable
  key forwarding. No unavailable server is claimed as successfully unbound.
- [x] Docs reflect native tmux modal keys, read-only doctor, private dedicated
  startup, supported CLI grammar, and the lack of atomic key-table compare-and-swap.

## Remaining Acceptance

- [ ] Rerun all four smoke entry points against a newly packaged archive.
- [ ] Verify the installed latency/theme revision with live F7
  open/hide/reopen and full-profile latency in the reported Claude/OpenCode
  invocations with helpers. Pre-optimization live F7 is not latest acceptance.
- [ ] Check terminal-theme behavior with the user's emulator policy without
  changing OS opacity or global tmux styles. Explicit app backgrounds and custom
  `pane-colours` remain caveats; PTY SGR coverage is not visual opacity acceptance.
- [ ] Measure the reported 2-5 second TUI startup latency separately; the fixed
  shell-snapshot attach-blocking mechanism does not establish all delays are gone.
- [ ] Track reviewed source and `Cargo.lock` in an explicitly requested maintainer
  commit. No commit or tag is authorized by this checklist alone.
- [ ] Run GitHub CI fmt, Clippy, tests, debug/release builds, core PTY, startup, autostart, and theme
  smoke on all four lanes across three native targets. Record run URLs and exact
  source/tool versions; local runs are not remote CI results.
- [ ] Complete Intel and Linux x86_64/glibc archive acceptance. ARM64 local
  results do not establish these archives work; Linux ARM64 remains source-only.
- [ ] Record authenticated/generating-session acceptance independently of all
  three successful no-model smokes. Do not use credentials without explicit consent.
- [ ] Review final public CLI/help and documentation against the exact source
  selected for publication, including the additional installer test result.
- [ ] If claiming WSL2 support, record a separate manual WSL2 run; otherwise keep
  it explicitly unverified. Do not imply Linux ARM64 binary support.

## Prepare

Only the maintainer creates source commits and tags. The workflows never commit,
rewrite history, add coauthor trailers, or create a source tag. This checklist
does not authorize an automation agent to do so.

1. Reconcile acceptance results and unresolved limitations. Update the changelog
   from Unreleased only when there is an actual release to describe.
2. Ensure the package version matches the intended tag exactly after removing
   its leading `v`, for example package `0.1.0` and tag `v0.1.0`. A prerelease
   version such as `0.1.0-rc.1` requires tag `v0.1.0-rc.1`.
3. Review and commit intended files as the maintainer, excluding `.codegraph/`,
   secrets, local state, and build output. The binary lockfile belongs in source.
4. Create and push the version tag yourself, pointing at the reviewed commit.
   Tag pushes alone do not publish a release.
5. From Actions, manually dispatch **Release** on the trusted default branch
   with that existing tag. Leave `prerelease` enabled during candidate testing.

## Automation

The workflow resolves the existing tag to a commit, reruns the reusable CI
matrix at that commit, and builds with `cargo +1.84.1 build --locked --release`.
Builds are native, not cross-compiled:

| Runner | Target |
| --- | --- |
| `ubuntu-22.04` | `x86_64-unknown-linux-gnu` |
| `macos-15-intel` | `x86_64-apple-darwin` |
| `macos-14` | `aarch64-apple-darwin` |

Runner host architecture is checked before packaging. Linux ARM64 stays
source-build-only despite Debian local validation; no native ARM64 artifact lane
has been added. Linux packaging remains Ubuntu 22.04 / glibc 2.35 baseline.
CI has two Linux lanes (checksum-pinned tmux 3.4 and 3.7c) plus
the two macOS lanes (Homebrew tmux, actual version logged). It installs Linux
test/build dependencies including procps, `cc`, bash, and zsh. Source tmux goes
only into the runner's temporary prefix; it is not included in release archives.
Local minimum-version checks do not substitute for a successful remote matrix.

`bash scripts/package-release.sh TAG TARGET [OUTPUT_DIRECTORY]` checks tag/package-version equality
and the native host, builds the release binary, and writes
`dist/agent-float-term-TAG-TARGET.tar.gz`. Each archive has one same-named root
directory containing only `agent-float-term`, `LICENSE`, and `README.md`. The
optional output directory defaults to `dist`; existing packages are never overwritten.

The publish job downloads only this run's three expected archives, generates
`SHA256SUMS`, and creates a **draft** GitHub release for the existing tag. Stable
drafts also include `agent-float-term.rb`, rendered from the exact source/template
and actual verified archive hashes. Prerelease drafts do not include a stable
formula. The original tag object and peeled commit are rechecked before upload. Only
that job receives `contents: write`; CI/build jobs have read-only repository
permissions. Third-party action references are pinned to verified full SHAs.
There is no signing, attestation, notarization, automatic source commit, or
automatic stable publication.

The workflow refuses to replace an existing release. If publication partially
fails, inspect the draft and assets manually; do not silently overwrite an
already published release or move its tag. A rerun may require deleting an
incomplete **draft** through an explicit maintainer action.

## Review The Draft

These checks apply to the eventual GitHub release assets, not the already tested
local macOS ARM64 archive. Local fixture/CLI self-management checks also do not
substitute for installing and updating from each published platform archive.

- [ ] Confirm tag and source commit match the reviewed source. Do not move tags.
- [ ] Confirm all three archives and `SHA256SUMS` exist and names match
  [installation details](installation.md#release-archives).
- [ ] For a stable draft, review the generated `agent-float-term.rb` asset against
  the accepted archives and complete the [tap handoff](publishing.md#homebrew-tap).
- [ ] Inspect archive contents: only binary, LICENSE, README; no `.codegraph/`,
  config, prompts, credentials, source checkout, or hidden local files.
- [ ] Download each archive onto the corresponding platform, verify its archive
  SHA-256, extract, and run `--version`, `--help`, and isolated acceptance tests.
- [ ] Confirm GNU libc and macOS minimum compatibility based on actual tests;
  do not extrapolate older OS support from a successful runner build.
- [ ] Test installation instructions and local binary update/rollback using
  archive and binary hashes correctly.
- [ ] Verify binary-only `update` versus owned-template refresh with
  `install --yes`; retain unselected configs and the recorded shell kind.
- [ ] Replace generated release notes with accurate highlights, known issues,
  tested versions, and explicit unverified platforms/harnesses.
- [ ] Keep candidate releases marked prerelease. Only publish as stable when
  readiness is established; only stable releases populate `/releases/latest`.
- [ ] Publish the reviewed draft manually and check the README download links.

## Validation Record

Earlier baseline totals below remain distinct from latest focused and Linux
results. The latest macOS full-suite rerun passed 59 tests. These checks
do not authorize changing the user's tmux config; the existing bad
`default-terminal` was left intact.

| Check | Result | Evidence |
| --- | --- | --- |
| Linux x86_64 CI / PTY | Pending | No run recorded |
| macOS Intel CI / PTY | Pending | No run recorded |
| macOS ARM64 CI / PTY | Pending | No run recorded |
| Latest macOS autostart regression | Passed locally | `tests/autostart_smoke.exp`, actual installed binary, fake Powerlevel10k cleanup; command/shell-snapshot checks 31-44 ms, zero autostarts |
| Latest core PTY worker / inspector / installer checks | Passed locally | Focused regression coverage, not updated full-suite totals |
| Latest Linux Rust full suite | Passed locally | 57 tests: 50 library + 2 CLI + 1 installer + 4 tmux-client |
| Latest macOS tmux-client integration | Passed locally | Four tests, included in the complete 59-test pass |
| Latest theme smoke | Passed locally | Linux 3.3a, macOS 3.5a/3.7c; actual SGR controls, style preservation/reset, explicit RGB, linked-window refusal |
| Latest macOS full-suite / release smoke | Passed locally | 59 Rust tests and all four release-binary smoke entry points; latest archive acceptance pending |
| Current live Claude/OpenCode with helpers | Doctor recognition and pre-optimization live F7 confirmed | Optimized revision installed on user's tmux 3.5a server; live visual/latency acceptance pending; no model-test claim |
| F7 latency | Isolated macOS benchmark recorded | Five cold / twenty-five warm per version; not the user's roughly one-second full-profile delay; no Linux percentiles or 100 ms guarantee |
| Reported 2-5 second TUI startup latency | Unmeasured | Demonstrated shell-snapshot attach-blocking mechanism fixed, not all startup delays verified resolved |
| macOS ARM64 final release and packaged engines | Passed locally | Both core PTY and startup; tmux 3.7c; native packaging succeeded |
| Debian 12 aarch64 final Rust / core PTY / startup | Passed locally | Rust 1.84.1, UID 0; private tmux 3.3a and 3.7c; pinned client and `/tmp` fallback |
| Claude Code 2.1.263 no-model smoke | Passed locally | macOS ARM64 / tmux 3.7c; official paths, zero mappings, unchanged PIDs |
| Codex 0.153.4 native and npm-wrapper no-model smoke | Passed locally | Temporary integrity-verified official package, zero mappings, unchanged PIDs; removed afterward |
| OpenCode 1.18.29 no-model smoke | Passed locally | macOS ARM64 / tmux 3.7c; official paths, zero mappings, unchanged PIDs |
| Authenticated/generating sessions | Pending | Not covered by no-model onboarding tests |
| WSL2 manual | Pending | Not verified |
| Existing local installer self-management tests | Passed | Rust fixtures and core PTY actual install/uninstall |
| Actual-CLI installer integration test | Passed locally | macOS ARM64 and unprivileged Linux ARM64; included in 48/46 totals |
| Published release install/update/rollback | Pending | No release published; local archive validation is separate |
