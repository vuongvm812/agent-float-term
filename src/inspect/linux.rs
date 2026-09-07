use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{ensure, Context, Result};

use super::linux_stat::parse_stat;
use super::{
    within_budget, Descriptors, ExecutableId, Identity, Input, MAX_ARGS, MAX_ARG_BYTES, MAX_FDS,
    MAX_PROCESSES,
};

pub(super) fn device(dev: u64) -> u64 {
    // SAFETY: these libc helpers only decode the integer device number; every dev_t
    // bit pattern is valid and no pointers or external resources are involved.
    let (major, minor) = unsafe { (libc::major(dev), libc::minor(dev)) };
    u64::from((minor & 0xff) | (major << 8) | ((minor & !0xff) << 12))
}

fn read_bounded(path: &str, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "process data exceeds inspection limit",
        ));
    }
    Ok(bytes)
}

pub(super) fn identity(pid: u32) -> Result<Option<Identity>> {
    let bytes = match read_bounded(&format!("/proc/{pid}/stat"), 8192) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("cannot read process identity"),
    };
    let value = parse_stat(&bytes)?;
    ensure!(value.pid == pid, "process identity mismatch");
    Ok(Some(value))
}

pub(super) fn list(group: u32, start: Instant) -> Result<Vec<Identity>> {
    let mut values = Vec::new();
    let mut count = 0;
    for entry in fs::read_dir("/proc").context("cannot enumerate processes")? {
        within_budget(start)?;
        let entry = entry.context("cannot enumerate process")?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        count += 1;
        ensure!(
            count <= MAX_PROCESSES,
            "process count exceeds inspection limit"
        );
        if let Some(value) = identity(pid)? {
            if value.group == group {
                values.push(value);
            }
        }
    }
    Ok(values)
}

pub(super) fn executable(pid: u32) -> Result<(PathBuf, ExecutableId)> {
    let executable =
        fs::read_link(format!("/proc/{pid}/exe")).context("cannot read process executable")?;
    // A removed/replaced executable must not resolve to a different file at its former path.
    use std::os::unix::ffi::OsStrExt;
    ensure!(
        !executable.as_os_str().as_bytes().ends_with(b" (deleted)"),
        "process executable was deleted"
    );
    let metadata =
        fs::metadata(format!("/proc/{pid}/exe")).context("cannot stat process executable")?;
    Ok((executable, ExecutableId::from_metadata(&metadata)))
}

pub(super) fn arguments(pid: u32) -> Result<Vec<OsString>> {
    let bytes = read_bounded(&format!("/proc/{pid}/cmdline"), MAX_ARG_BYTES)
        .context("cannot read process arguments")?;
    ensure!(
        !bytes.is_empty() && bytes.last() == Some(&0),
        "incomplete process arguments"
    );
    let argv: Vec<_> = bytes[..bytes.len() - 1]
        .split(|b| *b == 0)
        .map(|v| OsString::from_vec(v.to_vec()))
        .collect();
    ensure!(
        argv.len() <= MAX_ARGS,
        "argument count exceeds inspection limit"
    );
    Ok(argv)
}

pub(super) fn descriptors(pid: u32, devices: &[u64; 3], start: Instant) -> Result<Descriptors> {
    let mut evidence = Descriptors {
        stdin: Input::Other,
        extra_terminal: false,
    };
    // stat follows the procfs magic link for metadata only. Never open a target FD:
    // even a nonblocking read could consume the helper's protocol or terminal input.
    for (count, entry) in fs::read_dir(format!("/proc/{pid}/fd"))
        .context("cannot enumerate foreground helper FDs")?
        .enumerate()
    {
        within_budget(start)?;
        ensure!(
            count < MAX_FDS,
            "foreground helper FD count exceeds inspection limit"
        );
        let entry = entry.context("cannot enumerate foreground helper FD")?;
        let fd = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .context("invalid foreground helper FD metadata")?;
        if fd == 1 || fd == 2 {
            continue;
        }
        let metadata = match fs::metadata(entry.path()) {
            Ok(value) => value,
            // Unrelated event-loop FDs may close during enumeration. FD 0 must exist.
            Err(error) if fd >= 3 && error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).context("cannot inspect foreground helper FD metadata")
            }
        };
        let kind = metadata.file_type();
        let dev = device(metadata.rdev());
        if fd == 0 {
            evidence.stdin = if kind.is_fifo() {
                Input::Pipe
            } else if kind.is_socket() {
                Input::Socket
            } else if kind.is_char_device() && dev == devices[0] {
                Input::Null
            } else {
                Input::Other
            };
        } else if kind.is_char_device() && !devices.contains(&dev) {
            // Unknown character devices include /dev/tty and any PTY, regardless
            // of pathname or access flags. Only these three known devices are benign.
            evidence.extra_terminal = true;
        }
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_kernel_identity_and_command() {
        let pid = std::process::id();
        assert_eq!(identity(pid).unwrap().unwrap().pid, pid);
        let (exe, _) = executable(pid).unwrap();
        let argv = arguments(pid).unwrap();
        assert!(exe.is_absolute());
        assert!(!argv.is_empty());
    }
}
