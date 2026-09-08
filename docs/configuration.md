# JSON Configuration

The only application config is
`${XDG_CONFIG_HOME:-$HOME/.config}/agent-float-term/config.json`.
Generated `integration.sh` and `integration.tmux` are stored under
`${XDG_DATA_HOME:-$HOME/.local/share}/agent-float-term/`, alongside release payloads.
The config directory is reserved for user settings, not generated scripts.
No file is required for defaults, unless a legacy `config.toml` is present.
Use a JSON object, without comments or trailing commas:

```json
{
  "shortcut": "F7",
  "width": 80,
  "height": 80
}
```

| Field | Default | Validation |
| --- | --- | --- |
| `shortcut` | `"F7"` | Safe tmux key name; see below. |
| `width` | `80` | Integer percentage, 10 through 100 inclusive. |
| `height` | `80` | Integer percentage, 10 through 100 inclusive. |
| `shell` | `null` | Optional absolute path to an accessible executable regular file. |
| `harness_paths` | `[]` | Optional array of explicit executable mappings. |

Omitted fields use their individual defaults. Unknown fields, including `key`,
are errors. An empty file is not JSON; use `{}` to select all defaults.

## Shortcut

Function keys are **F1-F12**, not F13-F24. Single ASCII letters, digits, and `@`
are accepted. Named keys include `Space`, `Enter`, `Escape`, `BSpace`, `Tab`,
`BTab`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PPage`, `NPage`, `DC`, and
`IC`. Prefixes `C-`, `M-`, and `S-` may be combined without repetition, for
example `C-a`, `M-Space`, or `C-M-Left`. Commands and tmux format strings are not
valid shortcuts.

After changing the shortcut, reapply `bind` to the intended server and review
conflicts. Do not edit/rebind the same key concurrently with `bind` or
`uninstall`; tmux has no atomic key-table compare-and-swap. See
[key ownership](operations.md#existing-or-dedicated-tmux).

## Advanced Paths

This example requires replacing the harness path with a real executable:

```json
{
  "shell": "/bin/zsh",
  "harness_paths": [
    { "harness": "claude", "path": "/absolute/custom/claude" }
  ]
}
```

Supported harness names are `claude`, `codex`, and `opencode`. Paths must be
absolute UTF-8 paths without control characters or parent traversal, pointing
to executable regular files accessible to the current user. Duplicate or
conflicting mappings to the same canonical executable are rejected. Unknown
mapping fields are rejected too. JSON escapes quotes as `\"` and backslashes
as `\\`; it does not expand `~`, `$HOME`, or shell substitutions.

An explicit path identifies a custom installation; it neither wraps the CLI nor
bypasses foreground/argument/ancestry validation. See
[recognition limits](operations.md#recognition-limits). With `shell` omitted or
`null`, normal default shell selection applies. The float is a new shell, not
an exact clone of the AI process's environment, functions, or virtual environment.

## Migration And Safety

There is **no TOML fallback or automatic conversion**. If JSON is missing and
`config.toml` exists, loading fails with instructions to create `config.json`
and rename the old `key` field to `shortcut`. Convert dimensions and any advanced
paths to the JSON structure above; merely renaming a TOML file is not enough.
Alternatively, explicitly create `{}` to use defaults. Keep your old file as a
backup if desired. Once JSON exists, it alone is read; malformed JSON never
falls back to TOML. The loader does not write, delete, or rename either file.

The config must be a user-owned regular file, not a symlink, and must not be
group- or other-writable. Its immediate directory must likewise be user-owned,
not a symlink, and not writable by others. Prefer directory mode `0700` and file
mode `0600`. Files over 1 MiB and nonregular files are refused. Executable-path
checks and private application-directory security rules remain unchanged.

`HOME` must be set to a valid absolute path. Unset or empty XDG roots use these
fallbacks; relative or otherwise unsafe roots are errors, not silently ignored:

| Root | Fallback | Product directory |
| --- | --- | --- |
| `XDG_CONFIG_HOME` | `$HOME/.config` | `agent-float-term` |
| `XDG_DATA_HOME` | `$HOME/.local/share` | `agent-float-term` |
| `XDG_STATE_HOME` | `$HOME/.local/state` | `agent-float-term` |

The installed command remains under `$HOME/.local/bin`. Path discovery creates
nothing; private directory creation never chmods HOME or an existing XDG parent.
