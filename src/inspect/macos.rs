use std::ffi::OsString;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use libc::{proc_bsdinfo as BsdInfo, proc_listpids, proc_pidinfo, proc_pidpath};

use super::{
    within_budget, Descriptors, ExecutableId, Identity, Input, Liveness, MAX_ARGS, MAX_ARG_BYTES,
    MAX_FDS, MAX_PROCESSES,
};

// <libproc.h>; this selector is not exported by the pinned libc version.
const PROC_PGRP_ONLY: u32 = 2;

// Mirrored from the public SDK's <sys/proc_info.h>. libc 0.2.169 supplies
// vnode_info/vinfo_stat, but not these enclosing structs or selector constants.
const PROC_PIDLISTFDS: i32 = 1;
const PROC_PIDFDVNODEINFO: i32 = 1;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FdInfo {
    fd: i32,
    kind: u32,
}

#[repr(C)]
struct FileInfo {
    open_flags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guard_flags: u32,
}

#[repr(C)]
struct VnodeFdInfo {
    file: FileInfo,
    vnode: libc::vnode_info,
}

pub(super) fn descriptors(pid: u32, devices: &[u64; 3], start: Instant) -> Result<Descriptors> {
    let mut fds = vec![FdInfo::default(); MAX_FDS + 1];
    let bytes = fds.len() * size_of::<FdInfo>();
    // SAFETY: FdInfo mirrors proc_fdinfo's C layout and the initialized allocation
    // has exactly bytes writable bytes. libproc does not retain its pointer.
    let count = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            bytes as i32,
        )
    };
    ensure!(
        count > 0,
        "cannot enumerate foreground helper FDs (task permissions may deny metadata access)"
    );
    ensure!(
        (count as usize) < bytes && count as usize % size_of::<FdInfo>() == 0,
        "foreground helper FD count exceeds inspection limit"
    );
    let mut evidence = Descriptors {
        stdin: Input::Other,
        extra_terminal: false,
    };
    for fd in &fds[..count as usize / size_of::<FdInfo>()] {
        within_budget(start)?;
        ensure!(fd.fd >= 0, "invalid foreground helper FD metadata");
        if fd.fd == 1 || fd.fd == 2 {
            continue;
        }
        let input = match fd.kind {
            1 => {
                let mut info = MaybeUninit::<VnodeFdInfo>::uninit();
                // SAFETY: VnodeFdInfo mirrors vnode_fdinfo, including libc's SDK
                // vnode_info. The buffer is aligned and exactly the advertised size;
                // we assume initialization only after an exact full-size result.
                let count = unsafe {
                    libc::proc_pidfdinfo(
                        pid as i32,
                        fd.fd,
                        PROC_PIDFDVNODEINFO,
                        info.as_mut_ptr().cast(),
                        size_of::<VnodeFdInfo>() as i32,
                    )
                };
                if count <= 0 {
                    let error = io::Error::last_os_error();
                    if fd.fd >= 3 && error.raw_os_error() == Some(libc::EBADF) {
                        continue;
                    }
                    return Err(error).context("cannot inspect foreground helper FD metadata (task permissions may deny access)");
                }
                ensure!(
                    count as usize == size_of::<VnodeFdInfo>(),
                    "incomplete foreground helper vnode metadata"
                );
                // SAFETY: the exact-size successful call initialized the struct;
                // its fields are integer-only C structs with no invalid bit patterns.
                let info = unsafe { info.assume_init() };
                let stat = info.vnode.vi_stat;
                let kind = stat.vst_mode & libc::S_IFMT;
                let dev = u64::from(stat.vst_rdev);
                if fd.fd >= 3 && kind == libc::S_IFCHR && !devices.contains(&dev) {
                    evidence.extra_terminal = true;
                }
                if kind == libc::S_IFIFO {
                    Input::Pipe
                } else if kind == libc::S_IFSOCK {
                    Input::Socket
                } else if kind == libc::S_IFCHR && dev == devices[0] {
                    Input::Null
                } else {
                    Input::Other
                }
            }
            2 => Input::Socket,
            6 => Input::Pipe,
            // Known non-vnode types: shared memory/semaphores, kqueues, events,
            // network policy, channels, nexus. Unknown types fail conservatively.
            3 | 4 | 5 | 7 | 9 | 10 | 11 => Input::Other,
            _ => anyhow::bail!("unknown foreground helper FD type"),
        };
        if fd.fd == 0 {
            evidence.stdin = input;
        }
    }
    Ok(evidence)
}

pub(super) fn device(dev: u64) -> u64 {
    u64::from(dev as u32)
}

pub(super) fn identity(pid: u32) -> Result<Option<Identity>> {
    ensure!(pid > 0 && pid <= i32::MAX as u32, "invalid process PID");
    let mut info = MaybeUninit::<BsdInfo>::uninit();
    // SAFETY: BsdInfo has the C ABI layout; the pointer has exactly the advertised
    // writable size. We read it only if libproc reports that the entire struct was filled.
    // Clear this thread's errno so a zero result cannot inherit a stale ESRCH.
    let count = unsafe {
        *libc::__error() = 0;
        proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size_of::<BsdInfo>() as i32,
        )
    };
    if count <= 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        if error.raw_os_error() == Some(libc::ENOENT) {
            return super::confirm_missing(pid);
        }
        return Err(error).context("cannot read process identity");
    }
    ensure!(
        count as usize == size_of::<BsdInfo>(),
        "incomplete process identity"
    );
    // SAFETY: successful proc_pidinfo above initialized every byte of the C struct;
    // all fields are integers/byte arrays, for which all bit patterns are valid.
    let info = unsafe { info.assume_init() };
    ensure!(info.pbi_pid == pid, "process identity mismatch");
    Ok(Some(Identity {
        pid,
        parent: info.pbi_ppid,
        group: info.pbi_pgid,
        tty: u64::from(info.e_tdev),
        foreground: info.e_tpgid,
        started: (info.pbi_start_tvsec, info.pbi_start_tvusec),
        // <sys/proc.h>: SIDL=1, SRUN=2, SSLEEP=3, SSTOP=4, SZOMB=5.
        runnable: matches!(info.pbi_status, 2 | 3),
        liveness: match info.pbi_status {
            1..=4 => Liveness::Alive,
            5 => Liveness::Exited,
            _ => Liveness::Unknown,
        },
    }))
}

pub(super) fn list(group: u32, start: Instant) -> Result<Vec<Identity>> {
    let mut pids = vec![0i32; MAX_PROCESSES + 1];
    let bytes = pids.len() * size_of::<i32>();
    // SAFETY: pids is a live, initialized, aligned i32 allocation with bytes writable bytes.
    // libproc writes at most the supplied size and does not retain the pointer.
    let count = unsafe {
        proc_listpids(
            PROC_PGRP_ONLY,
            group,
            pids.as_mut_ptr().cast(),
            bytes as i32,
        )
    };
    ensure!(count > 0, "cannot enumerate processes");
    ensure!(
        (count as usize) < bytes && count as usize % size_of::<i32>() == 0,
        "process count exceeds inspection limit"
    );
    let mut values = Vec::new();
    for &pid in &pids[..count as usize / size_of::<i32>()] {
        within_budget(start)?;
        if pid > 0 {
            if let Some(value) = identity(pid as u32)? {
                values.push(value);
            }
        }
    }
    Ok(values)
}

pub(super) fn executable(pid: u32) -> Result<(PathBuf, ExecutableId)> {
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: path provides path.len() writable bytes, pid has been validated by identity,
    // and libproc does not retain the buffer. We check the returned length before use.
    let count = unsafe { proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    ensure!(
        count > 0 && (count as usize) < path.len(),
        "cannot read process executable"
    );
    let end = path
        .iter()
        .position(|b| *b == 0)
        .context("unterminated process executable")?;
    let executable = PathBuf::from(OsString::from_vec(path[..end].to_vec()));
    let metadata = std::fs::metadata(&executable).context("cannot stat process executable")?;
    Ok((executable, ExecutableId::from_metadata(&metadata)))
}

pub(super) fn arguments(pid: u32) -> Result<Vec<OsString>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut bytes = vec![0u8; MAX_ARG_BYTES];
    let mut length = bytes.len();
    // SAFETY: mib has three initialized c_int entries, bytes has length writable bytes,
    // and length points to a valid size_t. Null newp/zero newlen make this read-only.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    ensure!(
        result == 0 && length <= bytes.len(),
        "cannot read process arguments"
    );
    bytes.truncate(length);
    parse_args(&bytes)
}

fn parse_args(bytes: &[u8]) -> Result<Vec<OsString>> {
    ensure!(
        bytes.len() >= size_of::<i32>(),
        "truncated process arguments"
    );
    let argc = i32::from_ne_bytes(bytes[..4].try_into().expect("four-byte prefix"));
    ensure!(
        argc > 0 && argc as usize <= MAX_ARGS,
        "invalid process argument count"
    );
    // KERN_PROCARGS2: argc, executable\0, padding\0, argv[0]\0 ... argv[argc-1]\0.
    // The syscall may also return environment bytes; never parse, inspect, or retain them.
    let mut cursor = 4
        + bytes[4..]
            .iter()
            .position(|b| *b == 0)
            .context("missing executable terminator")?
        + 1;
    while bytes.get(cursor) == Some(&0) {
        cursor += 1;
    }
    let mut argv = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        let remaining = bytes.get(cursor..).context("truncated process argument")?;
        let end = remaining
            .iter()
            .position(|b| *b == 0)
            .context("missing argument terminator")?;
        argv.push(OsString::from_vec(remaining[..end].to_vec()));
        cursor += end + 1;
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bsd_info_matches_apple_abi() {
        assert_eq!(size_of::<FdInfo>(), 8);
        assert_eq!(size_of::<FileInfo>(), 24);
        assert_eq!(size_of::<libc::vinfo_stat>(), 136);
        assert_eq!(size_of::<libc::vnode_info>(), 152);
        assert_eq!(size_of::<VnodeFdInfo>(), 176);
        assert_eq!(std::mem::offset_of!(VnodeFdInfo, vnode), 24);
        assert_eq!(std::mem::offset_of!(libc::vinfo_stat, vst_rdev), 116);
        assert_eq!(size_of::<BsdInfo>(), 136);
        assert_eq!(std::mem::offset_of!(BsdInfo, pbi_pgid), 100);
        assert_eq!(std::mem::offset_of!(BsdInfo, e_tdev), 108);
        assert_eq!(std::mem::offset_of!(BsdInfo, e_tpgid), 112);
        assert_eq!(std::mem::offset_of!(BsdInfo, pbi_start_tvsec), 120);
    }

    #[test]
    fn arguments_stop_at_argc_and_preserve_empty_values() {
        let mut bytes = 3i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(b"/bin/test\0\0test\0\0hello world\0SECRET=ignored\0");
        assert_eq!(
            parse_args(&bytes).unwrap(),
            vec![
                OsString::from("test"),
                OsString::new(),
                OsString::from("hello world")
            ]
        );
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&bytes[..15]).is_err());
    }

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
