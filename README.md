# Harness Floating Terminal

An F7 floating shell beside Claude Code, Codex, or OpenCode, without wrapping
your AI CLI. Run your AI command normally in a tmux pane.

## Installation

Install tmux separately, then build from source with Rust 1.84.1:

```sh
git clone https://github.com/vuongvm812/agent-float-term.git
cd agent-float-term
cargo +1.84.1 build --locked --release
./target/release/agent-float-term install
# Review the preview before applying:
./target/release/agent-float-term install --yes
export PATH="$HOME/.local/bin:$PATH"
```

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
[Release checklist](docs/release-checklist.md) | [MIT license](LICENSE)
