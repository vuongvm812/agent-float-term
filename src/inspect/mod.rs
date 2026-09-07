//! Best-effort foreground-job inspection, not proof of native TUI keyboard ownership.
//! All failures are conservative; callers should pass the key through on errors.
//!
//! Supported ancestry is a bounded chain of recognized shells on the pane's TTY,
//! not an arbitrary descendant tree. Missing/exited group leaders, intervening
//! editors/SSH, nested PTYs, and stopped ancestors are rejected. Same-group helpers
//! must descend from a verified frontend and have nonterminal stdin and no extra TTY FDs.
//! Frontend launches with unsupported flags/layouts or overwritten titles/argv that
//! no longer preserve a recognizable command are unsupported; original argv cannot
//! be reconstructed. Revalidation is best effort, not an atomic keyboard-ownership check.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};

use crate::config::HarnessMapping;
use crate::harness::{self, Harness};

#[cfg(any(target_os = "linux", test))]
mod linux_stat;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod pty_tests;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;

const MAX_PROCESSES: usize = 16_384;
const MAX_ARG_BYTES: usize = 256 * 1024;
const MAX_ARGS: usize = 4096;
const MAX_SHELLS: usize = 16;
const MAX_MEMBERS: usize = 64;
const MAX_FDS: usize = 4096;
const BUDGET: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Decision {
    pub eligible: bool,
    pub reason: &'static str,
}

impl Decision {
    fn no(reason: &'static str) -> Self {
        Self {
            eligible: false,
            reason,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity {
    pid: u32,
    parent: u32,
    group: u32,
    tty: u64,
    foreground: u32,
    started: (u64, u64),
    runnable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Process {
    identity: Identity,
    executable: PathBuf,
    executable_id: ExecutableId,
    argv: Vec<OsString>,
    descriptors: Option<Descriptors>,
}

// Exclude access times and other activity-dependent metadata from revalidation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ExecutableId {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl ExecutableId {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Input {
    Pipe,
    Socket,
    Null,
    Other,
}

// Only retain evidence relevant to keyboard ownership, not pipe bytes, offsets,
// socket state, or the identities/count of unrelated event-loop descriptors.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Descriptors {
    stdin: Input,
    extra_terminal: bool,
}

impl Descriptors {
    fn helper(&self) -> bool {
        self.stdin != Input::Other && !self.extra_terminal
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn harmless_devices() -> Result<[u64; 3]> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let mut devices = [0; 3];
    for (index, path) in ["/dev/null", "/dev/random", "/dev/urandom"]
        .iter()
        .enumerate()
    {
        let metadata = std::fs::metadata(path).context("cannot identify nonterminal devices")?;
        ensure!(
            metadata.file_type().is_char_device(),
            "invalid nonterminal device"
        );
        devices[index] = platform::device(metadata.rdev());
    }
    Ok(devices)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    pane: Process,
    members: Vec<Process>,
    shells: Vec<Process>,
}

fn within_budget(start: Instant) -> Result<()> {
    ensure!(
        start.elapsed() < BUDGET,
        "process inspection budget exceeded"
    );
    Ok(())
}

/// Inspect the pane's actual foreground job, without hooks or process environments.
/// Unsupported/ambiguous jobs return a negative decision. OS/race/resource failures
/// return an error, which the caller must treat as ineligible as well.
pub fn eligible(pane_pid: u32, pane_tty: &Path, mappings: &[HarnessMapping]) -> Result<Decision> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        inspect(pane_pid, pane_tty, mappings)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (pane_pid, pane_tty, mappings);
        anyhow::bail!("process inspection is supported only on macOS and Linux")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn foreground(pane_pid: u32, device: u64) -> Result<u32> {
    // A tmux run-shell helper need not have this controlling terminal. In particular,
    // Linux may reject tcgetpgrp on the slave in that context; use kernel process metadata.
    let pane = platform::identity(pane_pid)?.context("pane process disappeared")?;
    ensure!(
        pane.tty == device && pane.foreground > 0,
        "cannot determine pane foreground process group"
    );
    Ok(pane.foreground)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn load(identity: Identity) -> Result<Process> {
    let pid = identity.pid;
    let (executable, executable_id) = platform::executable(pid)?;
    let executable = executable
        .canonicalize()
        .context("cannot resolve process executable")?;
    ensure!(
        ExecutableId::from_metadata(
            &std::fs::metadata(&executable).context("cannot stat process executable")?
        ) == executable_id,
        "process executable changed"
    );
    ensure!(
        platform::identity(pid)?.as_ref() == Some(&identity),
        "process changed during inspection"
    );
    Ok(Process {
        identity,
        executable,
        executable_id,
        argv: Vec::new(),
        descriptors: None,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn load_arguments(process: &mut Process) -> Result<()> {
    let mut argv = platform::arguments(process.identity.pid)?;
    if matches!(
        process.executable.file_name().and_then(|s| s.to_str()),
        Some("node" | "nodejs")
    ) {
        if let Some(entry) = argv.get_mut(1) {
            let path = Path::new(entry);
            if path.is_absolute() {
                *entry = path
                    .canonicalize()
                    .context("cannot resolve Node entrypoint")?
                    .into_os_string();
            }
        }
    }
    ensure!(
        platform::identity(process.identity.pid)?.as_ref() == Some(&process.identity),
        "process changed during inspection"
    );
    process.argv = argv;
    Ok(())
}

fn shell_chain(
    pane: &Process,
    leader: &Process,
    group: u32,
    mut read: impl FnMut(u32) -> Result<Process>,
) -> Result<Vec<Process>> {
    let mut shells: Vec<Process> = Vec::new();
    if leader.identity.pid == pane.identity.pid {
        return Ok(shells);
    }
    let mut parent = leader.identity.parent;
    loop {
        ensure!(
            shells.len() < MAX_SHELLS,
            "pane shell ancestry exceeds inspection limit"
        );
        ensure!(
            parent > 0
                && parent != leader.identity.pid
                && !shells.iter().any(|p| p.identity.pid == parent),
            "pane shell ancestry is disconnected or cyclic"
        );
        let shell = if parent == pane.identity.pid {
            pane.clone()
        } else {
            read(parent)?
        };
        ensure!(
            shell.identity.pid == parent,
            "shell ancestor identity mismatch"
        );
        ensure!(
            shell.identity.tty == pane.identity.tty
                && shell.identity.foreground == group
                && shell.identity.runnable
                && harness::is_shell(&shell.executable),
            "pane ancestor is not a live shell on the same foreground terminal"
        );
        // Waiting ancestor shells normally have their own (background) process groups.
        // Their terminal's foreground group, not their own group, must match the job.
        parent = shell.identity.parent;
        let reached_pane = shell.identity.pid == pane.identity.pid;
        shells.push(shell);
        if reached_pane {
            return Ok(shells);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn snapshot(
    pane_pid: u32,
    tty: u64,
    group: u32,
    mappings: &[HarnessMapping],
    start: Instant,
) -> Result<Snapshot> {
    let pane = platform::identity(pane_pid)?.context("pane process disappeared")?;
    ensure!(
        pane.tty == tty && pane.foreground == group,
        "pane terminal identity changed"
    );
    let mut members = Vec::new();
    for identity in platform::list(group, start)? {
        within_budget(start)?;
        if identity.group == group {
            ensure!(
                identity.tty == tty && identity.foreground == group,
                "foreground group terminal mismatch"
            );
            ensure!(
                members.len() < MAX_MEMBERS,
                "foreground member count exceeds inspection limit"
            );
            members.push(load(identity)?);
        }
    }
    members.sort_by_key(|p| p.identity.pid);
    if let Some(leader) = members.iter_mut().find(|p| p.identity.pid == group) {
        if matches!(
            leader.executable.file_name().and_then(|s| s.to_str()),
            Some("node" | "nodejs")
        ) || leader
            .executable
            .to_str()
            .and_then(harness::native_layout)
            .is_some()
            || mappings.iter().any(|m| m.path == leader.executable)
        {
            load_arguments(leader)?;
        }
        let primary = harness::classify(&leader.executable, &leader.argv, mappings);
        if let Some(primary) = primary
            .filter(|p| p.node_wrapper && matches!(p.harness, Harness::Codex | Harness::OpenCode))
        {
            for child in members
                .iter_mut()
                .filter(|p| p.identity.pid != group && p.identity.parent == group)
            {
                within_budget(start)?;
                let candidate = if mappings.iter().any(|m| m.path == child.executable) {
                    mappings
                        .iter()
                        .filter(|m| m.path == child.executable)
                        .all(|m| {
                            matches!(
                                (primary.harness, m.harness.as_str()),
                                (Harness::Codex, "codex") | (Harness::OpenCode, "opencode")
                            )
                        })
                } else {
                    child.executable.to_str().and_then(harness::native_layout)
                        == Some(primary.harness)
                };
                if candidate
                    && !matches!(
                        child.executable.file_name().and_then(|s| s.to_str()),
                        Some("node" | "nodejs")
                    )
                {
                    load_arguments(child)?;
                }
            }
        }
    }
    let leader = members.iter().find(|p| p.identity.pid == group);
    let frontends: Vec<_> = members
        .iter()
        .filter(|child| {
            child.identity.pid == group
                || leader.is_some_and(|leader| frontend_child(leader, child, mappings))
        })
        .map(|p| p.identity.pid)
        .collect();
    let devices = harmless_devices()?;
    for member in members
        .iter_mut()
        .filter(|p| !frontends.contains(&p.identity.pid))
    {
        within_budget(start)?;
        member.descriptors = Some(platform::descriptors(member.identity.pid, &devices, start)?);
        ensure!(
            platform::identity(member.identity.pid)?.as_ref() == Some(&member.identity),
            "foreground member changed during FD inspection"
        );
    }
    let pane = load(pane)?;
    let shells = if let Some(leader) = members.iter().find(|p| p.identity.pid == group) {
        shell_chain(&pane, leader, group, |pid| {
            within_budget(start)?;
            load(platform::identity(pid)?.context("shell ancestor disappeared")?)
        })?
    } else {
        Vec::new()
    };
    within_budget(start)?;
    Ok(Snapshot {
        pane,
        members,
        shells,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn inspect(pane_pid: u32, pane_tty: &Path, mappings: &[HarnessMapping]) -> Result<Decision> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};

    ensure!(
        pane_pid > 0 && pane_pid <= i32::MAX as u32,
        "invalid pane PID"
    );
    let start = Instant::now();
    let tty = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(pane_tty)
        .context("cannot open pane terminal")?;
    let metadata = tty.metadata().context("cannot stat pane terminal")?;
    ensure!(
        metadata.file_type().is_char_device(),
        "pane terminal is not a character device"
    );
    let device = platform::device(metadata.rdev());
    let group = foreground(pane_pid, device)?;
    let mut canonical_mappings = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        within_budget(start)?;
        ensure!(
            mapping.path.is_absolute(),
            "harness mapping must be absolute"
        );
        canonical_mappings.push(HarnessMapping {
            harness: mapping.harness.clone(),
            path: mapping
                .path
                .canonicalize()
                .context("cannot resolve harness mapping")?,
        });
    }
    let first = snapshot(pane_pid, device, group, &canonical_mappings, start)?;
    let decision = decide(&first.pane, &first.members, group, &canonical_mappings);
    if !decision.eligible {
        return Ok(decision);
    }
    ensure!(
        foreground(pane_pid, device)? == group,
        "foreground process group changed"
    );
    let second = snapshot(pane_pid, device, group, &canonical_mappings, start)?;
    ensure!(
        first == second && foreground(pane_pid, device)? == group,
        "foreground job changed during inspection"
    );
    within_budget(start)?;
    Ok(decision)
}

fn frontend_child(leader: &Process, child: &Process, mappings: &[HarnessMapping]) -> bool {
    let Some(primary) = harness::classify(&leader.executable, &leader.argv, mappings) else {
        return false;
    };
    let Some(secondary) = harness::classify(&child.executable, &child.argv, mappings) else {
        return false;
    };
    primary.node_wrapper
        && matches!(primary.harness, Harness::Codex | Harness::OpenCode)
        && child.identity.pid != leader.identity.pid
        && child.identity.parent == leader.identity.pid
        && secondary.harness == primary.harness
        && !secondary.node_wrapper
        && leader.argv[primary.args_offset..] == child.argv[secondary.args_offset..]
}

fn decide(
    pane: &Process,
    members: &[Process],
    group: u32,
    mappings: &[HarnessMapping],
) -> Decision {
    if !pane.identity.runnable || members.is_empty() || members.iter().any(|p| !p.identity.runnable)
    {
        return Decision::no("no live foreground harness");
    }
    let Some(leader) = members.iter().find(|p| p.identity.pid == group) else {
        return Decision::no("foreground group leader is not inspectable");
    };
    // snapshot has already validated the entire shell-only chain to the pane.
    let Some(primary) = harness::classify(&leader.executable, &leader.argv, mappings) else {
        return Decision::no("unrecognized harness or noninteractive arguments");
    };
    if members.len() > MAX_MEMBERS {
        return Decision::no("foreground member count exceeds inspection limit");
    }
    let mut native = None;
    if matches!(primary.harness, Harness::Codex | Harness::OpenCode) && primary.node_wrapper {
        for child in members
            .iter()
            .filter(|p| p.identity.pid != group && p.identity.parent == group)
        {
            if frontend_child(leader, child, mappings)
                && native.replace(child.identity.pid).is_some()
            {
                return Decision::no("multiple foreground native frontend candidates");
            }
        }
        if native.is_none() {
            return Decision::no("foreground processes are not a verified frontend pair");
        }
    }
    for member in members
        .iter()
        .filter(|p| p.identity.pid != group && Some(p.identity.pid) != native)
    {
        let Some(fds) = &member.descriptors else {
            return Decision::no("foreground helper FD metadata is unavailable");
        };
        if fds.stdin == Input::Other {
            return Decision::no("foreground helper stdin is not a pipe, socket, or /dev/null");
        }
        if fds.extra_terminal {
            return Decision::no("foreground helper has an extra terminal-capable FD");
        }
        let mut parent = member.identity.parent;
        let mut seen = vec![member.identity.pid];
        while parent != group && Some(parent) != native {
            if seen.contains(&parent) || seen.len() >= MAX_MEMBERS {
                return Decision::no("foreground helper ancestry is cyclic");
            }
            let Some(ancestor) = members.iter().find(|p| p.identity.pid == parent) else {
                return Decision::no("foreground helper ancestry is missing or unrelated");
            };
            if !ancestor
                .descriptors
                .as_ref()
                .is_some_and(Descriptors::helper)
            {
                return Decision::no(
                    "foreground helper ancestry includes a terminal-capable process",
                );
            }
            seen.push(parent);
            parent = ancestor.identity.parent;
        }
    }
    Decision {
        eligible: true,
        reason: if members.len() > 1 + usize::from(native.is_some()) {
            "recognized interactive frontend with nonterminal same-group helpers"
        } else if native.is_some() {
            "recognized interactive wrapper/native frontend pair"
        } else {
            "recognized interactive foreground harness"
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;

    fn process(pid: u32, parent: u32, exe: &str, argv: &[&str]) -> Process {
        Process {
            identity: Identity {
                pid,
                parent,
                group: 20,
                tty: 1,
                foreground: 20,
                started: (1, 0),
                runnable: true,
            },
            executable: exe.into(),
            executable_id: ExecutableId::default(),
            argv: argv.iter().map(OsString::from).collect(),
            descriptors: None,
        }
    }

    #[test]
    fn requires_live_group_leader() {
        let pane = process(10, 1, "/bin/zsh", &["zsh"]);
        let mut cli = process(
            20,
            10,
            "/home/alice/.local/share/claude/versions/2.1.1",
            &["claude"],
        );
        assert!(decide(&pane, &[cli.clone()], 20, &[]).eligible);
        cli.identity.runnable = false;
        assert!(!decide(&pane, &[cli.clone()], 20, &[]).eligible);
        cli.identity.runnable = true;
        assert!(!decide(&pane, &[cli.clone()], 21, &[]).eligible);
        assert!(decide(&cli, &[cli.clone()], 20, &[]).eligible);
        assert!(!decide(&pane, &[], 20, &[]).eligible);
    }

    #[test]
    fn nested_shell_chain_is_bounded_and_shell_only() {
        let mut pane = process(10, 1, "/bin/zsh", &["zsh"]);
        pane.identity.group = 10;
        let cli = process(20, 15, "/home/alice/.opencode/bin/opencode", &["opencode"]);
        let mut nested = process(15, 10, "/bin/bash", &["bash"]);
        nested.identity.group = 15;
        let chain = shell_chain(&pane, &cli, 20, |_| Ok(nested.clone())).unwrap();
        assert_eq!(chain, vec![nested.clone(), pane.clone()]);
        let mut middle = process(14, 10, "/bin/zsh", &["zsh"]);
        middle.identity.group = 14;
        let mut deeper = nested.clone();
        deeper.identity.parent = 14;
        let longer = shell_chain(&pane, &cli, 20, |pid| match pid {
            15 => Ok(deeper.clone()),
            14 => Ok(middle.clone()),
            _ => panic!("must only read the parent chain"),
        })
        .unwrap();
        assert_eq!(longer, vec![deeper, middle, pane.clone()]);
        assert!(
            shell_chain(&cli, &cli, 20, |_| panic!("no ancestors for pane leader"))
                .unwrap()
                .is_empty()
        );

        for exe in ["/usr/bin/nvim", "/usr/bin/ssh", "/tmp/bash"] {
            let mut invalid = nested.clone();
            invalid.executable = exe.into();
            assert!(shell_chain(&pane, &cli, 20, |_| Ok(invalid.clone())).is_err());
        }
        for field in [
            "tty",
            "foreground",
            "stopped",
            "cycle",
            "disconnected",
            "pid",
        ] {
            let mut invalid = nested.clone();
            match field {
                "tty" => invalid.identity.tty = 2,
                "foreground" => invalid.identity.foreground = 15,
                "stopped" => invalid.identity.runnable = false,
                "cycle" => invalid.identity.parent = 15,
                "disconnected" => invalid.identity.parent = 0,
                "pid" => invalid.identity.pid = 99,
                _ => unreachable!(),
            }
            assert!(
                shell_chain(&pane, &cli, 20, |_| Ok(invalid.clone())).is_err(),
                "{field}"
            );
        }
        assert!(shell_chain(&pane, &cli, 20, |_| bail!("ancestor disappeared")).is_err());
        let mut reads = 0;
        assert!(shell_chain(&pane, &cli, 20, |pid| {
            reads += 1;
            Ok(process(pid, pid + 100, "/bin/bash", &["bash"]))
        })
        .is_err());
        assert_eq!(reads, MAX_SHELLS);

        let mut invalid_pane = pane.clone();
        invalid_pane.executable = "/usr/bin/nvim".into();
        assert!(shell_chain(&invalid_pane, &cli, 20, |_| Ok(nested.clone())).is_err());
        invalid_pane = pane.clone();
        invalid_pane.identity.runnable = false;
        assert!(shell_chain(&invalid_pane, &cli, 20, |_| Ok(nested.clone())).is_err());

        let first = Snapshot {
            pane,
            members: vec![cli],
            shells: chain,
        };
        for field in ["started", "executable", "inode", "parent"] {
            let mut changed = first.clone();
            match field {
                "started" => changed.shells[0].identity.started.0 += 1,
                "executable" => changed.shells[0].executable = "/bin/zsh".into(),
                "inode" => changed.shells[0].executable_id.inode += 1,
                "parent" => changed.shells[0].identity.parent = 99,
                _ => unreachable!(),
            }
            assert_ne!(first, changed, "ancestor {field} must be revalidated");
        }
    }

    #[test]
    fn foreground_shell_does_not_authorize_background_harness() {
        let mut pane = process(10, 1, "/bin/zsh", &["zsh"]);
        pane.identity.group = 10;
        pane.identity.foreground = 15;
        let mut shell = process(15, 10, "/bin/bash", &["bash"]);
        shell.identity.group = 15;
        shell.identity.foreground = 15;
        // A background harness in group 20 is not among the foreground members.
        assert!(shell_chain(&pane, &shell, 15, |_| panic!("direct pane parent")).is_ok());
        assert!(!decide(&pane, &[shell], 15, &[]).eligible);
    }

    #[test]
    fn only_codex_wrapper_native_pair_is_allowed() {
        let pane = process(10, 1, "/bin/bash", &["bash"]);
        let wrapper = process(
            20,
            10,
            "/usr/bin/node",
            &[
                "node",
                "/usr/lib/node_modules/@openai/codex/bin/codex.js",
                "-p",
                "work",
            ],
        );
        let mut native = process(
            21,
            20,
            "/usr/lib/node_modules/@openai/codex/vendor/x86_64-unknown-linux-musl/codex/codex",
            &["codex", "-p", "work"],
        );
        assert!(!decide(&pane, &[wrapper.clone()], 20, &[]).eligible);
        assert!(decide(&pane, &[wrapper.clone(), native.clone()], 20, &[]).eligible);
        native.identity.parent = 10;
        assert!(!decide(&pane, &[wrapper.clone(), native.clone()], 20, &[]).eligible);
        native.identity.parent = 20;
        native.argv.push("exec".into());
        assert!(!decide(&pane, &[wrapper.clone(), native], 20, &[]).eligible);
        let other = process(22, 20, "/bin/sh", &["sh"]);
        assert!(!decide(&pane, &[wrapper, other], 20, &[]).eligible);
    }

    #[test]
    fn opencode_pair_requires_matching_interactive_arguments() {
        let pane = process(10, 1, "/bin/bash", &["bash"]);
        let wrapper = process(
            20,
            10,
            "/usr/bin/node",
            &[
                "node",
                "/usr/lib/node_modules/opencode-ai/bin/opencode",
                "attach",
                "http://localhost:4096",
            ],
        );
        let native = process(
            21,
            20,
            "/usr/lib/node_modules/opencode-linux-x64/bin/opencode",
            &["opencode", "attach", "http://localhost:4096"],
        );
        assert!(!decide(&pane, &[wrapper.clone()], 20, &[]).eligible);
        assert!(decide(&pane, &[wrapper.clone(), native.clone()], 20, &[]).eligible);
        let extra = process(22, 20, "/bin/sh", &["sh"]);
        assert!(!decide(&pane, &[wrapper, native, extra], 20, &[]).eligible);
    }

    #[test]
    fn helpers_need_live_bounded_ancestry_and_nonterminal_input() {
        let pane = process(10, 1, "/bin/bash", &[]);
        let leader = process(20, 10, "/home/alice/.opencode/bin/opencode", &["opencode"]);
        let mut members = vec![leader];
        for pid in 21..20 + MAX_MEMBERS as u32 {
            let mut helper = process(pid, pid - 1, "/usr/bin/node", &[]);
            helper.descriptors = Some(Descriptors {
                stdin: [Input::Pipe, Input::Socket, Input::Null][pid as usize % 3],
                extra_terminal: false,
            });
            members.push(helper);
        }
        assert!(decide(&pane, &members, 20, &[]).eligible);
        // Every intermediate process must be safe, not just the final helper.
        for field in [
            "stdin",
            "extra",
            "missing",
            "unrelated",
            "self-cycle",
            "cycle",
            "stopped",
            "unreadable",
        ] {
            let mut invalid = members.clone();
            match field {
                "stdin" => invalid[1].descriptors.as_mut().unwrap().stdin = Input::Other,
                "extra" => invalid[1].descriptors.as_mut().unwrap().extra_terminal = true,
                "missing" => invalid[1].identity.parent = 999,
                "unrelated" => invalid[1].identity.parent = 10,
                "self-cycle" => invalid[1].identity.parent = 21,
                "cycle" => invalid[1].identity.parent = 22,
                "stopped" => invalid[1].identity.runnable = false,
                "unreadable" => invalid[1].descriptors = None,
                _ => unreachable!(),
            }
            assert!(!decide(&pane, &invalid, 20, &[]).eligible, "{field}");
        }
        let mut extra = members.last().unwrap().clone();
        extra.identity.pid += 1;
        members.push(extra);
        assert!(!decide(&pane, &members, 20, &[]).eligible);
    }

    #[test]
    fn wrapper_helpers_must_be_rooted_at_the_verified_pair() {
        let pane = process(10, 1, "/bin/bash", &[]);
        let wrapper = process(
            20,
            10,
            "/usr/bin/node",
            &["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"],
        );
        let native = process(
            21,
            20,
            "/usr/lib/node_modules/@openai/codex/vendor/x86_64-unknown-linux-musl/codex/codex",
            &["codex"],
        );
        let mut helper = process(22, 21, "/usr/bin/node", &[]);
        helper.descriptors = Some(Descriptors {
            stdin: Input::Pipe,
            extra_terminal: false,
        });
        let mut members = vec![wrapper, native, helper];
        assert!(decide(&pane, &members, 20, &[]).eligible);
        members[2].identity.parent = 20;
        assert!(decide(&pane, &members, 20, &[]).eligible);
        members[1].argv.push("exec".into());
        assert!(!decide(&pane, &members, 20, &[]).eligible);
        members[1].argv.pop();
        let mut duplicate = members[1].clone();
        duplicate.identity.pid = 23;
        members.push(duplicate);
        assert!(!decide(&pane, &members, 20, &[]).eligible);
    }

    #[test]
    fn revalidation_retains_helper_identity_executable_and_relevant_fd_evidence() {
        let mut helper = process(21, 20, "/usr/bin/node", &[]);
        helper.descriptors = Some(Descriptors {
            stdin: Input::Pipe,
            extra_terminal: false,
        });
        for field in [
            "started",
            "parent",
            "executable",
            "inode",
            "mtime",
            "stdin",
            "terminal",
        ] {
            let mut changed = helper.clone();
            match field {
                "started" => changed.identity.started.0 += 1,
                "parent" => changed.identity.parent += 1,
                "executable" => changed.executable = "/usr/bin/editor".into(),
                "inode" => changed.executable_id.inode += 1,
                "mtime" => changed.executable_id.modified.0 += 1,
                "stdin" => changed.descriptors.as_mut().unwrap().stdin = Input::Socket,
                "terminal" => changed.descriptors.as_mut().unwrap().extra_terminal = true,
                _ => unreachable!(),
            }
            assert_ne!(helper, changed, "{field}");
        }
        assert!(within_budget(Instant::now() - BUDGET).is_err());
    }
}
