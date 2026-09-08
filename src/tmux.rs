//! tmux owns the terminals; this module owns only explicitly marked bindings and sessions.

use crate::config::{self, checked_path, private_dir, Config, Paths};
use crate::inspect::{Invocation, Liveness};
use crate::install::shell_quote;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod lifecycle;
mod routing;

pub use lifecycle::watch;

const GENERATION: &str = "@aft_generation";
const OWNER: &str = "@aft_owner";
const FLOAT_GENERATION: &str = "@aft_float_generation";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_STYLE: &str = "none,fg=terminal,bg=terminal";

#[derive(Clone)]
struct Tmux {
    binary: PathBuf,
    socket: PathBuf,
}

fn which(name: &str) -> Result<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| fs::metadata(path).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0))
        .context(format!("{name} is not on PATH"))?
        .canonicalize()
        .with_context(|| format!("resolve {name}"))
}

fn text(path: &Path) -> Result<&str> {
    checked_path(path)?;
    path.to_str().context("path must be UTF-8")
}

fn quiet() -> bool {
    env::var_os("AFT_QUIET").as_deref() == Some(OsStr::new("1"))
}

fn tmux_binary() -> Result<PathBuf> {
    if let Some(path) = env::var_os("AFT_TMUX_BINARY").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        checked_path(&path)?;
        ensure!(
            fs::metadata(&path).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0),
            "AFT_TMUX_BINARY is unavailable; use the client matching your running tmux server"
        );
        return Ok(path);
    }
    which("tmux")
}

fn token() -> Result<String> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut token = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write;
        write!(token, "{byte:02x}")?;
    }
    Ok(token)
}

fn valid_token(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_pane(value: &str) -> bool {
    value
        .strip_prefix('%')
        .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// tmux performs format expansion even on shell-quoted command strings.
fn literal_format(value: &str) -> String {
    value.replace('#', "##")
}

fn tmux_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
    )
}

fn capture(command: &mut Command, input: Option<&[u8]>) -> Result<Output> {
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start tmux command")?;
    let result = (|| {
        let mut stdout = child.stdout.take().context("missing stdout pipe")?;
        let mut stderr = child.stderr.take().context("missing stderr pipe")?;
        let mut stdin = child.stdin.take();
        for fd in [
            Some(stdout.as_raw_fd()),
            Some(stderr.as_raw_fd()),
            stdin.as_ref().map(AsRawFd::as_raw_fd),
        ]
        .into_iter()
        .flatten()
        {
            // SAFETY: these descriptors are owned by the live pipe handles above.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            ensure!(flags >= 0, "cannot inspect command pipe flags");
            // SAFETY: F_SETFL changes only the flags on the valid owned descriptor.
            let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
            ensure!(result >= 0, "cannot configure nonblocking command pipes");
        }
        let started = Instant::now();
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut out_done = false;
        let mut err_done = false;
        let mut remaining = input.unwrap_or_default();
        loop {
            ensure!(
                started.elapsed() < COMMAND_TIMEOUT,
                "tmux command timed out"
            );
            if remaining.is_empty() {
                stdin.take();
            }
            if let Some(pipe) = &mut stdin {
                match pipe.write(remaining) {
                    Ok(0) => bail!("tmux stdin closed"),
                    Ok(n) => remaining = &remaining[n..],
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(error) => return Err(error).context("write tmux input"),
                }
            }
            for (pipe, bytes, done) in [
                (&mut stdout as &mut dyn Read, &mut output, &mut out_done),
                (&mut stderr as &mut dyn Read, &mut errors, &mut err_done),
            ] {
                if *done {
                    continue;
                }
                let mut buffer = [0; 8192];
                match pipe.read(&mut buffer) {
                    Ok(0) => *done = true,
                    Ok(n) => {
                        bytes.extend_from_slice(&buffer[..n]);
                        ensure!(bytes.len() <= 1024 * 1024, "tmux output exceeds limit");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(error) => return Err(error).context("read tmux output"),
                }
            }
            if let Some(status) = child.try_wait()? {
                if out_done && err_done {
                    return Ok(Output {
                        status,
                        stdout: output,
                        stderr: errors,
                    });
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn checked_output(output: Output) -> Result<String> {
    ensure!(
        output.status.success(),
        "tmux: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?
        .trim_end_matches('\n')
        .to_owned())
}

impl Tmux {
    fn new(socket: PathBuf) -> Result<Self> {
        checked_path(&socket)?;
        let metadata = fs::symlink_metadata(&socket).context("tmux socket is unavailable")?;
        ensure!(
            metadata.file_type().is_socket() && metadata.uid() == config::uid(),
            "tmux socket must be owned by this user and must not be a symlink"
        );
        Ok(Self {
            binary: tmux_binary()?,
            socket,
        })
    }

    fn resolve(socket: Option<PathBuf>) -> Result<Self> {
        let socket = match socket {
            Some(path) => path,
            None => match env::var("TMUX").ok().filter(|s| !s.is_empty()) {
                Some(value) => {
                    let mut fields = value.rsplitn(3, ',');
                    fields.next();
                    fields.next();
                    PathBuf::from(
                        fields
                            .next()
                            .context("invalid TMUX environment; use --socket")?,
                    )
                }
                None => dedicated_socket()?,
            },
        };
        Self::new(socket)
            .context("select tmux server (run inside tmux or specify --socket)")?
            .matching_client()
    }

    fn client_version(&self) -> Result<String> {
        let value = checked_output(capture(Command::new(&self.binary).arg("-V"), None)?)?;
        Ok(value
            .strip_prefix("tmux ")
            .context("unexpected tmux client version")?
            .to_owned())
    }

    fn matching_client(mut self) -> Result<Self> {
        let server = self.output(&["display-message", "-p", "#{version}"])?;
        let client = self.client_version()?;
        if client == server {
            return Ok(self);
        }
        // Preserve an explicitly approved per-server client across login-shell PATH
        // changes. Never download a client or restart a server to resolve a mismatch.
        if let Some(record) = read_record(&record_path(&self.socket)?)? {
            if record.socket == self.socket {
                if let Some(binary) = record.binary {
                    let mut candidate = self.clone();
                    candidate.binary = binary;
                    if candidate
                        .client_version()
                        .is_ok_and(|version| version == server)
                    {
                        self.binary = candidate.binary;
                        return Ok(self);
                    }
                }
            }
        }
        bail!("tmux client {client} does not match running server {server}; terminal FD passing can fail. Set AFT_TMUX_BINARY to a matching {server} executable and run bind. Existing sessions were preserved")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command.arg("-S").arg(&self.socket).env_remove("TMUX");
        command
    }

    fn output(&self, args: &[&str]) -> Result<String> {
        checked_output(capture(self.command().args(args), None)?)
    }

    fn source(&self, command: &str) -> Result<()> {
        self.source_result(command).map(|_| ())
    }

    fn source_result(&self, command: &str) -> Result<String> {
        checked_output(capture(
            self.command().args(["source-file", "-"]),
            Some(command.as_bytes()),
        )?)
    }

    fn global(&self, option: &str) -> Result<String> {
        self.output(&["show-options", "-gqv", option])
    }

    fn generation(&self) -> Result<String> {
        let generation = self.global(GENERATION)?;
        ensure!(
            valid_token(&generation),
            "server has no valid agent-float-term ownership marker; run bind"
        );
        Ok(generation)
    }

    fn compatible(&self) -> Result<String> {
        let version = self.output(&["display-message", "-p", "#{version}"])?;
        ensure!(
            supported_version(&version),
            "running tmux {version} is unsupported; need 3.4 or newer"
        );
        let client = self.client_version()?;
        ensure!(client == version,
            "tmux client {client} does not match running server {version}; use a matching client via AFT_TMUX_BINARY (do not restart live sessions)");
        ensure!(
            self.global("exit-unattached")? != "on",
            "exit-unattached is on; disable it explicitly or use the dedicated server"
        );
        ensure!(
            self.global("destroy-unattached")? != "on",
            "destroy-unattached is on; disable it explicitly or use the dedicated server"
        );
        Ok(version)
    }

    fn key_binding(&self, key: &str) -> Result<Option<String>> {
        // tmux 3.7c sends a one-item listing to a status message, not stdout.
        // Listing all tables avoids that when root has only one binding.
        let all = self.output(&["list-keys"])?;
        Ok(all
            .lines()
            .filter(|line| {
                line.split_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|w| w == ["-T", "root"])
            })
            .filter_map(binding_line)
            .find(|(found, _)| found == key)
            .map(|(_, line)| line))
    }

    fn pane(&self, id: &str) -> Result<Pane> {
        ensure!(valid_pane(id), "invalid pane ID");
        let value = self.output(&[
            "display-message",
            "-p",
            "-t",
            id,
            "#{pane_id}|#{pane_pid}|#{pane_tty}|#{pane_dead}|#{pane_in_mode}|#{version}",
            ";",
            "show-options",
            "-gqv",
            GENERATION,
            ";",
            "show-options",
            "-gqv",
            "exit-unattached",
            ";",
            "show-options",
            "-gqv",
            "destroy-unattached",
            ";",
            "show-options",
            "-gqv",
            "default-shell",
            ";",
            "display-message",
            "-p",
            "-t",
            id,
            "#{pane_current_path}",
        ])?;
        let mut lines = value.lines();
        let fields: Vec<_> = lines
            .next()
            .context("missing pane metadata")?
            .split('|')
            .collect();
        ensure!(
            fields.len() == 6 && fields[0] == id && fields[3] == "0",
            "pane disappeared or is dead"
        );
        Ok(Pane {
            id: id.into(),
            pid: fields[1].parse()?,
            tty: fields[2].into(),
            in_mode: fields[4] != "0",
            server_version: fields[5].into(),
            generation: lines.next().context("missing server generation")?.into(),
            exit_unattached: lines.next().context("missing detach policy")? != "off",
            destroy_unattached: lines.next().context("missing session policy")? != "off",
            default_shell: lines.next().context("missing default shell")?.into(),
            cwd: lines.next().context("missing pane directory")?.into(),
        })
    }

    fn client(&self, pid: u32) -> Result<Client> {
        let all = self.output(&[
            "list-clients",
            "-F",
            "#{client_pid}|#{client_name}|#{pane_id}|#{session_id}",
        ])?;
        for line in all.lines() {
            let fields: Vec<_> = line.split('|').collect();
            if fields.len() == 4 && fields[0].parse::<u32>() == Ok(pid) {
                return Ok(Client {
                    pid,
                    name: fields[1].into(),
                    pane: fields[2].into(),
                    session: fields[3].into(),
                });
            }
        }
        bail!("invoking client disappeared")
    }

    fn message(&self, client: &Client, message: &str) {
        let _ = self.output(&["display-message", "-c", &client.name, message]);
    }

    fn forward(&self, pane: &Pane, client: &Client, key: &str) -> Result<()> {
        if env::var_os("AFT_RESTORE_INSTANCE").is_some() {
            return Ok(());
        }
        if self.client_by_name_is_on(client, &pane.id)? {
            self.output(&["send-keys", "-t", &pane.id, key])?;
        }
        Ok(())
    }

    fn client_by_name_is_on(&self, client: &Client, pane: &str) -> Result<bool> {
        let current = self.client(client.pid)?;
        Ok(
            current.name == client.name
                && current.session == client.session
                && current.pane == pane,
        )
    }

    fn popup_policy(&self, generation: &str) -> Result<()> {
        let metadata = self.output(&[
            "show-options",
            "-gqv",
            GENERATION,
            ";",
            "show-options",
            "-gqv",
            "exit-unattached",
            ";",
            "show-options",
            "-gqv",
            "destroy-unattached",
        ])?;
        ensure!(
            metadata.lines().collect::<Vec<_>>() == [generation, "off", "off"],
            "global generation or detach policies changed; popup was not opened"
        );
        Ok(())
    }

    fn popup_client_unchanged(&self, client: &Client, pane: &Pane) -> Result<bool> {
        let metadata = self.output(&[
            "show-options",
            "-gqv",
            GENERATION,
            ";",
            "show-options",
            "-gqv",
            "exit-unattached",
            ";",
            "show-options",
            "-gqv",
            "destroy-unattached",
            ";",
            "list-clients",
            "-F",
            "#{client_pid}|#{client_name}|#{session_id}|#{pane_id}|#{pane_in_mode}",
        ])?;
        let mut lines = metadata.lines();
        ensure!(lines.next() == Some(&pane.generation) && lines.next() == Some("off") && lines.next() == Some("off"),
            "global generation or detach policies changed during shell startup; popup was not opened");
        let expected = format!(
            "{}|{}|{}|{}|0",
            client.pid, client.name, client.session, pane.id
        );
        Ok(lines.any(|line| line == expected))
    }
}

struct Pane {
    id: String,
    pid: u32,
    tty: PathBuf,
    in_mode: bool,
    server_version: String,
    generation: String,
    exit_unattached: bool,
    destroy_unattached: bool,
    default_shell: PathBuf,
    cwd: PathBuf,
}
struct Client {
    pid: u32,
    name: String,
    pane: String,
    session: String,
}

// list-keys aligns columns according to the other keys in its table. Normalize
// only the command prefix, leaving the serialized command body byte-for-byte.
fn binding_line(line: &str) -> Option<(String, String)> {
    let words: Vec<_> = line.split_whitespace().collect();
    let table = words.iter().position(|word| *word == "-T")?;
    let key = *words.get(table + 2)?;
    let mut rest = line;
    for _ in 0..table + 3 {
        rest = rest.trim_start();
        rest = &rest[rest.find(char::is_whitespace).unwrap_or(rest.len())..];
    }
    Some((
        key.into(),
        format!(
            "{} root {} {}",
            words[..=table].join(" "),
            key,
            rest.trim_start()
        ),
    ))
}

fn supported_version(value: &str) -> bool {
    let Some((major, rest)) = value.split_once('.') else {
        return false;
    };
    let Ok(major) = major.parse::<u32>() else {
        return false;
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let Ok(minor) = digits.parse::<u32>() else {
        return false;
    };
    major > 3 || (major == 3 && minor >= 4)
}

fn runtime_dir() -> Result<PathBuf> {
    let directory = Paths::discover()?.state.join("runtime");
    private_dir(&directory)?;
    Ok(directory)
}

fn record_path(socket: &Path) -> Result<PathBuf> {
    Ok(Paths::discover()?.state.join("runtime").join(format!(
        "{:x}.json",
        Sha256::digest(socket.as_os_str().as_encoded_bytes())
    )))
}

fn lock(path: &Path) -> Result<File> {
    lock_for(path, Duration::from_secs(2))?.context("another agent-float-term operation is busy")
}

fn lock_for(path: &Path, timeout: Duration) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.uid() == config::uid() && metadata.mode() & 0o077 == 0,
        "unsafe runtime lock file"
    );
    let start = Instant::now();
    loop {
        // SAFETY: file owns this descriptor for the entire lifetime of the lock.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(file));
        }
        let error = std::io::Error::last_os_error();
        ensure!(
            error.kind() == std::io::ErrorKind::WouldBlock,
            "lock failed: {error}"
        );
        if start.elapsed() >= timeout {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    socket: PathBuf,
    binary: Option<PathBuf>,
    binary_stamp: Option<BinaryStamp>,
    server_version: Option<String>,
    generation: String,
    key: String,
    installed: String,
    prior_installed: Option<String>,
    previous: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct BinaryStamp {
    device: u64,
    inode: u64,
    length: u64,
    modified: i64,
    modified_ns: i64,
    mode: u32,
}

fn binary_stamp(path: &Path) -> Result<BinaryStamp> {
    let metadata = fs::metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.mode() & 0o111 != 0,
        "tmux client is not executable"
    );
    Ok(BinaryStamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        modified: metadata.mtime(),
        modified_ns: metadata.mtime_nsec(),
        mode: metadata.mode(),
    })
}

fn validate_popup_client(tmux: &Tmux, pane: &Pane) -> Result<()> {
    ensure!(
        supported_version(&pane.server_version),
        "running tmux is older than 3.4"
    );
    ensure!(
        valid_token(&pane.generation),
        "server ownership marker is unavailable; run bind"
    );
    ensure!(
        !pane.exit_unattached && !pane.destroy_unattached,
        "exit-unattached/destroy-unattached must be off; existing sessions were preserved"
    );
    if let Some(binding) = read_record(&record_path(&tmux.socket)?)? {
        ensure!(
            binding.generation == pane.generation,
            "server generation changed; run bind before opening a popup"
        );
        if binding.socket == tmux.socket
            && binding.generation == pane.generation
            && binding.binary.as_deref() == Some(tmux.binary.as_path())
            && binding.server_version.as_deref() == Some(pane.server_version.as_str())
            && binding.binary_stamp.as_ref() == Some(&binary_stamp(&tmux.binary)?)
        {
            return Ok(());
        }
    }
    // Old records and replaced clients require a fresh check. A verified, unchanged
    // executable need not be spawned for `-V` on every keypress.
    let client = tmux.client_version()?;
    ensure!(client == pane.server_version,
        "tmux client {client} does not match running server {}; run bind with a matching AFT_TMUX_BINARY", pane.server_version);
    Ok(())
}

fn read_record(path: &Path) -> Result<Option<Binding>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("open binding record"),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.uid() == config::uid() && metadata.mode() & 0o077 == 0,
        "unsafe binding record"
    );
    let mut bytes = Vec::new();
    file.take(128 * 1024).read_to_end(&mut bytes)?;
    let record: Binding = serde_json::from_slice(&bytes)?;
    if let Some(binary) = &record.binary {
        checked_path(binary)?;
    }
    ensure!(
        valid_token(&record.generation),
        "invalid binding generation"
    );
    Config {
        key: record.key.clone(),
        ..Config::default()
    }
    .validate()?;
    Ok(Some(record))
}

fn write_record(path: &Path, record: &Binding) -> Result<()> {
    atomic_private_write(path, &serde_json::to_vec_pretty(record)?)
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", token()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(path.parent().context("record parent")?)?.sync_all()?;
    Ok(())
}

fn helper_path() -> Result<PathBuf> {
    let executable = env::current_exe()?.canonicalize()?;
    let installed = Paths::discover()?.bin.join("agent-float-term");
    if installed
        .canonicalize()
        .is_ok_and(|path| path == executable)
    {
        Ok(installed)
    } else {
        Ok(executable)
    }
}

fn dispatch_command(tmux: &Tmux, key: &str) -> Result<String> {
    let helper = shell_quote(text(&helper_path()?)?);
    let tmux_binary = shell_quote(text(&tmux.binary)?);
    let socket = shell_quote(text(&tmux.socket)?);
    // Explicit product roots make the binding independent of a stale server environment.
    let paths = Paths::discover()?;
    let home = env::var("HOME").context("HOME must be UTF-8")?;
    let environment = format!(
        "AFT_TMUX_BINARY={} HOME={} XDG_CONFIG_HOME={} XDG_DATA_HOME={} XDG_STATE_HOME={}",
        shell_quote(text(&tmux.binary)?),
        shell_quote(&home),
        shell_quote(text(paths.config.parent().context("config root")?)?),
        shell_quote(text(paths.data.parent().context("data root")?)?),
        shell_quote(text(paths.state.parent().context("state root")?)?)
    );
    // Expand only the two tmux-generated numeric identifiers, never user-controlled paths.
    let prefix = literal_format(&format!(
        "if [ -x {helper} ]; then env {environment} {helper} dispatch --socket {socket}"
    ));
    let suffix = literal_format(&format!(
        " --key {}; else {tmux_binary} -S {socket} send-keys -t",
        shell_quote(key)
    ));
    Ok(format!("{prefix} --pane '#{{pane_id}}' --client-pid '#{{client_pid}}'{suffix} '#{{pane_id}}' {}; fi", shell_quote(key)))
}

fn restore(tmux: &Tmux, binding: &Binding) -> Result<bool> {
    if tmux.global(GENERATION)? != binding.generation {
        return Ok(false);
    }
    if !binding_matches(binding, tmux.key_binding(&binding.key)?.as_deref()) {
        return Ok(false);
    }
    match &binding.previous {
        Some(previous) => tmux.source(&format!("{previous}\n"))?,
        None => {
            tmux.output(&["unbind-key", "-T", "root", &binding.key])?;
        }
    }
    Ok(true)
}

fn binding_matches(binding: &Binding, live: Option<&str>) -> bool {
    live.is_some_and(|line| {
        line == binding.installed || binding.prior_installed.as_deref() == Some(line)
    })
}

pub fn bind(socket: Option<PathBuf>, replace_key: bool) -> Result<()> {
    let mut config = config::load()?;
    let tmux = Tmux::resolve(socket)?;
    let verified_stamp = binary_stamp(&tmux.binary)?;
    let server_version = tmux.compatible()?;
    ensure!(
        binary_stamp(&tmux.binary)? == verified_stamp,
        "tmux executable changed while checking its version; retry bind"
    );
    runtime_dir()?;
    let record_path = record_path(&tmux.socket)?;
    let _guard = lock(&record_path.with_extension("lock"))?;
    let mut generation = tmux.global(GENERATION)?;
    let existing = read_record(&record_path)?;
    let existing = existing.filter(|b| b.socket == tmux.socket && b.generation == generation);
    let initialize_generation = generation.is_empty();
    if initialize_generation {
        generation = token()?;
    }
    let command = dispatch_command(&tmux, &config.key)?;
    let float =
        format!("#{{&&:#{{==:#{{@aft_float_generation}},{generation}}},#{{!=:#{{@aft_owner}},}}}}");
    // Ask tmux to canonicalize aliases (e.g. C-i -> Tab) and serialize our intended
    // binding in an owned temporary table. Never adopt a post-write user binding.
    let table = format!("aft-probe-{}", token()?);
    let sentinel = if config.key == "F12" { "F11" } else { "F12" };
    tmux.output(&[
        "bind-key",
        "-T",
        &table,
        sentinel,
        "display-message",
        "AFT_PROBE_SENTINEL",
    ])?;
    tmux.output(&[
        "bind-key",
        "-T",
        &table,
        &config.key,
        "if-shell",
        "-F",
        &float,
        "detach-client",
        &format!("run-shell -b {}", tmux_quote(&command)),
    ])?;
    let serialized = tmux.output(&["list-keys", "-T", &table]);
    tmux.output(&["unbind-key", "-a", "-T", &table])?;
    let serialized = serialized?;
    let (canonical_key, installed) = serialized
        .lines()
        .filter_map(binding_line)
        .find(|(key, _)| key != sentinel)
        .context("cannot serialize binding")?;
    config.key = canonical_key;
    Config {
        key: config.key.clone(),
        ..Config::default()
    }
    .validate()
    .context("tmux's canonical key is unsupported; choose another key")?;
    let current = tmux.key_binding(&config.key)?;
    let ours = existing
        .as_ref()
        .is_some_and(|b| b.key == config.key && binding_matches(b, current.as_deref()));
    ensure!(
        current.is_none() || ours || replace_key,
        "{} is already bound; choose another key or explicitly use --replace-key",
        config.key
    );
    if initialize_generation {
        tmux.output(&["set-option", "-g", GENERATION, &generation])?;
    }
    ensure!(valid_token(&generation), "invalid server ownership marker");
    if let Some(old) = &existing {
        if old.key != config.key {
            restore(&tmux, old)?;
        }
    }
    let previous = if ours {
        existing.and_then(|b| b.previous)
    } else {
        current.clone()
    };
    let mut record = Binding {
        socket: tmux.socket.clone(),
        binary: Some(tmux.binary.clone()),
        binary_stamp: Some(verified_stamp),
        server_version: Some(server_version),
        generation,
        key: config.key.clone(),
        installed,
        prior_installed: if ours { current.clone() } else { None },
        previous,
    };
    // Journal the restoration value before changing the live key. An interrupted
    // apply can be recovered without guessing which previous binding was ours.
    write_record(&record_path, &record).context("record binding ownership")?;
    ensure!(
        tmux.key_binding(&config.key)? == current,
        "key changed during installation; left untouched"
    );
    tmux.source(&format!("{}\n", record.installed))?;
    ensure!(
        tmux.key_binding(&config.key)?.as_deref() == Some(&record.installed),
        "key changed while applying integration; user binding was not adopted"
    );
    record.prior_installed = None;
    write_record(&record_path, &record)?;
    if !quiet() {
        println!(
            "Bound {} on {} (AI invocations only; inside floats, hide).",
            config.key,
            tmux.socket.display()
        );
    }
    Ok(())
}

pub fn unbind_all() -> Result<()> {
    let directory = Paths::discover()?.state.join("runtime");
    if !directory.exists() {
        return Ok(());
    }
    private_dir(&directory)?;
    for entry in fs::read_dir(&directory)? {
        let path = entry?.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let _guard = lock(&path.with_extension("lock"))?;
        let Some(record) = read_record(&path)? else {
            continue;
        };
        let server = Tmux::new(record.socket.clone()).map(|mut tmux| {
            if let Some(binary) = &record.binary {
                tmux.binary = binary.clone();
            }
            tmux
        });
        match server {
            Ok(tmux) => {
                routing::cleanup_server(&tmux, &record.generation)?;
                match restore(&tmux, &record) {
                    Ok(true) => println!("Restored {} on {}", record.key, record.socket.display()),
                    Ok(false) => println!(
                        "Preserved changed or restarted server binding on {}",
                        record.socket.display()
                    ),
                    Err(error) => {
                        eprintln!("Preserved unavailable binding record: {error}");
                        continue;
                    }
                }
            }
            Err(_) => {
                eprintln!(
                    "Server unavailable: {}; missing-executable forwarding remains in place",
                    record.socket.display()
                );
                continue;
            }
        }
        fs::remove_file(path)?;
    }
    Ok(())
}

fn dedicated_socket() -> Result<PathBuf> {
    let base = env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    checked_path(&base)?;
    Ok(base
        .canonicalize()
        .context("runtime base directory must exist")?
        .join(format!("agent-float-term-{}", config::uid()))
        .join("tmux.sock"))
}

fn socket_directory(path: &Path) -> Result<()> {
    // Socket directories may live under root's sticky /tmp. Do not relax the
    // general installer checks for user-owned configuration/data directories.
    if !path.exists() {
        let parent = fs::metadata(path.parent().context("socket parent")?)?;
        let trusted = parent.uid() == config::uid() && parent.mode() & 0o022 == 0;
        let sticky =
            (parent.uid() == 0 || parent.uid() == config::uid()) && parent.mode() & 0o1000 != 0;
        ensure!(
            parent.is_dir() && (trusted || sticky),
            "unsafe socket parent directory"
        );
        use std::os::unix::fs::DirBuilderExt;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error).context("create socket directory"),
        }
    }
    private_dir(path)
}

fn dedicated_marker(path: &Path) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.uid() == config::uid() && metadata.mode() & 0o077 == 0,
        "unsafe dedicated-server marker"
    );
    let mut marker = String::new();
    file.take(64).read_to_string(&mut marker)?;
    ensure!(valid_token(&marker), "invalid dedicated-server marker");
    Ok(marker)
}

pub fn start() -> Result<()> {
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "start requires an interactive terminal"
    );
    if env::var_os("TMUX").is_some_and(|value| !value.is_empty()) {
        bind(None, false)?;
        if !quiet() {
            println!("Already inside tmux; no nested outer session was created.");
        }
        return Ok(());
    }
    let config = config::load()?;
    let socket = dedicated_socket()?;
    #[cfg(target_os = "macos")]
    ensure!(
        socket.as_os_str().len() < 104,
        "socket path is too long; select a shorter XDG_RUNTIME_DIR"
    );
    socket_directory(socket.parent().context("socket directory")?)?;
    let guard = lock(&socket.with_extension("lock"))?;
    let mut binary = tmux_binary()?;
    let version = checked_output(capture(Command::new(&binary).arg("-V"), None)?)?;
    ensure!(
        supported_version(version.trim_start_matches("tmux ")),
        "need tmux 3.4 or newer"
    );
    let session = format!("aft-work-{}", &token()?[..12]);
    let shell = config
        .shell
        .clone()
        .or_else(|| env::var_os("SHELL").map(PathBuf::from))
        .unwrap_or_else(|| "/bin/sh".into());
    checked_path(&shell)?;
    let marker_path = socket.with_extension("owner");
    let mut existed = socket.exists();
    if existed {
        let metadata = fs::symlink_metadata(&socket)?;
        ensure!(
            metadata.file_type().is_socket() && metadata.uid() == config::uid(),
            "unsafe dedicated socket"
        );
        if std::os::unix::net::UnixStream::connect(&socket)
            .err()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            dedicated_marker(&marker_path)
                .context("dead socket has no ownership evidence; inspect it manually")?;
            let current = fs::symlink_metadata(&socket)?;
            ensure!(
                metadata.ino() == current.ino() && metadata.dev() == current.dev(),
                "socket changed during recovery"
            );
            fs::remove_file(&socket)?;
            existed = false;
        }
    }
    if existed {
        let tmux = Tmux::new(socket.clone())?.matching_client()?;
        ensure!(
            tmux.global("@aft_dedicated")? == "1",
            "socket is not an owned dedicated server"
        );
        ensure!(
            tmux.generation()? == dedicated_marker(&marker_path)?,
            "dedicated-server generation changed"
        );
        tmux.compatible()?;
        binary = tmux.binary;
    }
    let mut command = Command::new(&binary);
    let cwd = env::current_dir()?;
    command
        .args(["-S"])
        .arg(&socket)
        .args(["-f", "/dev/null", "new-session", "-d", "-s", &session, "-c"])
        .arg(literal_format(text(&cwd)?))
        .args(["-e", "AFT_DISABLE=1"])
        .args(["-e", "AFT_QUIET="])
        .args(["-e", &format!("AFT_TMUX_BINARY={}", text(&binary)?)])
        .arg(&shell)
        .arg("-l");
    checked_output(capture(&mut command, None)?)?;
    let mut tmux = Tmux::new(socket.clone())?;
    tmux.binary = binary;
    if !existed {
        let generation = token()?;
        tmux.output(&["set-option", "-g", GENERATION, &generation])?;
        tmux.output(&["set-option", "-g", "@aft_dedicated", "1"])?;
        tmux.output(&["set-option", "-g", "status", "off"])?;
        atomic_private_write(&marker_path, generation.as_bytes())?;
    }
    drop(guard);
    if let Err(error) = bind(Some(socket), false) {
        eprintln!("Integration failed; your shell is retained as {session}: {error}");
    }
    let status = tmux
        .command()
        .args(["attach-session", "-E", "-t", &format!("={session}")])
        .status()?;
    ensure!(
        status.success(),
        "tmux attachment failed; session {session} was preserved"
    );
    Ok(())
}

fn alive(pid: &str) -> bool {
    let Ok(pid) = pid.parse::<i32>() else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only queries process existence/permission; it sends no signal.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn dispatch(socket: PathBuf, pane: String, client_pid: u32, key: String) -> Result<()> {
    Config {
        key: key.clone(),
        ..Config::default()
    }
    .validate()?;
    let tmux = Tmux::new(socket)?;
    let pane = tmux.pane(&pane)?;
    let client = tmux.client(client_pid)?;
    if client.pane != pane.id {
        return Ok(());
    }
    let mut config = match config::load() {
        Ok(config) => config,
        Err(_) => return tmux.forward(&pane, &client, &key),
    };
    // Use tmux's installed spelling (for example Tab for C-i). Shortcut edits
    // take effect on bind; dimensions still reload on every opening.
    if let Some(binding) = read_record(&record_path(&tmux.socket)?)?
        .filter(|binding| binding.socket == tmux.socket && binding.generation == pane.generation)
    {
        config.key = binding.key;
    }
    if pane.in_mode {
        return tmux.forward(&pane, &client, &key);
    }
    let invocation = crate::inspect::eligible(pane.pid, &pane.tty, &config.harness_paths)
        .ok()
        .filter(|decision| decision.eligible)
        .and_then(|decision| decision.invocation);
    let Some(invocation) = invocation else {
        return tmux.forward(&pane, &client, &key);
    };
    match popup(&tmux, &pane, &client, &config, invocation) {
        Ok(()) => Ok(()),
        Err(error) => {
            let detail: String = format!("pane {}: {error:#}", pane.id)
                .chars()
                .take(1024)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let _ = tmux.output(&["set-option", "-g", "@aft_last_error", &detail]);
            // Error context contains our operations, never inspected argv or terminal contents.
            tmux.message(
                &client,
                "agent-float-term: popup failed; run doctor (shells are preserved)",
            );
            eprintln!("agent-float-term: {error:#}");
            Ok(())
        }
    }
}

fn popup(
    tmux: &Tmux,
    pane: &Pane,
    client: &Client,
    config: &Config,
    invocation: Invocation,
) -> Result<()> {
    validate_popup_client(tmux, pane)?;
    let generation = &pane.generation;
    let lock_path = lifecycle::parent_lock(generation, &pane.id)?;
    let guard = lock(&lock_path)?;
    let listing = tmux.output(&[
        "list-sessions",
        "-F",
        "#{session_id}|#{@aft_owner}|#{@aft_float_generation}|#{session_attached}|#{@aft_worker}|#{window_id}|#{pane_id}|#{window_linked}|#{@aft_invocation}|#{@aft_instance}",
    ])?;
    let owned: Vec<_> = listing
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split('|').collect();
            (fields.len() == 10 && fields[1] == pane.id && fields[2] == generation)
                .then_some(fields)
        })
        .collect();
    if let Some(instance) = env::var_os("AFT_RESTORE_INSTANCE") {
        let Some(instance) = instance.to_str().filter(|value| valid_token(value)) else {
            return Ok(());
        };
        let Some(fields) = owned.iter().find(|fields| fields[9] == instance) else {
            return Ok(());
        };
        if !lifecycle::restore_allowed(tmux, fields[0], instance, invocation, client)? {
            return Ok(());
        }
    }
    // Old identity-less sessions are retained, never adopted or automatically killed.
    for fields in &owned {
        if let Ok(old) = serde_json::from_str::<Invocation>(fields[8]) {
            if old != invocation && old.liveness() == Liveness::Exited && valid_token(fields[9]) {
                lifecycle::kill_owned(tmux, fields[0], generation, &pane.id, fields[9], old)?;
            }
        }
    }
    let matching: Vec<_> = owned
        .iter()
        .filter(|fields| {
            serde_json::from_str::<Invocation>(fields[8]).ok() == Some(invocation)
                && valid_token(fields[9])
        })
        .collect();
    ensure!(
        matching.len() <= 1,
        "multiple sessions claim this pane; inspect sessions before continuing"
    );
    let (target, window, float_pane, instance) = if let Some(fields) = matching.first() {
        if fields[3] != "0" || alive(fields[4]) {
            tmux.message(
                client,
                "Harness Floating Terminal is already open or opening in another client",
            );
            return Ok(());
        }
        ensure!(
            fields[7] == "0" && lifecycle::exclusive_windows(tmux, fields[0])?,
            "owned floating window is linked to another session; left untouched"
        );
        (
            fields[0].to_owned(),
            fields[5].to_owned(),
            fields[6].to_owned(),
            fields[9].to_owned(),
        )
    } else {
        let cwd = text(&pane.cwd)?;
        ensure!(
            Path::new(&cwd).is_dir(),
            "parent pane directory is unavailable"
        );
        let shell = match &config.shell {
            Some(shell) => shell.clone(),
            None => pane.default_shell.clone(),
        };
        checked_path(&shell)?;
        tmux.popup_policy(generation)?;
        let instance = token()?;
        let session = format!("aft-{}-{}-{instance}", &generation[..12], &pane.id[1..]);
        let target = tmux.output(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id}",
            "-s",
            &session,
            "-c",
            &literal_format(cwd),
            "-e",
            "AFT_DISABLE=1",
            "-e",
            &format!("AFT_TMUX_BINARY={}", text(&tmux.binary)?),
            text(&shell)?,
            "-i",
        ])?;
        let metadata = tmux.output(&[
            "set-option",
            "-t",
            &target,
            OWNER,
            &pane.id,
            ";",
            "set-option",
            "-t",
            &target,
            FLOAT_GENERATION,
            generation,
            ";",
            "set-option",
            "-t",
            &target,
            "@aft_instance",
            &instance,
            ";",
            "set-option",
            "-t",
            &target,
            "@aft_invocation",
            &serde_json::to_string(&invocation)?,
            ";",
            "set-option",
            "-t",
            &target,
            "destroy-unattached",
            "off",
            ";",
            "set-option",
            "-t",
            &target,
            "status",
            "off",
            ";",
            "display-message",
            "-p",
            "-t",
            &target,
            "#{window_id}|#{pane_id}|#{window_linked}",
        ])?;
        let fields: Vec<_> = metadata.split('|').collect();
        ensure!(
            fields.len() == 3 && fields[2] == "0",
            "new floating window is linked elsewhere or unavailable"
        );
        (target, fields[0].to_owned(), fields[1].to_owned(), instance)
    };
    lifecycle::start_watcher(tmux, &target, &instance)?;
    if !tmux.popup_client_unchanged(client, pane)? {
        return Ok(());
    }
    // Revalidate after session creation, which can run a shell's startup files.
    if !crate::inspect::eligible(pane.pid, &pane.tty, &config.harness_paths)
        .is_ok_and(|d| d.eligible && d.invocation == Some(invocation))
    {
        return Ok(());
    }
    if env::var_os("AFT_RESTORE_INSTANCE").is_some()
        && !lifecycle::restore_allowed(tmux, &target, &instance, invocation, client)?
    {
        return Ok(());
    }
    let worker = std::process::id().to_string();
    // Keep the safety checks at the mutation boundary, not just in an earlier
    // snapshot. All style changes are local to this exclusively owned window/pane.
    let condition = [
        lifecycle::ownership_condition(generation, &pane.id, &instance, invocation)?,
        "#{==:#{session_grouped},0}".into(),
        "#{==:#{m:*1*,#{W:#{window_linked}}},0}".into(),
        "#{==:#{session_attached},0}".into(),
        "#{==:#{window_linked},0}".into(),
        "#{==:#{exit-unattached},0}".into(),
        format!("#{{==:#{{window_id}},{window}}}"),
        format!("#{{==:#{{pane_id}},{float_pane}}}"),
    ]
    .into_iter()
    .reduce(|left, right| format!("#{{&&:{left},{right}}}"))
    .context("missing claim condition")?;
    let body = format!(
        "set-option -t {target} detach-on-destroy on ; \
         set-option -w -t {window} remain-on-exit off ; \
         set-option -w -t {window} window-style {style} ; \
         set-option -w -t {window} window-active-style {style} ; \
         set-option -p -t {float_pane} window-style {style} ; \
         set-option -p -t {float_pane} window-active-style {style} ; \
         set-option -t {target} @aft_worker {worker} ; \
         set-option -t {target} @aft_viewer '' ; \
         set-option -t {target} @aft_visible 1 ; \
         set-option -t {target} @aft_routing 0 ; \
         set-option -t {target} @aft_origin_client {origin_pid} ; \
         set-option -t {target} @aft_origin_name {origin_name} ; \
         set-option -t {target} @aft_origin_session {origin_session} ; \
         set-option -g @aft_last_error '' ; display-message -p AFT_READY",
        target = tmux_quote(&target),
        window = tmux_quote(&window),
        float_pane = tmux_quote(&float_pane),
        style = tmux_quote(TERMINAL_STYLE),
        origin_pid = client.pid,
        origin_name = tmux_quote(&client.name),
        origin_session = tmux_quote(&client.session),
    );
    let reply = tmux.source_result(&format!(
        "if-shell -F -t {} {} {} {}\n",
        tmux_quote(&target),
        tmux_quote(&condition),
        tmux_quote(&body),
        tmux_quote("display-message -p AFT_CHANGED")
    ))?;
    ensure!(
        reply == "AFT_READY",
        "floating session changed, is linked, or is already attached; left untouched"
    );
    let result = (|| -> Result<std::process::ExitStatus> {
        let command = lifecycle::viewer_command(tmux, &target, &instance, &worker)?;
        routing::prepare(tmux, pane, client, &target, config)?;
        drop(guard);
        // This client waits only while the popup is visible. Detaching the inner client
        // exits its command, which closes -E without terminating the retained shell.
        Ok(tmux
            .command()
            .args([
                "display-popup",
                "-E",
                "-s",
                "fg=terminal,bg=terminal",
                "-S",
                "fg=terminal,bg=terminal",
                "-c",
                &client.name,
                "-t",
                &pane.id,
                "-w",
                &format!("{}%", config.width),
                "-h",
                &format!("{}%", config.height),
                "-x",
                "C",
                "-y",
                "C",
                &command,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()?)
    })();
    // A single in-server conditional clears only this worker's claim. There is no
    // read/clear round trip and a late cleanup cannot erase a newer viewer claim.
    let _guard = lock(&lock_path).ok();
    let closed_by_route = tmux.output(&[
        "if-shell",
        "-F",
        "-t",
        &target,
        &format!("#{{&&:#{{==:#{{@aft_instance}},{instance}}},#{{==:#{{@aft_worker}},{worker}}}}}"),
        &format!(
            "display-message -p -t {target} '#{{@aft_routing}}' ; set-option -t {target} @aft_worker '' ; set-option -t {target} @aft_viewer ''",
            target = tmux_quote(&target)
        ),
    ]).is_ok_and(|reason| reason == "1");
    routing::finished(tmux, &target, &worker);
    ensure!(
        result?.success() || closed_by_route || invocation.liveness() == Liveness::Exited,
        "tmux popup command failed"
    );
    Ok(())
}

#[derive(Debug)]
struct Float {
    name: String,
    id: String,
    owner: String,
    attached: u32,
    orphan: bool,
}

fn owned_sessions(tmux: &Tmux) -> Result<Vec<Float>> {
    let generation = tmux.generation()?;
    let all = tmux.output(&[
        "list-sessions",
        "-F",
        "#{session_id}|#{@aft_owner}|#{@aft_float_generation}|#{session_attached}",
    ])?;
    let mut result = Vec::new();
    for line in all.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() != 4 || fields[2] != generation || !valid_pane(fields[1]) {
            continue;
        }
        // Names are only presentation; metadata and the exact server establish ownership.
        result.push(Float {
            name: tmux.output(&["display-message", "-p", "-t", fields[0], "#{session_name}"])?,
            id: fields[0].into(),
            owner: fields[1].into(),
            attached: fields[3].parse()?,
            orphan: tmux.pane(fields[1]).is_err(),
        });
    }
    Ok(result)
}

pub fn sessions(socket: Option<PathBuf>) -> Result<()> {
    let tmux = Tmux::resolve(socket)?;
    let floats = owned_sessions(&tmux)?;
    if floats.is_empty() {
        println!("No owned floating shells on {}", tmux.socket.display());
    }
    for float in floats {
        println!(
            "{}\towner={}\tviewers={}\t{}",
            float.name,
            float.owner,
            float.attached,
            if float.orphan { "orphan" } else { "retained" }
        );
    }
    Ok(())
}

pub fn cleanup(socket: Option<PathBuf>, session: Option<String>, yes: bool) -> Result<()> {
    let tmux = Tmux::resolve(socket)?;
    let generation = tmux.generation()?;
    let floats = owned_sessions(&tmux)?;
    if let Some(name) = &session {
        ensure!(
            floats.iter().any(|f| &f.name == name && f.orphan),
            "no owned orphan session matches {name}"
        );
    }
    for float in floats
        .into_iter()
        .filter(|f| f.orphan && session.as_ref().is_none_or(|name| name == &f.name))
    {
        println!(
            "{} {} (all jobs in this shell will terminate)",
            if yes { "Remove" } else { "Would remove" },
            float.name
        );
        if !yes {
            continue;
        }
        let lock_path = lifecycle::parent_lock(&generation, &float.owner)?;
        let _guard = lock(&lock_path)?;
        let still_owned = owned_sessions(&tmux)?
            .into_iter()
            .any(|f| f.id == float.id && f.orphan && f.attached == 0);
        ensure!(
            still_owned
                && tmux.generation()? == generation
                && lifecycle::exclusive_windows(&tmux, &float.id)?,
            "session changed, is attached, grouped, or linked; left untouched"
        );
        let condition = [
            format!("#{{==:#{{@aft_float_generation}},{generation}}}"),
            format!("#{{==:#{{@aft_owner}},{}}}", float.owner),
            "#{==:#{session_attached},0}".into(),
            "#{==:#{session_grouped},0}".into(),
            "#{==:#{m:*1*,#{W:#{window_linked}}},0}".into(),
        ]
        .into_iter()
        .reduce(|left, right| format!("#{{&&:{left},{right}}}"))
        .unwrap();
        tmux.output(&[
            "if-shell",
            "-F",
            "-t",
            &float.id,
            &condition,
            &format!("kill-session -t {}", tmux_quote(&float.id)),
        ])?;
    }
    if !yes {
        println!("Preview only; repeat with --yes to terminate owned orphan shells.");
    }
    Ok(())
}

pub fn doctor(socket: Option<PathBuf>) -> Result<()> {
    println!(
        "agent-float-term {} / {} {}",
        env!("CARGO_PKG_VERSION"),
        env::consts::OS,
        env::consts::ARCH
    );
    let config = config::load()?;
    println!(
        "Configuration: valid; key={}, size={}x{}%, explicit mappings={}",
        config.key,
        config.width,
        config.height,
        config.harness_paths.len()
    );
    for harness in ["claude", "codex", "opencode"] {
        println!(
            "{harness}: {}",
            if which(harness).is_ok() {
                "on PATH (not an activation guarantee)"
            } else {
                "not on PATH"
            }
        );
    }
    let tmux = Tmux::resolve(socket)?;
    tmux.compatible()?;
    println!("tmux server: {} (compatible)", tmux.socket.display());
    println!(
        "tmux client: {} ({})",
        tmux.binary.display(),
        tmux.client_version()?
    );
    let last_error = tmux.global("@aft_last_error")?;
    if !last_error.is_empty() {
        println!("Last popup failure: {last_error}");
    }
    let record = read_record(&record_path(&tmux.socket)?)?;
    let installed = match record {
        Some(record) => {
            tmux.global(GENERATION)? == record.generation
                && binding_matches(&record, tmux.key_binding(&record.key)?.as_deref())
        }
        None => false,
    };
    println!(
        "Owned key binding: {}",
        if installed {
            "intact"
        } else {
            "absent or changed; run bind"
        }
    );
    if let Ok(pane) = env::var("TMUX_PANE") {
        if let Ok(pane) = tmux.pane(&pane) {
            let decision = crate::inspect::eligible(pane.pid, &pane.tty, &config.harness_paths);
            println!(
                "Current pane: {}",
                match decision {
                    Ok(decision) => decision.reason,
                    Err(_) => "inspection unavailable or ambiguous; key passes through",
                }
            );
        }
    }
    println!("Diagnostics contain no process arguments, environments, or terminal contents.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_floor_and_identifiers() {
        for value in ["3.4", "3.7c", "4.0"] {
            assert!(supported_version(value));
        }
        for value in ["3.2a", "3.3", "3.3a", "3.3z", "2.9", "unknown", "next-3.4"] {
            assert!(!supported_version(value));
        }
        assert!(valid_pane("%123"));
        for value in ["%", "%1;kill-server", "main:1", "%1\n"] {
            assert!(!valid_pane(value));
        }
        assert!(valid_token(&token().unwrap()));
    }

    #[test]
    fn format_values_are_literal() {
        assert_eq!(
            literal_format("/a/#{pane_id}/#(touch bad)"),
            "/a/##{pane_id}/##(touch bad)"
        );
        assert_eq!(tmux_quote("a\"b\\c$d"), "\"a\\\"b\\\\c\\$d\"");
    }

    #[test]
    fn interrupted_rebind_retains_both_owned_states() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binding.json");
        let mut binding = Binding {
            socket: "/tmp/owned.sock".into(),
            binary: Some("/usr/bin/tmux".into()),
            binary_stamp: None,
            server_version: None,
            generation: token().unwrap(),
            key: "M-C-@".into(),
            installed: "new helper".into(),
            prior_installed: Some("old helper".into()),
            previous: Some("original user binding".into()),
        };
        write_record(&path, &binding).unwrap();
        let pending = read_record(&path).unwrap().unwrap();
        assert!(binding_matches(&pending, Some("old helper")));
        assert!(binding_matches(&pending, Some("new helper")));
        assert!(!binding_matches(&pending, Some("later user binding")));
        assert_eq!(pending.previous.as_deref(), Some("original user binding"));
        binding.prior_installed = None;
        write_record(&path, &binding).unwrap();
        assert!(!binding_matches(
            &read_record(&path).unwrap().unwrap(),
            Some("old helper")
        ));
    }

    #[test]
    fn socket_directory_allows_sticky_temp_without_relaxing_installer() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o1777)).unwrap();
        let socket_dir = directory.path().join("socket-dir");
        socket_directory(&socket_dir).unwrap();
        assert_eq!(fs::metadata(&socket_dir).unwrap().mode() & 0o777, 0o700);
        assert!(private_dir(&directory.path().join("installer-dir")).is_err());
        symlink(&socket_dir, directory.path().join("symlink")).unwrap();
        assert!(socket_directory(&directory.path().join("symlink")).is_err());
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(socket_directory(&directory.path().join("unsafe")).is_err());
    }
}
