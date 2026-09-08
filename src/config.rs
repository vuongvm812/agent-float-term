//! User configuration and private application directories.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessMapping {
    pub harness: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(rename = "shortcut")]
    pub key: String,
    pub width: u8,
    pub height: u8,
    pub shell: Option<PathBuf>,
    pub harness_paths: Vec<HarnessMapping>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            key: "F7".into(),
            width: 80,
            height: 80,
            shell: None,
            harness_paths: Vec::new(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        let mut key = self.key.as_str();
        let mut modifiers = HashSet::new();
        while key.starts_with("C-") || key.starts_with("M-") || key.starts_with("S-") {
            if !modifiers.insert(&key[..2]) {
                bail!("duplicate shortcut modifier");
            }
            key = &key[2..];
        }
        let function_key = key.strip_prefix('F').is_some_and(|n| {
            n.parse::<u8>()
                .is_ok_and(|n| (1..=12).contains(&n) && key == format!("F{n}"))
        });
        if !(function_key
            || (key.len() == 1 && (key.as_bytes()[0].is_ascii_alphanumeric() || key == "@"))
            || matches!(
                key,
                "Space"
                    | "Enter"
                    | "Escape"
                    | "BSpace"
                    | "Tab"
                    | "BTab"
                    | "Up"
                    | "Down"
                    | "Left"
                    | "Right"
                    | "Home"
                    | "End"
                    | "PPage"
                    | "NPage"
                    | "DC"
                    | "IC"
            ))
        {
            bail!("shortcut must be a safe tmux key (for example F7, C-a, or M-Space)");
        }
        if !(10..=100).contains(&self.width) || !(10..=100).contains(&self.height) {
            bail!("width and height must each be between 10 and 100");
        }
        if let Some(shell) = &self.shell {
            executable(shell).context("invalid shell")?;
        }
        let mut mappings = HashSet::new();
        for mapping in &self.harness_paths {
            if !matches!(mapping.harness.as_str(), "claude" | "codex" | "opencode") {
                bail!(
                    "unsupported harness {:?}; expected claude, codex, or opencode",
                    mapping.harness
                );
            }
            executable(&mapping.path).context("invalid harness mapping")?;
            if !mappings.insert(fs::canonicalize(&mapping.path)?) {
                bail!(
                    "duplicate or conflicting harness mapping for {}",
                    mapping.path.display()
                );
            }
        }
        Ok(())
    }
}

fn executable(path: &Path) -> Result<()> {
    checked_path(path)?;
    let metadata = fs::metadata(path).with_context(|| format!("inspect {}", path.display()))?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        bail!("{} must be an executable regular file", path.display());
    }
    // access checks the current user's actual permissions, not just an arbitrary execute bit.
    let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
    // SAFETY: cpath is a valid NUL-terminated string and access does not retain its pointer.
    if unsafe { libc::access(cpath.as_ptr(), libc::X_OK) } != 0 {
        return Err(std::io::Error::last_os_error()).context("executable is not accessible");
    }
    Ok(())
}

/// Discover paths without creating anything. Every member is a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub bin: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let home = env::var_os("HOME").context("HOME is not set")?;
        Self::from_environment(PathBuf::from(home), |name| env::var_os(name))
    }

    fn from_environment(
        home: PathBuf,
        lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Self> {
        checked_path(&home).context("invalid HOME")?;
        let xdg = |name: &str, fallback: &str| -> Result<PathBuf> {
            let root = lookup(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(fallback));
            checked_path(&root).with_context(|| format!("invalid {name}"))?;
            Ok(root.join("agent-float-term"))
        };
        Ok(Self {
            config: xdg("XDG_CONFIG_HOME", ".config")?,
            data: xdg("XDG_DATA_HOME", ".local/share")?,
            state: xdg("XDG_STATE_HOME", ".local/state")?,
            bin: home.join(".local/bin"),
        })
    }
}

pub fn load() -> Result<Config> {
    load_file(&Paths::discover()?.config.join("config.json"))
}

fn load_file(path: &Path) -> Result<Config> {
    if let Some(parent) = path.parent() {
        match fs::symlink_metadata(parent) {
            Ok(metadata)
                if !metadata.is_dir()
                    || metadata.uid() != uid()
                    || metadata.mode() & 0o022 != 0 =>
            {
                bail!("configuration directory must be user-owned, not writable by others or a symlink");
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error).context("inspect configuration directory"),
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let legacy = path.with_file_name("config.toml");
            match fs::symlink_metadata(&legacy) {
                Ok(_) => bail!(
                    "{} is no longer supported and was left unchanged; create {} as a JSON object, \
                     rename 'key' to 'shortcut', and convert any shell/harness_paths settings; \
                     use {{}} for defaults. TOML is not loaded",
                    legacy.display(),
                    path.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("inspect legacy configuration {}", legacy.display())
                    });
                }
            }
            return Ok(Config::default());
        }
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid() || metadata.mode() & 0o022 != 0 {
        bail!("configuration must be a user-owned regular file, not writable by others");
    }
    let mut text = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 1024 * 1024 {
        bail!("configuration exceeds 1 MiB");
    }
    // Serde's struct deserializer also accepts positional arrays; config is object-only.
    if !text.trim_start().starts_with('{') {
        bail!(
            "invalid config.json at {}; expected a JSON object",
            path.display()
        );
    }
    let config: Config = serde_json::from_str(&text).with_context(|| {
        format!(
            "invalid config.json at {}; expected a JSON object",
            path.display()
        )
    })?;
    config.validate()?;
    Ok(config)
}

pub(crate) fn uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

pub(crate) fn checked_path(path: &Path) -> Result<()> {
    let text = path.to_str().context("paths must be valid UTF-8")?;
    if !path.is_absolute()
        || text.chars().any(char::is_control)
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!("path must be absolute, without control characters or '..': {path:?}");
    }
    Ok(())
}

/// Create a private product directory; never chmod HOME or an existing XDG parent.
/// Existing product directories with unsafe ownership/permissions are rejected.
pub fn private_dir(path: &Path) -> Result<()> {
    checked_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
                bail!(
                    "{} must be a private, user-owned directory (mode 0700), not a symlink",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_user_dir(path.parent().context("directory has no parent")?)?;
            match DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error).context("create private directory"),
            }
            private_dir(path)?;
            fs::File::open(path.parent().context("directory has no parent")?)?.sync_all()?;
        }
        Err(error) => return Err(error).context("inspect private directory"),
    }
    Ok(())
}

pub(crate) fn ensure_user_dir(path: &Path) -> Result<()> {
    checked_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.uid() != uid() || metadata.mode() & 0o022 != 0 {
                bail!(
                    "{} must be a user-owned directory, not writable by others or a symlink",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_user_dir(path.parent().context("directory has no parent")?)?;
            match DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error).context("create user directory"),
            }
            ensure_user_dir(path)?;
            fs::File::open(path.parent().context("directory has no parent")?)?.sync_all()?;
        }
        Err(error) => return Err(error).context("inspect user directory"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn defaults_unknown_fields_and_validation() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(config.key, "F7");
        assert_eq!((config.width, config.height), (80, 80));
        config.validate().unwrap();
        for text in [
            r#"{"widht":80}"#,
            r#"{"key":"F7"}"#,
            r#"{"shortcut":"F7","shortcut":"F8"}"#,
            r#"{"width":"80"}"#,
            r#"{"width":80.5}"#,
            r#"{"height":-1}"#,
            r#"{"height":256}"#,
            r#"{"shortcut":"F7",}"#,
            "",
            "width = 80",
        ] {
            assert!(serde_json::from_str::<Config>(text).is_err(), "{text:?}");
        }
        let config: Config =
            serde_json::from_str(r#"{"shortcut":"C-a","width":10,"height":100}"#).unwrap();
        assert_eq!(config.key, "C-a");
        config.validate().unwrap();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["shortcut"], "C-a");
        assert!(json.get("key").is_none());
        let partial: Config = serde_json::from_str(r#"{"height":10}"#).unwrap();
        assert_eq!((partial.width, partial.height), (80, 10));
        assert_eq!(partial.key, "F7");
        for dimension in [0, 9, 101, 255] {
            for (width, height) in [(dimension, 80), (80, dimension)] {
                assert!(Config {
                    width,
                    height,
                    ..Config::default()
                }
                .validate()
                .is_err());
            }
        }
        for dimension in [10, 100] {
            Config {
                width: dimension,
                height: dimension,
                ..Config::default()
            }
            .validate()
            .unwrap();
        }
        for key in [
            "F7; run-shell bad",
            "#{pane_id}",
            "'",
            "\n",
            "-T",
            "F0",
            "F025",
            "C-C-a",
        ] {
            assert!(
                Config {
                    key: key.into(),
                    ..Config::default()
                }
                .validate()
                .is_err(),
                "{key:?}"
            );
        }
        for key in ["F7", "C-a", "M-Space", "C-M-Left"] {
            Config {
                key: key.into(),
                ..Config::default()
            }
            .validate()
            .unwrap();
        }
        assert!(Config {
            width: 9,
            ..Config::default()
        }
        .validate()
        .is_err());
        assert!(Config {
            height: 101,
            ..Config::default()
        }
        .validate()
        .is_err());
        assert!(Config {
            shell: Some("relative".into()),
            ..Config::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn executable_mappings_and_config_files() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("tool");
        fs::write(&executable, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mapping = HarnessMapping {
            harness: "claude".into(),
            path: executable.clone(),
        };
        let config = Config {
            shell: Some(executable.clone()),
            harness_paths: vec![mapping.clone()],
            ..Config::default()
        };
        let path = temp.path().join("config.json");
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let loaded = load_file(&path).unwrap();
        assert_eq!(loaded.shell, config.shell);
        assert_eq!(loaded.harness_paths[0].path, executable);
        assert_eq!(loaded.harness_paths[0].harness, "claude");
        fs::remove_file(&path).unwrap();
        assert!(Config {
            harness_paths: vec![HarnessMapping {
                harness: "unknown".into(),
                path: executable.clone()
            }],
            ..Config::default()
        }
        .validate()
        .is_err());
        assert!(Config {
            harness_paths: vec![mapping.clone(), mapping],
            ..Config::default()
        }
        .validate()
        .is_err());
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Config {
            shell: Some(executable),
            ..Config::default()
        }
        .validate()
        .is_err());
        assert_eq!(load_file(&path).unwrap().key, "F7");
        fs::write(&path, r#"{"height":101}"#).unwrap();
        assert!(load_file(&path).is_err());
        fs::remove_file(&path).unwrap();
        symlink("missing", &path).unwrap();
        assert!(load_file(&path).is_err());
    }

    #[test]
    fn unsafe_config_permissions_and_symlink_directory_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("product");
        private_dir(&directory).unwrap();
        let path = directory.join("config.json");
        fs::write(&path, r#"{"shortcut":"F7"}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(load_file(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        load_file(&path).unwrap();
        let alias = temp.path().join("alias");
        symlink(&directory, &alias).unwrap();
        assert!(load_file(&alias.join("config.json")).is_err());
        assert_eq!(fs::read(&path).unwrap(), br#"{"shortcut":"F7"}"#);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(load_file(&path).is_err());
    }

    #[test]
    fn json_only_preserves_legacy_files_and_reports_migration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let legacy = temp.path().join("config.toml");
        let old_text = "key = 'F8'\nwidth = 70\n";
        fs::write(&legacy, old_text).unwrap();
        let error = load_file(&path).unwrap_err().to_string();
        assert!(error.contains("config.toml"));
        assert!(error.contains(&format!("create {}", path.display())));
        assert!(error.contains("'key' to 'shortcut'"));
        assert!(!path.exists());
        fs::write(&path, r#"{"shortcut":"F9"}"#).unwrap();
        assert_eq!(load_file(&path).unwrap().key, "F9");
        fs::write(&path, old_text).unwrap();
        assert!(load_file(&path)
            .unwrap_err()
            .to_string()
            .contains("invalid config.json"));
        assert_eq!(fs::read_to_string(&legacy).unwrap(), old_text);
        assert_eq!(fs::read_to_string(&path).unwrap(), old_text);
        fs::remove_file(&path).unwrap();
        fs::remove_file(&legacy).unwrap();
        symlink("missing", &legacy).unwrap();
        assert!(load_file(&path)
            .unwrap_err()
            .to_string()
            .contains("TOML is not loaded"));
        assert_eq!(fs::read_link(&legacy).unwrap(), Path::new("missing"));
    }

    #[test]
    fn oversized_and_nonregular_json_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        for text in ["[]", r#"["F7",80,80,null,[]]"#, "null", "true", ""] {
            fs::write(&path, text).unwrap();
            assert!(load_file(&path)
                .unwrap_err()
                .to_string()
                .contains("expected a JSON object"));
        }
        fs::write(&path, " ".repeat(1024 * 1024 + 1)).unwrap();
        assert!(load_file(&path)
            .unwrap_err()
            .to_string()
            .contains("exceeds 1 MiB"));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(load_file(&path).is_err());
    }

    #[test]
    fn private_directories_do_not_change_parent_permissions() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let product = temp.path().join("product");
        private_dir(&product).unwrap();
        private_dir(&product).unwrap();
        assert_eq!(fs::metadata(temp.path()).unwrap().mode() & 0o777, 0o755);
        assert_eq!(fs::metadata(&product).unwrap().mode() & 0o777, 0o700);
        let link = temp.path().join("link");
        symlink(&product, &link).unwrap();
        assert!(private_dir(&link).is_err());
        fs::set_permissions(&product, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&product).is_err());
    }

    #[test]
    fn xdg_paths_and_rejected_relative_roots() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let defaults = Paths::from_environment(home.clone(), |_| None).unwrap();
        assert_eq!(defaults.config, home.join(".config/agent-float-term"));
        assert_eq!(defaults.data, home.join(".local/share/agent-float-term"));
        assert_eq!(defaults.state, home.join(".local/state/agent-float-term"));
        assert_eq!(defaults.bin, home.join(".local/bin"));
        let overridden = Paths::from_environment(home.clone(), |name| {
            Some(temp.path().join(name).into_os_string())
        })
        .unwrap();
        assert_eq!(
            overridden.config,
            temp.path().join("XDG_CONFIG_HOME/agent-float-term")
        );
        assert_eq!(
            overridden.data,
            temp.path().join("XDG_DATA_HOME/agent-float-term")
        );
        assert_eq!(
            overridden.state,
            temp.path().join("XDG_STATE_HOME/agent-float-term")
        );
        assert_eq!(overridden.bin, defaults.bin);
        assert_eq!(
            Paths::from_environment(home.clone(), |_| Some("".into())).unwrap(),
            defaults
        );
        assert!(Paths::from_environment(home, |_| Some("relative".into())).is_err());
        assert!(Paths::from_environment("relative-home".into(), |_| None).is_err());
        assert!(!temp.path().join("home").exists());
    }
}
