//! Exercise the public installer against executable payloads, not only file fixtures.
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn invoke(binary: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_RUNTIME_DIR", home.join("runtime"))
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

struct BrewFixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    stable: PathBuf,
    keg: PathBuf,
    socket: PathBuf,
}

impl BrewFixture {
    fn new() -> Self {
        // Keep the private socket below macOS's sockaddr_un length limit.
        let temp = tempfile::Builder::new()
            .prefix("aft-brew-")
            .tempdir_in("/tmp")
            .unwrap();
        let home = temp.path().canonicalize().unwrap();
        let prefix = home.join("brew");
        let keg = prefix.join("Cellar/agent-float-term/1/bin/agent-float-term");
        fs::create_dir_all(keg.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_agent-float-term"), &keg).unwrap();
        fs::set_permissions(&keg, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(prefix.join("opt")).unwrap();
        fs::create_dir_all(prefix.join("bin")).unwrap();
        std::os::unix::fs::symlink(
            "../Cellar/agent-float-term/1",
            prefix.join("opt/agent-float-term"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "../Cellar/agent-float-term/1/bin/agent-float-term",
            prefix.join("bin/agent-float-term"),
        )
        .unwrap();
        let stable = prefix.join("opt/agent-float-term/bin/agent-float-term");
        let socket = home.join("s");
        success(invoke(
            Path::new("tmux"),
            &home,
            &[
                "-S",
                socket.to_str().unwrap(),
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "test",
                "sleep 120",
            ],
        ));
        Self {
            _temp: temp,
            home,
            stable,
            keg,
            socket,
        }
    }

    fn bind(&self, binary: &Path) -> Output {
        invoke(
            binary,
            &self.home,
            &["bind", "--socket", self.socket.to_str().unwrap()],
        )
    }
}

impl Drop for BrewFixture {
    fn drop(&mut self) {
        let _ = invoke(
            Path::new("tmux"),
            &self.home,
            &["-S", self.socket.to_str().unwrap(), "kill-server"],
        );
    }
}

#[test]
fn homebrew_first_bind_registers_opt_once_without_owning_payload_or_startup() {
    for entry in [
        "opt/agent-float-term/bin/agent-float-term",
        "bin/agent-float-term",
        "Cellar/agent-float-term/1/bin/agent-float-term",
    ] {
        let f = BrewFixture::new();
        let binary = f.home.join("brew").join(entry);
        let manifest = f.home.join("state/agent-float-term/install.json");
        let data = f.home.join("data/agent-float-term");
        let bytes = fs::read(&f.keg).unwrap();
        fs::write(f.home.join(".tmux.conf"), "# untouched tmux\n").unwrap();
        fs::write(f.home.join(".zshrc"), "# untouched shell\n").unwrap();
        for args in [
            vec!["--help"],
            vec!["--version"],
            vec!["doctor", "--socket", f.socket.to_str().unwrap()],
        ] {
            success(invoke(&binary, &f.home, &args));
            assert!(!f.home.join("state").exists());
            assert!(!f.home.join("data").exists());
        }
        let output = invoke(&binary, &f.home, &["start"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
        let output = invoke(
            &binary,
            &f.home,
            &["bind", "--socket", f.home.join("absent").to_str().unwrap()],
        );
        assert!(!output.status.success());
        assert!(!f.home.join("state").exists());
        assert!(!f.home.join("data").exists());

        success(f.bind(&binary));
        let before = fs::read(&manifest).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(value["format"], 3);
        assert_eq!(value["external"]["path"], f.stable.to_str().unwrap());
        assert_eq!(value["external"]["active"], true);
        assert!(value["blocks"].as_array().unwrap().is_empty());
        assert!(value["releases"].as_array().unwrap().is_empty());
        assert!(value["current"].is_null() && value["previous"].is_null());
        for name in ["integration.tmux", "integration.sh"] {
            assert!(fs::read_to_string(data.join(name))
                .unwrap()
                .contains(f.stable.to_str().unwrap()));
        }
        // Even user edits to templates are not a request to reinstall on a later bind.
        fs::write(data.join("integration.sh"), "# edited template\n").unwrap();
        success(f.bind(&binary));
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert_eq!(
            fs::read_to_string(data.join("integration.sh")).unwrap(),
            "# edited template\n"
        );
        assert_eq!(
            fs::read_to_string(f.home.join(".tmux.conf")).unwrap(),
            "# untouched tmux\n"
        );
        assert_eq!(
            fs::read_to_string(f.home.join(".zshrc")).unwrap(),
            "# untouched shell\n"
        );
        assert!(!data.join("current").exists());
        assert!(!data.join("releases").exists());
        assert!(!f.home.join(".local/bin/agent-float-term").exists());
        assert_eq!(fs::read(&f.keg).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&f.keg).unwrap().permissions().mode() & 0o7777,
            0o755
        );
    }
}

#[test]
fn homebrew_first_use_preserves_existing_modes_paths_and_roots() {
    for case in ["managed", "cargo", "sibling", "roots", "inactive"] {
        let f = BrewFixture::new();
        let source = Path::new(env!("CARGO_BIN_EXE_agent-float-term"));
        let manifest = f.home.join("state/agent-float-term/install.json");
        match case {
            "managed" => {
                success(invoke(source, &f.home, &["install", "--yes"]));
            }
            "cargo" => {
                let cargo = f.home.join(".cargo/bin/agent-float-term");
                fs::create_dir_all(cargo.parent().unwrap()).unwrap();
                fs::copy(source, &cargo).unwrap();
                success(invoke(
                    &cargo,
                    &f.home,
                    &[
                        "install",
                        "--external-binary",
                        cargo.to_str().unwrap(),
                        "--yes",
                    ],
                ));
            }
            "sibling" => {
                let sibling = f.home.join("brew/bin/agent-float-term");
                success(invoke(
                    &sibling,
                    &f.home,
                    &[
                        "install",
                        "--external-binary",
                        sibling.to_str().unwrap(),
                        "--yes",
                    ],
                ));
            }
            _ => {
                success(f.bind(&f.stable));
                if case == "inactive" {
                    fs::write(
                        f.home.join("data/agent-float-term/integration.sh"),
                        "# user edit\n",
                    )
                    .unwrap();
                    success(invoke(&f.stable, &f.home, &["uninstall", "--yes"]));
                }
            }
        }
        let before = fs::read(&manifest).unwrap();
        let output = if case == "roots" {
            let mut value: serde_json::Value = serde_json::from_slice(&before).unwrap();
            value["paths"]["data"] = f.home.join("other-data").to_str().unwrap().into();
            fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
            f.bind(&f.stable)
        } else {
            f.bind(&f.stable)
        };
        assert!(!output.status.success(), "{case}");
        let error = String::from_utf8_lossy(&output.stderr);
        if case == "roots" {
            assert!(error.contains("directory roots do not match"), "{error}");
        } else {
            assert!(error.contains("explicitly uninstall"), "{case}: {error}");
            assert_eq!(fs::read(&manifest).unwrap(), before, "{case}");
        }
    }
}

#[test]
fn homebrew_first_use_rejects_unsafe_or_mismatched_package_and_state() {
    for case in ["target", "opt", "state", "config", "data"] {
        let f = BrewFixture::new();
        match case {
            "target" => fs::set_permissions(&f.keg, fs::Permissions::from_mode(0o777)).unwrap(),
            "opt" => {
                let other = f.home.join("brew/Cellar/agent-float-term/2/bin");
                fs::create_dir_all(&other).unwrap();
                fs::copy(&f.keg, other.join("agent-float-term")).unwrap();
                let opt = f.home.join("brew/opt/agent-float-term");
                fs::remove_file(&opt).unwrap();
                std::os::unix::fs::symlink("../Cellar/agent-float-term/2", &opt).unwrap();
            }
            "state" => {
                fs::create_dir(f.home.join("state")).unwrap();
                fs::set_permissions(f.home.join("state"), fs::Permissions::from_mode(0o777))
                    .unwrap();
            }
            "config" => {
                fs::create_dir_all(f.home.join("config/agent-float-term")).unwrap();
                fs::write(
                    f.home.join("config/agent-float-term/config.json"),
                    "{\"width\":0}",
                )
                .unwrap();
            }
            "data" => {
                fs::create_dir_all(f.home.join("data/agent-float-term")).unwrap();
                fs::set_permissions(
                    f.home.join("data/agent-float-term"),
                    fs::Permissions::from_mode(0o777),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(!f.bind(&f.keg).status.success(), "{case}");
        assert!(
            !f.home.join("state/agent-float-term/install.json").exists(),
            "{case}"
        );
        assert!(
            !f.home.join("data/agent-float-term/integration.sh").exists(),
            "{case}"
        );
    }
}

#[test]
fn generic_package_roots_do_not_autoregister_on_bind() {
    for root in [".cargo", "archive", "source", "custom-prefix"] {
        let f = BrewFixture::new();
        let binary = f.home.join(root).join("bin/agent-float-term");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::copy(&f.keg, &binary).unwrap();
        success(f.bind(&binary));
        assert!(!f.home.join("state/agent-float-term/install.json").exists());
        assert!(!f.home.join("data").exists());
    }
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

#[test]
fn native_package_default_and_custom_cargo_roots_preserve_bytes_and_mode() {
    for root in [".cargo", ".local", "custom cargo root ' # \" $ [*]"] {
        let temp = tempfile::Builder::new()
            .prefix("aft native ")
            .tempdir()
            .unwrap();
        let home = temp.path();
        let binary = Path::new(env!("CARGO_BIN_EXE_agent-float-term"));
        let package_bin = home.join(root).join("bin/agent-float-term");
        fs::create_dir_all(package_bin.parent().unwrap()).unwrap();
        fs::copy(binary, &package_bin).unwrap();
        fs::set_permissions(&package_bin, fs::Permissions::from_mode(0o755)).unwrap();
        let original = fs::read(&package_bin).unwrap();
        let data = home.join("data/agent-float-term");
        let manifest = home.join("state/agent-float-term/install.json");
        let rc = home.join("selected shell ' rc");
        fs::write(&rc, "# user setting without newline").unwrap();
        let args = [
            "install",
            "--external-binary",
            package_bin.to_str().unwrap(),
            "--shell-config",
            rc.to_str().unwrap(),
            "--shell-kind",
            "bash",
        ];
        let preview = success(invoke(&package_bin, home, &args));
        assert!(preview.contains("integration-only"));
        assert!(!home.join("state").exists());
        assert!(!home.join("data").exists());
        assert!(!home.join("config").exists());
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "# user setting without newline"
        );
        // Identical bytes at a different path are not the executing package registration.
        let rejected = invoke(binary, home, &args);
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("running installation source"));
        let mut apply = args.to_vec();
        apply.push("--yes");
        success(invoke(&package_bin, home, &apply));
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(value["format"], 3);
        assert_eq!(value["external"]["path"], package_bin.to_str().unwrap());
        assert_eq!(value["external"]["active"], true);
        assert!(value["releases"].as_array().unwrap().is_empty());
        assert!(value["current"].is_null() && value["previous"].is_null());
        assert!(!data.join("releases").exists());
        assert!(!data.join("current").exists());
        if root != ".local" {
            assert!(!home.join(".local/bin/agent-float-term").exists());
        }
        let template = fs::read_to_string(data.join("integration.sh")).unwrap();
        assert!(template.contains(&agent_float_term::install::shell_quote(
            package_bin.to_str().unwrap()
        )));
        success(invoke(
            Path::new("/bin/bash"),
            home,
            &["-n", data.join("integration.sh").to_str().unwrap()],
        ));
        let before = fs::read(&manifest).unwrap();
        success(invoke(&package_bin, home, &["install", "--yes"]));
        assert_eq!(fs::read(&manifest).unwrap(), before);
        for args in [
            vec!["rollback"],
            vec!["update", "--from", "/missing", "--sha256", "invalid"],
        ] {
            let output = invoke(&package_bin, home, &args);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("package manager"));
            assert_eq!(fs::read(&manifest).unwrap(), before);
        }
        assert_eq!(fs::read(&package_bin).unwrap(), original);
        assert_eq!(
            fs::metadata(&package_bin).unwrap().permissions().mode() & 0o7777,
            0o755
        );
        success(invoke(&package_bin, home, &["uninstall"]));
        assert_eq!(fs::read(&manifest).unwrap(), before);
        success(invoke(&package_bin, home, &["uninstall", "--yes"]));
        assert!(!manifest.exists());
        assert!(!data.join("integration.sh").exists());
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "# user setting without newline"
        );
        assert_eq!(fs::read(&package_bin).unwrap(), original);
        assert_eq!(
            fs::metadata(&package_bin).unwrap().permissions().mode() & 0o7777,
            0o755
        );
        assert!(!fs::symlink_metadata(&package_bin)
            .unwrap()
            .file_type()
            .is_symlink());
        success(invoke(&package_bin, home, &["--version"]));
    }
}
