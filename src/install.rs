//! Explicit, user-local installation. Install/uninstall without `yes` are read-only previews.
//! `yes` authorizes payload/templates, not implicit edits to shell or tmux user configuration.
//! Each user integration requires its own explicit configuration path; omitted integrations
//! are retained on reinstall, except exact owned references migrated to a new layout.
//! Run an extracted binary's `install --yes` directly, rather
//! than placing an unowned regular file at the managed bin-symlink destination first.
//! Updates accept local native executables only; no downloads or version probes are performed.

#[path = "install/fsops.rs"]
mod fsops;

use crate::config::{checked_path, ensure_user_dir, private_dir, Paths};
use anyhow::{bail, Context, Result};
use fsops::{lock, regular, snapshot, Content, Snapshot, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const BEGIN: &str = "# BEGIN agent-float-term managed v1";
const END: &str = "# END agent-float-term managed v1";
const BINARY: &str = "agent-float-term";

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Opt into editing this tmux configuration. None never selects a default user file.
    pub tmux_config: Option<PathBuf>,
    /// Independently opt into editing this shell configuration; does not imply tmux integration.
    pub shell_config: Option<PathBuf>,
    /// Bash or zsh. Login-shell inference is used only when shell_config is supplied.
    pub shell_kind: Option<String>,
    /// Apply the previewed plan. This does not opt into either user configuration integration.
    pub yes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedText {
    path: PathBuf,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    paths: Paths,
    shell_kind: String,
    current: Option<String>,
    previous: Option<String>,
    releases: Vec<String>,
    files: Vec<OwnedText>,
    blocks: Vec<OwnedText>,
}

fn manifest(paths: &Paths) -> Result<(Snapshot, Option<Manifest>)> {
    let before = snapshot(&paths.state.join("install.json"))?;
    let value = regular(&before)?
        .map(serde_json::from_slice::<Manifest>)
        .transpose()
        .context("invalid installation manifest")?;
    if let Some(value) = &value {
        if !matches!(value.format, 1 | 2) || value.paths != *paths {
            bail!("installation manifest version or directory roots do not match");
        }
        if !matches!(value.shell_kind.as_str(), "bash" | "zsh")
            || value.releases.iter().any(|s| !valid_digest(s))
            || value
                .current
                .iter()
                .chain(value.previous.iter())
                .any(|s| !value.releases.contains(s))
            || value.files.len() > 2
            || value.blocks.len() > 2
        {
            bail!("invalid installation manifest entries");
        }
        let root = if value.format == 1 {
            &paths.config
        } else {
            &paths.data
        };
        let mut names = std::collections::HashSet::new();
        for file in &value.files {
            if (file.path != root.join("integration.tmux")
                && file.path != root.join("integration.sh"))
                || !names.insert(file.path.file_name())
            {
                bail!("manifest contains an unexpected owned file");
            }
        }
        let mut kinds = std::collections::HashSet::new();
        let mut targets = std::collections::HashSet::new();
        for block in &value.blocks {
            user_target(paths, &block.path)?;
            block_range(block.text.as_bytes(), &block.text)?;
            if !kinds.insert(block_kind(block)?) || !targets.insert(&block.path) {
                bail!("manifest contains duplicate integration kinds or targets");
            }
        }
    }
    Ok((before, value))
}

fn valid_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// POSIX shell word quoting, also usable by other integration modules.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// Both callers explicitly enable tmux format expansion. Protect literal '#' first,
// then quote for tmux's command parser (which is not the shell parser).
fn tmux_quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('#', "##")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
    )
}

fn glob_quote(text: &str) -> String {
    let mut quoted = String::new();
    for c in text.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | '{' | '}') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted
}

fn native_binary(path: &Path) -> Result<Vec<u8>> {
    checked_path(path)?;
    let file = snapshot(path)?;
    let Content::File(bytes, mode) = file.content else {
        bail!("update/install source must be a regular file, not a symlink");
    };
    if mode & 0o111 == 0 || mode & 0o6000 != 0 {
        bail!("source must be executable and must not be setuid/setgid");
    }
    #[cfg(target_os = "linux")]
    let native = bytes.len() >= 64
        && bytes.starts_with(b"\x7fELF")
        && matches!(bytes[4], 1 | 2)
        && matches!(bytes[5], 1 | 2)
        && bytes[6] == 1;
    #[cfg(target_os = "macos")]
    let native = bytes.len() >= 32
        && matches!(
            &bytes[..4],
            b"\xfe\xed\xfa\xce"
                | b"\xce\xfa\xed\xfe"
                | b"\xfe\xed\xfa\xcf"
                | b"\xcf\xfa\xed\xfe"
                | b"\xca\xfe\xba\xbe"
                | b"\xbe\xba\xfe\xca"
                | b"\xca\xfe\xba\xbf"
                | b"\xbf\xba\xfe\xca"
        );
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let native = false;
    if !native {
        bail!("source is not a recognizable native ELF/Mach-O executable for this OS");
    }
    Ok(bytes)
}

fn absolute(path: PathBuf) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    checked_path(&path)?;
    Ok(path)
}

fn user_target(paths: &Paths, path: &Path) -> Result<()> {
    checked_path(path)?;
    if [&paths.config, &paths.data, &paths.state, &paths.bin]
        .iter()
        .any(|root| path.starts_with(root))
    {
        bail!(
            "user configuration cannot overlap installer-owned directories: {}",
            path.display()
        );
    }
    Ok(())
}

fn integration(
    paths: &Paths,
    options: &InstallOptions,
) -> Result<(String, Vec<OwnedText>, Vec<OwnedText>)> {
    let kind = options.shell_kind.clone().unwrap_or_else(|| {
        options
            .shell_config
            .as_ref()
            .and_then(|_| env::var_os("SHELL"))
            .and_then(|s| {
                Path::new(&s)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "bash".into())
    });
    if !matches!(kind.as_str(), "bash" | "zsh") {
        bail!("automatic shell integration supports bash and zsh only; specify --shell-kind");
    }
    let tmux_path = options.tmux_config.clone().map(absolute).transpose()?;
    let shell_path = options.shell_config.clone().map(absolute).transpose()?;
    for path in tmux_path.iter().chain(shell_path.iter()) {
        user_target(paths, path)?;
    }
    if tmux_path.is_some() && tmux_path == shell_path {
        bail!("tmux and shell configuration must be different files");
    }
    for path in [&paths.config, &paths.data, &paths.state, &paths.bin] {
        checked_path(path)?;
    }
    let binary = shell_quote(
        paths
            .bin
            .join(BINARY)
            .to_str()
            .context("non-UTF-8 binary path")?,
    );
    let tmux_file = paths.data.join("integration.tmux");
    let shell_file = paths.data.join("integration.sh");
    // Setness matters: even `-ic ''` is a tool command, not a human prompt.
    let guards = format!(
        r#"[ -z "${{BASH_EXECUTION_STRING+x}}" ] && [ -z "${{ZSH_EXECUTION_STRING+x}}" ] &&
       [ -z "${{ZSH_SCRIPT+x}}" ] &&
       [ -z "${{SSH_CONNECTION-}}" ] && [ -z "${{SSH_CLIENT-}}" ] && [ -z "${{SSH_TTY-}}" ] &&
       [ -z "${{AFT_DISABLE-}}" ] && [ -z "${{AFT_STARTING-}}" ] &&
       [ -z "${{_AFT_AUTO_STARTED-}}" ] && [ -x {binary} ]"#
    );
    let start = format!(
        r#"if {guards} && [ -t 0 ] && [ -t 1 ]; then
      _AFT_AUTO_STARTED=1
      AFT_STARTING=1 AFT_QUIET=1 {binary} start || printf '%s\n' 'agent-float-term: startup failed; continuing this shell' >&2
    fi"#
    );
    let initialize = if kind == "zsh" {
        format!(
            r#"if {guards} && (( ! ${{+functions[__aft_initialize]}} )); then
    __aft_initialize() {{
      emulate -L zsh
      # Instant-prompt cleanup normally precedes us; allow just one late cleanup.
      if [[ -o interactive ]] && {guards} &&
         {{ [ ! -t 0 ] || [ ! -t 1 ]; }} && [ -z "${{_AFT_INIT_RETRIED-}}" ]; then
        _AFT_INIT_RETRIED=1
        return 0
      fi
      precmd_functions=("${{precmd_functions[@]:#__aft_initialize}}")
      unset _AFT_INIT_RETRIED
      unfunction __aft_initialize
      {start}
      return 0
    }}
    typeset -ga precmd_functions
    precmd_functions+=(__aft_initialize)
  fi"#
        )
    } else {
        start
    };
    let files = vec![
        OwnedText {
            path: tmux_file.clone(),
            text: format!("# Generated by agent-float-term; local bind refuses key collisions.\nrun-shell {}\n", tmux_quote(&format!("{binary} bind"))),
        },
        OwnedText {
            path: shell_file.clone(),
            text: format!("# Generated by agent-float-term for {kind}.\ncase $- in\n  *i*)\n  {initialize}\n  ;;\nesac\n"),
        },
    ];
    let mut blocks = Vec::new();
    if let Some(path) = tmux_path {
        blocks.push(OwnedText {
            path,
            text: format!(
                "{BEGIN}\nsource-file -q -F {}\n{END}\n",
                tmux_quote(&glob_quote(
                    tmux_file.to_str().context("non-UTF-8 integration path")?
                ))
            ),
        });
    }
    if let Some(path) = shell_path {
        blocks.push(OwnedText {
            path,
            text: format!(
                "{BEGIN}\nif [ -r {0} ]; then . {0}; fi\n{END}\n",
                shell_quote(shell_file.to_str().context("non-UTF-8 integration path")?)
            ),
        });
    }
    Ok((kind, files, blocks))
}

// Version-1 manifests already persist the exact generated text, but not an integration
// kind. Identify that existing format without changing the persisted schema.
fn block_kind(block: &OwnedText) -> Result<&'static str> {
    let body = block
        .text
        .trim_start_matches('\n')
        .strip_prefix(BEGIN)
        .and_then(|text| text.strip_prefix('\n'))
        .context("invalid recorded integration block")?;
    if body.starts_with("source-file ") {
        Ok("tmux")
    } else if body.starts_with("if [ -r ") {
        Ok("shell")
    } else {
        bail!("unknown recorded integration block kind");
    }
}

fn occurrences(bytes: &[u8], needle: &[u8]) -> Vec<usize> {
    bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(i, s)| (s == needle).then_some(i))
        .collect()
}

fn block_range(bytes: &[u8], expected: &str) -> Result<std::ops::Range<usize>> {
    if expected.is_empty() || !expected.contains(BEGIN) || !expected.contains(END) {
        bail!("invalid recorded managed block");
    }
    let begins = occurrences(bytes, b"# BEGIN agent-float-term");
    let ends = occurrences(bytes, b"# END agent-float-term");
    let exact = occurrences(bytes, expected.as_bytes());
    if begins.len() != 1 || ends.len() != 1 || exact.len() != 1 || begins[0] >= ends[0] {
        bail!("managed block is missing, malformed, duplicated, or user-edited; refusing to overwrite it");
    }
    let start = exact[0];
    let marker = begins[0];
    if (marker > 0 && bytes[marker - 1] != b'\n') || !expected.ends_with('\n') {
        bail!("managed markers must occupy complete lines");
    }
    Ok(start..start + expected.len())
}

fn append_block(bytes: &[u8], block: &mut OwnedText) -> Result<Vec<u8>> {
    if !occurrences(bytes, b"# BEGIN agent-float-term").is_empty()
        || !occurrences(bytes, b"# END agent-float-term").is_empty()
    {
        bail!("unowned or malformed agent-float-term markers; manual review is required");
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        // Include the separator in our ownership record so uninstall restores the exact bytes.
        block.text.insert(0, '\n');
    }
    let mut result = bytes.to_vec();
    result.extend_from_slice(block.text.as_bytes());
    Ok(result)
}

fn file_content(before: &Snapshot, bytes: Vec<u8>) -> Content {
    let mode = match before.content {
        Content::File(_, mode) => mode,
        _ => 0o600,
    };
    Content::File(bytes, mode)
}

fn owned_link(path: &Path, target: Option<&Path>) -> Result<Snapshot> {
    let before = snapshot(path)?;
    match (&before.content, target) {
        (Content::Missing, None) => (),
        (Content::Link(actual), Some(expected)) if actual == expected => (),
        _ => bail!(
            "refusing missing, changed, or unowned installation pointer: {}",
            path.display()
        ),
    }
    Ok(before)
}

fn release_binary(paths: &Paths, id: &str) -> PathBuf {
    paths.data.join("releases").join(id).join(BINARY)
}

fn release_target(id: &str) -> PathBuf {
    PathBuf::from("releases").join(id)
}

fn verify_release(paths: &Paths, id: &str) -> Result<()> {
    if !valid_digest(id) {
        bail!("invalid release digest");
    }
    fs::symlink_metadata(paths.data.join("releases").join(id))
        .context("release directory is missing")?;
    private_dir(&paths.data.join("releases"))?;
    private_dir(&paths.data.join("releases").join(id))?;
    let bytes = native_binary(&release_binary(paths, id))?;
    if digest(&bytes) != id {
        bail!("installed release was modified: {id}");
    }
    Ok(())
}

fn write_manifest(
    tx: &mut Transaction,
    paths: &Paths,
    before: &Snapshot,
    value: &Manifest,
) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    tx.change(
        &paths.state.join("install.json"),
        before,
        Content::File(bytes, 0o600),
    )
}

/// Print a plan and return without creating even a lock file unless `options.yes` is true.
/// Reinstall retains unselected integrations, except exact owned references that
/// must move with the generated scripts during a layout migration.
pub fn install(options: InstallOptions) -> Result<()> {
    // macOS may report the invoked symlink, including our stable installed entry
    // point. Resolve our own image; explicit update sources still reject symlinks.
    let source = env::current_exe()?
        .canonicalize()
        .context("resolve running executable")?;
    install_at(&Paths::discover()?, options, &source)
}

fn install_at(paths: &Paths, options: InstallOptions, source: &Path) -> Result<()> {
    integration(paths, &options)?;
    let _lock = if options.yes {
        Some(lock(&paths.state)?)
    } else {
        None
    };
    let (manifest_before, old) = manifest(paths)?;
    let migrating = old.as_ref().is_some_and(|value| value.format == 1);
    let mut effective = options.clone();
    if let Some(old) = &old {
        if options.shell_config.is_none() {
            effective.shell_kind = Some(old.shell_kind.clone());
        }
        if migrating {
            for installed in &old.blocks {
                match block_kind(installed)? {
                    "shell" if effective.shell_config.is_none() => {
                        effective.shell_config = Some(installed.path.clone())
                    }
                    "tmux" if effective.tmux_config.is_none() => {
                        effective.tmux_config = Some(installed.path.clone())
                    }
                    _ => (),
                }
            }
        }
    }
    let (kind, files, mut blocks) = integration(paths, &effective)?;
    println!("Install agent-float-term from {}", source.display());
    println!(
        "  Releases: {}",
        paths.data.join("releases/<sha256>").display()
    );
    println!(
        "  Stable binary: {} -> {}",
        paths.bin.join(BINARY).display(),
        paths.data.join("current").join(BINARY).display()
    );
    for file in &files {
        println!("  Owned integration: {}", file.path.display());
    }
    for block in &blocks {
        println!("  Managed block: {}", block.path.display());
    }
    if migrating {
        println!("  Migrate legacy layout: update exact recorded startup blocks and remove only intact legacy scripts.");
        for file in &old.as_ref().context("missing migration manifest")?.files {
            if !files.iter().any(|new| new.path == file.path) {
                println!(
                    "  Remove after migration (only if intact): {}",
                    file.path.display()
                );
            }
        }
    } else {
        println!("  Unselected user configurations will not be edited; existing integration blocks are retained.");
    }
    println!("  Private manifest/backups: {}", paths.state.display());
    if !options.yes {
        println!("Preview only; pass --yes to apply. No files changed.");
        return Ok(());
    }
    let bytes = native_binary(source)?;
    let id = digest(&bytes);
    if let Some(old) = &old {
        for selected in &blocks {
            for installed in &old.blocks {
                let same_kind = block_kind(selected)? == block_kind(installed)?;
                if same_kind != (selected.path == installed.path) {
                    bail!("selected integration conflicts with the recorded target {}; uninstall the old integration before changing its target", installed.path.display());
                }
            }
        }
    }
    let mut edits = Vec::new();
    let mut removals = Vec::new();
    if let Some(old) = old.as_ref().filter(|_| migrating) {
        for file in &old.files {
            if files.iter().any(|new| new.path == file.path) {
                continue;
            }
            let before = snapshot(&file.path)?;
            if regular(&before)? != Some(file.text.as_bytes()) {
                bail!(
                    "legacy integration is missing or edited; migration left it untouched: {}",
                    file.path.display()
                );
            }
            removals.push((file.path.clone(), before, Content::Missing));
        }
    }
    for file in &files {
        let before = snapshot(&file.path)?;
        let existing = regular(&before)?;
        let owned = old
            .as_ref()
            .and_then(|m| m.files.iter().find(|f| f.path == file.path));
        match (existing, owned) {
            (None, None) => (),
            (Some(bytes), Some(owned)) if bytes == owned.text.as_bytes() => (),
            _ => bail!(
                "owned integration is missing, edited, or pre-existing: {}",
                file.path.display()
            ),
        }
        edits.push((
            file.path.clone(),
            before,
            Content::File(file.text.as_bytes().to_vec(), 0o600),
        ));
    }
    for block in &mut blocks {
        let before = snapshot(&block.path)?;
        let bytes = regular(&before)?.unwrap_or_default();
        let replacement = if let Some(owned) = old
            .as_ref()
            .and_then(|m| m.blocks.iter().find(|b| b.path == block.path))
        {
            let range = block_range(bytes, &owned.text)
                .with_context(|| block.path.display().to_string())?;
            if owned.text.starts_with('\n') {
                block.text.insert(0, '\n');
            }
            let mut replacement = bytes.to_vec();
            replacement.splice(range, block.text.bytes());
            replacement
        } else {
            append_block(bytes, block).with_context(|| block.path.display().to_string())?
        };
        let content = file_content(&before, replacement);
        edits.push((block.path.clone(), before, content));
    }
    private_dir(&paths.data)?;
    ensure_user_dir(&paths.bin)?;
    private_dir(&paths.data.join("releases"))?;
    let current_before = owned_link(
        &paths.data.join("current"),
        old.as_ref()
            .and_then(|m| m.current.as_deref())
            .map(release_target)
            .as_deref(),
    )?;
    let bin_before = owned_link(
        &paths.bin.join(BINARY),
        old.as_ref()
            .and_then(|m| m.current.as_ref())
            .map(|_| paths.data.join("current").join(BINARY))
            .as_deref(),
    )?;
    let mut next = old.clone().unwrap_or(Manifest {
        format: 2,
        paths: paths.clone(),
        shell_kind: kind.clone(),
        current: None,
        previous: None,
        releases: Vec::new(),
        files: Vec::new(),
        blocks: Vec::new(),
    });
    if let Some(current) = &next.current {
        verify_release(paths, current)?;
    }
    let mut tx = Transaction::new(&paths.state)?;
    stage_release(&mut tx, paths, &id, &bytes)?;
    // Install the new targets and repoint exact managed blocks before removing
    // legacy scripts. The transaction restores every changed file on failure.
    for (path, before, content) in edits.into_iter().chain(removals) {
        tx.change(&path, &before, content)?;
    }
    if next.current.as_deref() != Some(&id) {
        next.previous = next.current.replace(id.clone());
    }
    if !next.releases.contains(&id) {
        next.releases.push(id.clone());
    }
    next.format = 2;
    next.files = files;
    if options.shell_config.is_some() {
        next.shell_kind = kind;
    }
    for block in blocks {
        if let Some(installed) = next.blocks.iter_mut().find(|old| old.path == block.path) {
            *installed = block;
        } else {
            next.blocks.push(block);
        }
    }
    tx.change(
        &paths.data.join("current"),
        &current_before,
        Content::Link(release_target(&id)),
    )?;
    tx.change(
        &paths.bin.join(BINARY),
        &bin_before,
        Content::Link(paths.data.join("current").join(BINARY)),
    )?;
    write_manifest(&mut tx, paths, &manifest_before, &next)?;
    tx.commit()?;
    println!("Installed {id}. Only selected or migration-owned startup blocks were edited; existing tmux sessions are untouched.");
    Ok(())
}

fn stage_release(tx: &mut Transaction, paths: &Paths, id: &str, bytes: &[u8]) -> Result<()> {
    let directory = paths.data.join("releases").join(id);
    private_dir(&directory)?;
    let path = directory.join(BINARY);
    let before = snapshot(&path)?;
    match &before.content {
        Content::Missing => tx.change(&path, &before, Content::File(bytes.to_vec(), 0o700))?,
        Content::File(existing, mode) if existing == bytes && *mode == 0o700 => (),
        _ => bail!(
            "release destination is not an intact owned executable: {}",
            path.display()
        ),
    }
    Ok(())
}

/// Activate a checksum-verified local executable. Never execute the supplied file to inspect it.
pub fn update(from: &Path, sha256: &str) -> Result<()> {
    update_at(&Paths::discover()?, &absolute(from.into())?, sha256)
}

fn update_at(paths: &Paths, from: &Path, sha256: &str) -> Result<()> {
    let expected = sha256.to_ascii_lowercase();
    if !valid_digest(&expected) {
        bail!("--sha256 must be exactly 64 hexadecimal characters");
    }
    let bytes = native_binary(from)?;
    let id = digest(&bytes);
    if id != expected {
        bail!("SHA-256 mismatch: expected {expected}, got {id}; nothing activated");
    }
    let _lock = lock(&paths.state)?;
    private_dir(&paths.data)?;
    let (before, value) = manifest(paths)?;
    let mut value = value.context("not installed; run install --yes first")?;
    let current = value
        .current
        .as_deref()
        .context("installation has been uninstalled")?;
    verify_release(paths, current)?;
    let pointer = owned_link(&paths.data.join("current"), Some(&release_target(current)))?;
    owned_link(
        &paths.bin.join(BINARY),
        Some(&paths.data.join("current").join(BINARY)),
    )?;
    if id == current {
        println!("Already using {id}; nothing changed.");
        return Ok(());
    }
    let mut tx = Transaction::new(&paths.state)?;
    stage_release(&mut tx, paths, &id, &bytes)?;
    value.previous = value.current.replace(id.clone());
    if !value.releases.contains(&id) {
        value.releases.push(id.clone());
    }
    tx.change(
        &paths.data.join("current"),
        &pointer,
        Content::Link(release_target(&id)),
    )?;
    write_manifest(&mut tx, paths, &before, &value)?;
    tx.commit()?;
    println!("Activated {id}; rollback is available. Existing sessions were not changed.");
    Ok(())
}

pub fn rollback() -> Result<()> {
    rollback_at(&Paths::discover()?)
}

fn rollback_at(paths: &Paths) -> Result<()> {
    let _lock = lock(&paths.state)?;
    private_dir(&paths.data)?;
    let (before, value) = manifest(paths)?;
    let mut value = value.context("not installed")?;
    let previous = value
        .previous
        .clone()
        .context("no previous release available")?;
    let current = value
        .current
        .as_deref()
        .context("installation has been uninstalled")?;
    verify_release(paths, &previous)?;
    let pointer = owned_link(&paths.data.join("current"), Some(&release_target(current)))?;
    owned_link(
        &paths.bin.join(BINARY),
        Some(&paths.data.join("current").join(BINARY)),
    )?;
    let mut tx = Transaction::new(&paths.state)?;
    value.previous = value.current.replace(previous.clone());
    tx.change(
        &paths.data.join("current"),
        &pointer,
        Content::Link(release_target(&previous)),
    )?;
    write_manifest(&mut tx, paths, &before, &value)?;
    tx.commit()?;
    println!("Rolled back to {previous}; existing sessions were not changed.");
    Ok(())
}

/// Remove only manifest-owned, unmodified content. Never terminate tmux or shell sessions.
/// This filesystem-only entry point does not restore live tmux bindings. CLI callers
/// should use `uninstall_with_hook` to restore runtime-owned bindings before removal.
pub fn uninstall(yes: bool) -> Result<()> {
    uninstall_at(&Paths::discover()?, yes)
}

fn uninstall_at(paths: &Paths, yes: bool) -> Result<()> {
    uninstall_at_with_hook(paths, yes, || Ok(()))
}

/// Preview the file-removal plan, then restore runtime integration before removing files.
/// The hook runs once, under the installer lock, only with `yes` and an existing manifest,
/// after file checks but before any planned removal. An error aborts removal and keeps
/// installed files. The hook must not reenter the installer or terminate user sessions;
/// restore only bindings whose exact ownership the runtime can verify.
///
/// A preview (`yes == false`) never invokes the hook. Runtime bindings still need an
/// executable-absence fallback for servers not reachable during this cleanup. Runtime
/// changes are not rolled back if a subsequent filesystem operation fails.
pub fn uninstall_with_hook(yes: bool, before_remove: impl FnOnce() -> Result<()>) -> Result<()> {
    uninstall_at_with_hook(&Paths::discover()?, yes, before_remove)
}

fn uninstall_at_with_hook(
    paths: &Paths,
    yes: bool,
    before_remove: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let _lock = if yes { Some(lock(&paths.state)?) } else { None };
    let (_, preview) = manifest(paths)?;
    let Some(preview) = preview else {
        println!("No installation manifest; nothing to remove.");
        return Ok(());
    };
    println!("Uninstall owned installation at {}", paths.data.display());
    for block in &preview.blocks {
        println!(
            "  Remove unmodified managed block: {}",
            block.path.display()
        );
    }
    for file in &preview.files {
        println!("  Remove unmodified integration: {}", file.path.display());
    }
    println!("  Remove owned binary pointers and checksum-intact release payloads; retain private backups and user configuration.");
    if !yes {
        println!("Preview only; pass --yes to apply. No files changed.");
        return Ok(());
    }
    private_dir(&paths.data)?;
    let (before, value) = manifest(paths)?;
    let mut value = value.context("manifest disappeared concurrently")?;
    let mut edits = Vec::new();
    let mut retained_blocks = Vec::new();
    for block in &value.blocks {
        let removal = (|| -> Result<()> {
            let before = snapshot(&block.path)?;
            let Some(bytes) = regular(&before)? else {
                return Ok(());
            };
            let mut range = block_range(bytes, &block.text)?;
            // A separator we originally inserted may now separate two user-owned
            // lines if the user appended configuration after our block.
            if block.text.starts_with('\n')
                && range.end < bytes.len()
                && range.start > 0
                && bytes[range.start - 1] != b'\n'
            {
                range.start += 1;
            }
            let mut bytes = bytes.to_vec();
            bytes.drain(range);
            let content = file_content(&before, bytes);
            edits.push((block.path.clone(), before, content));
            Ok(())
        })();
        if let Err(error) = removal {
            eprintln!("Preserving {}: {error:#}", block.path.display());
            retained_blocks.push(block.clone());
        }
    }
    let mut retained_files = Vec::new();
    for file in &value.files {
        let removal = (|| -> Result<()> {
            let before = snapshot(&file.path)?;
            match regular(&before)? {
                None => Ok(()),
                Some(bytes) if bytes == file.text.as_bytes() => {
                    edits.push((file.path.clone(), before, Content::Missing));
                    Ok(())
                }
                _ => bail!("owned file was edited"),
            }
        })();
        if let Err(error) = removal {
            eprintln!("Preserving {}: {error:#}", file.path.display());
            retained_files.push(file.clone());
        }
    }
    let mut retained_pointer = false;
    if let Some(current) = &value.current {
        for (path, target) in [
            (
                paths.bin.join(BINARY),
                paths.data.join("current").join(BINARY),
            ),
            (paths.data.join("current"), release_target(current)),
        ] {
            let removal = (|| -> Result<()> {
                if snapshot(&path)?.content == Content::Missing {
                    return Ok(());
                }
                let before = owned_link(&path, Some(&target))?;
                edits.push((path.clone(), before, Content::Missing));
                Ok(())
            })();
            if let Err(error) = removal {
                eprintln!("Preserving {}: {error:#}", path.display());
                retained_pointer = true;
            }
        }
    }
    let mut retained_releases = Vec::new();
    for id in &value.releases {
        let path = release_binary(paths, id);
        let removal = (|| -> Result<()> {
            if snapshot(&path)?.content == Content::Missing {
                return Ok(());
            }
            verify_release(paths, id)?;
            let before = snapshot(&path)?;
            if regular(&before)?.map(digest).as_deref() != Some(id) {
                bail!("release changed concurrently");
            }
            edits.push((path.clone(), before, Content::Missing));
            Ok(())
        })();
        if let Err(error) = removal {
            eprintln!("Preserving {}: {error:#}", path.display());
            retained_releases.push(id.clone());
        }
    }
    value.blocks = retained_blocks;
    value.files = retained_files;
    value.releases = retained_releases;
    value.current = None;
    value.previous = None;
    before_remove()
        .context("runtime integration cleanup failed; installed files were not removed")?;
    let mut tx = Transaction::new(&paths.state)?;
    for (path, before, content) in edits {
        tx.change(&path, &before, content)?;
    }
    if value.blocks.is_empty()
        && value.files.is_empty()
        && value.releases.is_empty()
        && !retained_pointer
    {
        tx.change(&paths.state.join("install.json"), &before, Content::Missing)?;
    } else {
        write_manifest(&mut tx, paths, &before, &value)?;
        eprintln!("Some changed/unowned content was preserved; the residual manifest is retained for review.");
    }
    tx.commit()?;
    // Empty directories only: never recursively delete user additions or private backups.
    for id in &preview.releases {
        if !value.releases.contains(id) {
            let _ = fs::remove_dir(paths.data.join("releases").join(id));
        }
    }
    let _ = fs::remove_dir(paths.data.join("releases"));
    println!("Uninstalled owned content. Sessions, user configuration, user edits, and private backups were preserved.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

    struct Fixture {
        _temp: tempfile::TempDir,
        paths: Paths,
        options: InstallOptions,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp
                .path()
                .join("home # '$\\\" [*?]{x} #{socket_path} #(false) space");
            private_dir(&home).unwrap();
            let paths = Paths {
                config: home.join(".config/agent-float-term"),
                data: home.join(".local/share/agent-float-term"),
                state: home.join(".local/state/agent-float-term"),
                bin: home.join(".local/bin"),
            };
            let options = InstallOptions {
                tmux_config: Some(home.join(".tmux.conf")),
                shell_config: Some(home.join(".bashrc")),
                shell_kind: Some("bash".into()),
                yes: true,
            };
            let source = home.join("download");
            write_binary(&source, 1);
            Self {
                _temp: temp,
                paths,
                options,
                source,
            }
        }

        fn install(&self) -> Result<()> {
            install_at(&self.paths, self.options.clone(), &self.source)
        }
    }

    fn write_binary(path: &Path, discriminator: u8) {
        let mut bytes = vec![0u8; 64];
        #[cfg(target_os = "macos")]
        bytes[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
        #[cfg(target_os = "linux")]
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[63] = discriminator;
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn preview_has_no_filesystem_side_effects() {
        let fixture = Fixture::new();
        let mut options = fixture.options.clone();
        options.yes = false;
        install_at(&fixture.paths, options, Path::new("/nonexistent/source")).unwrap();
        uninstall_at(&fixture.paths, false).unwrap();
        for path in [
            &fixture.paths.config,
            &fixture.paths.data,
            &fixture.paths.state,
            &fixture.paths.bin,
        ] {
            assert!(!path.exists());
        }
        assert!(!fixture.options.tmux_config.as_ref().unwrap().exists());
        assert!(!fixture.options.shell_config.as_ref().unwrap().exists());
    }

    fn legacy_install(fixture: &Fixture) -> Manifest {
        fixture.install().unwrap();
        let mut value = manifest(&fixture.paths).unwrap().1.unwrap();
        let mut legacy_paths = fixture.paths.clone();
        legacy_paths.data = fixture.paths.config.clone();
        let (_, files, mut blocks) = integration(&legacy_paths, &fixture.options).unwrap();
        private_dir(&fixture.paths.config).unwrap();
        for (current, legacy) in value.files.iter().zip(&files) {
            fs::write(&legacy.path, &legacy.text).unwrap();
            fs::remove_file(&current.path).unwrap();
        }
        for (current, legacy) in value.blocks.iter().zip(&mut blocks) {
            if current.text.starts_with('\n') {
                legacy.text.insert(0, '\n');
            }
            let mut bytes = fs::read(&current.path).unwrap();
            let range = block_range(&bytes, &current.text).unwrap();
            bytes.splice(range, legacy.text.bytes());
            fs::write(&legacy.path, bytes).unwrap();
        }
        value.format = 1;
        value.files = files;
        value.blocks = blocks;
        fs::write(
            fixture.paths.state.join("install.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        value
    }

    #[test]
    fn legacy_layout_migration_preserves_settings_and_unrelated_startup_bytes() {
        for kind in ["bash", "zsh"] {
            let mut fixture = Fixture::new();
            fixture.options.shell_kind = Some(kind.into());
            for path in [&fixture.options.shell_config, &fixture.options.tmux_config]
                .into_iter()
                .flatten()
            {
                fs::write(path, "# user prefix without newline").unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o640)).unwrap();
            }
            let legacy = legacy_install(&fixture);
            let settings = fixture.paths.config.join("config.json");
            fs::write(&settings, "{\"shortcut\":\"F8\",\"width\":70}").unwrap();
            let settings_before = snapshot(&settings).unwrap();
            for block in &legacy.blocks {
                let mut bytes = fs::read(&block.path).unwrap();
                bytes.extend_from_slice(b"# user suffix\n");
                fs::write(&block.path, bytes).unwrap();
            }
            let paths: Vec<_> = legacy
                .files
                .iter()
                .chain(&legacy.blocks)
                .map(|f| f.path.clone())
                .chain([
                    fixture.paths.state.join("install.json"),
                    fixture.paths.data.join("current"),
                ])
                .collect();
            let before: Vec<_> = paths.iter().map(|path| snapshot(path).unwrap()).collect();
            install_at(
                &fixture.paths,
                InstallOptions::default(),
                Path::new("/no/source/required/for/preview"),
            )
            .unwrap();
            for (path, original) in paths.iter().zip(&before) {
                assert_eq!(snapshot(path).unwrap(), *original);
            }
            assert!(!fixture.paths.data.join("integration.sh").exists());

            let options = InstallOptions {
                yes: true,
                ..InstallOptions::default()
            };
            install_at(&fixture.paths, options.clone(), &fixture.source).unwrap();
            let migrated = manifest(&fixture.paths).unwrap().1.unwrap();
            assert_eq!(migrated.format, 2);
            assert_eq!(migrated.shell_kind, kind);
            assert_eq!(migrated.blocks.len(), legacy.blocks.len());
            for file in &migrated.files {
                assert_eq!(file.path.parent(), Some(fixture.paths.data.as_path()));
                assert_eq!(fs::read_to_string(&file.path).unwrap(), file.text);
            }
            for file in &legacy.files {
                assert!(!file.path.exists());
            }
            for block in &migrated.blocks {
                let bytes = fs::read(&block.path).unwrap();
                let range = block_range(&bytes, &block.text).unwrap();
                assert_eq!(&bytes[..range.start], b"# user prefix without newline");
                assert_eq!(&bytes[range.end..], b"# user suffix\n");
                assert_eq!(fs::metadata(&block.path).unwrap().mode() & 0o777, 0o640);
            }
            assert_eq!(snapshot(&settings).unwrap(), settings_before);
            let manifest_before = snapshot(&fixture.paths.state.join("install.json")).unwrap();
            install_at(&fixture.paths, options, &fixture.source).unwrap();
            assert_eq!(
                snapshot(&fixture.paths.state.join("install.json")).unwrap(),
                manifest_before
            );
            uninstall_at(&fixture.paths, true).unwrap();
            assert_eq!(snapshot(&settings).unwrap(), settings_before);
            for block in &migrated.blocks {
                assert_eq!(
                    fs::read_to_string(&block.path).unwrap(),
                    "# user prefix without newline\n# user suffix\n"
                );
            }
            for file in &migrated.files {
                assert!(!file.path.exists());
            }
        }
    }

    #[test]
    fn legacy_migration_refuses_modified_missing_and_unowned_content() {
        for case in [
            "edited script",
            "missing script",
            "edited block",
            "missing block",
            "occupied destination",
            "symlink destination",
        ] {
            let fixture = Fixture::new();
            let legacy = legacy_install(&fixture);
            let destination = fixture.paths.data.join("integration.sh");
            match case {
                "edited script" => fs::write(&legacy.files[1].path, "# user script\n").unwrap(),
                "missing script" => fs::remove_file(&legacy.files[1].path).unwrap(),
                "edited block" => {
                    fs::write(&legacy.blocks[1].path, "# user removed the block\n").unwrap()
                }
                "missing block" => fs::remove_file(&legacy.blocks[1].path).unwrap(),
                "occupied destination" => fs::write(&destination, &legacy.files[1].text).unwrap(),
                "symlink destination" => symlink(&legacy.files[1].path, &destination).unwrap(),
                _ => unreachable!(),
            }
            let paths: Vec<_> = legacy
                .files
                .iter()
                .chain(&legacy.blocks)
                .map(|file| file.path.clone())
                .chain([
                    fixture.paths.state.join("install.json"),
                    fixture.paths.data.join("current"),
                    fixture.paths.data.join("integration.tmux"),
                    destination,
                ])
                .collect();
            let before: Vec<_> = paths.iter().map(|path| snapshot(path).unwrap()).collect();
            assert!(
                install_at(
                    &fixture.paths,
                    InstallOptions {
                        yes: true,
                        ..InstallOptions::default()
                    },
                    &fixture.source
                )
                .is_err(),
                "{case}"
            );
            for (path, original) in paths.iter().zip(before) {
                assert_eq!(
                    snapshot(path).unwrap(),
                    original,
                    "{case}: {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn manifest_layout_version_and_unique_owned_files_are_enforced() {
        let fixture = Fixture::new();
        fixture.install().unwrap();
        let path = fixture.paths.state.join("install.json");
        let value = manifest(&fixture.paths).unwrap().1.unwrap();
        for case in ["version", "wrong root", "duplicate"] {
            let mut invalid = value.clone();
            match case {
                "version" => invalid.format = 3,
                "wrong root" => {
                    invalid.files[0].path = fixture.paths.config.join("integration.tmux")
                }
                "duplicate" => invalid.files[1] = invalid.files[0].clone(),
                _ => unreachable!(),
            }
            fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(manifest(&fixture.paths).is_err(), "{case}");
        }
    }

    #[test]
    fn payload_only_install_never_selects_default_user_configs() {
        for existing in [false, true] {
            let fixture = Fixture::new();
            let home = fixture.source.parent().unwrap();
            let defaults = [
                home.join(".tmux.conf"),
                home.join(".bashrc"),
                home.join(".zshrc"),
            ];
            if existing {
                for path in &defaults {
                    // Even an unowned/malformed marker is irrelevant without path consent.
                    fs::write(path, format!("# keep this file\n{BEGIN}\n")).unwrap();
                }
            }
            let originals: Vec<_> = defaults
                .iter()
                .map(|path| snapshot(path).unwrap())
                .collect();
            install_at(
                &fixture.paths,
                InstallOptions {
                    yes: true,
                    ..InstallOptions::default()
                },
                &fixture.source,
            )
            .unwrap();
            for (path, original) in defaults.iter().zip(&originals) {
                assert_eq!(snapshot(path).unwrap(), *original);
            }
            let (_, value) = manifest(&fixture.paths).unwrap();
            let value = value.unwrap();
            assert!(value.blocks.is_empty());
            assert_eq!(value.files.len(), 2);
            for file in &value.files {
                assert!(file.path.is_file());
            }
            assert_eq!(
                fs::read(fixture.paths.bin.join(BINARY)).unwrap(),
                fs::read(&fixture.source).unwrap()
            );

            // The OS can report the canonical digest payload as current_exe on reinstall.
            let installed_source = fs::canonicalize(fixture.paths.bin.join(BINARY)).unwrap();
            install_at(
                &fixture.paths,
                InstallOptions {
                    yes: true,
                    ..InstallOptions::default()
                },
                &installed_source,
            )
            .unwrap();
            uninstall_at(&fixture.paths, true).unwrap();
            for (path, original) in defaults.iter().zip(&originals) {
                assert_eq!(snapshot(path).unwrap(), *original);
            }
        }
    }

    #[test]
    fn integration_flags_select_independent_paths() {
        for selected in ["tmux", "bash", "zsh"] {
            let fixture = Fixture::new();
            let home = fixture.source.parent().unwrap();
            let defaults = [
                home.join(".tmux.conf"),
                home.join(".bashrc"),
                home.join(".zshrc"),
            ];
            for path in &defaults {
                fs::write(path, "# user content\n").unwrap();
            }
            let originals: Vec<_> = defaults
                .iter()
                .map(|path| snapshot(path).unwrap())
                .collect();
            let index = match selected {
                "tmux" => 0,
                "bash" => 1,
                _ => 2,
            };
            let options = InstallOptions {
                tmux_config: (index == 0).then(|| defaults[index].clone()),
                shell_config: (index != 0).then(|| defaults[index].clone()),
                shell_kind: (index != 0).then(|| selected.into()),
                yes: true,
            };
            install_at(&fixture.paths, options.clone(), &fixture.source).unwrap();
            let (_, value) = manifest(&fixture.paths).unwrap();
            assert_eq!(value.as_ref().unwrap().blocks.len(), 1);
            assert_eq!(value.as_ref().unwrap().blocks[0].path, defaults[index]);
            for (i, (path, original)) in defaults.iter().zip(&originals).enumerate() {
                if i != index {
                    assert_eq!(snapshot(path).unwrap(), *original);
                }
            }
            let installed = snapshot(&defaults[index]).unwrap();
            install_at(&fixture.paths, options, &fixture.source).unwrap();
            install_at(
                &fixture.paths,
                InstallOptions {
                    yes: true,
                    ..InstallOptions::default()
                },
                &fixture.source,
            )
            .unwrap();
            assert_eq!(snapshot(&defaults[index]).unwrap(), installed);
            let (_, retained) = manifest(&fixture.paths).unwrap();
            assert_eq!(
                retained.as_ref().unwrap().blocks[0].text,
                value.as_ref().unwrap().blocks[0].text
            );
            assert_eq!(retained.unwrap().shell_kind, value.unwrap().shell_kind);
        }
    }

    #[test]
    fn plain_reinstall_refreshes_owned_template_with_recorded_shell_kind() {
        for kind in ["bash", "zsh"] {
            let mut fixture = Fixture::new();
            fixture.options.shell_kind = Some(kind.into());
            fixture.install().unwrap();
            let manifest_path = fixture.paths.state.join("install.json");
            let mut value = manifest(&fixture.paths).unwrap().1.unwrap();
            let template = value
                .files
                .iter_mut()
                .find(|file| file.path.ends_with("integration.sh"))
                .unwrap();
            template.text = "# previous owned template\n".into();
            fs::write(&template.path, &template.text).unwrap();
            fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
            // An unselected, even user-edited rc must not be rewritten during refresh.
            let rc = fixture.options.shell_config.as_ref().unwrap();
            fs::write(rc, "# user edited the managed block too\n").unwrap();
            let before = snapshot(rc).unwrap();
            install_at(
                &fixture.paths,
                InstallOptions {
                    yes: true,
                    ..InstallOptions::default()
                },
                &fixture.source,
            )
            .unwrap();
            let refreshed = manifest(&fixture.paths).unwrap().1.unwrap();
            assert_eq!(refreshed.shell_kind, kind);
            let (_, expected, _) = integration(&fixture.paths, &fixture.options).unwrap();
            assert_eq!(refreshed.files[1].text, expected[1].text);
            assert_eq!(
                fs::read_to_string(&expected[1].path).unwrap(),
                expected[1].text
            );
            assert_eq!(snapshot(rc).unwrap(), before);
        }
    }

    #[test]
    fn adding_an_integration_retains_unselected_edited_blocks() {
        for shell_first in [false, true] {
            let fixture = Fixture::new();
            let shell = InstallOptions {
                tmux_config: None,
                ..fixture.options.clone()
            };
            let tmux = InstallOptions {
                shell_config: None,
                shell_kind: None,
                ..fixture.options.clone()
            };
            let (first, second) = if shell_first {
                (shell, tmux)
            } else {
                (tmux, shell)
            };
            install_at(&fixture.paths, first.clone(), &fixture.source).unwrap();
            let first_path = first
                .shell_config
                .as_ref()
                .or(first.tmux_config.as_ref())
                .unwrap();
            let edited = fs::read_to_string(first_path)
                .unwrap()
                .replace(END, "# user changed the end marker");
            fs::write(first_path, &edited).unwrap();
            let before = snapshot(first_path).unwrap();
            install_at(&fixture.paths, second, &fixture.source).unwrap();
            install_at(
                &fixture.paths,
                InstallOptions {
                    yes: true,
                    ..InstallOptions::default()
                },
                &fixture.source,
            )
            .unwrap();
            assert_eq!(snapshot(first_path).unwrap(), before);
            assert_eq!(manifest(&fixture.paths).unwrap().1.unwrap().blocks.len(), 2);
            assert!(install_at(&fixture.paths, first, &fixture.source).is_err());
        }
    }

    #[test]
    fn changing_a_recorded_integration_target_requires_uninstall() {
        let fixture = Fixture::new();
        fixture.install().unwrap();
        let different = fixture.source.parent().unwrap().join("different.tmux.conf");
        let before = snapshot(&fixture.paths.state.join("install.json")).unwrap();
        let options = InstallOptions {
            tmux_config: Some(different.clone()),
            shell_config: None,
            ..fixture.options.clone()
        };
        assert!(install_at(&fixture.paths, options, &fixture.source).is_err());
        assert!(!different.exists());
        assert_eq!(
            snapshot(&fixture.paths.state.join("install.json")).unwrap(),
            before
        );
    }

    #[test]
    fn downloaded_binary_must_not_preoccupy_managed_bin_destination() {
        let fixture = Fixture::new();
        ensure_user_dir(&fixture.paths.bin).unwrap();
        let binary = fixture.paths.bin.join(BINARY);
        write_binary(&binary, 1);
        let before = snapshot(&binary).unwrap();
        assert!(install_at(
            &fixture.paths,
            InstallOptions {
                yes: true,
                ..InstallOptions::default()
            },
            &binary
        )
        .is_err());
        assert_eq!(snapshot(&binary).unwrap(), before);
        assert!(!fixture.paths.data.join("current").exists());
    }

    #[test]
    fn uninstall_hook_requires_consent_and_runs_before_file_removal() {
        let fixture = Fixture::new();
        uninstall_at_with_hook(&fixture.paths, false, || panic!("preview called hook")).unwrap();
        assert!(!fixture.paths.state.exists());
        uninstall_at_with_hook(&fixture.paths, true, || {
            panic!("no installation called hook")
        })
        .unwrap();
        fixture.install().unwrap();
        let manifest_before = snapshot(&fixture.paths.state.join("install.json")).unwrap();
        uninstall_at_with_hook(&fixture.paths, false, || panic!("preview called hook")).unwrap();
        assert_eq!(
            snapshot(&fixture.paths.state.join("install.json")).unwrap(),
            manifest_before
        );
        let called = std::cell::Cell::new(false);
        uninstall_at_with_hook(&fixture.paths, true, || {
            called.set(true);
            assert!(fixture.paths.bin.join(BINARY).is_file());
            assert!(fixture.paths.data.join("integration.tmux").is_file());
            assert!(
                fs::read_to_string(fixture.options.tmux_config.as_ref().unwrap())
                    .unwrap()
                    .contains(BEGIN)
            );
            Ok(())
        })
        .unwrap();
        assert!(called.get());
        assert!(!fixture.paths.bin.join(BINARY).exists());
    }

    #[test]
    fn failing_uninstall_hook_keeps_installed_files() {
        let fixture = Fixture::new();
        fixture.install().unwrap();
        let paths = [
            fixture.paths.bin.join(BINARY),
            fixture.paths.data.join("current"),
            fixture.paths.state.join("install.json"),
            fixture.paths.data.join("integration.sh"),
            fixture.options.tmux_config.clone().unwrap(),
            fixture.options.shell_config.clone().unwrap(),
        ];
        let originals: Vec<_> = paths.iter().map(|path| snapshot(path).unwrap()).collect();
        assert!(uninstall_at_with_hook(&fixture.paths, true, || bail!(
            "binding ownership could not be verified"
        ))
        .is_err());
        for (path, original) in paths.iter().zip(originals) {
            assert_eq!(snapshot(path).unwrap(), original);
        }
    }

    #[test]
    fn install_idempotency_and_byte_preserving_uninstall() {
        let fixture = Fixture::new();
        let tmux = fixture.options.tmux_config.as_ref().unwrap();
        let shell = fixture.options.shell_config.as_ref().unwrap();
        let original = b"# user bytes\r\n\xff no final newline";
        fs::write(tmux, original).unwrap();
        fs::write(shell, b"# shell without final newline").unwrap();
        fixture.install().unwrap();
        let installed = snapshot(tmux).unwrap();
        let manifest_path = fixture.paths.state.join("install.json");
        let recorded = snapshot(&manifest_path).unwrap();
        fixture.install().unwrap();
        assert_eq!(snapshot(tmux).unwrap(), installed);
        assert_eq!(snapshot(&manifest_path).unwrap(), recorded);
        assert_eq!(fs::metadata(&manifest_path).unwrap().mode() & 0o777, 0o600);
        let mut extended = fs::read(tmux).unwrap();
        extended.extend_from_slice(b"# later user addition\n");
        fs::write(tmux, extended).unwrap();
        uninstall_at(&fixture.paths, false).unwrap();
        assert!(manifest_path.exists());
        uninstall_at(&fixture.paths, true).unwrap();
        assert_eq!(
            fs::read(tmux).unwrap(),
            [original.as_slice(), b"\n# later user addition\n"].concat()
        );
        assert_eq!(fs::read(shell).unwrap(), b"# shell without final newline");
        assert!(!manifest_path.exists());
        assert!(fs::symlink_metadata(fixture.paths.bin.join(BINARY)).is_err());
        assert!(fixture.paths.state.join("backups").is_dir());
        for backup in fs::read_dir(fixture.paths.state.join("backups")).unwrap() {
            assert_eq!(backup.unwrap().metadata().unwrap().mode() & 0o777, 0o600);
        }
        uninstall_at(&fixture.paths, true).unwrap();
    }

    #[test]
    fn malformed_and_unowned_blocks_are_never_adopted() {
        for text in [
            format!("{BEGIN}\n"),
            format!("{END}\n{BEGIN}\n"),
            format!("{BEGIN}\n{END}\n"),
            "# BEGIN agent-float-term managed v9\n".into(),
        ] {
            let fixture = Fixture::new();
            let path = fixture.options.tmux_config.as_ref().unwrap();
            fs::write(path, &text).unwrap();
            assert!(fixture.install().is_err());
            assert_eq!(fs::read(path).unwrap(), text.as_bytes());
            assert!(!fixture.paths.data.join("current").exists());
        }
    }

    #[test]
    fn edited_blocks_and_files_survive_uninstall() {
        let fixture = Fixture::new();
        fixture.install().unwrap();
        let tmux = fixture.options.tmux_config.as_ref().unwrap();
        let edited = fs::read_to_string(tmux)
            .unwrap()
            .replace("source-file", "# user edited source-file");
        fs::write(tmux, &edited).unwrap();
        let integration = fixture.paths.data.join("integration.sh");
        fs::write(&integration, "# user-owned now\n").unwrap();
        assert!(fixture.install().is_err());
        uninstall_at(&fixture.paths, true).unwrap();
        assert_eq!(fs::read_to_string(tmux).unwrap(), edited);
        assert_eq!(
            fs::read_to_string(integration).unwrap(),
            "# user-owned now\n"
        );
        let (_, manifest) = manifest(&fixture.paths).unwrap();
        let manifest = manifest.unwrap();
        assert_eq!(manifest.blocks.len(), 1);
        assert_eq!(manifest.files.len(), 1);
        assert!(manifest.current.is_none());
    }

    #[test]
    fn symlinks_and_unowned_binary_are_refused() {
        let fixture = Fixture::new();
        let user_file = fixture.source.parent().unwrap().join("real-shell");
        fs::write(&user_file, "keep").unwrap();
        symlink(&user_file, fixture.options.shell_config.as_ref().unwrap()).unwrap();
        assert!(fixture.install().is_err());
        assert_eq!(fs::read_to_string(&user_file).unwrap(), "keep");
        fs::remove_file(fixture.options.shell_config.as_ref().unwrap()).unwrap();
        ensure_user_dir(&fixture.paths.bin).unwrap();
        fs::write(fixture.paths.bin.join(BINARY), "not ours").unwrap();
        assert!(fixture.install().is_err());
        assert_eq!(
            fs::read(fixture.paths.bin.join(BINARY)).unwrap(),
            b"not ours"
        );
    }

    #[test]
    fn update_checksum_activation_rollback_and_modified_payload() {
        let fixture = Fixture::new();
        fixture.install().unwrap();
        let original = fs::read_link(fixture.paths.data.join("current")).unwrap();
        write_binary(&fixture.source, 2);
        let bytes = fs::read(&fixture.source).unwrap();
        let id = digest(&bytes);
        assert!(update_at(&fixture.paths, &fixture.source, &"0".repeat(64)).is_err());
        assert!(update_at(&fixture.paths, &fixture.source, "xyz").is_err());
        assert_eq!(
            fs::read_link(fixture.paths.data.join("current")).unwrap(),
            original
        );
        update_at(&fixture.paths, &fixture.source, &id.to_uppercase()).unwrap();
        assert_eq!(
            fs::read_link(fixture.paths.data.join("current")).unwrap(),
            release_target(&id)
        );
        assert_eq!(fs::read(fixture.paths.bin.join(BINARY)).unwrap(), bytes);
        let before = snapshot(&fixture.paths.state.join("install.json")).unwrap();
        update_at(&fixture.paths, &fixture.source, &id).unwrap();
        assert_eq!(
            snapshot(&fixture.paths.state.join("install.json")).unwrap(),
            before
        );
        rollback_at(&fixture.paths).unwrap();
        assert_eq!(
            fs::read_link(fixture.paths.data.join("current")).unwrap(),
            original
        );
        rollback_at(&fixture.paths).unwrap();
        assert_eq!(
            fs::read_link(fixture.paths.data.join("current")).unwrap(),
            release_target(&id)
        );
        let payload = release_binary(&fixture.paths, &id);
        fs::write(&payload, b"user modified").unwrap();
        uninstall_at(&fixture.paths, true).unwrap();
        assert_eq!(fs::read(&payload).unwrap(), b"user modified");
    }

    #[test]
    fn transaction_detects_concurrent_edits_and_reverts_earlier_changes() {
        let fixture = Fixture::new();
        private_dir(&fixture.paths.state).unwrap();
        let first = fixture.source.parent().unwrap().join("first");
        let second = fixture.source.parent().unwrap().join("second");
        fs::write(&first, "first original").unwrap();
        fs::write(&second, "second original").unwrap();
        let before = snapshot(&second).unwrap();
        {
            let mut tx = Transaction::new(&fixture.paths.state).unwrap();
            tx.change(
                &first,
                &snapshot(&first).unwrap(),
                Content::File(b"replacement".to_vec(), 0o600),
            )
            .unwrap();
            fs::write(&second, "concurrent edit").unwrap();
            assert!(tx
                .change(
                    &second,
                    &before,
                    Content::File(b"bad overwrite".to_vec(), 0o600)
                )
                .is_err());
        }
        assert_eq!(fs::read(first).unwrap(), b"first original");
        assert_eq!(fs::read(second).unwrap(), b"concurrent edit");
    }

    #[test]
    fn installer_lock_serializes_writers() {
        let fixture = Fixture::new();
        let held = lock(&fixture.paths.state).unwrap();
        assert!(lock(&fixture.paths.state).is_err());
        drop(held);
        lock(&fixture.paths.state).unwrap();
    }

    #[test]
    fn interrupted_transaction_recovers_and_preserves_later_edits() {
        let fixture = Fixture::new();
        private_dir(&fixture.paths.state).unwrap();
        let original = snapshot(&fixture.source).unwrap();
        let mut tx = Transaction::new(&fixture.paths.state).unwrap();
        tx.change(
            &fixture.source,
            &original,
            Content::File(b"interrupted".to_vec(), 0o600),
        )
        .unwrap();
        std::mem::forget(tx); // Simulate process death, without running the rollback destructor.
        assert!(fixture.paths.state.join("transaction.json").is_file());
        drop(lock(&fixture.paths.state).unwrap());
        assert_eq!(snapshot(&fixture.source).unwrap().content, original.content);
        assert!(!fixture.paths.state.join("transaction.json").exists());

        let mut tx = Transaction::new(&fixture.paths.state).unwrap();
        tx.change(
            &fixture.source,
            &snapshot(&fixture.source).unwrap(),
            Content::File(b"interrupted again".to_vec(), 0o600),
        )
        .unwrap();
        std::mem::forget(tx);
        fs::write(&fixture.source, b"later user edit").unwrap();
        assert!(lock(&fixture.paths.state).is_err());
        assert_eq!(fs::read(&fixture.source).unwrap(), b"later user edit");
        assert!(fixture.paths.state.join("transaction.json").exists());
    }

    #[test]
    fn shell_quoting_and_noninteractive_guard() {
        use std::process::Command;
        let fixture = Fixture::new();
        let word = "space ' \" $ ` # \\ ; $(false)";
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s' {}", shell_quote(word)))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, word.as_bytes());
        for program in ["/bin/bash", "/bin/zsh"] {
            if !Path::new(program).exists() {
                continue;
            }
            let (_, files, _) = integration(
                &fixture.paths,
                &InstallOptions {
                    shell_kind: Some(program.trim_start_matches("/bin/").into()),
                    ..fixture.options.clone()
                },
            )
            .unwrap();
            let shell = &files[1].text;
            assert!(!shell.contains("exec "));
            assert!(shell.contains("AFT_STARTING=1 AFT_QUIET=1"));
            let output = Command::new(program)
                .arg("-c")
                .arg(format!("{shell}\nprintf '%s' survived"))
                .env("HOME", fixture.source.parent().unwrap())
                .env("ZDOTDIR", fixture.source.parent().unwrap())
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
            assert_eq!(output.stdout, b"survived");
        }
    }

    // Real stdin-driven interactive shells, not `-ic`, exercise actual prompt hooks.
    fn shell_session(
        program: &str,
        home: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        input: &str,
        tty: (bool, bool),
    ) -> String {
        use std::io::{Read, Write};
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: valid output pointers and null optional arguments request defaults.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0);
        // SAFETY: openpty returned new owned descriptors.
        let (mut master_file, slave_file) =
            unsafe { (fs::File::from_raw_fd(master), fs::File::from_raw_fd(slave)) };
        for fd in [master, slave] {
            // SAFETY: openpty returned owned, live descriptors. dup2 onto stdio in
            // the child clears CLOEXEC; unrelated parallel fixtures must not inherit these.
            let result = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            assert_eq!(result, 0);
        }
        let mut command = Command::new(program);
        if program.ends_with("bash") {
            if !args.contains(&"-lic") {
                command.arg("--noprofile");
            }
            command.arg("--rcfile").arg(home.join(".bashrc"));
        } else {
            command.arg("-d");
        }
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "dumb")
            .env("HOME", home)
            .env("ZDOTDIR", home)
            .envs(envs.iter().copied())
            .stdin(if tty.0 {
                Stdio::from(slave_file.try_clone().unwrap())
            } else {
                Stdio::piped()
            })
            .stdout(if tty.1 {
                Stdio::from(slave_file.try_clone().unwrap())
            } else {
                Stdio::null()
            })
            .stderr(Stdio::from(slave_file.try_clone().unwrap()));
        // SAFETY: only async-signal-safe syscalls run between fork and exec. The slave
        // descriptor remains open until the child has acquired its controlling tty.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 || libc::ioctl(slave, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        drop(slave_file);
        if tty.0 {
            master_file.write_all(input.as_bytes()).unwrap();
        } else {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        // SAFETY: master is a valid open descriptor; nonblocking reads allow a deadline.
        let result = unsafe { libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) };
        assert_ne!(result, -1);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            while let Ok(count) = master_file.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                output.extend_from_slice(&buffer[..count]);
            }
            if let Some(status) = child.try_wait().unwrap() {
                // Collect any final output written between the last read and exit.
                let _ = master_file.read_to_end(&mut output);
                assert!(
                    status.success(),
                    "{program} {args:?}: {status}: {}",
                    String::from_utf8_lossy(&output)
                );
                return String::from_utf8_lossy(&output).into_owned();
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!(
                    "{program} {args:?} hung: {}",
                    String::from_utf8_lossy(&output)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn shell_tty_guards_failure_and_recursion() {
        let fixture = Fixture::new();
        ensure_user_dir(&fixture.paths.bin).unwrap();
        let home = fixture.source.parent().unwrap();
        let marker = home.join("started");
        let binary = fixture.paths.bin.join(BINARY);
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s:%s:%s:%s\\n' \"$1\" \"$AFT_STARTING\" \"$AFT_QUIET\" \"${{TMUX-}}\" >> {}\nexit 17\n",
                shell_quote(marker.to_str().unwrap())
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        for program in ["/bin/bash", "/bin/zsh"] {
            if !Path::new(program).exists() {
                continue;
            }
            let kind = program.trim_start_matches("/bin/");
            let (_, files, _) = integration(
                &fixture.paths,
                &InstallOptions {
                    shell_kind: Some(kind.into()),
                    ..fixture.options.clone()
                },
            )
            .unwrap();
            let rc = home.join(format!(".{kind}rc"));
            let prompt = if kind == "zsh" { "unsetopt zle\n" } else { "" };
            fs::write(
                &rc,
                format!("PS1='aft-test> '\n{prompt}{0}\n{0}\n", files[1].text),
            )
            .unwrap();
            for guard in [
                None,
                Some("TMUX"),
                Some("SSH_CONNECTION"),
                Some("SSH_CLIENT"),
                Some("SSH_TTY"),
                Some("AFT_DISABLE"),
                Some("AFT_STARTING"),
                Some("_AFT_AUTO_STARTED"),
            ] {
                let _ = fs::remove_file(&marker);
                let envs: Vec<_> = guard.map(|name| (name, "1")).into_iter().collect();
                let output = shell_session(
                    program,
                    home,
                    &["-i"],
                    &envs,
                    &format!(
                        ". {}\nprintf 'survived\\n'\nexit 0\n",
                        shell_quote(rc.to_str().unwrap())
                    ),
                    (true, true),
                );
                if guard.is_none() || guard == Some("TMUX") {
                    let tmux = if guard.is_some() { "1" } else { "" };
                    assert_eq!(
                        fs::read_to_string(&marker).unwrap(),
                        format!("start:1:1:{tmux}\n"),
                        "{program}"
                    );
                    assert_eq!(
                        output
                            .matches("startup failed; continuing this shell")
                            .count(),
                        1,
                        "{output}"
                    );
                } else {
                    assert!(!marker.exists(), "{program}: {guard:?}");
                }
                assert!(output.contains("survived"), "{output}");
            }
            for tty in [(false, true), (true, false), (false, false)] {
                let _ = fs::remove_file(&marker);
                shell_session(program, home, &["-i"], &[], ":\n:\nexit 0\n", tty);
                assert!(!marker.exists(), "{program}: {tty:?}");
            }
        }
    }

    #[test]
    fn shell_command_strings_and_zsh_scripts_never_initialize() {
        for kind in ["bash", "zsh"] {
            let program = format!("/bin/{kind}");
            if !Path::new(&program).exists() {
                continue;
            }
            let fixture = Fixture::new();
            let home = fixture.source.parent().unwrap();
            ensure_user_dir(&fixture.paths.bin).unwrap();
            let marker = home.join("started");
            let sourced = home.join("sourced");
            let binary = fixture.paths.bin.join(BINARY);
            fs::write(
                &binary,
                format!(
                    "#!/bin/sh\nprintf started >> {}\n",
                    shell_quote(marker.to_str().unwrap())
                ),
            )
            .unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            let (_, files, _) = integration(
                &fixture.paths,
                &InstallOptions {
                    shell_kind: Some(kind.into()),
                    ..fixture.options.clone()
                },
            )
            .unwrap();
            let rc = home.join(format!(".{kind}rc"));
            fs::write(&rc, format!("{}\nprintf sourced >> {}\nif typeset -f __aft_initialize >/dev/null; then printf hook >> {}; fi\n",
                files[1].text, shell_quote(sourced.to_str().unwrap()), shell_quote(marker.to_str().unwrap()))).unwrap();
            let source = format!(". {}\n", shell_quote(rc.to_str().unwrap()));
            let background = format!("( {source} ) & wait");
            fs::write(home.join(".bash_profile"), &source).unwrap();
            let script = home.join("interactive-script");
            fs::write(&script, format!("{source}\nexit 0\n")).unwrap();
            let mut cases = vec![
                vec!["-ic", ":"],
                vec!["-lic", ":"],
                vec!["-ic", ""],
                vec!["-lic", ""],
                vec!["-c", &source],
                vec!["-c", &background],
            ];
            if kind == "zsh" {
                cases.push(vec!["-i", script.to_str().unwrap()]);
            }
            for args in cases {
                for tmux in ["", "fixture-server,1,0"] {
                    let _ = fs::remove_file(&sourced);
                    shell_session(&program, home, &args, &[("TMUX", tmux)], "", (true, true));
                    assert!(
                        sourced.exists(),
                        "{kind} {args:?} did not source the fixture rc"
                    );
                    assert!(
                        !marker.exists(),
                        "{kind} {args:?} installed a hook or started"
                    );
                }
            }
        }
    }

    #[test]
    fn zsh_prompt_cleanup_order_bounded_retry_and_hook_removal() {
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        for restore_at in [1, 2, 99] {
            for guard in [
                "",
                "AFT_DISABLE=1",
                "AFT_STARTING=1",
                "SSH_CONNECTION=remote",
                "BASH_EXECUTION_STRING=''",
                "ZSH_EXECUTION_STRING=''",
                "ZSH_SCRIPT=''",
            ] {
                let fixture = Fixture::new();
                let home = fixture.source.parent().unwrap();
                ensure_user_dir(&fixture.paths.bin).unwrap();
                let marker = home.join("started");
                let hooks = home.join("hooks");
                let binary = fixture.paths.bin.join(BINARY);
                fs::write(&binary, format!("#!/bin/sh\n[ -t 0 ] && [ -t 1 ] || exit 19\nprintf '%s:%s:%s:%s\\n' \"$1\" \"$AFT_STARTING\" \"$AFT_QUIET\" \"$TMUX\" >> {}\n", shell_quote(marker.to_str().unwrap()))).unwrap();
                fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
                let (_, files, blocks) = integration(
                    &fixture.paths,
                    &InstallOptions {
                        shell_kind: Some("zsh".into()),
                        shell_config: Some(home.join(".zshrc")),
                        ..fixture.options.clone()
                    },
                )
                .unwrap();
                private_dir(&fixture.paths.data).unwrap();
                fs::write(&files[1].path, &files[1].text).unwrap();
                let block = &blocks
                    .iter()
                    .find(|block| block.path.ends_with(".zshrc"))
                    .unwrap()
                    .text;
                // Model p10k's rc-time redirection and existing precmd cleanup without
                // changing the installer's bottom-of-rc source block or other hooks.
                fs::write(
                    home.join(".zshrc"),
                    format!(
                        r#"PS1='aft-test> '
unsetopt zle
exec 3<&0 4>&1 </dev/null > /dev/null
prompt_count=0
fixture_cleanup() {{
  (( ++prompt_count ))
  if (( prompt_count == {restore_at} )); then exec 0<&3 1>&4; fi
  {guard}
  return 0
}}
fixture_other() {{ print -r -- other >> {hooks}; }}
precmd_functions=(fixture_cleanup fixture_other)
{block}
{block}
# Keep stdin available for the next prompt even while stdout is still redirected.
exec 0<&3
"#,
                        hooks = shell_quote(hooks.to_str().unwrap())
                    ),
                )
                .unwrap();
                let inspection = format!(
                    r#":
:
print -r -- "${{precmd_functions[*]}}:${{+functions[__aft_initialize]}}:${{_AFT_INIT_RETRIED-unset}}" >> {}
exit 0
"#,
                    shell_quote(hooks.to_str().unwrap())
                );
                shell_session(
                    "/bin/zsh",
                    home,
                    &["-i"],
                    &[("TMUX", "fixture-server,1,0")],
                    &inspection,
                    (true, true),
                );
                if restore_at <= 2 && guard.is_empty() {
                    assert_eq!(
                        fs::read_to_string(&marker).unwrap(),
                        "start:1:1:fixture-server,1,0\n"
                    );
                } else {
                    assert!(!marker.exists());
                }
                let actual = fs::read_to_string(&hooks).unwrap();
                assert!(
                    actual.contains("fixture_cleanup fixture_other:0:unset\n"),
                    "{actual}"
                );
                assert_eq!(
                    actual.lines().filter(|line| *line == "other").count(),
                    4,
                    "{actual}"
                );
            }
        }
    }

    #[test]
    fn tmux_format_quoting_uses_inherited_socket() {
        use std::process::Command;
        if Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("tmux unavailable; skipping isolated integration check");
            return;
        }
        let fixture = Fixture::new();
        let socket_dir = tempfile::tempdir().unwrap();
        let socket = socket_dir.path().join("tmux.sock");
        struct Server(PathBuf);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = Command::new("tmux")
                    .arg("-S")
                    .arg(&self.0)
                    .arg("kill-server")
                    .env_remove("TMUX")
                    .output();
            }
        }
        let _server = Server(socket.clone());
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "aft-test",
                "/bin/sleep 60",
            ])
            .env_remove("TMUX")
            .env("HOME", fixture.source.parent().unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure_user_dir(&fixture.paths.bin).unwrap();
        private_dir(&fixture.paths.data).unwrap();
        let marker = fixture.source.parent().unwrap().join("bound");
        let binary = fixture.paths.bin.join(BINARY);
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$1\" \"$TMUX\" > {}\n",
                shell_quote(marker.to_str().unwrap())
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let (_, files, blocks) = integration(&fixture.paths, &fixture.options).unwrap();
        fs::write(&files[0].path, &files[0].text).unwrap();
        fs::write(&blocks[0].path, &blocks[0].text).unwrap();
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .arg("source-file")
            .arg(glob_quote(blocks[0].path.to_str().unwrap()))
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual =
            fs::read_to_string(&marker).expect("quoted integration must invoke our fixture binary");
        assert!(actual.starts_with("1\nbind\n"), "{actual:?}");
        assert!(actual.contains(socket.to_str().unwrap()), "{actual:?}");
    }
}
