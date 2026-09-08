//! Invocation ownership and detached supervision. No terminal contents are inspected.

use super::*;
use std::os::unix::process::CommandExt;

const METADATA_INTERVAL: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) fn parent_lock(generation: &str, pane: &str) -> Result<PathBuf> {
    ensure!(
        valid_token(generation) && valid_pane(pane),
        "invalid float owner"
    );
    Ok(runtime_dir()?.join(format!("aft-{}-{}.lock", &generation[..12], &pane[1..])))
}

fn watcher_lock(instance: &str) -> Result<PathBuf> {
    ensure!(valid_token(instance), "invalid float instance");
    Ok(runtime_dir()?.join(format!("watch-{instance}.lock")))
}

struct WatchLock {
    path: PathBuf,
    file: File,
}

impl WatchLock {
    fn acquire(path: &Path) -> Result<Option<Self>> {
        loop {
            let Some(file) = lock_for(path, Duration::ZERO)? else {
                return Ok(None);
            };
            let guard = Self {
                path: path.into(),
                file,
            };
            // A contender may have opened the previous watcher's inode just
            // before it was unlinked. Never supervise while locking that inode.
            if guard.current() {
                return Ok(Some(guard));
            }
        }
    }

    fn current(&self) -> bool {
        self.file
            .metadata()
            .ok()
            .zip(fs::symlink_metadata(&self.path).ok())
            .is_some_and(|(held, named)| held.dev() == named.dev() && held.ino() == named.ino())
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        // Unlink before unlocking; acquire rechecks the inode after flock.
        // In particular, a late drop must not unlink a replacement watcher's lock.
        if self.current() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn valid_session(session: &str) -> bool {
    session
        .strip_prefix('$')
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
}

fn all(conditions: impl IntoIterator<Item = String>) -> String {
    conditions
        .into_iter()
        .reduce(|a, b| format!("#{{&&:{a},{b}}}"))
        .unwrap_or_else(|| "0".into())
}

// tmux's format parser treats commas and closing braces specially, even inside
// shell quotes. The literal modifier prevents JSON from becoming format syntax.
fn format_literal(value: &str) -> String {
    format!(
        "#{{l:{}}}",
        value
            .replace('#', "##")
            .replace(',', "#,")
            .replace('}', "#}")
    )
}

pub(super) fn ownership_condition(
    generation: &str,
    owner: &str,
    instance: &str,
    invocation: Invocation,
) -> Result<String> {
    Ok(all([
        format!("#{{==:#{{@aft_float_generation}},{generation}}}"),
        format!("#{{==:#{{@aft_generation}},{generation}}}"),
        format!("#{{==:#{{@aft_owner}},{owner}}}"),
        format!("#{{==:#{{@aft_instance}},{instance}}}"),
        format!(
            "#{{==:#{{@aft_invocation}},{}}}",
            format_literal(&serde_json::to_string(&invocation)?)
        ),
    ]))
}

pub(super) fn exclusive_windows(tmux: &Tmux, session: &str) -> Result<bool> {
    let windows = tmux.output(&[
        "list-windows",
        "-t",
        session,
        "-F",
        "#{session_grouped}|#{window_linked}",
    ])?;
    Ok(!windows.is_empty() && windows.lines().all(|line| line == "0|0"))
}

/// Caller holds the parent lock. A server-side guard rechecks every window at
/// the kill boundary, including noncurrent windows added since the snapshot.
pub(super) fn kill_owned(
    tmux: &Tmux,
    session: &str,
    generation: &str,
    owner: &str,
    instance: &str,
    invocation: Invocation,
) -> Result<()> {
    ensure!(
        valid_session(session) && valid_token(instance),
        "invalid float identity"
    );
    if invocation.liveness() != Liveness::Exited || tmux.global(GENERATION)? != generation {
        return Ok(());
    }
    let condition = all([
        ownership_condition(generation, owner, instance, invocation)?,
        "#{==:#{session_grouped},0}".into(),
        "#{==:#{m:*1*,#{W:#{window_linked}}},0}".into(),
    ]);
    tmux.output(&[
        "if-shell",
        "-F",
        "-t",
        session,
        &condition,
        &format!("kill-session -t {}", tmux_quote(session)),
    ])?;
    // Also reclaim a lock left by a crashed watcher during invocation rollover.
    drop(WatchLock::acquire(&watcher_lock(instance)?)?);
    cleanup_routing(tmux, session, instance, false);
    Ok(())
}

fn cleanup_routing(tmux: &Tmux, session: &str, instance: &str, unbound: bool) {
    // A failed metadata command does not prove a session disappeared. A live
    // linked/grouped float keeps its tables even when integration is unbound.
    let Ok(sessions) = tmux.output(&["list-sessions", "-F", "#{session_id}"]) else {
        return;
    };
    let gone = !sessions.lines().any(|id| id == session);
    if gone
        || (unbound
            && exclusive_windows(tmux, session).unwrap_or(false)
            && tmux
                .output(&["show-options", "-qv", "-t", session, "@aft_instance"])
                .is_ok_and(|current| current == instance))
    {
        let _ = routing::cleanup(tmux, instance);
    }
}

fn helper(tmux: &Tmux) -> Result<Command> {
    let mut command = Command::new(helper_path()?);
    // Inherit HOME/XDG roots, and pin the approved per-server executable even
    // when the caller selected it from a binding record rather than its env.
    command
        .env("AFT_TMUX_BINARY", &tmux.binary)
        .env_remove("TMUX")
        .env_remove("AFT_RESTORE_INSTANCE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no Rust state in the child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

pub(super) fn start_watcher(tmux: &Tmux, session: &str, instance: &str) -> Result<()> {
    // Never take the parent lock in watch before publishing readiness: popup
    // holds it while waiting here. The separate lifetime lock deduplicates watch.
    let path = watcher_lock(instance)?;
    let ready = || -> Result<bool> {
        let metadata = tmux.output(&[
            "display-message",
            "-p",
            "-t",
            session,
            "#{@aft_instance}|#{@aft_watch_ready}|#{@aft_watcher}",
        ])?;
        let fields: Vec<_> = metadata.split('|').collect();
        Ok(fields.len() == 3
            && fields[0] == instance
            && fields[1] == instance
            && alive(fields[2])
            && WatchLock::acquire(&path)?.is_none())
    };
    if ready()? {
        return Ok(());
    }
    let mut child = helper(tmux)?
        .args(["watch", "--socket"])
        .arg(&tmux.socket)
        .args(["--session", session, "--instance", instance])
        .spawn()?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        if ready()? {
            // Reap if this was a duplicate helper. Otherwise retain no wait on
            // the watcher; the OS adopts it after this popup worker exits.
            let _ = child.try_wait();
            return Ok(());
        }
        if child.try_wait()?.is_some() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("lifecycle watcher did not become ready; shell was preserved");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

struct State {
    generation: String,
    owner: String,
    instance: String,
    invocation: Invocation,
    attached: bool,
    worker: String,
    visible: bool,
    routing: bool,
    origin_pid: String,
    origin_name: String,
    origin_session: String,
    restore_table: String,
    clients: Vec<Vec<String>>,
}

impl State {
    fn read(tmux: &Tmux, session: &str) -> Result<Self> {
        let value = tmux.output(&[
            "show-options", "-gqv", GENERATION, ";",
            "display-message", "-p", "-t", session,
            "#{session_id}|#{@aft_float_generation}|#{@aft_owner}|#{@aft_instance}|#{@aft_invocation}|#{session_attached}|#{@aft_worker}|#{@aft_visible}|#{@aft_routing}|#{@aft_origin_client}|#{@aft_origin_name}|#{@aft_origin_session}|#{@aft_restore_table}",
            ";", "list-clients", "-F",
            "#{client_pid}|#{client_name}|#{session_id}|#{pane_id}|#{pane_in_mode}|#{client_key_table}|#{client_flags}",
        ])?;
        Self::parse(&value, session)
    }

    fn parse(value: &str, session: &str) -> Result<Self> {
        let mut lines = value.lines();
        let generation = lines.next().context("missing generation")?;
        let fields: Vec<_> = lines
            .next()
            .context("float disappeared")?
            .split('|')
            .collect();
        ensure!(
            fields.len() == 13
                && fields[0] == session
                && fields[1] == generation
                && valid_token(generation)
                && valid_pane(fields[2])
                && valid_token(fields[3]),
            "float ownership changed"
        );
        Ok(Self {
            generation: generation.into(),
            owner: fields[2].into(),
            instance: fields[3].into(),
            invocation: serde_json::from_str(fields[4])?,
            attached: fields[5] != "0",
            worker: fields[6].into(),
            visible: fields[7] == "1",
            routing: fields[8] == "1",
            origin_pid: fields[9].into(),
            origin_name: fields[10].into(),
            origin_session: fields[11].into(),
            restore_table: fields[12].into(),
            clients: lines
                .map(|line| line.split('|').map(str::to_owned).collect())
                .collect(),
        })
    }

    fn matches(&self, original: &Self) -> bool {
        self.generation == original.generation
            && self.owner == original.owner
            && self.instance == original.instance
            && self.invocation == original.invocation
    }

    fn origin(&self) -> Option<&[String]> {
        self.clients
            .iter()
            .find(|fields| {
                fields.len() == 7 && fields[0] == self.origin_pid && fields[1] == self.origin_name
            })
            .map(Vec::as_slice)
    }

    fn can_restore(&self) -> bool {
        self.visible
            && !self.routing
            && !self.attached
            && !alive(&self.worker)
            && self.origin().is_some_and(|client| {
                client[2] == self.origin_session
                    && client[3] == self.owner
                    && idle_client(client, &self.restore_table)
            })
    }
}

fn idle_client(client: &[String], table: &str) -> bool {
    // This only selects a candidate. routing::restore must enter through native
    // client input handling so prompts/overlays get first refusal, not dispatch.
    client.len() == 7
        && client[4] == "0"
        && !table.is_empty()
        && client[5] == table
        && client[6].split(',').any(|flag| flag == "attached")
        && !client[6].split(',').any(|flag| {
            matches!(
                flag,
                "suspended" | "read-only" | "control-mode" | "overlay" | "prompt"
            )
        })
}

pub(super) fn restore_allowed(
    tmux: &Tmux,
    session: &str,
    instance: &str,
    invocation: Invocation,
    client: &Client,
) -> Result<bool> {
    let state = State::read(tmux, session)?;
    Ok(state.instance == instance
        && state.invocation == invocation
        && state.can_restore()
        && state.origin_pid == client.pid.to_string()
        && state.origin_name == client.name
        && state.origin_session == client.session
        && state.owner == client.pane)
}

pub(super) fn viewer_command(
    tmux: &Tmux,
    session: &str,
    instance: &str,
    worker: &str,
) -> Result<String> {
    let binary = shell_quote(text(&tmux.binary)?);
    let socket = shell_quote(text(&tmux.socket)?);
    let condition = all([
        format!("#{{==:#{{@aft_instance}},{instance}}}"),
        format!("#{{==:#{{@aft_worker}},{worker}}}"),
    ]);
    // The shell's PID survives exec into attach-session. Publish it only under
    // this viewer claim, and do not attach at all if the claim was superseded.
    let claim = shell_quote(&format!(
        "set-option -t {} @aft_viewer ",
        tmux_quote(session)
    ));
    Ok(format!(
        "if [ \"$({binary} -S {socket} if-shell -F -t {target} {condition} {claim}\"$$\"'; display-message -p AFT_VIEWER')\" = AFT_VIEWER ]; then exec {binary} -T RGB -S {socket} attach-session -E -t {target}; fi",
        target = shell_quote(session), condition = shell_quote(&condition),
    ))
}

/// Hidden helper entry point. Ownership loss is a normal, non-destructive exit.
pub fn watch(socket: PathBuf, session: String, instance: String) -> Result<()> {
    ensure!(
        valid_session(&session) && valid_token(&instance),
        "invalid watcher target"
    );
    let tmux = Tmux::new(socket)?;
    let _watch_guard = match WatchLock::acquire(&watcher_lock(&instance)?)? {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let original = match State::read(&tmux, &session) {
        Ok(state) if state.instance == instance => state,
        _ => return Ok(()),
    };
    let record_path = record_path(&tmux.socket)?;
    let bound = || -> Result<bool> {
        Ok(read_record(&record_path)?.is_some_and(|record| {
            record.socket == tmux.socket && record.generation == original.generation
        }))
    };
    if !bound()? {
        cleanup_routing(&tmux, &session, &instance, true);
        return Ok(());
    }
    let pid = std::process::id().to_string();
    let condition = ownership_condition(
        &original.generation,
        &original.owner,
        &instance,
        original.invocation,
    )?;
    tmux.output(&[
        "if-shell",
        "-F",
        "-t",
        &session,
        &condition,
        &format!(
            "set-option -t {} @aft_watcher {pid} ; set-option -t {} @aft_watch_ready {instance}",
            tmux_quote(&session),
            tmux_quote(&session)
        ),
    ])?;
    let mut metadata_at = Instant::now();
    let mut restore_at = Instant::now();
    loop {
        let started = Instant::now();
        let liveness = original.invocation.liveness();
        if liveness == Liveness::Exited {
            // Retry lock contention: a creator may still be completing its
            // readiness handshake, and must be allowed to release this lock.
            if let Some(_guard) = lock_for(
                &parent_lock(&original.generation, &original.owner)?,
                Duration::ZERO,
            )? {
                if !bound()? {
                    cleanup_routing(&tmux, &session, &instance, true);
                    return Ok(());
                }
                if let Ok(current) = State::read(&tmux, &session) {
                    if current.matches(&original) {
                        kill_owned(
                            &tmux,
                            &session,
                            &original.generation,
                            &original.owner,
                            &instance,
                            original.invocation,
                        )?;
                    }
                }
                cleanup_routing(&tmux, &session, &instance, false);
                return Ok(());
            }
        }
        if started >= metadata_at {
            metadata_at = started + METADATA_INTERVAL;
            if !bound()? {
                cleanup_routing(&tmux, &session, &instance, true);
                return Ok(());
            }
            let current = match State::read(&tmux, &session) {
                Ok(state) if state.matches(&original) => state,
                _ => {
                    cleanup_routing(&tmux, &session, &instance, false);
                    return Ok(());
                }
            };
            if current.attached && alive(&current.worker) && !current.routing {
                if let Some(client) = current.origin() {
                    if client[2] != current.origin_session || client[3] != current.owner {
                        close_viewer(&tmux, &session, &current)?;
                    }
                }
            }
            if liveness == Liveness::Alive && started >= restore_at && current.can_restore() {
                if let Ok(pid) = current.origin_pid.parse() {
                    let client = Client {
                        pid,
                        name: current.origin_name.clone(),
                        pane: current.owner.clone(),
                        session: current.origin_session.clone(),
                    };
                    let _ = routing::restore(&tmux, &instance, &client);
                }
                restore_at = started + Duration::from_secs(1);
            }
        }
        thread::sleep(POLL_INTERVAL.saturating_sub(started.elapsed()));
    }
}

fn close_viewer(tmux: &Tmux, session: &str, state: &State) -> Result<()> {
    // Detach only the nested client whose PID was leased by this popup worker.
    // Unlike display-popup -C this cannot close a replacement, foreign overlay.
    let viewer = tmux.output(&["display-message", "-p", "-t", session, "#{@aft_viewer}"])?;
    if !alive(&viewer) {
        return Ok(());
    }
    let Some(client) = state
        .clients
        .iter()
        .find(|client| client.len() == 7 && client[0] == viewer && client[2] == session)
    else {
        return Ok(());
    };
    let moved_origin = all([
        format!(
            "#{{==:#{{client_pid}},{}}}",
            format_literal(&state.origin_pid)
        ),
        format!(
            "#{{==:#{{client_name}},{}}}",
            format_literal(&state.origin_name)
        ),
        format!(
            "#{{||:#{{!=:#{{session_id}},{}}},#{{!=:#{{pane_id}},{}}}}}",
            format_literal(&state.origin_session),
            state.owner
        ),
    ]);
    let leased_viewer = all([
        format!("#{{==:#{{client_pid}},{viewer}}}"),
        format!("#{{==:#{{client_name}},{}}}", format_literal(&client[1])),
        format!("#{{==:#{{session_id}},{session}}}"),
    ]);
    let condition = all([
        ownership_condition(
            &state.generation,
            &state.owner,
            &state.instance,
            state.invocation,
        )?,
        format!("#{{==:#{{@aft_worker}},{}}}", format_literal(&state.worker)),
        format!("#{{==:#{{@aft_viewer}},{viewer}}}"),
        "#{!=:#{@aft_routing},1}".into(),
        format!("#{{m:*1*,#{{L:{moved_origin}}}}}"),
        format!("#{{m:*1*,#{{L:{leased_viewer}}}}}"),
    ]);
    tmux.output(&[
        "if-shell",
        "-F",
        "-t",
        session,
        &condition,
        &format!("detach-client -t {}", tmux_quote(&client[1])),
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_candidates_require_the_original_table_and_normal_client() {
        let mut client: Vec<String> =
            ["1", "/dev/pts/1", "$1", "%1", "0", "root", "attached,UTF-8"]
                .map(str::to_owned)
                .into();
        assert!(idle_client(&client, "root"));
        assert!(!idle_client(&client, ""));
        assert!(!idle_client(&client, "other"));
        client[5] = "custom-default".into();
        assert!(idle_client(&client, "custom-default"));
        client[5] = "root".into();
        for index in [4, 5, 6] {
            let saved = client[index].clone();
            client[index] = String::new();
            assert!(!idle_client(&client, "root"));
            client[index] = saved;
        }
        for flag in [
            "overlay",
            "prompt",
            "suspended",
            "read-only",
            "control-mode",
        ] {
            client[6] = format!("attached,{flag}");
            assert!(!idle_client(&client, "root"));
        }
    }

    #[test]
    fn identities_and_format_literals_are_not_commands() {
        assert!(valid_session("$12"));
        for session in ["", "$", "$1;kill-server", "main", "$1\n"] {
            assert!(!valid_session(session));
        }
        assert_eq!(
            format_literal("{\"a\":1,\"b\":2}"),
            "#{l:{\"a\":1#,\"b\":2#}}"
        );
        assert_eq!(format_literal("#{pane_id}"), "#{l:##{pane_id#}}");
    }

    #[test]
    fn ownership_and_restore_require_exact_identity_and_intent() {
        let generation = "a".repeat(32);
        let instance = "b".repeat(32);
        let invocation = r#"{"frontend":{"pid":42,"started":[123,456]},"wrapper":null}"#;
        let transcript = format!("{generation}\n$2|{generation}|%1|{instance}|{invocation}|0||1|0|1|/dev/pts/1|$1|root\n1|/dev/pts/1|$1|%1|0|root|attached,UTF-8");
        let original = State::parse(&transcript, "$2").unwrap();
        let mut current = State::parse(&transcript, "$2").unwrap();
        assert!(current.matches(&original));
        assert!(current.can_restore());
        current.visible = false;
        assert!(!current.can_restore());
        current.visible = true;
        current.routing = true;
        assert!(!current.can_restore());
        current.routing = false;
        current.attached = true;
        assert!(!current.can_restore());
        current.attached = false;
        current.worker = std::process::id().to_string();
        assert!(!current.can_restore());
        current.worker.clear();
        for index in 0..4 {
            let saved = current.clients[0][index].clone();
            current.clients[0][index].push('9');
            assert!(!current.can_restore());
            current.clients[0][index] = saved;
        }
        current.instance = "c".repeat(32);
        assert!(!current.matches(&original));
        current.instance = original.instance.clone();
        current.invocation = serde_json::from_str(&invocation.replace("123", "124")).unwrap();
        assert!(!current.matches(&original));
        assert!(State::parse(&transcript, "$3").is_err());
        assert!(State::parse(&transcript.replacen(&generation, &"c".repeat(32), 1), "$2").is_err());
        assert!(State::parse(&transcript.replace(invocation, ""), "$2").is_err());
        let marker = serde_json::to_string(&original.invocation).unwrap();
        assert!(!marker.contains('|'));
        let condition =
            ownership_condition(&generation, "%1", &instance, original.invocation).unwrap();
        for option in [
            "@aft_generation",
            "@aft_float_generation",
            "@aft_owner",
            "@aft_instance",
            "@aft_invocation",
        ] {
            assert!(condition.contains(option));
        }
    }

    #[test]
    fn viewer_shell_claim_is_valid_with_quoted_paths() {
        let tmux = Tmux {
            binary: "/not installed/tmux'client".into(),
            socket: "/not installed/socket'path".into(),
        };
        let command = viewer_command(&tmux, "$2", &"a".repeat(32), "42").unwrap();
        assert!(command.contains("@aft_instance"));
        assert!(command.contains("@aft_worker"));
        assert!(command.contains("@aft_viewer"));
        // Syntax check only: no tmux executable or socket is used by this test.
        assert!(Command::new("/bin/sh")
            .args(["-n", "-c", &command])
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn watcher_lifetime_lock_does_not_take_parent_lock() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("parent.lock");
        let watcher = directory.path().join("watch.lock");
        let _parent = lock(&parent).unwrap();
        let watcher_guard = WatchLock::acquire(&watcher).unwrap().unwrap();
        assert!(WatchLock::acquire(&watcher).unwrap().is_none());
        assert!(lock_for(&parent, Duration::ZERO).unwrap().is_none());
        drop(watcher_guard);
        assert!(!watcher.exists());
        let stale = WatchLock::acquire(&watcher).unwrap().unwrap();
        fs::remove_file(&watcher).unwrap();
        let replacement = WatchLock::acquire(&watcher).unwrap().unwrap();
        drop(stale);
        assert!(watcher.exists());
        assert!(WatchLock::acquire(&watcher).unwrap().is_none());
        drop(replacement);
        assert!(!watcher.exists());
    }
}
