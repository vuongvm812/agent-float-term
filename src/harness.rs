//! Deliberately small allowlists, not executable-name or prompt heuristics.
//! Installation paths identify supported layouts; they do not authenticate code.
//! Overwritten process titles/argv are not reconstructed from names or environment.
//! If the remaining argv is unrecognizable, even a known installation is rejected.

use std::ffi::OsString;
use std::path::Path;

use crate::config::HarnessMapping;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Harness {
    Claude,
    Codex,
    OpenCode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Recognized {
    pub harness: Harness,
    pub node_wrapper: bool,
    pub args_offset: usize,
}

fn named(name: &str) -> Option<Harness> {
    match name {
        "claude" => Some(Harness::Claude),
        "codex" => Some(Harness::Codex),
        "opencode" => Some(Harness::OpenCode),
        _ => None,
    }
}

fn mapped(path: &Path, mappings: &[HarnessMapping]) -> Option<Harness> {
    let mut matches = mappings.iter().filter(|m| m.path == path);
    let harness = named(&matches.next()?.harness)?;
    // Ambiguous mappings must not silently select a parser.
    if matches.any(|m| named(&m.harness) != Some(harness)) {
        return None;
    }
    Some(harness)
}

fn home_relative(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/root/") {
        return Some(rest);
    }
    let rest = path
        .strip_prefix("/Users/")
        .or_else(|| path.strip_prefix("/home/"))?;
    let (user, rest) = rest.split_once('/')?;
    (!user.is_empty()).then_some(rest)
}

fn version(value: &str) -> bool {
    value.starts_with(|c: char| c.is_ascii_digit())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_+".contains(&c))
}

fn codex_target(value: &str) -> bool {
    matches!(
        value,
        "aarch64-apple-darwin"
            | "x86_64-apple-darwin"
            | "aarch64-unknown-linux-musl"
            | "x86_64-unknown-linux-musl"
            | "aarch64-unknown-linux-gnu"
            | "x86_64-unknown-linux-gnu"
    )
}

fn platform_package(value: &str, prefix: &str) -> bool {
    let Some(value) = value.strip_prefix(prefix) else {
        return false;
    };
    matches!(
        value,
        "darwin-arm64"
            | "darwin-x64"
            | "linux-arm64"
            | "linux-x64"
            | "linux-arm64-musl"
            | "linux-x64-musl"
            | "linux-x64-baseline"
            | "linux-x64-baseline-musl"
            | "darwin-x64-baseline"
    )
}

pub(crate) fn native_layout(path: &str) -> Option<Harness> {
    if let Some(rest) = home_relative(path) {
        if rest
            .strip_prefix(".local/share/claude/versions/")
            .is_some_and(version)
        {
            return Some(Harness::Claude);
        }
        if rest == ".opencode/bin/opencode" {
            return Some(Harness::OpenCode);
        }
    }
    if let Some(rest) = path
        .strip_prefix("/opt/homebrew/Cellar/")
        .or_else(|| path.strip_prefix("/usr/local/Cellar/"))
    {
        let parts: Vec<_> = rest.split('/').collect();
        if let [package, release, "bin", binary] = parts.as_slice() {
            if package == binary && version(release) {
                return match *package {
                    "codex" => Some(Harness::Codex),
                    "opencode" => Some(Harness::OpenCode),
                    _ => None,
                };
            }
        }
    }
    if let Some(rest) = path
        .strip_prefix("/opt/homebrew/Caskroom/")
        .or_else(|| path.strip_prefix("/usr/local/Caskroom/"))
    {
        let parts: Vec<_> = rest.split('/').collect();
        if let ["codex", release, "bin", "codex"] = parts.as_slice() {
            if version(release) {
                return Some(Harness::Codex);
            }
        }
        if let [package, release, binary] = parts.as_slice() {
            if version(release) {
                if *package == "claude-code" && *binary == "claude" {
                    return Some(Harness::Claude);
                }
                if *package == "codex" && binary.strip_prefix("codex-").is_some_and(codex_target) {
                    return Some(Harness::Codex);
                }
            }
        }
    }
    // Match the complete package-relative executable, never a package-name substring.
    let (_, package) = path.rsplit_once("/node_modules/")?;
    let parts: Vec<_> = package.split('/').collect();
    match parts.as_slice() {
        ["@openai", pkg, "vendor", target, "codex" | "bin", "codex"]
            if (*pkg == "codex" || platform_package(pkg, "codex-")) && codex_target(target) =>
        {
            Some(Harness::Codex)
        }
        [pkg, "bin", "opencode"] if platform_package(pkg, "opencode-") => Some(Harness::OpenCode),
        ["opencode-ai", "bin", ".opencode"] => Some(Harness::OpenCode),
        ["@anthropic-ai", pkg, "claude"] if platform_package(pkg, "claude-code-") => {
            Some(Harness::Claude)
        }
        _ => None,
    }
}

fn node_entrypoint(path: &Path) -> Option<Harness> {
    let path = path.to_str()?;
    let (_, package) = path.rsplit_once("/node_modules/")?;
    match package {
        "@anthropic-ai/claude-code/cli.js" => Some(Harness::Claude),
        "@openai/codex/bin/codex.js" => Some(Harness::Codex),
        "opencode-ai/bin/opencode" => Some(Harness::OpenCode),
        _ => None,
    }
}

/// Inputs are canonical executable/entrypoint/mapping paths supplied by the inspector.
/// No process environments are read, and argument contents never enter diagnostics.
pub(crate) fn classify(
    executable: &Path,
    argv: &[OsString],
    mappings: &[HarnessMapping],
) -> Option<Recognized> {
    if !executable.is_absolute() || argv.is_empty() {
        return None;
    }
    let node = matches!(executable.file_name()?.to_str()?, "node" | "nodejs");
    let (harness, offset) = if node {
        // Node flags, eval, loaders, and relative entrypoints are deliberately unsupported.
        let entry = Path::new(argv.get(1)?);
        if !entry.is_absolute() {
            return None;
        }
        let harness = if mappings.iter().any(|m| m.path == entry) {
            mapped(entry, mappings)?
        } else {
            node_entrypoint(entry)?
        };
        (harness, 2)
    } else {
        let harness = if mappings.iter().any(|m| m.path == executable) {
            mapped(executable, mappings)?
        } else {
            native_layout(executable.to_str()?)?
        };
        (harness, 1)
    };
    let argv0 = Path::new(&argv[0]).file_name()?;
    let expected_name = if node {
        "node"
    } else {
        match harness {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::OpenCode => "opencode",
        }
    };
    // Besides rejecting rewritten argv, this prevents a malformed macOS argv[0]
    // from shifting an option into the otherwise ignored program-name slot.
    if argv0 != expected_name && Some(argv0) != executable.file_name() {
        return None;
    }
    interactive(harness, &argv[offset..]).then_some(Recognized {
        harness,
        node_wrapper: node,
        args_offset: offset,
    })
}

fn interactive(harness: Harness, args: &[OsString]) -> bool {
    let mut index = 0;
    let mut positional = false;
    let mut command = false;
    while index < args.len() {
        let Some(arg) = args[index].to_str() else {
            return false;
        };
        index += 1;
        if arg == "--" {
            // Supporting arbitrary arguments after -- would obscure subcommand selection.
            return false;
        }
        if arg.starts_with('-') {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg, None), |(k, v)| (k, Some(v)));
            if harness == Harness::Claude && matches!(flag, "--resume" | "-r") {
                // Commander-style optional selector: consume one non-option value,
                // not the prompt slot. Later options/subcommands still need checking.
                if let Some(value) = inline {
                    if flag != "--resume" || value.is_empty() {
                        return false;
                    }
                } else if let Some(value) = args.get(index) {
                    let Some(value) = value.to_str() else {
                        return false;
                    };
                    if value.is_empty() {
                        return false;
                    }
                    if !value.starts_with('-') {
                        index += 1;
                    }
                }
                continue;
            }
            let (switches, values): (&[&str], &[&str]) = match harness {
                Harness::Claude => (
                    &[
                        "--continue",
                        "-c",
                        "--verbose",
                        "--dangerously-skip-permissions",
                        "--allow-dangerously-skip-permissions",
                        "--fork-session",
                        "--ide",
                        "--strict-mcp-config",
                        "--disable-slash-commands",
                    ],
                    &[
                        "--model",
                        "--permission-mode",
                        "--system-prompt",
                        "--append-system-prompt",
                        "--settings",
                        "--setting-sources",
                        "--session-id",
                        "--agent",
                        "--agents",
                        "--mcp-config",
                    ],
                ),
                Harness::Codex => (
                    &[
                        "--full-auto",
                        "--dangerously-bypass-approvals-and-sandbox",
                        "--oss",
                        "--search",
                        "--no-alt-screen",
                    ],
                    &[
                        "--profile",
                        "-p",
                        "--model",
                        "-m",
                        "--config",
                        "-c",
                        "--sandbox",
                        "-s",
                        "--ask-for-approval",
                        "-a",
                        "--cd",
                        "-C",
                        "--image",
                        "-i",
                        "--add-dir",
                        "--enable",
                        "--disable",
                        "--local-provider",
                    ],
                ),
                Harness::OpenCode => (
                    &["--print-logs", "--continue", "-c", "--fork"],
                    &[
                        "--model",
                        "-m",
                        "--agent",
                        "--session",
                        "-s",
                        "--prompt",
                        "--port",
                        "--hostname",
                        "--log-level",
                        "--password",
                        "--dir",
                    ],
                ),
            };
            if harness == Harness::Codex
                && command
                && matches!(flag, "--last" | "--all")
                && inline.is_none()
            {
                continue;
            }
            if switches.contains(&flag) && inline.is_none() {
                continue;
            }
            if !values.contains(&flag) {
                // Includes Claude -p/--print, help/version, and every unknown flag.
                return false;
            }
            if let Some(value) = inline {
                if !flag.starts_with("--") || value.is_empty() {
                    return false;
                }
            } else {
                let Some(value) = args.get(index).and_then(|v| v.to_str()) else {
                    return false;
                };
                if value.is_empty() || value.starts_with('-') {
                    return false;
                }
                index += 1;
            }
        } else {
            if !positional && !command {
                let allowed_command = match harness {
                    Harness::Claude => false,
                    Harness::Codex => matches!(arg, "resume" | "fork"),
                    Harness::OpenCode => arg == "attach",
                };
                if allowed_command {
                    command = true;
                    continue;
                }
                let forbidden_command = match harness {
                    Harness::Claude => matches!(
                        arg,
                        "auth"
                            | "help"
                            | "agents"
                            | "remote-control"
                            | "rc"
                            | "bridge"
                            | "config"
                            | "doctor"
                            | "install"
                            | "mcp"
                            | "plugin"
                            | "setup-token"
                            | "update"
                            | "upgrade"
                    ),
                    Harness::Codex => matches!(
                        arg,
                        "exec"
                            | "e"
                            | "review"
                            | "login"
                            | "logout"
                            | "mcp"
                            | "mcp-server"
                            | "app"
                            | "app-server"
                            | "completion"
                            | "sandbox"
                            | "debug"
                            | "apply"
                            | "a"
                            | "cloud"
                            | "features"
                            | "help"
                    ),
                    Harness::OpenCode => matches!(
                        arg,
                        "run"
                            | "serve"
                            | "web"
                            | "auth"
                            | "mcp"
                            | "models"
                            | "upgrade"
                            | "uninstall"
                            | "stats"
                            | "export"
                            | "import"
                            | "github"
                            | "pr"
                            | "session"
                            | "agent"
                            | "debug"
                            | "completion"
                            | "help"
                            | "acp"
                    ),
                };
                if forbidden_command {
                    return false;
                }
            }
            // One initial prompt/project/selector is supported; no guesses about extra operands.
            if positional || arg.is_empty() {
                return false;
            }
            positional = true;
        }
    }
    true
}

pub(crate) fn is_shell(executable: &Path) -> bool {
    let Some(path) = executable.to_str() else {
        return false;
    };
    let Some(name) = executable.file_name().and_then(|v| v.to_str()) else {
        return false;
    };
    if !matches!(name, "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh") {
        return false;
    }
    if matches!(
        executable.parent().and_then(|p| p.to_str()),
        Some("/bin" | "/usr/bin" | "/usr/local/bin")
    ) {
        return true;
    }
    let Some(rest) = path
        .strip_prefix("/opt/homebrew/Cellar/")
        .or_else(|| path.strip_prefix("/usr/local/Cellar/"))
    else {
        return false;
    };
    let parts: Vec<_> = rest.split('/').collect();
    matches!(parts.as_slice(), [package, release, "bin", binary] if *package == name && *binary == name && version(release))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn mode_parsing_is_harness_specific() {
        for (harness, argv, expected) in [
            (Harness::Claude, vec![], true),
            (Harness::Claude, vec!["--print", "hello"], false),
            (Harness::Claude, vec!["-p", "hello"], false),
            (Harness::Claude, vec!["-phello"], false),
            (Harness::Claude, vec!["--model=sonnet", "hello"], true),
            (Harness::Claude, vec!["--resume"], true),
            (Harness::Claude, vec!["--resume=session"], true),
            (Harness::Claude, vec!["mcp", "serve"], false),
            (Harness::Claude, vec!["remote-control"], false),
            (Harness::Claude, vec!["help"], false),
            (Harness::Codex, vec!["-p", "work"], true),
            (
                Harness::Codex,
                vec!["--profile=work", "exec", "hello"],
                false,
            ),
            (Harness::Codex, vec!["e", "hello"], false),
            (Harness::Codex, vec!["resume", "abc"], true),
            (Harness::Codex, vec!["resume", "--last"], true),
            (Harness::Codex, vec!["--unknown"], false),
            (Harness::Codex, vec!["--model"], false),
            (Harness::Codex, vec!["--model", "--help"], false),
            (Harness::Codex, vec!["--", "exec"], false),
            (Harness::OpenCode, vec!["run", "hello"], false),
            (
                Harness::OpenCode,
                vec!["attach", "http://localhost:4096"],
                true,
            ),
            (Harness::OpenCode, vec!["--model", "provider/model"], true),
            (Harness::OpenCode, vec!["serve"], false),
        ] {
            assert_eq!(
                interactive(harness, &args(&argv)),
                expected,
                "{harness:?} {argv:?}"
            );
        }
    }

    #[test]
    fn native_layouts_not_basenames() {
        for (path, expected) in [
            ("/tmp/claude", None),
            ("/usr/local/bin/codex", None),
            ("/tmp/.opencode/bin/opencode", None),
            ("/home/alice/.opencode/bin/opencode", Some(Harness::OpenCode)),
            ("/Users/alice/.local/share/claude/versions/2.1.1", Some(Harness::Claude)),
            ("/opt/homebrew/Caskroom/codex/0.98.0/codex-aarch64-apple-darwin", Some(Harness::Codex)),
            ("/opt/homebrew/Caskroom/codex/0.153.4/bin/codex", Some(Harness::Codex)),
            ("/usr/local/lib/node_modules/@openai/codex/vendor/x86_64-unknown-linux-musl/codex/codex", Some(Harness::Codex)),
            ("/usr/local/lib/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/codex/codex", Some(Harness::Codex)),
            ("/usr/local/lib/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex", Some(Harness::Codex)),
            ("/tmp/node_modules/opencode-linux-x64/bin/opencode", Some(Harness::OpenCode)),
            ("/tmp/node_modules/not-codex/bin/codex", None),
        ] {
            assert_eq!(native_layout(path), expected, "{path}");
        }
    }

    #[test]
    fn claude_resume_consumes_only_its_optional_selector() {
        for (argv, expected) in [
            (vec!["-r"], true),
            (vec!["--resume", "session-id"], true),
            (vec!["-r", "session-id", "hello"], true),
            (vec!["--resume=session-id", "hello"], true),
            (vec!["--resume", "--model", "sonnet"], true),
            (vec!["--resume", "mcp"], true), // A selector here, not a subcommand.
            (vec!["--resume", "session-id", "mcp"], false),
            (vec!["-r", "session-id", "auth"], false),
            (vec!["--resume=session-id", "help"], false),
            (vec!["--resume", "session-id", "--unknown"], false),
            (vec!["--resume", "--unknown"], false),
            (vec!["-r", "--print"], false),
            (vec!["-r", "session-id", "-p", "hello"], false),
            (vec!["--resume="], false),
            (vec!["-r", ""], false),
            (vec!["-rsession-id"], false),
            (vec!["-r=session-id"], false),
        ] {
            assert_eq!(
                interactive(Harness::Claude, &args(&argv)),
                expected,
                "{argv:?}"
            );
        }
    }

    #[test]
    fn unrecognizable_overwritten_titles_are_not_inferred_from_installation() {
        assert!(classify(
            Path::new("/home/alice/.local/share/claude/versions/2.1.1"),
            &args(&["claude: working"]),
            &[]
        )
        .is_none());
        assert!(classify(Path::new("/usr/bin/node"), &args(&["opencode"]), &[]).is_none());
    }

    #[test]
    fn node_requires_exact_entrypoint() {
        let node = Path::new("/usr/bin/node");
        assert!(classify(
            node,
            &args(&[
                "node",
                "/usr/lib/node_modules/@openai/codex/bin/codex.js",
                "-p",
                "work"
            ]),
            &[]
        )
        .is_some());
        for argv in [
            vec!["node", "/tmp/codex.js"],
            vec!["node", "--eval", "codex"],
            vec!["node", "node_modules/@openai/codex/bin/codex.js"],
            vec![
                "node",
                "/usr/lib/node_modules/@openai/codex/bin/codex.js",
                "exec",
            ],
        ] {
            assert!(classify(node, &args(&argv), &[]).is_none());
        }
    }

    #[test]
    fn custom_mapping_opts_in_but_does_not_bypass_mode_check() {
        let executable = Path::new("/opt/custom/ai");
        let mappings = vec![HarnessMapping {
            harness: "codex".into(),
            path: executable.into(),
        }];
        assert!(classify(executable, &args(&["ai"]), &[]).is_none());
        assert!(classify(executable, &args(&["ai", "-p", "work"]), &mappings).is_some());
        assert!(classify(executable, &args(&["ai", "exec"]), &mappings).is_none());
        let ambiguous = vec![
            HarnessMapping {
                harness: "codex".into(),
                path: executable.into(),
            },
            HarnessMapping {
                harness: "claude".into(),
                path: executable.into(),
            },
        ];
        assert!(classify(executable, &args(&["ai"]), &ambiguous).is_none());
    }
}
