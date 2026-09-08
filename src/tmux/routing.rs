//! Keep application input in the float, but dispatch shortcuts on its main client.
use super::*;
use std::collections::HashSet;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreBinding {
    generation: String,
    client_pid: u32,
    table: String,
    key: String,
    installed: String,
}

impl RestoreBinding {
    fn matches(&self, line: &str) -> bool {
        header(line) == Some((self.table.clone(), self.key.clone()))
            && binding_line(line).is_some_and(|(_, normalized)| normalized == self.installed)
    }
}

fn record(instance: &str) -> Result<PathBuf> {
    ensure!(valid_token(instance), "invalid routing instance");
    Ok(runtime_dir()?.join(format!("route-{instance}.route")))
}

fn read_binding(path: &Path) -> Result<RestoreBinding> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == config::uid()
            && metadata.mode() & 0o077 == 0
            && metadata.len() <= 128 * 1024,
        "unsafe routing record"
    );
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 128 * 1024, "routing record exceeds limit");
    Ok(serde_json::from_slice(&bytes)?)
}

// Only decode the serialized binding header, never interpret a user's commands.
fn header(line: &str) -> Option<(String, String)> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut word = String::new();
        let mut quote = None;
        while let Some(c) = chars.next() {
            match (quote, c) {
                (Some(q), c) if q == c => quote = None,
                (None, '\'' | '"') => quote = Some(c),
                (_, '\\') if quote != Some('\'') => word.push(chars.next()?),
                (None, c) if c.is_whitespace() => break,
                _ => word.push(c),
            }
        }
        if quote.is_some() {
            return None;
        }
        words.push(word);
        if let Some(i) = words.iter().position(|word| word == "-T") {
            if words.len() == i + 3 {
                return Some((words[i + 1].clone(), words[i + 2].clone()));
            }
        }
    }
    None
}

pub(super) fn prepare(
    tmux: &Tmux,
    pane: &Pane,
    client: &Client,
    target: &str,
    config: &Config,
) -> Result<()> {
    let instance = tmux.output(&["show-options", "-qv", "-t", target, "@aft_instance"])?;
    ensure!(valid_token(&instance), "float has no routing identity");
    let options = tmux.output(&[
        "show-options",
        "-Av",
        "-t",
        &client.session,
        "prefix",
        ";",
        "show-options",
        "-Av",
        "-t",
        &client.session,
        "prefix2",
        ";",
        "show-options",
        "-Av",
        "-t",
        &client.session,
        "key-table",
    ])?;
    let fields: Vec<_> = options.lines().collect();
    ensure!(
        fields.len() == 3,
        "cannot read main client's key configuration"
    );
    ensure!(
        fields[..2].iter().all(|key| *key != config.key),
        "float shortcut conflicts with the main tmux prefix"
    );
    let table = format!("aft-input-{instance}");
    let prefix_table = format!("aft-prefix-{instance}");
    let quoted_target = tmux_quote(target);
    let quoted_client = tmux_quote(&client.name);
    let worker = std::process::id().to_string();
    let _binding_lock = lock(&record_path(&tmux.socket)?.with_extension("lock"))?;
    let listing = tmux.output(&["list-keys"])?;
    let keys: Vec<_> = listing.lines().filter_map(header).collect();

    // A synthetic User key has no terminal byte sequence. Native tmux input
    // handling ignores it in prompts/overlays instead of opening over their UI.
    // No user binding is changed, and no real function key is injected.
    let path = record(&instance)?;
    let previous = if path.exists() {
        let restored = read_binding(&path)?;
        ensure!(
            restored.generation == pane.generation
                && listing.lines().any(|line| restored.matches(line)),
            "owned restore binding changed; left untouched"
        );
        Some(restored)
    } else {
        None
    };
    let restore = if previous
        .as_ref()
        .is_some_and(|old| old.client_pid == client.pid && old.table == fields[2])
    {
        previous.context("missing restore binding")?
    } else {
        let user_keys = tmux.output(&["show-options", "-sq", "user-keys"])?;
        let occupied: HashSet<_> = keys.iter().map(|(_, key)| key.as_str()).collect();
        let key = (0..1000)
            .rev()
            .map(|i| format!("User{i}"))
            .find(|key| {
                !occupied.contains(key.as_str())
                    && !user_keys
                        .lines()
                        .any(|line| line.starts_with(&format!("user-keys[{}]", &key[4..])))
            })
            .context("no unused tmux User key is available for restoration")?;
        let command = format!(
            "export AFT_RESTORE_INSTANCE={instance}; {}",
            dispatch_command(tmux, &config.key)?
        );
        let condition = format!("#{{&&:#{{==:#{{client_pid}},{}}},#{{&&:#{{==:#{{pane_id}},{}}},#{{==:#{{session_id}},{}}}}}}}", client.pid, pane.id, client.session);
        tmux.source(&format!(
            "bind-key -T {} {} if-shell -F {} {}\n",
            tmux_quote(fields[2]),
            tmux_quote(&key),
            tmux_quote(&condition),
            tmux_quote(&format!("run-shell {}", tmux_quote(&command)))
        ))?;
        let installed = tmux
            .output(&["list-keys"])?
            .lines()
            .find(|line| header(line) == Some((fields[2].into(), key.clone())))
            .and_then(binding_line)
            .context("restore binding was not installed")?
            .1;
        let restored = RestoreBinding {
            generation: pane.generation.clone(),
            client_pid: client.pid,
            table: fields[2].into(),
            key,
            installed,
        };
        atomic_private_write(&path, &serde_json::to_vec(&restored)?)?;
        if let Some(old) = previous {
            if tmux
                .output(&["list-keys"])?
                .lines()
                .any(|line| old.matches(line))
            {
                tmux.output(&["unbind-key", "-T", &old.table, &old.key])?;
            }
        }
        restored
    };

    let route = |prefix: Option<&str>| {
        let prefix = prefix
            .map(|key| format!("send-keys -K -c {quoted_client} {} ; ", tmux_quote(key)))
            .unwrap_or_default();
        // L changes client context, not pane/session context. Only use client
        // identity here; the supervisor separately tracks origin navigation.
        let main = format!("#{{==:#{{client_pid}},{}}}", client.pid);
        let condition = format!("#{{&&:#{{==:#{{@aft_instance}},{instance}}},#{{&&:#{{==:#{{client_pid}},#{{@aft_viewer}}}},#{{m:*1*,#{{L:{main}}}}}}}}}");
        let body = format!("set-option -t {quoted_target} @aft_routing 1 ; display-popup -C -c {quoted_client} ; {prefix}send-keys -K -c {quoted_client}");
        format!(
            "if-shell -F {} {} detach-client",
            tmux_quote(&condition),
            tmux_quote(&body)
        )
    };
    // Nested PTYs coalesce rapid shortcuts. Disable only the float's timing-based
    // paste guess; explicit bracketed paste still bypasses its key bindings.
    let mut commands =
        format!(
        "set-option -t {quoted_target} prefix None\nset-option -t {quoted_target} prefix2 None\n\
         set-option -t {quoted_target} key-table {table}\n\
         set-option -t {quoted_target} assume-paste-time 0\n\
         set-option -t {quoted_target} @aft_route_worker {worker}\n\
         set-option -t {quoted_target} @aft_restore_key {}\n\
         set-option -t {quoted_target} @aft_restore_table {}\n",
        tmux_quote(&restore.key), tmux_quote(&restore.table));
    // Rebuild only tables with this unguessable, invocation-owned name.
    let _ = tmux.output(&["unbind-key", "-a", "-T", &table]);
    let _ = tmux.output(&["unbind-key", "-a", "-T", &prefix_table]);
    let otherwise = if keys
        .iter()
        .any(|(source, key)| source == fields[2] && key == "Any")
    {
        route(None)
    } else {
        format!(
            "if-shell -F '#{{==:#{{@aft_routing}},1}}' {} 'send-keys'",
            tmux_quote(&format!("send-keys -K -c {quoted_client}"))
        )
    };
    commands.push_str(&format!(
        "bind-key -T {table} Any {}\n",
        tmux_quote(&otherwise)
    ));
    for (source, key) in &keys {
        if source == fields[2]
            && key != &config.key
            && key != &restore.key
            && !key.contains("Mouse")
            && !key.starts_with("Wheel")
            && key != "Any"
        {
            commands.push_str(&format!(
                "bind-key -T {table} {} {}\n",
                tmux_quote(key),
                tmux_quote(&route(None))
            ));
        }
    }
    for (index, prefix) in fields[..2]
        .iter()
        .enumerate()
        .filter(|(_, key)| **key != "None")
    {
        let capture = format!("{prefix_table}-{index}");
        let _ = tmux.output(&["unbind-key", "-a", "-T", &capture]);
        commands.push_str(&format!(
            "bind-key -T {table} {} {}\nbind-key -T {capture} Any {}\n",
            tmux_quote(prefix),
            tmux_quote(&format!("switch-client -T {capture}")),
            tmux_quote(&route(Some(prefix)))
        ));
        commands.push_str(&format!(
            "bind-key -T {capture} {} {}\n",
            tmux_quote(&config.key),
            tmux_quote(&format!(
                "set-option -t {quoted_target} @aft_visible 0 ; detach-client"
            ))
        ));
    }
    // Inspect the retained shell, not the nested viewer process. Only a positive
    // non-editor result may yield to main; unavailable metadata keeps input local.
    let process = tmux.output(&[
        "display-message",
        "-p",
        "-t",
        target,
        "#{pane_pid}|#{pane_tty}",
    ])?;
    let (pid, tty) = process
        .split_once('|')
        .context("missing float terminal identity")?;
    let pid: u32 = pid.parse().context("invalid float process PID")?;
    let predicate = literal_format(&format!(
        "{} navigation-to-main --pane-pid {pid} --pane-tty {} >/dev/null 2>&1",
        shell_quote(text(&helper_path()?)?),
        shell_quote(tty)
    ));
    for key in ["C-h", "C-j", "C-k", "C-l"] {
        let fallback = if let Some(index) = fields[..2].iter().position(|prefix| *prefix == key) {
            format!("switch-client -T {prefix_table}-{index}")
        } else if keys
            .iter()
            .any(|(source, bound)| source == fields[2] && bound == key)
        {
            route(None)
        } else {
            otherwise.clone()
        };
        let navigation = format!(
            "if-shell {} {} send-keys",
            tmux_quote(&predicate),
            tmux_quote(&fallback)
        );
        let body = format!(
            "if-shell -F '#{{==:#{{@aft_routing}},1}}' {} {}",
            tmux_quote(&format!("send-keys -K -c {quoted_client}")),
            tmux_quote(&navigation)
        );
        commands.push_str(&format!(
            "bind-key -T {table} {key} {}\n",
            tmux_quote(&body)
        ));
    }
    commands.push_str(&format!(
        "bind-key -T {table} {} {}\n",
        tmux_quote(&config.key),
        tmux_quote(&format!(
            "set-option -t {quoted_target} @aft_visible 0 ; detach-client"
        ))
    ));
    tmux.source(&commands)
}

pub(super) fn finished(tmux: &Tmux, target: &str, worker: &str) {
    let _ = tmux.output(&[
        "if-shell",
        "-F",
        "-t",
        target,
        &format!("#{{==:#{{@aft_route_worker}},{worker}}}"),
        &format!("set-option -t {} @aft_routing 0", tmux_quote(target)),
    ]);
}

pub(super) fn restore(tmux: &Tmux, instance: &str, client: &Client) -> Result<()> {
    let binding = read_binding(&record(instance)?)?;
    ensure!(
        tmux.global(GENERATION)? == binding.generation,
        "restore generation changed"
    );
    ensure!(
        tmux.output(&["list-keys"])?
            .lines()
            .any(|line| binding.matches(line)),
        "restore key changed; user binding was not invoked"
    );
    tmux.output(&["send-keys", "-K", "-c", &client.name, &binding.key])?;
    Ok(())
}

pub(super) fn cleanup(tmux: &Tmux, instance: &str) -> Result<()> {
    let _guard = lock(&record_path(&tmux.socket)?.with_extension("lock"))?;
    cleanup_locked(tmux, instance)
}

// The caller serializes changes to server bindings.
pub(super) fn cleanup_server(tmux: &Tmux, generation: &str) -> Result<()> {
    if tmux.global(GENERATION)? != generation {
        return Ok(());
    }
    for entry in fs::read_dir(runtime_dir()?)? {
        let path = entry?.path();
        if path.extension() != Some(OsStr::new("route")) {
            continue;
        }
        let Some(instance) = path
            .file_stem()
            .and_then(OsStr::to_str)
            .and_then(|s| s.strip_prefix("route-"))
        else {
            continue;
        };
        if valid_token(instance) && read_binding(&path)?.generation == generation {
            cleanup_locked(tmux, instance)?;
        }
    }
    Ok(())
}

fn cleanup_locked(tmux: &Tmux, instance: &str) -> Result<()> {
    let path = record(instance)?;
    if let Ok(binding) = read_binding(&path) {
        if tmux.global(GENERATION)? != binding.generation {
            return Ok(());
        }
        if tmux
            .output(&["list-keys"])?
            .lines()
            .any(|line| binding.matches(line))
        {
            tmux.output(&["unbind-key", "-T", &binding.table, &binding.key])?;
        }
        fs::remove_file(path)?;
    } else {
        return Ok(());
    }
    for table in [
        format!("aft-input-{instance}"),
        format!("aft-prefix-{instance}-0"),
        format!("aft-prefix-{instance}-1"),
    ] {
        let _ = tmux.output(&["unbind-key", "-a", "-T", &table]);
    }
    let sessions = tmux.output(&[
        "list-sessions",
        "-F",
        "#{session_id}|#{@aft_instance}|#{key-table}",
    ])?;
    for line in sessions.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() == 3
            && fields[1] == instance
            && fields[2] == format!("aft-input-{instance}")
            && lifecycle::exclusive_windows(tmux, fields[0])?
        {
            tmux.output(&[
                "set-option",
                "-u",
                "-t",
                fields[0],
                "key-table",
                ";",
                "set-option",
                "-u",
                "-t",
                fields[0],
                "prefix",
                ";",
                "set-option",
                "-u",
                "-t",
                fields[0],
                "prefix2",
                ";",
                "set-option",
                "-u",
                "-t",
                fields[0],
                "assume-paste-time",
                ";",
                "set-option",
                "-t",
                fields[0],
                "@aft_visible",
                "0",
            ])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_headers_preserve_quoted_keys_without_parsing_commands() {
        assert_eq!(
            header("bind-key -r -T root C-h select-pane -L"),
            Some(("root".into(), "C-h".into()))
        );
        assert_eq!(
            header("bind-key -T prefix '\"' split-window"),
            Some(("prefix".into(), "\"".into()))
        );
        assert_eq!(
            header("bind-key -T root \\# display-message hello"),
            Some(("root".into(), "#".into()))
        );
        assert_eq!(
            header("bind-key -N 'a long note' -T root Space send-keys"),
            Some(("root".into(), "Space".into()))
        );
        assert!(header("bind-key -T 'unfinished").is_none());
    }
}
