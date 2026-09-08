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
    assert!(home.join("data/agent-float-term/integration.sh").is_file());
    assert!(home
        .join("data/agent-float-term/integration.tmux")
        .is_file());
    assert!(!home.join("config/agent-float-term").exists());
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

#[test]
fn actual_installer_migrates_legacy_layout_without_startup_flags() {
    let temp = tempfile::Builder::new()
        .prefix("aft migration ")
        .tempdir()
        .unwrap();
    let home = temp.path();
    let binary = Path::new(env!("CARGO_BIN_EXE_agent-float-term"));
    let rc = home.join(".zshrc");
    let data = home.join("data/agent-float-term");
    let config = home.join("config/agent-float-term");
    let manifest = home.join("state/agent-float-term/install.json");
    fs::write(&rc, "# user setting\n").unwrap();
    success(invoke(
        binary,
        home,
        &[
            "install",
            "--shell-config",
            rc.to_str().unwrap(),
            "--shell-kind",
            "zsh",
            "--yes",
        ],
    ));

    fs::create_dir_all(&config).unwrap();
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    legacy["format"] = 1.into();
    for file in legacy["files"].as_array_mut().unwrap() {
        let source = std::path::PathBuf::from(file["path"].as_str().unwrap());
        let target = config.join(source.file_name().unwrap());
        fs::rename(source, &target).unwrap();
        file["path"] = target.to_str().unwrap().into();
    }
    let block = legacy["blocks"][0]["text"]
        .as_str()
        .unwrap()
        .replace(data.to_str().unwrap(), config.to_str().unwrap());
    legacy["blocks"][0]["text"] = block.clone().into();
    fs::write(
        &rc,
        format!("# user setting\n{block}# appended user setting\n"),
    )
    .unwrap();
    fs::write(&manifest, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    fs::write(config.join("config.json"), "{\"height\":60}").unwrap();
    let before = fs::read(&rc).unwrap();
    let preview = success(invoke(binary, home, &["install"]));
    assert!(preview.contains("Migrate legacy layout"));
    assert_eq!(fs::read(&rc).unwrap(), before);
    success(invoke(binary, home, &["install", "--yes"]));
    let current: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(current["format"], 2);
    assert_eq!(current["shell_kind"], "zsh");
    assert!(data.join("integration.sh").is_file());
    assert!(data.join("integration.tmux").is_file());
    assert!(!config.join("integration.sh").exists());
    assert!(!config.join("integration.tmux").exists());
    let migrated = fs::read_to_string(&rc).unwrap();
    assert!(migrated.contains(data.join("integration.sh").to_str().unwrap()));
    assert!(migrated.starts_with("# user setting\n"));
    assert!(migrated.ends_with("# appended user setting\n"));
    success(invoke(binary, home, &["uninstall", "--yes"]));
    assert_eq!(
        fs::read_to_string(&rc).unwrap(),
        "# user setting\n# appended user setting\n"
    );
    assert_eq!(
        fs::read_to_string(config.join("config.json")).unwrap(),
        "{\"height\":60}"
    );
}
