//! Exercise the gate from outside the inspected process's controlling terminal.
use std::ffi::OsString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use super::*;

struct Job(Child);

impl Drop for Job {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn terminal() -> (File, File, PathBuf) {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: both output descriptor pointers are valid. Null optional arguments ask
    // the OS for default terminal settings, without writing any name buffer.
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
    // SAFETY: successful openpty transferred two distinct owned descriptors to us.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // SAFETY: these are owned live FDs; F_SETFD changes only inheritance flags.
        let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_eq!(result, 0);
    }
    let mut name = [0u8; 1024];
    // SAFETY: slave is live and name is writable for the stated length; ttyname_r
    // does not retain the buffer. A successful call NUL-terminates the result.
    let result =
        unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr().cast(), name.len()) };
    assert_eq!(result, 0);
    let end = name.iter().position(|b| *b == 0).unwrap();
    let tty = PathBuf::from(OsString::from_vec(name[..end].to_vec()));
    (master, slave, tty)
}

#[test]
fn mapped_foreground_job_on_separate_pty_and_stopped_job() {
    let (_master, slave, tty) = terminal();
    let executable = Path::new("/bin/sleep").canonicalize().unwrap();
    let mut command = Command::new(&executable);
    command
        .arg("30")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: the pre-exec closure performs only async-signal-safe libc calls and
    // constructs OS errors; stdin has already been set to the child's PTY slave.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut job = Job(command.spawn().unwrap());
    let pid = job.0.id();
    let mappings = [HarnessMapping {
        harness: "codex".into(),
        path: executable,
    }];
    // This test explicitly opts sleep into the classifier. It tests OS plumbing,
    // not the authenticity of an AI binary or its actual keyboard ownership.
    let decision = eligible(pid, &tty, &mappings).unwrap();
    assert!(decision.eligible);
    let invocation = decision.invocation.unwrap();
    assert_eq!(invocation.liveness(), Liveness::Alive);
    assert!(!eligible(pid, &tty, &[]).unwrap().eligible);
    assert!(eligible(pid, Path::new("/dev/null"), &mappings).is_err());
    // SAFETY: pid is the live child owned by Job; SIGSTOP changes no parent memory.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
    let deadline = Instant::now();
    while platform::identity(pid).unwrap().unwrap().runnable {
        assert!(deadline.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!eligible(pid, &tty, &mappings).unwrap().eligible);
    assert_eq!(invocation.liveness(), Liveness::Alive);
    // Leave the child unreaped so zombie metadata also counts as an ended invocation.
    job.0.kill().unwrap();
    let deadline = Instant::now();
    while invocation.liveness() != Liveness::Exited {
        assert!(deadline.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(5));
    }
    job.0.wait().unwrap();
    assert_eq!(invocation.liveness(), Liveness::Exited);
    assert_eq!(confirm_missing(pid).unwrap(), None);
}

struct FixtureJob(Child);

impl Drop for FixtureJob {
    fn drop(&mut self) {
        // SAFETY: this PID belongs to our isolated fixture. Its handler reaps its
        // children before exiting; no live user process or process group is signaled.
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if let Some(status) = self.0.try_wait().ok().flatten() {
                if !std::thread::panicking() {
                    assert!(
                        status.success(),
                        "fixture detected consumed protocol input or failed cleanup"
                    );
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // SAFETY: setsid made this fixture's PID its private process group ID.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

// A standalone C executable avoids Rust test-harness threads after fork. The
// helpers deliberately exec a binary named node with an overwritten npm-style
// argv and nonexistent argv[1]; inspecting their argv would incorrectly fail.
const WORKERS: &str = r#"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile sig_atomic_t running = 1;
static void stop(int sig) { (void)sig; running = 0; }
static void check(int ok) { if (!ok) _exit(90); }
static void reap(pid_t child) {
    kill(child, SIGTERM);
    int status = 0;
    pid_t result;
    do { result = waitpid(child, &status, 0); } while (result < 0 && errno == EINTR);
    check(result == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
static void input(const char *mode) {
    if (!strcmp(mode, "tty-input") || !strcmp(mode, "editor-chain")) return;
    int pair[2];
    if (!strcmp(mode, "socket")) {
        check(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    } else {
        check(pipe(pair) == 0);
    }
    check(write(pair[1], "protocol bytes must remain unread", 32) == 32);
    check(dup2(pair[0], 0) == 0);
    close(pair[0]);
    close(pair[1]);
    if (!strcmp(mode, "null") || !strcmp(mode, "regular-input")) {
        int fd = open(!strcmp(mode, "null") ? "/dev/null" : "/bin/sh", O_RDONLY);
        check(fd >= 0 && dup2(fd, 0) == 0);
        close(fd);
    }
    if (!strcmp(mode, "closed-input")) close(0);
}
static void worker(const char *mode, int ready, int depth) {
    signal(SIGTERM, stop);
    pid_t child = -1;
    if (depth) {
        child = fork();
        check(child >= 0);
        if (!child) {
            input("pipe");
            worker("pipe", ready, 0);
            _exit(0);
        }
    }
    const char *extra = NULL;
    if (!strcmp(mode, "extra-tty")) extra = "/dev/tty";
    if (!strcmp(mode, "extra-unknown")) extra = "/dev/zero";
    if (!strcmp(mode, "extra-null")) extra = "/dev/null";
    if (!strcmp(mode, "extra-random")) extra = "/dev/random";
    if (!strcmp(mode, "extra-urandom")) extra = "/dev/urandom";
    if (extra) check(open(extra, O_RDONLY | O_NONBLOCK) >= 3);
    if (!strcmp(mode, "extra-pty")) check(dup(2) >= 3);
    check(isatty(2)); /* gopls-style terminal stderr is intentional. */
    check(write(ready, "R", 1) == 1);
    close(ready);
    while (running) {
        if (!strcmp(mode, "churn")) {
            int pair[2];
            check(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
            check(write(pair[1], "volatile", 8) == 8);
            int fd = open("/dev/null", O_RDONLY);
            check(fd >= 0);
            close(fd);
            close(pair[0]);
            close(pair[1]);
        } else {
            usleep(1000);
        }
    }
    if (child > 0) reap(child);
    if (strcmp(mode, "tty-input") && strcmp(mode, "editor-chain") &&
        strcmp(mode, "null") && strcmp(mode, "regular-input") && strcmp(mode, "closed-input")) {
        char bytes[32];
        check(read(0, bytes, sizeof(bytes)) == 32);
        check(!memcmp(bytes, "protocol bytes must remain unread", 32));
    }
}
int main(int argc, char **argv) {
    if (argc == 6 && !strcmp(argv[5], "worker")) {
        worker(argv[2], atoi(argv[3]), atoi(argv[4]));
        return 0;
    }
    check(argc == 2);
    /* Other parallel test threads can be between openpty and CLOEXEC. Isolate
       the frontend before creating its intentional worker descriptors. */
    long max_fd = sysconf(_SC_OPEN_MAX);
    check(max_fd > 0 && max_fd <= INT_MAX);
    for (int fd = 3; fd < max_fd; fd++) close(fd);
    signal(SIGTERM, stop);
    char helper[4096];
    check(strlen(argv[0]) + 8 < sizeof(helper));
    strcpy(helper, argv[0]);
    char *base = strrchr(helper, '/');
    check(base != NULL);
    strcpy(base + 1, "node");
    int ready[2];
    check(pipe(ready) == 0);
    int depth = !strcmp(argv[1], "chain") || !strcmp(argv[1], "editor-chain");
    pid_t children[3];
    for (int i = 0; i < 3; i++) {
        children[i] = fork();
        check(children[i] >= 0);
        if (!children[i]) {
            close(ready[0]);
            const char *mode = i == 0 ? argv[1] : (i == 1 ? "socket" : "null");
            input(mode);
            char fd[24];
            snprintf(fd, sizeof(fd), "%d", ready[1]);
            char *args[] = { "npm exec renamed MCP helper", "/nonexistent/renamed-entrypoint",
                (char *)mode, fd, (i == 0 && depth) ? "1" : "0", "worker", NULL };
            execv(helper, args);
            _exit(91);
        }
    }
    close(ready[1]);
    for (int i = 0; i < 3 + depth; i++) {
        char byte;
        check(read(ready[0], &byte, 1) == 1 && byte == 'R');
    }
    close(ready[0]);
    check(write(1, "READY\n", 6) == 6);
    while (running) usleep(1000);
    for (int i = 0; i < 3; i++) reap(children[i]);
    return 0;
}
"#;

#[test]
fn same_group_helpers_use_fd_metadata_not_names_or_arguments() {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("workers.c");
    let executable = directory.path().join("frontend");
    std::fs::write(&source, WORKERS).unwrap();
    assert!(Command::new("cc")
        .args([
            "-std=c11",
            "-D_DEFAULT_SOURCE",
            "-Wall",
            "-Wextra",
            "-Werror"
        ])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap()
        .success());
    std::fs::copy(&executable, directory.path().join("node")).unwrap();
    let mappings = [HarnessMapping {
        harness: "claude".into(),
        path: executable.canonicalize().unwrap(),
    }];
    for (mode, expected) in [
        ("pipe", true),
        ("socket", true),
        ("null", true),
        ("chain", true),
        ("extra-null", true),
        ("extra-random", true),
        ("extra-urandom", true),
        ("churn", true),
        ("tty-input", false),
        ("editor-chain", false),
        ("extra-tty", false),
        ("extra-pty", false),
        ("extra-unknown", false),
        ("regular-input", false),
        ("closed-input", false),
    ] {
        let (mut master, slave, tty) = terminal();
        let mut command = Command::new(&executable);
        command
            .arg(mode)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe libc calls run between fork and exec;
        // stdin is the fixture's owned PTY slave, not a user pane.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let job = FixtureJob(command.spawn().unwrap());
        // SAFETY: master is an owned live PTY FD; this enables bounded readiness polling.
        let result = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
        assert_eq!(result, 0);
        let start = Instant::now();
        let mut output = Vec::new();
        while !output.windows(5).any(|w| w == b"READY") {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "fixture readiness: {mode}"
            );
            let mut bytes = [0; 128];
            match master.read(&mut bytes) {
                Ok(count) => output.extend_from_slice(&bytes[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("fixture readiness {mode}: {error}"),
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut invocation = None;
        for _ in 0..if mode == "churn" { 10 } else { 1 } {
            let decision = eligible(job.0.id(), &tty, &mappings)
                .unwrap_or_else(|error| panic!("{mode}: {error:#}"));
            assert_eq!(decision.eligible, expected, "{mode}: {}", decision.reason);
            assert_eq!(decision.invocation.is_some(), expected);
            if let Some(current) = decision.invocation {
                assert_eq!(current.liveness(), Liveness::Alive);
                if let Some(previous) = invocation {
                    assert_eq!(current, previous);
                }
                invocation = Some(current);
            }
        }
        let snapshot = snapshot(
            job.0.id(),
            platform::device(std::fs::metadata(&tty).unwrap().rdev()),
            job.0.id(),
            &mappings,
            Instant::now(),
        )
        .unwrap();
        assert!(snapshot.members.len() >= 4);
        for helper in snapshot
            .members
            .iter()
            .filter(|p| p.identity.pid != job.0.id())
        {
            assert!(helper.argv.is_empty(), "helper argv must never be loaded");
        }
    }
}
