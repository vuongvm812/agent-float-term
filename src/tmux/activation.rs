//! Bounded, non-creating discovery and client selection for live-server activation.
use super::*;
use std::cell::Cell;
use std::collections::HashSet;

const MAX_ENTRIES: usize = 256;
const MAX_SERVERS: usize = 32;
thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub(super) fn check_deadline() -> Result<()> {
    ensure!(DEADLINE.get().is_none_or(|end| Instant::now() < end),
        "bind --all exceeded its 60-second budget; retry or use bind --socket for remaining servers");
    Ok(())
}

// Walk lexical parents rather than canonicalizing away an untrusted symlink.
// macOS's root-owned /tmp and /var aliases are the only exceptions.
fn directory(path: &Path, private: bool) -> Result<bool> {
    checked_path(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let mut metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("inspect discovery directory"),
        };
        if metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && matches!(current.to_str(), Some("/tmp" | "/var"))
        {
            current = current.canonicalize()?;
            metadata = fs::symlink_metadata(&current)?;
        }
        let owned = metadata.uid() == config::uid();
        let sticky = metadata.mode() & 0o1000 != 0;
        ensure!(
            metadata.is_dir()
                && (owned || metadata.uid() == 0)
                && (metadata.mode() & 0o022 == 0 || sticky),
            "unsafe directory {} (ownership, permissions, or symlink)",
            current.display()
        );
    }
    let metadata = fs::metadata(path)?;
    let sticky = !private && metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
    ensure!(
        sticky
            || (metadata.uid() == config::uid()
                && metadata.mode() & if private { 0o077 } else { 0o022 } == 0),
        "unsafe owned directory {}",
        path.display()
    );
    Ok(true)
}

pub(super) fn socket_identity(path: &Path) -> Result<Option<(u64, u64)>> {
    checked_path(path)?;
    if !directory(path.parent().context("socket parent")?, false)? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect discovered socket"),
    };
    ensure!(
        metadata.file_type().is_socket() && metadata.uid() == config::uid(),
        "unsafe socket {} (not an owned socket or is a symlink)",
        path.display()
    );
    Ok(Some((metadata.dev(), metadata.ino())))
}

pub(super) fn recorded_client(tmux: &Tmux) -> Result<Option<Tmux>> {
    let path = record_path(&tmux.socket)?;
    if !directory(path.parent().context("record parent")?, true)? {
        return Ok(None);
    }
    let Some(record) = read_record(&path)? else {
        return Ok(None);
    };
    if record.socket != tmux.socket {
        return Ok(None);
    }
    let Some(binary) = record.binary else {
        return Ok(None);
    };
    let Some(stamp) = record.binary_stamp else {
        return Ok(None);
    };
    // The private record is approval for this exact executable, not a replaced file.
    if binary_stamp(&binary).ok().as_ref() != Some(&stamp) {
        return Ok(None);
    }
    let candidate = Tmux {
        binary,
        socket: tmux.socket.clone(),
    };
    let verified = (|| -> Result<bool> {
        let version = candidate.client_version()?;
        Ok(supported_version(&version)
            && candidate.output(&["display-message", "-p", "#{version}"])? == version
            && record.server_version.as_deref() == Some(version.as_str())
            && candidate.global(GENERATION)? == record.generation
            && binary_stamp(&candidate.binary)? == stamp)
    })();
    Ok(verified.unwrap_or(false).then_some(candidate))
}

pub(super) fn cached_client(tmux: &Tmux, version: &str) -> Result<Option<Tmux>> {
    if !supported_version(version)
        || version.len() > 64
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.')
    {
        return Ok(None);
    }
    let mut parent = Paths::discover()?.data;
    for part in ["", "compat", &format!("tmux-{version}"), "bin"] {
        parent.push(part);
        if !directory(&parent, part.is_empty()).context(
            "no compatible client: unsafe preserved tmux cache; use AFT_TMUX_BINARY and run bind",
        )? {
            return Ok(None);
        }
        let metadata = fs::metadata(&parent)?;
        ensure!(metadata.uid() == config::uid() && metadata.mode() & 0o022 == 0,
            "no compatible client: preserved tmux cache must be user-owned and not group/world writable");
    }
    let binary = parent.join("tmux");
    let metadata = match fs::symlink_metadata(&binary) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("no compatible client: inspect preserved tmux"),
    };
    ensure!(metadata.is_file() && metadata.uid() == config::uid()
        && metadata.mode() & 0o022 == 0 && metadata.mode() & 0o100 != 0,
        "no compatible client: preserved tmux must be an owned safe regular executable, not a symlink; use AFT_TMUX_BINARY and run bind");
    let stamp = binary_stamp(&binary)?;
    let candidate = Tmux {
        binary,
        socket: tmux.socket.clone(),
    };
    let verified = (|| -> Result<bool> {
        Ok(candidate.client_version()? == version
            && candidate.output(&["display-message", "-p", "#{version}"])? == version
            && binary_stamp(&candidate.binary)? == stamp)
    })()
    .context(
        "no compatible client: cannot verify preserved tmux; use AFT_TMUX_BINARY and run bind",
    )?;
    ensure!(verified,
        "no compatible client: preserved tmux version or executable identity changed; use AFT_TMUX_BINARY and run bind");
    Ok(Some(candidate))
}

/// Hot-reload only existing owned servers, preserving conflicting root bindings.
/// Discovery never creates directories, removes sockets, or starts a server.
pub fn bind_all() -> Result<()> {
    struct Budget(Option<Instant>);
    impl Drop for Budget {
        fn drop(&mut self) {
            DEADLINE.set(self.0);
        }
    }
    let _budget = Budget(DEADLINE.replace(Some(Instant::now() + Duration::from_secs(60))));
    let paths = Paths::discover()?;
    let mut errors = Vec::new();
    let mut sockets = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |socket: PathBuf| -> Result<()> {
        check_deadline()?;
        if let Some(identity) = socket_identity(&socket)? {
            if seen.insert(identity) {
                ensure!(
                    sockets.len() < MAX_SERVERS,
                    "more than {MAX_SERVERS} sockets; use bind --socket for remaining servers"
                );
                sockets.push((socket, identity));
            }
        }
        Ok(())
    };
    // Registered spellings come first so aliases retain their restoration journal.
    let runtime = paths.state.join("runtime");
    let records = (|| -> Result<()> {
        if !directory(&runtime, true)? {
            return Ok(());
        }
        for (index, entry) in fs::read_dir(&runtime)?.enumerate() {
            ensure!(
                index < MAX_ENTRIES,
                "runtime listing exceeds {MAX_ENTRIES} entries"
            );
            check_deadline()?;
            let path = entry?.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let result = (|| -> Result<()> {
                let Some(record) = read_record(&path)? else {
                    return Ok(());
                };
                ensure!(
                    record_path(&record.socket)? == path,
                    "binding record filename does not match socket hash"
                );
                add(record.socket)
            })();
            if let Err(error) = result {
                errors.push(format!("{}: {error:#}", path.display()));
            }
        }
        Ok(())
    })();
    if let Err(error) = records {
        errors.push(format!("{}: {error:#}", runtime.display()));
    }
    if let Some(value) = env::var_os("TMUX").filter(|v| !v.is_empty()) {
        let result = (|| -> Result<()> {
            let value = value.to_str().context("TMUX must be UTF-8")?;
            let socket = value
                .rsplitn(3, ',')
                .nth(2)
                .context("invalid TMUX environment")?;
            add(PathBuf::from(socket))
        })();
        if let Err(error) = result {
            errors.push(format!("TMUX: {error:#}"));
        }
    }
    let tmp = env::var_os("TMUX_TMPDIR").filter(|value| !value.is_empty());
    let xdg = env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty());
    let envtmp = env::var_os("TMPDIR").filter(|value| !value.is_empty());
    let runtime_override = tmp.is_some() || xdg.is_some() || envtmp.is_some();
    let uid = config::uid();
    let mut roots = vec![
        PathBuf::from(tmp.as_deref().unwrap_or(OsStr::new("/tmp"))).join(format!("tmux-{uid}"))
    ];
    if let Some(base) = &xdg {
        roots.push(PathBuf::from(base).join(format!("tmux-{uid}")));
    }
    for root in roots {
        let result = (|| -> Result<()> {
            if !directory(&root, true)? {
                return Ok(());
            }
            for (index, entry) in fs::read_dir(&root)?.enumerate() {
                ensure!(
                    index < MAX_ENTRIES,
                    "socket listing exceeds {MAX_ENTRIES} entries"
                );
                check_deadline()?;
                let path = entry?.path();
                if let Err(error) = add(path.clone()) {
                    errors.push(format!("{}: {error:#}", path.display()));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("{}: {error:#}", root.display()));
        }
    }
    // Explicit runtime overrides scope even dedicated discovery: never add a
    // default /tmp fallback alongside an explicit TMUX_TMPDIR/XDG/TMPDIR.
    let base = xdg
        .or(envtmp)
        .or(tmp)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let path = env::temp_dir();
            if path.is_absolute() {
                path
            } else {
                PathBuf::from("/tmp")
            }
        });
    let mut dedicated = vec![base.join(format!("agent-float-term-{uid}/tmux.sock"))];
    if !runtime_override {
        dedicated.push(PathBuf::from(format!(
            "/tmp/agent-float-term-{uid}/tmux.sock"
        )));
    }
    for socket in dedicated {
        if let Err(error) = add(socket.clone()) {
            errors.push(format!("{}: {error:#}", socket.display()));
        }
    }
    let mut activated = 0;
    for (socket, identity) in sockets {
        let result = (|| -> Result<()> {
            check_deadline()?;
            ensure!(
                socket_identity(&socket)? == Some(identity),
                "socket changed during discovery"
            );
            let tmux = Tmux::new(socket.clone())?.matching_client()?;
            bind_selected(tmux, config::load()?, false, Some(identity))
        })();
        if let Err(error) = result {
            errors.push(format!("{}: {error:#}", socket.display()));
        } else {
            activated += 1;
        }
    }
    let errors: Vec<String> = errors
        .into_iter()
        .map(|error| {
            error
                .chars()
                .take(2048)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect()
        })
        .collect();
    println!(
        "Activated {activated} existing tmux server(s); no servers were started or restarted."
    );
    ensure!(errors.is_empty(), "some tmux servers were not activated; successful bindings and existing sessions were preserved. Resolve conflicts/unsafe paths or select a matching AFT_TMUX_BINARY, then retry bind --all or bind --socket PATH:\n{}", errors.join("\n"));
    Ok(())
}
