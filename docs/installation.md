# Installation Details

See the [README](../README.md#installation) for the four installation methods.
The runtime minimum is tmux **3.4**, tested with isolated lifecycle/navigation
scenarios on macOS alongside 3.5a and 3.7c. Install tmux
separately using a trusted system package or official source release, then check
`tmux -V`. Ubuntu 22.04's default 3.2a and Debian 12's 3.3a are below that
minimum. The CI dependency installer is for disposable runners, not host setup.

Your terminal must deliver the configured shortcut to tmux; some keyboards need
Fn+F7. Install your AI CLI normally; this project does not install AI tools.

Cargo v0.2.2 is published. The first-use Homebrew flow below requires a newer
release containing that change; it does not alter existing v0.2.2 binaries.
Homebrew and archive downloads require the matching public GitHub assets.
A Git source build is available now. Do not mix methods without first reviewing
[switching installation methods](operations.md#switching-installation-methods).

## Homebrew

The custom tap is [vuongvm812/tap](https://github.com/vuongvm812/homebrew-tap).
It distributes the project's release archives, not a Homebrew source build or
an official `homebrew/core` formula. Supported formula baselines are Apple Silicon
macOS 14+, Intel macOS 15+, and Linux x86_64 with glibc 2.35+. These conservative
limits follow the build hosts; older OS compatibility has not been established.
Linux ARM64 and Alpine/musl have no formula binary. The formula depends on tmux.

After the release containing first-use registration and its formula are public:

```sh
brew tap vuongvm812/tap
brew install vuongvm812/tap/agent-float-term
```

Proceed to [activation](#activate): the first `bind` or interactive `start`
validates the Homebrew layout and registers the stable `opt` path automatically.
It creates only user-local integration templates and a manifest, not a binary
copy or startup configuration edits. Root bindings and float helpers follow that
path after `brew upgrade` and old-keg cleanup. Existing registrations are retained;
conflicting installation modes/paths require explicit migration. `doctor`, help,
and version checks remain read-only.

This happens on first application use, **not in a Homebrew post-install hook**.
The package install environment may not represent the eventual user's home;
registration instead belongs to the user who actually runs the application.

Older v0.2.2 binaries still need the explicit integration-only setup:

```sh
aft="$(brew --prefix agent-float-term)/bin/agent-float-term"
"$aft" install --external-binary "$aft"
# Review, then apply the same options:
"$aft" install --external-binary "$aft" --yes
```

Use the stable `opt` prefix, not a versioned `Cellar` path. Do not substitute a new
formula's instructions for capabilities absent from its actual release binary.

## Cargo

Install from [crates.io](https://crates.io/crates/agent-float-term):

```sh
cargo install agent-float-term --locked
# Default root only; use the actual installation root if configured differently.
aft="${CARGO_HOME:-$HOME/.cargo}/bin/agent-float-term"
"$aft" install --external-binary "$aft"
# Review, then apply the same options:
"$aft" install --external-binary "$aft" --yes
```

This downloads source from crates.io and **compiles locally**, unlike Homebrew
or a release archive. Install Rust/Cargo and a native C compiler/linker (Xcode
Command Line Tools on macOS; a system build toolchain on Linux). Rust 1.84.1 is
used for release verification; `--locked` uses the crate's shipped lockfile.
Cargo does not install the runtime dependency tmux.

Cargo chooses its installation root from `--root`, `CARGO_INSTALL_ROOT`, the
`install.root` Cargo setting, `CARGO_HOME`, then `~/.cargo`, in that order. Register
the actual absolute binary path, rather than resolving an older copy from PATH.
For a custom root, make the choice explicit for both install and future updates:

```sh
cargo_root="$HOME/tools/cargo"
cargo install agent-float-term --locked --root "$cargo_root"
aft="$cargo_root/bin/agent-float-term"
"$aft" install --external-binary "$aft"
# Repeat with --yes after review.
```

Add that root's `bin` directory to your own PATH configuration. Do not use the
plain managed installer on the Cargo executable for a fresh registration: use
`--external-binary` so Cargo retains ownership, including for `--root ~/.local`.

## Release Archives

[Repository](https://github.com/vuongvm812/agent-float-term) |
[All releases](https://github.com/vuongvm812/agent-float-term/releases) |
[Latest stable](https://github.com/vuongvm812/agent-float-term/releases/latest) |
[Version 0.1.0](https://github.com/vuongvm812/agent-float-term/releases/tag/v0.1.0)

Select an actually published release. Drafts are not public downloads and
prereleases do not populate the latest stable link. The filenames below illustrate
v0.1.0, not a guarantee that version is available. Native archives plus `SHA256SUMS` are:

| Platform | Archive for v0.1.0 |
| --- | --- |
| macOS Apple Silicon | `agent-float-term-v0.1.0-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `agent-float-term-v0.1.0-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `agent-float-term-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` |

Linux ARM64 is source-build-only despite earlier Debian validation. Linux GNU
archives target Ubuntu 22.04 / glibc 2.35; older glibc and Alpine/musl are not
supported by those archives. macOS binaries are not notarized. Other platforms,
including WSL2, remain unverified; native Windows is unsupported.

Use a fresh download directory. This example selects macOS Apple Silicon;
set `version` to the published tag and `target` to the appropriate triple above:

```sh
version=v0.1.0
target=aarch64-apple-darwin
archive="agent-float-term-${version}-${target}.tar.gz"
base="https://github.com/vuongvm812/agent-float-term/releases/download/${version}"
curl --fail --location --remote-name "${base}/${archive}"
curl --fail --location --remote-name "${base}/SHA256SUMS"
shasum -a 256 "$archive"
# On Linux, use: sha256sum "$archive"
```

Compare the hash with the exact archive filename in `SHA256SUMS` before
extracting. Stop on a mismatch. Checksums detect corruption, not compromise of
the release source: they are not an independent signature.

```sh
tar -xzf "$archive"
"./agent-float-term-${version}-${target}/agent-float-term" install
# Only after reviewing the preview:
"./agent-float-term-${version}-${target}/agent-float-term" install --yes
export PATH="$HOME/.local/bin:$PATH"
agent-float-term --help
```

Run the installer directly from the extracted directory. It creates a
digest-addressed payload and an owned symlink at
`$HOME/.local/bin/agent-float-term`. Do not first copy a regular binary there:
unowned destinations are refused. The archive contains the binary, LICENSE,
and README; integration templates are generated by the binary.

The PATH export affects only this shell. Add it to your own startup config if
needed; the installer does not do that for you or fetch and pipe code to a shell.

## Build From Source

Install Rust 1.84.1 and a native C compiler/linker, then build the checkout:

```sh
git clone https://github.com/vuongvm812/agent-float-term.git
cd agent-float-term
cargo +1.84.1 build --locked --release
./target/release/agent-float-term install
# Review, then apply:
./target/release/agent-float-term install --yes
export PATH="$HOME/.local/bin:$PATH"
```

The same managed installer is used for source builds and extracted archives.
The PATH change above lasts only for this shell. Add it to your own startup
configuration if needed. The release matrix does not contain a Linux ARM64
binary; that platform requires local compilation.

## Opt-In Startup

`install` previews changes by default; `--yes` applies them. Each config flag
independently authorizes integration with the selected file. With neither flag,
a fresh installation modifies no shell or tmux startup file.

```sh
agent-float-term install --tmux-config "$HOME/.tmux.conf"
# Only after reviewing the preview:
agent-float-term install --tmux-config "$HOME/.tmux.conf" --yes
```

Select the config your server actually loads, which may instead be
`$HOME/.config/tmux/tmux.conf`. Do not create a competing config for this tool or
replace your tmux configuration wholesale. For a running server, use `bind` and
the intended socket; review conflicts before using `--replace-key`.

Shell integration can be selected independently for automatic startup:

```sh
agent-float-term install --shell-config "$HOME/.zshrc" --shell-kind zsh
# Repeat with --yes only after reviewing the proposed edits.
```

Use `--shell-kind bash` with a bash startup file. Both config flags may be used
together. Existing tmux is initialized without a nested outer session or a
foreign-key overwrite; outside tmux, dedicated startup does not load personal
tmux config. Interactive/TTY, command-string, SSH, and recursion guards apply;
see [startup details](operations.md#existing-or-dedicated-tmux).

Reinstalling retains existing integrations; omitting flags does not remove them.
`install --yes` refreshes owned generated templates using the recorded shell
kind without editing unselected user configs. Edited/unowned templates are
refused. `update` replaces only the binary, not the templates; see
[updates and rollback](operations.md#local-update-and-rollback).

For a registered Homebrew/Cargo installation, plain `install` retains its external
mode/path and refreshes only owned templates when applied. If selecting startup
files during the **first** registration, include `--external-binary "$aft"`
alongside those flags. Registration checks that the path resolves to the running
executable; run it from the selected package, not a different copy on PATH.

The earlier config-directory layout is an exception to unselected-block retention:
reinstall also migrates exact recorded references to the data directory. The
preview lists every migration target; user settings and surrounding bytes remain
untouched.

## Activate

After completing one installation method, confirm that PATH selects its executable
with `command -v agent-float-term`. In the tmux server you intend to use:

```sh
agent-float-term bind
agent-float-term doctor
claude
# Or: codex / opencode
```

Outside tmux, use `agent-float-term start` for a dedicated normal shell. Existing
servers do not need to restart. A tmux package upgrade can leave a running server
on an older version: client/server compatibility is separate from application
updates; see [tmux client selection](operations.md#tmux-client-selection).
