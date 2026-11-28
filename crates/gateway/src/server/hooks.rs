use std::{collections::HashSet, path::PathBuf, sync::Arc};

use tracing::{info, warn};

use antre_sessions::store::SessionStore;

use super::seed_content::{
    DCG_GUARD_HANDLER_SH, DCG_GUARD_HOOK_MD, EXAMPLE_HOOK_MD, EXAMPLE_SKILL_MD, TMUX_SKILL_MD,
};

// ── Hook seeding helpers ─────────────────────────────────────────────────────

/// Seed a skeleton example hook into `~/.antre/hooks/example/` on first run.
pub(crate) fn seed_example_hook() {
    let hook_dir = antre_config::data_dir().join("hooks/example");
    let hook_md = hook_dir.join("HOOK.md");
    if hook_md.exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&hook_dir) {
        tracing::debug!("could not create example hook dir: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&hook_md, EXAMPLE_HOOK_MD) {
        tracing::debug!("could not write example HOOK.md: {e}");
    }
}

/// Approve the hooks Antre seeds itself (example, dcg-guard) for content
/// pinning, but only while their files are exactly what Antre writes, so a
/// seeder refresh after an upgrade does not hold them back and a user edit
/// is not approved behind the user's back.
#[cfg(feature = "mods")]
pub(crate) async fn approve_seeded_hooks() {
    let hooks_dir = antre_config::data_dir().join("hooks");
    let seeded: [(&str, &[(&str, &str)]); 2] = [
        ("example", &[("HOOK.md", EXAMPLE_HOOK_MD)]),
        ("dcg-guard", &[
            ("HOOK.md", DCG_GUARD_HOOK_MD),
            ("handler.sh", DCG_GUARD_HANDLER_SH),
        ]),
    ];
    for (name, files) in seeded {
        let dir = hooks_dir.join(name);
        if !seeded_files_match(&dir, files) {
            continue;
        }
        if let Err(error) = crate::mods::approve_shell_hook(name, dir).await {
            tracing::debug!(hook = name, %error, "could not approve seeded hook");
        }
    }
}

#[cfg(feature = "mods")]
fn seeded_files_match(dir: &std::path::Path, files: &[(&str, &str)]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    if entries.count() != files.len() {
        return false;
    }
    files.iter().all(|(file, expected)| {
        std::fs::read_to_string(dir.join(file)).is_ok_and(|content| content == *expected)
    })
}

/// Marker string that must be present in an up-to-date seeded `handler.sh`.
pub(crate) const DCG_GUARD_HANDLER_FINGERPRINT: &str = "export PATH=";

/// Marker string that must be present in an up-to-date seeded `HOOK.md`.
pub(crate) const DCG_GUARD_HOOK_MD_FINGERPRINT: &str = "uv tool install destructive-command-guard";

/// Seed the `dcg-guard` hook into `~/.antre/hooks/dcg-guard/` on first run,
/// and refresh on-disk files that predate the PATH-fix in #626.
pub(crate) async fn seed_dcg_guard_hook() {
    let hook_dir = antre_config::data_dir().join("hooks/dcg-guard");
    let hook_md = hook_dir.join("HOOK.md");
    let handler = hook_dir.join("handler.sh");

    let dir_ok = match std::fs::create_dir_all(&hook_dir) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("could not create dcg-guard hook dir: {e}");
            false
        },
    };

    if dir_ok {
        let hook_md_needs_write = match std::fs::read_to_string(&hook_md) {
            Ok(existing) => !existing.contains(DCG_GUARD_HOOK_MD_FINGERPRINT),
            Err(_) => true,
        };
        if hook_md_needs_write && let Err(e) = std::fs::write(&hook_md, DCG_GUARD_HOOK_MD) {
            tracing::debug!("could not write dcg-guard HOOK.md: {e}");
        }

        let handler_needs_write = match std::fs::read_to_string(&handler) {
            Ok(existing) => !existing.contains(DCG_GUARD_HANDLER_FINGERPRINT),
            Err(_) => true,
        };
        if handler_needs_write {
            if let Err(e) = std::fs::write(&handler, DCG_GUARD_HANDLER_SH) {
                tracing::debug!("could not write dcg-guard handler.sh: {e}");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&handler, std::fs::Permissions::from_mode(0o755));
            }
            if !hook_md_needs_write {
                tracing::info!(
                    "dcg-guard: refreshed stale handler.sh to apply PATH augmentation fix"
                );
            }
        }
    }

    log_dcg_guard_status().await;
}

/// PATH augmentation prepended by the dcg-guard handler script.
pub(crate) const DCG_GUARD_EXTRA_PATH_DIRS: &[&str] =
    &[".local/bin", "/usr/local/bin", "/opt/homebrew/bin"];

/// Fallback `$HOME` used by `resolve_dcg_binary`.
pub(crate) const DCG_GUARD_HOME_FALLBACK: &str = "/root";

/// Resolve `dcg` using the same augmented `PATH` as the handler script.
fn resolve_dcg_binary() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    let home_path = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DCG_GUARD_HOME_FALLBACK));
    for rel in DCG_GUARD_EXTRA_PATH_DIRS {
        if rel.starts_with('/') {
            dirs.push(PathBuf::from(rel));
        } else {
            dirs.push(home_path.join(rel));
        }
    }

    let existing = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
    for entry in existing.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(entry));
    }

    for dir in dirs {
        let candidate = dir.join("dcg");
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&candidate)
                    && meta.permissions().mode() & 0o111 != 0
                {
                    return Some(candidate);
                }
                continue;
            }
            #[cfg(not(unix))]
            {
                return Some(candidate);
            }
        }
    }

    None
}

/// Emit a single startup log line describing whether the dcg-guard is active.
async fn log_dcg_guard_status() {
    let Some(path) = resolve_dcg_binary() else {
        tracing::warn!(
            "dcg-guard: 'dcg' not found on PATH; destructive command guard is INACTIVE. \
             Install dcg from https://github.com/Dicklesworthstone/destructive_command_guard"
        );
        return;
    };

    let version = tokio::process::Command::new(&path)
        .arg("--version")
        .output()
        .await
        .ok()
        .and_then(|out| {
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown version".to_string());

    tracing::info!(
        dcg_path = %path.display(),
        "dcg-guard: dcg {version} detected, guard active"
    );
}

/// Seed built-in personal skills into `~/.antre/skills/`.
pub(crate) fn seed_example_skill() {
    seed_skill_if_missing("template-skill", EXAMPLE_SKILL_MD);
    seed_skill_if_missing("tmux", TMUX_SKILL_MD);
}

fn seed_skill_if_missing(name: &str, content: &str) {
    let skill_dir = antre_config::data_dir().join(format!("skills/{name}"));
    let skill_md = skill_dir.join("SKILL.md");
    if skill_md.exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&skill_dir) {
        tracing::debug!("could not create {name} skill dir: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&skill_md, content) {
        tracing::debug!("could not write {name} SKILL.md: {e}");
    }
}

// ── Hook discovery ───────────────────────────────────────────────────────────

/// Metadata for built-in hooks (compiled Rust, always active).
fn builtin_hook_metadata() -> Vec<(
    &'static str,
    &'static str,
    Vec<antre_common::hooks::HookEvent>,
    &'static str,
)> {
    use antre_common::hooks::HookEvent;
    vec![
        (
            "command-logger",
            "Logs all slash-command invocations to a JSONL audit file at ~/.antre/logs/commands.log.",
            vec![HookEvent::Command],
            "crates/plugins/src/bundled/command_logger.rs",
        ),
        (
            "session-memory",
            "Saves the conversation history to a markdown file in the memory directory when a session is reset or a new session is created, making it searchable for future sessions.",
            vec![HookEvent::Command],
            "crates/plugins/src/bundled/session_memory.rs",
        ),
        (
            "auto-checkpoint",
            "Snapshots files before Write/Edit/MultiEdit tool calls so /rollback can restore them.",
            vec![HookEvent::BeforeToolCall],
            "crates/tools/src/auto_checkpoint.rs",
        ),
    ]
}

/// Discover hooks from the filesystem, check eligibility, and build a
/// [`HookRegistry`] plus a `Vec<DiscoveredHookInfo>` for the web UI.
/// Built-in hooks contributed by optional gateway services.
#[cfg_attr(not(feature = "canvas"), allow(unused_variables))]
pub(crate) fn builtin_service_hooks(
    services: &crate::services::GatewayServices,
) -> Vec<Arc<dyn antre_common::hooks::HookHandler>> {
    #[cfg_attr(not(feature = "canvas"), allow(unused_mut))]
    let mut hooks = Vec::new();
    #[cfg(feature = "canvas")]
    hooks.extend(crate::canvas::hooks(services.canvas.as_ref()));
    hooks
}

#[cfg(test)]
pub(crate) async fn discover_and_build_hooks(
    disabled: &HashSet<String>,
    session_store: Option<&Arc<SessionStore>>,
) -> (
    Option<Arc<antre_common::hooks::HookRegistry>>,
    Vec<crate::state::DiscoveredHookInfo>,
) {
    discover_and_build_hooks_with(disabled, session_store, Vec::new()).await
}

/// [`discover_and_build_hooks`] plus built-in handlers owned by optional
/// services (e.g. the canvas), which must survive hook reloads.
pub(crate) async fn discover_and_build_hooks_with(
    disabled: &HashSet<String>,
    session_store: Option<&Arc<SessionStore>>,
    extra_builtin: Vec<Arc<dyn antre_common::hooks::HookHandler>>,
) -> (
    Option<Arc<antre_common::hooks::HookRegistry>>,
    Vec<crate::state::DiscoveredHookInfo>,
) {
    use antre_plugins::{
        bundled::{command_logger::CommandLoggerHook, session_memory::SessionMemoryHook},
        hook_discovery::{FsHookDiscoverer, HookDiscoverer, HookSource},
        hook_eligibility::check_hook_eligibility,
        shell_hook::ShellHookHandler,
    };

    let config = antre_config::discover_and_load();
    let discoverer = FsHookDiscoverer::new(FsHookDiscoverer::default_paths());
    let discovered = discoverer.discover().await.unwrap_or_default();
    let session_export_mode = config.memory.session_export;

    let mut registry = antre_common::hooks::HookRegistry::new();
    let mut info_list = Vec::with_capacity(discovered.len());
    let approvals = shell_hook_approvals(&discovered).await;

    for (parsed, source) in &discovered {
        let meta = &parsed.metadata;
        let elig = check_hook_eligibility(meta);
        let is_disabled = disabled.contains(&meta.name);
        let needs_approval = approvals.get(&meta.name).cloned();
        let is_enabled = elig.eligible && !is_disabled && needs_approval.is_none();

        if !elig.eligible {
            info!(
                hook = %meta.name,
                source = ?source,
                missing_os = elig.missing_os,
                missing_bins = ?elig.missing_bins,
                missing_env = ?elig.missing_env,
                "hook ineligible, skipping"
            );
        }

        let raw_content =
            std::fs::read_to_string(parsed.source_path.join("HOOK.md")).unwrap_or_default();

        let source_str = match source {
            HookSource::Project => "project",
            HookSource::User => "user",
            HookSource::Bundled => "bundled",
        };

        info_list.push(crate::state::DiscoveredHookInfo {
            name: meta.name.clone(),
            description: meta.description.clone(),
            emoji: meta.emoji.clone(),
            events: meta.events.iter().map(|e| e.to_string()).collect(),
            command: meta.command.clone(),
            timeout: meta.timeout,
            priority: meta.priority,
            source: source_str.to_string(),
            source_path: parsed.source_path.display().to_string(),
            eligible: elig.eligible,
            missing_os: elig.missing_os,
            missing_bins: elig.missing_bins.clone(),
            missing_env: elig.missing_env.clone(),
            enabled: is_enabled,
            body: raw_content,
            body_html: crate::services::markdown_to_html(&parsed.body),
            call_count: 0,
            failure_count: 0,
            avg_latency_ms: 0,
            needs_approval,
        });

        if is_enabled && let Some(ref command) = meta.command {
            let handler = ShellHookHandler::new(
                meta.name.clone(),
                command.clone(),
                meta.events.clone(),
                std::time::Duration::from_secs(meta.timeout),
                meta.env.clone(),
                Some(parsed.source_path.clone()),
            )
            .with_protocol(meta.protocol);
            registry.register(Arc::new(handler));
        }
    }

    let filesystem_hook_names = discovered
        .iter()
        .map(|(parsed, _source)| parsed.metadata.name.as_str())
        .collect::<HashSet<_>>();
    let mut config_hook_count = 0;
    let config_path = antre_config::config_dir()
        .map(|path| path.join("antre.toml").display().to_string())
        .unwrap_or_else(|| "antre.toml".to_string());
    let mut config_hook_names = HashSet::new();

    if let Some(hooks_config) = config.hooks.as_ref() {
        for hook in &hooks_config.hooks {
            if filesystem_hook_names.contains(hook.name.as_str()) {
                warn!(
                    hook = %hook.name,
                    "config hook conflicts with filesystem hook; keeping filesystem hook"
                );
                continue;
            }

            if !config_hook_names.insert(hook.name.as_str()) {
                warn!(
                    hook = %hook.name,
                    "duplicate config hook name; keeping first config hook"
                );
                continue;
            }

            let events = hook
                .events
                .iter()
                .filter_map(|event| match event.parse::<antre_common::hooks::HookEvent>() {
                    Ok(event) => Some(event),
                    Err(e) => {
                        warn!(hook = %hook.name, event = %event, error = %e, "skipping invalid config hook event");
                        None
                    },
                })
                .collect::<Vec<_>>();
            let event_names = events.iter().map(|event| event.to_string()).collect();
            let is_enabled = !disabled.contains(&hook.name) && !events.is_empty();
            config_hook_count += 1;

            info_list.push(crate::state::DiscoveredHookInfo {
                name: hook.name.clone(),
                description: String::new(),
                emoji: None,
                events: event_names,
                command: Some(hook.command.clone()),
                timeout: hook.timeout,
                priority: 0,
                source: "config".to_string(),
                source_path: config_path.clone(),
                eligible: !events.is_empty(),
                missing_os: false,
                missing_bins: vec![],
                missing_env: vec![],
                enabled: is_enabled,
                body: String::new(),
                body_html: "<p><em>Shell hook declared in antre.toml.</em></p>".to_string(),
                call_count: 0,
                failure_count: 0,
                avg_latency_ms: 0,
                needs_approval: None,
            });

            if is_enabled {
                let handler = ShellHookHandler::new(
                    hook.name.clone(),
                    hook.command.clone(),
                    events,
                    std::time::Duration::from_secs(hook.timeout),
                    hook.env.clone(),
                    None,
                );
                registry.register(Arc::new(handler));
            }
        }
    }

    // ── Built-in hooks (compiled Rust, always active) ──────────────────
    {
        let data = antre_config::data_dir();
        let mut builtin: Vec<(&str, Arc<dyn antre_common::hooks::HookHandler>)> = Vec::new();

        let log_path =
            CommandLoggerHook::default_path().unwrap_or_else(|| data.join("logs/commands.log"));
        builtin.push((
            "hook.command-logger",
            Arc::new(CommandLoggerHook::new(log_path)),
        ));

        if let Some(store) = session_store
            && !matches!(session_export_mode, antre_config::SessionExportMode::Off)
        {
            let memory_hook = SessionMemoryHook::new(data.clone(), Arc::clone(store));
            builtin.push(("hook.session-memory", Arc::new(memory_hook)));
        }

        // Auto-checkpoint: snapshot files before Write/Edit/MultiEdit tool calls.
        let checkpoint_manager = Arc::new(antre_tools::checkpoints::CheckpointManager::new(data));
        let auto_cp = antre_tools::auto_checkpoint::AutoCheckpointHook::new(checkpoint_manager);
        builtin.push(("hook.auto-checkpoint", Arc::new(auto_cp)));

        register_builtin_and_mod_hooks(&mut registry, &mut info_list, builtin, disabled);
        for handler in extra_builtin {
            registry.register(handler);
        }
    }

    for (name, description, events, source_file) in builtin_hook_metadata() {
        let enabled = if name == "session-memory" {
            !matches!(session_export_mode, antre_config::SessionExportMode::Off)
        } else {
            true
        };
        info_list.push(crate::state::DiscoveredHookInfo {
            name: name.to_string(),
            description: description.to_string(),
            emoji: Some("\u{2699}\u{fe0f}".to_string()),
            events: events.iter().map(|e| e.to_string()).collect(),
            command: None,
            timeout: 0,
            priority: 0,
            source: "builtin".to_string(),
            source_path: source_file.to_string(),
            eligible: true,
            missing_os: false,
            missing_bins: vec![],
            missing_env: vec![],
            enabled,
            body: String::new(),
            body_html: format!(
                "<p><em>Built-in hook implemented in Rust.</em></p><p>{}</p>",
                description
            ),
            call_count: 0,
            failure_count: 0,
            avg_latency_ms: 0,
            needs_approval: None,
        });
    }

    if !info_list.is_empty() {
        info!(
            "{} hook(s) discovered ({} filesystem shell, {} config shell, {} built-in), {} registered",
            info_list.len(),
            discovered.len(),
            config_hook_count,
            info_list.len() - discovered.len() - config_hook_count,
            registry.handler_names().len()
        );
    }

    (Some(Arc::new(registry)), info_list)
}

type Discovered = [(
    antre_plugins::hook_metadata::ParsedHook,
    antre_plugins::hook_discovery::HookSource,
)];

/// Filesystem shell hooks held back by content pinning (`name → reason`).
#[cfg(feature = "mods")]
async fn shell_hook_approvals(
    discovered: &Discovered,
) -> std::collections::HashMap<String, String> {
    let hooks = discovered
        .iter()
        .map(|(parsed, _)| (parsed.metadata.name.clone(), parsed.source_path.clone()))
        .collect();
    crate::mods::shell_hook_approvals(hooks).await
}

#[cfg(not(feature = "mods"))]
async fn shell_hook_approvals(
    _discovered: &Discovered,
) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}

/// Register built-in hooks through the mods contribution registry (as
/// `core/*`, next to mod hooks) when the runtime is on, directly otherwise.
fn register_builtin_and_mod_hooks(
    registry: &mut antre_common::hooks::HookRegistry,
    info_list: &mut Vec<crate::state::DiscoveredHookInfo>,
    builtin: Vec<(&str, Arc<dyn antre_common::hooks::HookHandler>)>,
    disabled: &HashSet<String>,
) {
    #[cfg(feature = "mods")]
    if let Some(service) = crate::mods::service() {
        match service.replace_core_hooks(builtin.clone()) {
            Ok(()) => {
                // Built-in hooks stay always active; mod hooks follow the
                // Hooks page enable/disable switch like shell hooks.
                for handler in service.hook_handlers() {
                    if !disabled.contains(handler.name()) {
                        registry.register(handler);
                    }
                }
                info_list.extend(mod_hook_infos(&service, disabled));
                return;
            },
            Err(error) => warn!(%error, "could not register built-in hooks through mods"),
        }
    }
    let _ = (&info_list, disabled);
    for (_, handler) in builtin {
        registry.register(handler);
    }
}

#[cfg(feature = "mods")]
fn mod_hook_infos(
    service: &antre_mods::service::ModService,
    disabled: &HashSet<String>,
) -> Vec<crate::state::DiscoveredHookInfo> {
    service
        .registry()
        .hooks()
        .into_iter()
        .filter(|(id, _)| matches!(id.owner(), antre_mods::Owner::Mod(_)))
        .map(|(id, handler)| crate::state::DiscoveredHookInfo {
            name: handler.name().to_string(),
            description: format!("WASM hook contributed by mod {}", id.owner()),
            emoji: None,
            events: handler.events().iter().map(ToString::to_string).collect(),
            command: None,
            timeout: 0,
            priority: handler.priority(),
            source: "mod".to_string(),
            source_path: id.to_string(),
            eligible: true,
            missing_os: false,
            missing_bins: vec![],
            missing_env: vec![],
            enabled: !disabled.contains(handler.name()),
            body: String::new(),
            body_html: "<p><em>Hook provided by an installed mod.</em></p>".to_string(),
            call_count: 0,
            failure_count: 0,
            avg_latency_ms: 0,
            needs_approval: None,
        })
        .collect()
}

#[allow(clippy::unwrap_used)]
#[cfg(all(test, feature = "mods"))]
mod seeded_tests {
    use super::seeded_files_match;

    #[test]
    fn seeded_files_must_match_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let files: &[(&str, &str)] = &[("HOOK.md", "seed"), ("handler.sh", "run")];
        assert!(!seeded_files_match(dir.path(), files));
        std::fs::write(dir.path().join("HOOK.md"), "seed").unwrap();
        std::fs::write(dir.path().join("handler.sh"), "run").unwrap();
        assert!(seeded_files_match(dir.path(), files));
        std::fs::write(dir.path().join("handler.sh"), "run; curl evil").unwrap();
        assert!(!seeded_files_match(dir.path(), files));
        std::fs::write(dir.path().join("handler.sh"), "run").unwrap();
        std::fs::write(dir.path().join("extra.sh"), "x").unwrap();
        assert!(!seeded_files_match(dir.path(), files));
        assert!(!seeded_files_match(&dir.path().join("missing"), files));
    }
}
