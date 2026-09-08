# Harness Floating Terminal

An F7 floating shell beside Claude Code, Codex, or OpenCode, without wrapping
your AI CLI. Run your AI command normally in a tmux pane.

## Installation

Requires **tmux 3.4+** and macOS or Linux. Install your AI CLI normally.
Homebrew supplies tmux; other methods require you to install it separately.

**Publication pending:** Homebrew, crates.io, and release archives are prepared
but not published yet. Use a Git source build until the respective channel is
available. Choose one method; switching an existing installation requires the
[migration steps](docs/operations.md#switching-installation-methods).

### Homebrew

After the stable release and tap formula are published:

```sh
brew tap vuongvm812/tap
brew install vuongvm812/tap/agent-float-term
aft="$(brew --prefix agent-float-term)/bin/agent-float-term"
"$aft" install --external-binary "$aft"
# Review the preview before applying:
"$aft" install --external-binary "$aft" --yes
```

Homebrew installs a release binary. Integration setup does not copy it elsewhere;
`brew upgrade` owns binary updates. See [platform limits](docs/installation.md#homebrew).

### Cargo

After the crate is published, using the default Cargo installation root:

```sh
cargo install agent-float-term --locked
aft="${CARGO_HOME:-$HOME/.cargo}/bin/agent-float-term"
"$aft" install --external-binary "$aft"
# Review the preview before applying:
"$aft" install --external-binary "$aft" --yes
```

**Cargo compiles locally:** Rust/Cargo and a native build toolchain are required.
Rust 1.84.1 is the release toolchain. Cargo owns the executable and updates;
see [custom installation roots](docs/installation.md#cargo) if your Cargo settings
override the default.

### Release Archives

After publication, download the archive for your OS/architecture from
[GitHub Releases](https://github.com/vuongvm812/agent-float-term/releases), verify
its entry in `SHA256SUMS`, then run the extracted binary's `install` preview and
repeat with `--yes`. No Rust compiler is needed.

Follow the [download and verification commands](docs/installation.md#release-archives).
The installer owns a local copy at `~/.local/bin/agent-float-term`; do not copy a
regular executable to that destination before running the installer.

### Build From Source

```sh
git clone https://github.com/vuongvm812/agent-float-term.git
cd agent-float-term
cargo +1.84.1 build --locked --release
./target/release/agent-float-term install
# Review the preview before applying:
./target/release/agent-float-term install --yes
export PATH="$HOME/.local/bin:$PATH"
```

### Activate

Ensure the selected binary directory is on `PATH`: Homebrew's `bin`, Cargo's
installation-root `bin`, or `~/.local/bin` for managed archive/source installs.
Inside your existing tmux server:

```sh
agent-float-term bind
agent-float-term doctor
claude
# Or: codex / opencode
```

Press **F7** to open or hide the float. Shell state survives hiding, but **exiting
the AI terminates its float and terminal jobs**. The next AI run starts fresh.

Tmux shortcuts operate on the main terminal. Switching away leaves the float in
its original AI pane; returning restores it unless you explicitly hid it with F7.
Prompts and choosers temporarily take keyboard focus from the float.

Fresh installation edits no startup files unless explicitly selected. Review key
conflicts before using `bind --replace-key`. Outside tmux, `agent-float-term start`
opens a dedicated tmux shell. See [installation](docs/installation.md) for automatic
startup and [operations](docs/operations.md) for update, rollback, and uninstall.

Generated integration scripts live in `~/.local/share/agent-float-term/`, not the
config directory. Reinstalling migrates intact legacy scripts and managed references.

## Configuration

Create `${XDG_CONFIG_HOME:-$HOME/.config}/agent-float-term/config.json` if needed:

```json
{
  "shortcut": "F7",
  "width": 80,
  "height": 80
}
```

These are the defaults. Dimensions are integer percentages from **10 to 100**.
Omitted fields use defaults; unknown fields, comments, and trailing commas are
rejected. JSON only: an old `config.toml` is preserved but never loaded. If it
exists without JSON, an error explains migration; `{}` explicitly uses defaults.
Use `shortcut`, not `key`, and rebind the intended server after changing it.
See [configuration details](docs/configuration.md) for safe shortcuts, advanced
`shell`/`harness_paths`, file permissions, and XDG rules.

## Compatible Versions

| Dependency | Compatible / Tested Version |
| --- | --- |
| tmux | **3.4+**; current PTY tests pass on 3.4, 3.5a, and 3.7c. Client and running server versions must match. |
| Rust / Cargo | **1.84.1** for source builds; not needed to run an installed binary. |
| Claude Code | 2.1.263, previously smoke-tested without model requests. |
| Codex | 0.153.4, previously smoke-tested without model requests. |
| OpenCode | 1.18.29, previously smoke-tested without model requests. |
| OS | macOS and Linux. Native Windows is unsupported. |

See the [validation matrix](docs/compatibility.md) for platform coverage and test
limits. This tool adds no telemetry or prompt logging; tmux is not a sandbox.

[Contributing](CONTRIBUTING.md) | [Changelog](CHANGELOG.md) |
[Publishing](docs/publishing.md) | [Release checklist](docs/release-checklist.md) |
[MIT license](LICENSE)
