//! Exercise client selection through the public CLI against isolated real servers.
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

struct Server {
    home: TempDir,
    binary: PathBuf,
    socket: PathBuf,
    path: OsString,
    shim: PathBuf,
}

impl Server {
    fn new() -> Self {
        let path = env::var_os("PATH").expect("integration tests require tmux on PATH");
        let binary = env::split_paths(&path)
            .map(|directory| directory.join("tmux"))
            .find(|candidate| {
                fs::metadata(candidate).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
            })
            .expect("integration tests require tmux 3.3a or newer on PATH")
            .canonicalize()
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        // Keep the socket name short even under macOS's long default TMPDIR.
        let socket = home.path().join("s");
        assert!(socket.as_os_str().len() < 104, "socket path too long");
        for directory in ["config", "data", "runtime", "bin"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(home.path().join(directory))
                .unwrap();
        }
        let shim = home.path().join("bin/tmux");
        // Only the reported version differs; metadata uses the real wire protocol.
        fs::write(
            &shim,
            format!(
                "#!/bin/sh\nif [ \"$1\" = '-V' ]; then\n  printf '%s\\n' 'tmux 0.0'\n  exit 0\nfi\nexec '{}' \"$@\"\n",
                binary.to_str().unwrap().replace('\'', "'\\''")
            ),
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
        let server = Self {
            home,
            binary,
            socket,
            path,
            shim,
        };
        success(server.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "fixture",
            "/bin/sh",
        ]));
        assert!(success(server.tmux(&["list-clients"])).is_empty());
        server
    }

    fn command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("XDG_RUNTIME_DIR", self.home.path().join("runtime"))
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("AFT_TMUX_BINARY")
            .current_dir(self.home.path())
            .stdin(Stdio::null());
        command
    }

    fn tmux(&self, args: &[&str]) -> Output {
        self.command(&self.binary)
            .arg("-S")
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap()
    }

    fn cli(&self, args: &[&str], explicit_binary: Option<&Path>) -> Output {
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_agent-float-term")));
        let path = env::join_paths(
            std::iter::once(self.shim.parent().unwrap().to_path_buf())
                .chain(env::split_paths(&self.path)),
        )
        .unwrap();
        command
            .env("PATH", path)
            .args(args)
            .arg("--socket")
            .arg(&self.socket);
        if let Some(binary) = explicit_binary {
            command.env("AFT_TMUX_BINARY", binary);
        }
        command.output().unwrap()
    }

    fn record(&self) -> serde_json::Value {
        let records: Vec<_> = fs::read_dir(self.home.path().join("state/agent-float-term/runtime"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        assert_eq!(records.len(), 1, "expected one per-socket binding record");
        serde_json::from_slice(&fs::read(&records[0]).unwrap()).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Never use the PATH shim or the user's default server for teardown.
        let _ = self
            .command(&self.binary)
            .arg("-S")
            .arg(&self.socket)
            .arg("kill-server")
            .output();
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "status: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn mismatch_without_record_rejects_doctor_and_bind_without_mutations() {
    let server = Server::new();
    let version = success(server.tmux(&["display-message", "-p", "#{version}"]));
    assert_ne!(version.trim(), "0.0");
    success(server.tmux(&[
        "bind-key",
        "-T",
        "root",
        "F7",
        "display-message",
        "fixture existing binding",
    ]));
    let keys = success(server.tmux(&["list-keys"]));
    let sessions = success(server.tmux(&["list-sessions", "-F", "#{session_id}:#{session_name}"]));
    assert!(!server.home.path().join("state").exists());

    for args in [&["doctor"][..], &["bind", "--replace-key"][..]] {
        let output = server.cli(args, None);
        assert!(
            !output.status.success(),
            "{args:?} accepted a mismatched client"
        );
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains(&format!(
                "tmux client 0.0 does not match running server {}",
                version.trim()
            )),
            "{error}"
        );
        assert!(error.contains("AFT_TMUX_BINARY"), "{error}");
        assert!(error.contains("run bind"), "{error}");
        assert!(
            error.contains("Existing sessions were preserved"),
            "{error}"
        );
        assert_eq!(success(server.tmux(&["list-keys"])), keys);
        assert!(success(server.tmux(&["show-options", "-gqv", "@aft_generation"])).is_empty());
        assert_eq!(
            success(server.tmux(&["list-sessions", "-F", "#{session_id}:#{session_name}"])),
            sessions
        );
        assert!(!server.home.path().join("state").exists());
    }
}

#[test]
fn bind_rejects_executable_replaced_during_version_check_without_mutations() {
    for explicit in [false, true] {
        let server = Server::new();
        let version = success(server.tmux(&["display-message", "-p", "#{version}"]));
        assert_ne!(version.trim(), "0.0");
        success(server.tmux(&[
            "bind-key",
            "-T",
            "root",
            "F7",
            "display-message",
            "fixture existing binding",
        ]));
        let keys = success(server.tmux(&["list-keys"]));
        let sessions =
            success(server.tmux(&["list-sessions", "-F", "#{session_id}:#{session_name}"]));
        let counter = server.home.path().join("version-calls");
        fs::write(&counter, "").unwrap();
        fs::set_permissions(&counter, fs::Permissions::from_mode(0o600)).unwrap();
        let replacement = server.home.path().join("bin/replacement");
        fs::copy(&server.shim, &replacement).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).unwrap();
        let payload = fs::read(&replacement).unwrap();
        // matching_client checks -V first. Replace on compatible's second call,
        // inside bind's stamp bracket, while still reporting the matching version.
        fs::write(
            &server.shim,
            format!(
                "#!/bin/sh\nset -eu\nif [ \"$1\" = '-V' ]; then\n  printf 'call\\n' >> '{counter}'\n  if [ \"$(wc -l < '{counter}')\" -eq 2 ]; then\n    mv '{replacement}' '{shim}'\n  fi\n  printf '%s\\n' 'tmux {version}'\n  exit 0\nfi\nexec '{binary}' \"$@\"\n",
                counter = counter.to_str().unwrap().replace('\'', "'\\''"),
                replacement = replacement.to_str().unwrap().replace('\'', "'\\''"),
                shim = server.shim.to_str().unwrap().replace('\'', "'\\''"),
                version = version.trim().replace('\'', "'\\''"),
                binary = server.binary.to_str().unwrap().replace('\'', "'\\''"),
            ),
        )
        .unwrap();
        assert!(!server.home.path().join("state").exists());

        let output = server.cli(
            &["bind", "--replace-key"],
            explicit.then_some(server.shim.as_path()),
        );
        assert!(
            !output.status.success(),
            "accepted version race (explicit={explicit})"
        );
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("tmux executable changed while checking its version; retry bind"),
            "wrong refusal (explicit={explicit}): {error}"
        );
        assert_eq!(fs::read_to_string(&counter).unwrap(), "call\ncall\n");
        assert!(
            !replacement.exists(),
            "replacement was not moved into place"
        );
        assert_eq!(fs::read(&server.shim).unwrap(), payload);
        assert_eq!(
            success(server.command(&server.shim).arg("-V").output().unwrap()),
            "tmux 0.0\n"
        );
        assert_eq!(success(server.tmux(&["list-keys"])), keys);
        assert!(success(server.tmux(&["show-options", "-gqv", "@aft_generation"])).is_empty());
        assert_eq!(
            success(server.tmux(&["list-sessions", "-F", "#{session_id}:#{session_name}"])),
            sessions
        );
        let runtime = server.home.path().join("state/agent-float-term/runtime");
        match fs::read_dir(&runtime) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.unwrap().path();
                    assert_eq!(
                        path.extension().and_then(|extension| extension.to_str()),
                        Some("lock"),
                        "failed bind persisted state or a false cached stamp: {}",
                        path.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot inspect {}: {error}", runtime.display()),
        }
    }
}

#[test]
fn approved_client_is_reused_and_preserved_across_path_mismatch_rebinds() {
    let server = Server::new();
    let version = success(server.tmux(&["display-message", "-p", "#{version}"]));
    success(server.cli(&["bind"], Some(&server.binary)));
    let record = server.record();
    assert_eq!(record["binary"], server.binary.to_str().unwrap());
    assert_eq!(record["socket"], server.socket.to_str().unwrap());
    assert_eq!(record["key"], "F7");
    let installed = record["installed"].as_str().unwrap();
    assert!(installed.contains("AFT_TMUX_BINARY="), "{installed}");
    assert!(
        installed.contains(server.binary.to_str().unwrap()),
        "{installed}"
    );
    assert!(
        !installed.contains(server.shim.to_str().unwrap()),
        "{installed}"
    );
    let keys = success(server.tmux(&["list-keys"]));
    let client = format!(
        "tmux client: {} ({})",
        server.binary.display(),
        version.trim()
    );

    // Each invocation starts fresh with only the mismatched shim selected by PATH.
    for _ in 0..2 {
        let doctor = success(server.cli(&["doctor"], None));
        assert!(doctor.lines().any(|line| line == client), "{doctor}");
        assert!(doctor.contains("Owned key binding: intact"), "{doctor}");
        assert!(!doctor.contains(server.shim.to_str().unwrap()), "{doctor}");
        assert_eq!(server.record(), record, "doctor must not rewrite state");
        success(server.cli(&["bind"], None));
        assert_eq!(success(server.tmux(&["list-keys"])), keys);
        assert_eq!(
            server.record(),
            record,
            "rebind must retain the approved client"
        );
    }
}

#[test]
fn doctor_reports_last_popup_failure_without_creating_state() {
    let server = Server::new();
    success(server.tmux(&[
        "set-option",
        "-g",
        "@aft_last_error",
        "pane %0: fixture popup failure",
    ]));
    let keys = success(server.tmux(&["list-keys"]));
    assert!(!server.home.path().join("state").exists());
    let doctor = success(server.cli(&["doctor"], Some(&server.binary)));
    assert!(
        doctor
            .lines()
            .any(|line| line == "Last popup failure: pane %0: fixture popup failure"),
        "{doctor}"
    );
    assert!(
        doctor.contains("Owned key binding: absent or changed; run bind"),
        "{doctor}"
    );
    assert!(!doctor.contains("show-options"), "{doctor}");
    assert!(!doctor.contains("set-option"), "{doctor}");
    assert_eq!(success(server.tmux(&["list-keys"])), keys);
    assert!(!server.home.path().join("state").exists());
}
