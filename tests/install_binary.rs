//! Exercise the public installer against executable payloads, not only file fixtures.
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn invoke(binary: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("SHELL", "/bin/sh")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("AFT_TMUX_BINARY")
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn actual_payload_install_update_rollback_and_uninstall() {
    let temp = tempfile::Builder::new()
        .prefix("aft install ")
        .tempdir()
        .unwrap();
    let home = temp.path();
    let binary = Path::new(env!("CARGO_BIN_EXE_agent-float-term"));
    let installed = home.join(".local/bin/agent-float-term");
    let expected = format!("agent-float-term {}", env!("CARGO_PKG_VERSION"));

    assert!(success(invoke(binary, home, &["install"])).contains("Preview only"));
    assert!(!home.join("state").exists());
    success(invoke(binary, home, &["install", "--yes"]));
    assert_eq!(
        success(invoke(&installed, home, &["--version"])).trim(),
        expected
    );
    assert!(!home.join(".tmux.conf").exists());
    assert!(!home.join(".bashrc").exists());
    let initial = installed.canonicalize().unwrap();
    success(invoke(&installed, home, &["install", "--yes"]));
    assert_eq!(initial, installed.canonicalize().unwrap());

    // An unchanged, signed system executable supplies a second harmless payload.
    // Appending bytes to a Mach-O fixture would invalidate its signature on ARM64.
    let update = home.join("alternate-payload");
    fs::copy("/usr/bin/true", &update).unwrap();
    let hash = format!("{:x}", Sha256::digest(fs::read(&update).unwrap()));
    let update_path = update.to_str().unwrap();
    let rejected = invoke(
        binary,
        home,
        &["update", "--from", update_path, "--sha256", &"0".repeat(64)],
    );
    assert!(!rejected.status.success());
    assert_eq!(initial, installed.canonicalize().unwrap());
    success(invoke(
        binary,
        home,
        &["update", "--from", update_path, "--sha256", &hash],
    ));
    assert_ne!(initial, installed.canonicalize().unwrap());
    success(invoke(&installed, home, &[]));
    success(invoke(binary, home, &["rollback"]));
    assert_eq!(initial, installed.canonicalize().unwrap());
    assert_eq!(
        success(invoke(&installed, home, &["--version"])).trim(),
        expected
    );

    assert!(success(invoke(&installed, home, &["uninstall"])).contains("Preview only"));
    assert!(installed.exists());
    success(invoke(&installed, home, &["uninstall", "--yes"]));
    assert!(!installed.exists());
    success(invoke(binary, home, &["uninstall", "--yes"]));
}
