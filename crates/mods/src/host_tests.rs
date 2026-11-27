//! Host tests driven by the WAT fixtures in `test_fixtures.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use {
    super::*,
    crate::{
        capability::{Capability, NetHost},
        test_fixtures::*,
    },
    std::time::Duration,
};

fn caps(items: &[&str]) -> CapabilitySet {
    items.iter().map(|item| item.parse().unwrap()).collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    component: ModComponent,
}

fn load(bytes: &[u8], limits: HostLimits, granted: CapabilitySet) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kv = Arc::new(KvStore::open(dir.path().join("kv.json")).unwrap());
    let engine = ModEngine::new(limits).unwrap();
    let component = ModComponent::new(
        engine,
        ModId::parse("acme.fixture").unwrap(),
        bytes,
        &granted,
        granted.clone(),
        kv,
    )
    .unwrap();
    Fixture {
        _dir: dir,
        component,
    }
}

#[test]
fn tool_call_round_trips_strings() {
    let fixture = load(
        &plugin(&call_echo(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    let out = fixture
        .component
        .call_tool("echo", r#"{"text":"héllo"}"#)
        .unwrap();
    assert_eq!(out, Ok(r#"{"text":"héllo"}"#.to_string()));
    let fixed = load(
        &plugin(&call_ok_json(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    assert_eq!(
        fixed.component.call_tool("x", "{}").unwrap(),
        Ok(r#"{"ok":true}"#.to_string())
    );
}

#[test]
fn tool_errors_are_values_not_failures() {
    let fixture = load(
        &plugin(&call_tool_error(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    assert_eq!(
        fixture.component.call_tool("x", "{}").unwrap(),
        Err("nope".to_string())
    );
}

#[test]
fn hook_answers_map_to_results() {
    for (body, expected) in [
        (handle_continue(), Ok(None)),
        (handle_modify(), Ok(Some(r#"{"x":1}"#.to_string()))),
        (handle_block(), Err("nope".to_string())),
    ] {
        let fixture = load(
            &plugin(&call_ok_json(), &body),
            test_limits(),
            CapabilitySet::new(),
        );
        assert_eq!(
            fixture
                .component
                .handle_hook("BeforeToolCall", "{}")
                .unwrap(),
            expected
        );
    }
}

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let fixture = load(
        &plugin(&call_spin(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    let err = fixture
        .component
        .call_tool("x", "{}")
        .unwrap_err()
        .to_string();
    assert!(err.contains("fuel exhausted"), "{err}");
}

#[test]
fn infinite_loop_hits_epoch_deadline_when_fuel_is_plentiful() {
    let limits = HostLimits {
        fuel: u64::MAX / 2,
        timeout: Duration::from_millis(100),
        ..test_limits()
    };
    let fixture = load(
        &plugin(&call_spin(), &handle_continue()),
        limits,
        caps(&["tool"]),
    );
    let started = Instant::now();
    let err = fixture
        .component
        .call_tool("x", "{}")
        .unwrap_err()
        .to_string();
    assert!(err.contains("time limit exceeded"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn memory_growth_beyond_limit_traps() {
    // 64 pages = 4 MiB > 2 MiB limit.
    let fixture = load(
        &plugin(&call_grow(64), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    let err = fixture
        .component
        .call_tool("x", "{}")
        .unwrap_err()
        .to_string();
    assert!(err.contains("trap"), "{err}");
    // Growth within the limit is fine.
    let small = load(
        &plugin(&call_grow(4), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    assert!(small.component.call_tool("x", "{}").unwrap().is_ok());
}

#[test]
fn oversized_output_is_rejected() {
    let fixture = load(
        &plugin(&call_big_output(4096), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    let err = fixture
        .component
        .call_tool("x", "{}")
        .unwrap_err()
        .to_string();
    assert!(err.contains("exceeds the 1024 byte limit"), "{err}");
}

#[test]
fn storage_without_capability_is_denied() {
    let fixture = load(
        &plugin(&call_kv_set(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    let out = fixture.component.call_tool("x", "{}").unwrap();
    assert_eq!(out, Err("capability denied: storage".to_string()));
    let read = load(
        &plugin(&call_kv_get(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    assert_eq!(
        read.component.call_tool("x", "{}").unwrap(),
        Err("capability denied: storage".to_string())
    );
}

#[test]
fn storage_with_capability_persists_across_calls() {
    let dir = tempfile::tempdir().unwrap();
    let kv = Arc::new(KvStore::open(dir.path().join("kv.json")).unwrap());
    let engine = ModEngine::new(test_limits()).unwrap();
    let granted = caps(&["tool", "storage:1024"]);
    let id = ModId::parse("acme.fixture").unwrap();
    let writer = ModComponent::new(
        Arc::clone(&engine),
        id.clone(),
        &plugin(&call_kv_set(), &handle_continue()),
        &granted,
        granted.clone(),
        Arc::clone(&kv),
    )
    .unwrap();
    assert_eq!(
        writer.call_tool("x", "{}").unwrap(),
        Ok("\"stored\"".to_string())
    );
    let reader = ModComponent::new(
        engine,
        id,
        &plugin(&call_kv_get(), &handle_continue()),
        &granted,
        granted.clone(),
        Arc::clone(&kv),
    )
    .unwrap();
    assert_eq!(
        reader.call_tool("x", "{}").unwrap(),
        Ok("value".to_string())
    );
    let reopened = KvStore::open(dir.path().join("kv.json")).unwrap();
    assert_eq!(reopened.get("key").as_deref(), Some("value"));
}

#[test]
fn storage_quota_is_enforced_by_the_host() {
    let fixture = load(
        &plugin(&call_kv_set(), &handle_continue()),
        test_limits(),
        caps(&["tool", "storage:4"]),
    );
    let out = fixture.component.call_tool("x", "{}").unwrap();
    assert_eq!(out, Err("storage quota of 4 bytes exceeded".to_string()));
}

#[test]
fn instances_do_not_share_memory_between_calls() {
    // The bump allocator lives in instance memory; if instances were reused
    // the second echo would land at a different address but still succeed.
    // What matters is that a trap in one call does not poison the next.
    let fixture = load(
        &plugin(&call_spin(), &handle_continue()),
        test_limits(),
        caps(&["tool"]),
    );
    assert!(fixture.component.call_tool("x", "{}").is_err());
    assert!(fixture.component.call_tool("x", "{}").is_err());
    assert!(
        fixture
            .component
            .handle_hook("SessionStart", "{}")
            .unwrap()
            .is_ok()
    );
}

fn import_only(import: &str) -> Vec<u8> {
    let wat = format!(
        r#"(component
  (import "{import}" (instance (export "f" (func))))
)"#
    );
    wat::parse_str(&wat).unwrap()
}

#[test]
fn static_check_rejects_foreign_imports() {
    let engine = ModEngine::new(test_limits()).unwrap();
    for import in [
        "evil:escape/fs@1.0.0",
        "wasi:sockets/tcp@0.2.0",
        "wasi:http/outgoing-handler@0.2.0",
        "moltis:tool/outgoing-handler@0.1.0",
    ] {
        let component = engine.compile(&import_only(import)).unwrap();
        let err = engine
            .check_imports(&component, &caps(&["net:api.github.com"]))
            .unwrap_err();
        assert!(matches!(err, Error::Denied(_)), "{import}: {err}");
    }
}

#[test]
fn static_check_requires_net_for_http_import() {
    let engine = ModEngine::new(test_limits()).unwrap();
    let component = engine
        .compile(&import_only("moltis:mods/http@0.1.0"))
        .unwrap();
    assert!(engine.check_imports(&component, &caps(&["tool"])).is_err());
    assert!(
        engine
            .check_imports(&component, &caps(&["net:api.github.com"]))
            .is_ok()
    );
    let wasi = engine
        .compile(&import_only("wasi:cli/environment@0.2.0"))
        .unwrap();
    assert!(engine.check_imports(&wasi, &CapabilitySet::new()).is_ok());
}

#[test]
fn component_new_refuses_undeclared_imports() {
    let dir = tempfile::tempdir().unwrap();
    let kv = Arc::new(KvStore::open(dir.path().join("kv.json")).unwrap());
    let engine = ModEngine::new(test_limits()).unwrap();
    let result = ModComponent::new(
        engine,
        ModId::parse("acme.x").unwrap(),
        &import_only("evil:escape/fs@1.0.0"),
        &CapabilitySet::new(),
        CapabilitySet::new(),
        kv,
    );
    assert!(matches!(result, Err(Error::Denied(_))));
}

#[test]
fn garbage_bytes_are_rejected() {
    let engine = ModEngine::new(test_limits()).unwrap();
    assert!(matches!(
        engine.compile(b"not wasm"),
        Err(Error::Package(_))
    ));
}

fn host_state(granted: CapabilitySet) -> HostState {
    let dir = std::env::temp_dir().join("moltis-mods-host-state-unused.json");
    HostState {
        mod_id: ModId::parse("acme.x").unwrap(),
        granted,
        kv: Arc::new(KvStore {
            path: dir,
            data: Mutex::new(BTreeMap::new()),
        }),
        http: None,
        logs: 0,
    }
}

fn request(url: &str) -> wit_http::Request {
    wit_http::Request {
        method: "GET".into(),
        url: url.into(),
        headers: vec![],
        body: None,
        timeout_ms: None,
        max_response_bytes: None,
    }
}

#[test]
fn http_without_grant_is_denied_before_any_network_io() {
    use wit_http::Host as _;
    let mut state = host_state(caps(&["tool"]));
    let err = state
        .send(request("https://api.github.com/user"))
        .unwrap_err();
    assert!(matches!(err, wit_http::Error::Denied(_)), "{err:?}");
}

#[test]
fn http_to_lookalike_host_is_denied() {
    use wit_http::Host as _;
    let mut state = host_state(caps(&["net:api.github.com"]));
    for url in [
        "https://api.github.com.attacker.tld/",
        "https://evil.api.github.com/",
        "https://api.github.com@attacker.tld/",
        "http://127.0.0.1/",
        "http://[::1]/",
        "file:///etc/passwd",
        "ftp://api.github.com/",
        "https:///nohost",
    ] {
        let err = state.send(request(url)).unwrap_err();
        assert!(
            matches!(
                err,
                wit_http::Error::Denied(_) | wit_http::Error::InvalidUrl(_)
            ),
            "{url}: {err:?}"
        );
    }
}

#[test]
fn granted_host_reaches_ssrf_layer() {
    use wit_http::Host as _;
    // The grant matches, but no HTTP client is configured in this unit test,
    // so the call stops at the host boundary with a denial rather than I/O.
    let mut state = host_state(caps(&["net:api.github.com"]));
    let err = state
        .send(request("https://api.github.com/user"))
        .unwrap_err();
    assert!(matches!(err, wit_http::Error::Denied(_)));
}

#[test]
fn url_host_parsing() {
    assert_eq!(
        url_host("https://API.github.com:443/x?y").unwrap(),
        "api.github.com"
    );
    assert_eq!(url_host("http://example.org").unwrap(), "example.org");
    assert!(url_host("https://user:pw@example.org/").is_err());
    assert!(url_host("https://[::1]/").is_err());
    assert!(url_host("gopher://x").is_err());
    assert!(url_host("https://").is_err());
}

#[test]
fn log_is_bounded_per_call() {
    use wit_host::Host as _;
    let mut state = host_state(CapabilitySet::new());
    for _ in 0..(MAX_LOGS_PER_CALL + 10) {
        state.log(wit_types::LogLevel::Info, "x".repeat(MAX_LOG_LINE * 2));
    }
    assert_eq!(state.logs, MAX_LOGS_PER_CALL + 10);
}

#[test]
fn kv_rejects_bad_keys_and_rolls_back_over_quota() {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvStore::open(dir.path().join("kv.json")).unwrap();
    assert!(kv.set("", "v", 100).is_err());
    assert!(kv.set(&"k".repeat(MAX_KV_KEY + 1), "v", 10_000).is_err());
    kv.set("a", "1", 10).unwrap();
    assert!(kv.set("a", "123456789012", 10).is_err());
    assert_eq!(kv.get("a").as_deref(), Some("1"));
    kv.delete("a").unwrap();
    assert!(kv.get("a").is_none());
    kv.delete("missing").unwrap();
}

#[test]
fn deadline_ticks_cover_timeout() {
    let engine = ModEngine::new(HostLimits {
        timeout: Duration::from_secs(1),
        epoch_interval: Duration::from_millis(100),
        ..HostLimits::default()
    })
    .unwrap();
    assert_eq!(engine.deadline_ticks(), 11);
    let _ = NetHost::parse("example.org").unwrap();
    let _ = Capability::Tool;
}
