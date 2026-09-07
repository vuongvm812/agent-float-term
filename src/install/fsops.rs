//! No-follow snapshots, cooperative locking, and atomic same-directory replacement.

use crate::config::{ensure_user_dir, private_dir, uid};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const LIMIT: u64 = 256 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Content {
    Missing,
    File(Vec<u8>, u32),
    Link(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub content: Content,
    stamp: Option<FileStamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn stamp(m: &fs::Metadata) -> FileStamp {
    FileStamp {
        device: m.dev(),
        inode: m.ino(),
        uid: m.uid(),
        mode: m.mode(),
        length: m.len(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
    }
}

pub(super) fn snapshot(path: &Path) -> Result<Snapshot> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Snapshot {
                content: Content::Missing,
                stamp: None,
            });
        }
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    if metadata.uid() != uid() {
        bail!("refusing non-user-owned path: {}", path.display());
    }
    let before = stamp(&metadata);
    let content = if metadata.file_type().is_symlink() {
        Content::Link(fs::read_link(path)?)
    } else if metadata.is_file() && metadata.mode() & 0o6022 == 0 {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if stamp(&file.metadata()?) != before {
            bail!("concurrent change to {}", path.display());
        }
        let mut bytes = Vec::new();
        file.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            bail!("{} exceeds the 256 MiB safety limit", path.display());
        }
        Content::File(bytes, metadata.mode() & 0o777)
    } else {
        bail!(
            "refusing non-regular or externally writable file: {}",
            path.display()
        );
    };
    if stamp(&fs::symlink_metadata(path)?) != before {
        bail!("concurrent change to {}", path.display());
    }
    Ok(Snapshot {
        content,
        stamp: Some(before),
    })
}

pub(super) fn regular(snapshot: &Snapshot) -> Result<Option<&[u8]>> {
    match &snapshot.content {
        Content::Missing => Ok(None),
        Content::File(bytes, _) => Ok(Some(bytes)),
        Content::Link(_) => bail!("refusing to read or edit a symlink"),
    }
}

pub(super) fn unique(parent: &Path, prefix: &str) -> PathBuf {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!(
        ".{prefix}-{}-{time}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

pub(super) fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

fn create_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    Ok(())
}

fn replace(path: &Path, expected: &Snapshot, content: &Content) -> Result<Snapshot> {
    let parent = path.parent().context("target has no parent")?;
    ensure_user_dir(parent)?;
    let staged = unique(parent, "aft-stage");
    let result = (|| {
        let replacement = match content {
            Content::Missing => Snapshot {
                content: Content::Missing,
                stamp: None,
            },
            Content::File(bytes, mode) => {
                create_file(&staged, bytes, *mode)?;
                snapshot(&staged)?
            }
            Content::Link(target) => {
                symlink(target, &staged)?;
                snapshot(&staged)?
            }
        };
        if snapshot(path)? != *expected {
            bail!(
                "concurrent change to {}; nothing overwritten",
                path.display()
            );
        }
        match content {
            Content::Missing => {
                if expected.content != Content::Missing {
                    fs::remove_file(path)?;
                }
            }
            _ => fs::rename(&staged, path)?,
        }
        // A rename may change ctime, but never adopt a concurrent editor's replacement
        // as our own: rollback must preserve it even if it arrives immediately afterward.
        Ok(match snapshot(path) {
            Ok(actual)
                if actual.content == replacement.content
                    && actual.stamp.map(|s| (s.device, s.inode))
                        == replacement.stamp.map(|s| (s.device, s.inode)) =>
            {
                actual
            }
            _ => replacement,
        })
    })();
    if fs::symlink_metadata(&staged).is_ok() {
        let _ = fs::remove_file(&staged);
    }
    result
}

pub(super) struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: this File still owns a live descriptor. Explicit unlock also releases
        // the lock if a concurrent fork briefly inherited the descriptor before exec.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub(super) fn lock(state: &Path) -> Result<Lock> {
    private_dir(state)?;
    let path = state.join("install.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
        bail!("unsafe installer lock file");
    }
    // SAFETY: file owns a live descriptor; flock neither closes it nor retains a pointer.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error()).context("another installer is running");
    }
    if stamp(&fs::symlink_metadata(path)?) != stamp(&metadata) {
        bail!("installer lock was replaced concurrently");
    }
    let held = Lock(file);
    recover(state)?;
    Ok(held)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Fingerprint {
    Missing,
    File { sha256: String, mode: u32 },
    Link(PathBuf),
}

fn fingerprint(content: &Content) -> Fingerprint {
    match content {
        Content::Missing => Fingerprint::Missing,
        Content::File(bytes, mode) => Fingerprint::File {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            mode: *mode,
        },
        Content::Link(path) => Fingerprint::Link(path.clone()),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalEntry {
    path: PathBuf,
    before: Fingerprint,
    after: Fingerprint,
    backup: Option<PathBuf>,
}

fn journal_write(path: &Path, entries: &[JournalEntry]) -> Result<()> {
    let before = snapshot(path)?;
    regular(&before)?;
    replace(
        path,
        &before,
        &Content::File(serde_json::to_vec(entries)?, 0o600),
    )?;
    sync_dir(path.parent().context("journal has no parent")?)
}

fn journal_remove(path: &Path) -> Result<()> {
    let before = snapshot(path)?;
    regular(&before)?;
    replace(path, &before, &Content::Missing)?;
    sync_dir(path.parent().context("journal has no parent")?)
}

/// The journal is write-ahead: after a crash each target is either its old value or
/// its intended new value. Anything else is a later edit and requires manual review.
fn recover(state: &Path) -> Result<()> {
    let path = state.join("transaction.json");
    let before = snapshot(&path)?;
    let Some(bytes) = regular(&before)? else {
        return Ok(());
    };
    let entries: Vec<JournalEntry> =
        serde_json::from_slice(bytes).context("invalid recovery journal")?;
    for entry in entries.iter().rev() {
        crate::config::checked_path(&entry.path)?;
        let current = snapshot(&entry.path)?;
        let actual = fingerprint(&current.content);
        if actual == entry.before {
            continue;
        }
        if actual != entry.after {
            bail!("interrupted installation and subsequent edit to {}; preserved it. Review {} and private backups before retrying", entry.path.display(), path.display());
        }
        let original = match &entry.before {
            Fingerprint::Missing => Content::Missing,
            Fingerprint::Link(target) => Content::Link(target.clone()),
            Fingerprint::File { mode, .. } => {
                let backup = entry.backup.as_ref().context("missing recovery backup")?;
                crate::config::checked_path(backup)?;
                if backup.parent() != Some(state.join("backups").as_path()) || *mode & !0o777 != 0 {
                    bail!("invalid recovery backup path or permissions");
                }
                let backup = snapshot(backup)?;
                Content::File(
                    regular(&backup)?
                        .context("recovery backup disappeared")?
                        .to_vec(),
                    *mode,
                )
            }
        };
        if fingerprint(&original) != entry.before {
            bail!("recovery backup was modified");
        }
        replace(&entry.path, &current, &original)?;
        sync_dir(entry.path.parent().context("target has no parent")?)?;
    }
    journal_remove(&path)?;
    eprintln!("Recovered an interrupted installation; private backups were retained.");
    Ok(())
}

pub(super) struct Transaction {
    backups: PathBuf,
    journal: PathBuf,
    entries: Vec<JournalEntry>,
    changes: Vec<(PathBuf, Snapshot, Snapshot)>,
    committed: bool,
}

impl Transaction {
    pub fn new(state: &Path) -> Result<Self> {
        let backups = state.join("backups");
        private_dir(&backups)?;
        let journal = state.join("transaction.json");
        if snapshot(&journal)?.content != Content::Missing {
            bail!("unrecovered installer transaction");
        }
        Ok(Self {
            backups,
            journal,
            entries: Vec::new(),
            changes: Vec::new(),
            committed: false,
        })
    }

    pub fn change(&mut self, path: &Path, before: &Snapshot, content: Content) -> Result<()> {
        if before.content == content {
            if snapshot(path)? != *before {
                bail!("concurrent change to {}", path.display());
            }
            return Ok(());
        }
        let backup = if let Content::File(bytes, mode) = &before.content {
            let backup = unique(&self.backups, "backup");
            create_file(&backup, bytes, 0o600)?;
            create_file(
                &backup.with_extension("json"),
                &serde_json::to_vec_pretty(&serde_json::json!({
                    "path": path, "mode": mode, "sha256": format!("{:x}", Sha256::digest(bytes)),
                }))?,
                0o600,
            )?;
            sync_dir(&self.backups)?;
            eprintln!("Private backup: {}", backup.display());
            Some(backup)
        } else {
            None
        };
        self.entries.push(JournalEntry {
            path: path.into(),
            before: fingerprint(&before.content),
            after: fingerprint(&content),
            backup,
        });
        journal_write(&self.journal, &self.entries)?;
        let after = match replace(path, before, &content) {
            Ok(after) => after,
            Err(error) => {
                // replace only returns failure before its atomic mutation. This pending
                // entry is not ours to recover if a concurrent editor changed the target.
                self.entries.pop();
                journal_write(&self.journal, &self.entries)?;
                return Err(error);
            }
        };
        self.changes.push((path.into(), before.clone(), after));
        sync_dir(path.parent().context("target has no parent")?)?;
        Ok(())
    }

    pub fn commit(mut self) -> Result<()> {
        journal_remove(&self.journal)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut reverted = true;
        for (path, before, after) in self.changes.iter().rev() {
            if let Err(error) = replace(path, after, &before.content)
                .and_then(|_| sync_dir(path.parent().expect("validated parent")))
            {
                reverted = false;
                eprintln!(
                    "Could not revert {} safely: {error:#}; private backups are in {}",
                    path.display(),
                    self.backups.display()
                );
            }
        }
        if reverted {
            if let Err(error) = journal_remove(&self.journal) {
                eprintln!("Could not clear recovery journal: {error:#}");
            }
        }
    }
}
