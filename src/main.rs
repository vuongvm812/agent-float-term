use agent_float_term::{install, tmux};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "A persistent F7 shell beside your unwrapped AI CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Preview a user-local install; apply only with --yes
    Install {
        /// Integration-only install; the package manager owns this stable executable
        #[arg(long, value_name = "ABSOLUTE_STABLE_PATH")]
        external_binary: Option<PathBuf>,
        /// Explicitly opt into editing this tmux configuration
        #[arg(long, value_name = "PATH")]
        tmux_config: Option<PathBuf>,
        /// Explicitly opt into editing this shell configuration
        #[arg(long, value_name = "PATH")]
        shell_config: Option<PathBuf>,
        /// Shell syntax for integration (otherwise inferred from SHELL)
        #[arg(long, value_parser = ["bash", "zsh"])]
        shell_kind: Option<String>,
        /// Apply the plan; does not select user configuration files
        #[arg(long)]
        yes: bool,
    },
    /// Bind the configured key in an existing tmux server
    Bind {
        /// Existing tmux server socket
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
        /// Explicitly replace a conflicting key binding
        #[arg(long)]
        replace_key: bool,
    },
    /// Open a normal shell in a dedicated tmux environment, not an AI CLI
    Start,
    /// Diagnose dependencies, configuration, and a tmux server
    Doctor {
        #[arg(long, value_name = "PATH", help = "Existing tmux server socket")]
        socket: Option<PathBuf>,
    },
    /// List owned floating sessions, including orphans
    Sessions {
        #[arg(long, value_name = "PATH", help = "Existing tmux server socket")]
        socket: Option<PathBuf>,
    },
    /// Preview cleanup of owned orphan sessions; apply only with --yes
    Cleanup {
        #[arg(long, value_name = "PATH", help = "Existing tmux server socket")]
        socket: Option<PathBuf>,
        /// Limit cleanup to this owned orphan session
        #[arg(long, value_name = "NAME")]
        session: Option<String>,
        /// Terminate the selected orphan sessions and their jobs
        #[arg(long)]
        yes: bool,
    },
    /// Activate a local native binary after verifying its SHA-256
    Update {
        /// Local binary to install; nothing is downloaded
        #[arg(long, value_name = "PATH")]
        from: PathBuf,
        /// Expected SHA-256 of the local binary
        #[arg(long, value_name = "HASH")]
        sha256: String,
    },
    /// Restore the retained previous binary release
    Rollback,
    /// Preview removal of owned installation and bindings; apply with --yes
    Uninstall {
        /// Remove verified owned content; preserve sessions and user edits
        #[arg(long)]
        yes: bool,
    },
    #[command(hide = true)]
    Dispatch {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        pane: String,
        #[arg(long)]
        client_pid: u32,
        #[arg(long)]
        key: String,
    },
    #[command(hide = true)]
    Watch {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        session: String,
        #[arg(long)]
        instance: String,
    },
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Install {
            external_binary,
            tmux_config,
            shell_config,
            shell_kind,
            yes,
        } => install::install(install::InstallOptions {
            external_binary,
            tmux_config,
            shell_config,
            shell_kind,
            yes,
        }),
        Command::Bind {
            socket,
            replace_key,
        } => tmux::bind(socket, replace_key),
        Command::Start => tmux::start(),
        Command::Doctor { socket } => tmux::doctor(socket),
        Command::Sessions { socket } => tmux::sessions(socket),
        Command::Cleanup {
            socket,
            session,
            yes,
        } => tmux::cleanup(socket, session, yes),
        Command::Update { from, sha256 } => install::update(&from, &sha256),
        Command::Rollback => install::rollback(),
        Command::Uninstall { yes } => install::uninstall_with_hook(yes, tmux::unbind_all),
        Command::Dispatch {
            socket,
            pane,
            client_pid,
            key,
        } => tmux::dispatch(socket, pane, client_pid, key),
        Command::Watch {
            socket,
            session,
            instance,
        } => tmux::watch(socket, session, instance),
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let failed = error.use_stderr();
            let _ = error.print();
            return if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("agent-float-term: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_contract() {
        Cli::command().debug_assert();
        let help = Cli::command().render_help().to_string();
        assert!(!help.contains("dispatch"));
        assert!(help.contains("Preview a user-local install"));
        assert!(Cli::try_parse_from(["aft", "start"]).is_ok());
        assert!(Cli::try_parse_from(["aft", "start", "claude"]).is_err());
        assert!(Cli::try_parse_from(["aft", "start", "--socket", "/tmp/s"]).is_err());
        assert!(matches!(
            Cli::try_parse_from(["aft", "install"]).unwrap().command,
            Command::Install {
                external_binary: None,
                tmux_config: None,
                shell_config: None,
                shell_kind: None,
                yes: false,
            }
        ));
        assert!(Cli::try_parse_from(["aft", "install", "--external-binary"]).is_err());
        assert!(
            Cli::try_parse_from(["aft", "install", "--external-binary", "/stable/bin/aft"]).is_ok()
        );
        assert!(Cli::try_parse_from([
            "aft",
            "install",
            "--external-binary",
            "/one",
            "--external-binary",
            "/two"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["aft", "install", "--integration-only"]).is_err());
    }

    #[test]
    fn dispatch_requires_explicit_context() {
        let args = [
            "aft",
            "dispatch",
            "--socket",
            "/tmp/s",
            "--pane",
            "%1",
            "--client-pid",
            "42",
            "--key",
            "F7",
        ];
        assert!(matches!(
            Cli::try_parse_from(args).unwrap().command,
            Command::Dispatch { client_pid: 42, .. }
        ));
        assert!(Cli::try_parse_from(&args[..8]).is_err());
        let mut invalid = args;
        invalid[7] = "not-a-pid";
        assert!(Cli::try_parse_from(invalid).is_err());
    }
}
