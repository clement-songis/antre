use {
    super::ProviderRegistry,
    anyhow::Context as _,
    moltis_agents::model::ChatMessage,
    moltis_config::schema::{ProviderEntry, ProvidersConfig},
    secrecy::Secret,
    serde_json::Value,
    std::{
        collections::HashMap,
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    },
};

struct Captured {
    request_line: String,
    authorization: Option<String>,
    body: Value,
}

fn capture_one_request() -> anyhow::Result<(String, mpsc::Receiver<anyhow::Result<Captured>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind test server")?;
    let base_url = format!("http://{}", listener.local_addr().context("local addr")?);
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Captured> {
            let (mut stream, _) = listener.accept().context("accept request")?;
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 4096];
            let (head, body_start) = loop {
                let read = stream.read(&mut chunk).context("read request")?;
                anyhow::ensure!(read > 0, "connection closed before headers");
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    break (String::from_utf8_lossy(&buffer[..pos]).to_string(), pos + 4);
                }
            };
            let header = |name: &str| {
                head.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case(name)
                        .then(|| value.trim().to_string())
                })
            };
            let content_length: usize = header("content-length")
                .and_then(|value| value.parse().ok())
                .context("content length")?;
            while buffer.len() - body_start < content_length {
                let read = stream.read(&mut chunk).context("read body")?;
                anyhow::ensure!(read > 0, "connection closed before body");
                buffer.extend_from_slice(&chunk[..read]);
            }
            let body = serde_json::from_slice(&buffer[body_start..body_start + content_length])
                .context("json body")?;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 53\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"ok\"}}],\"usage\":{}}")
                .context("write response")?;
            Ok(Captured {
                request_line: head.lines().next().unwrap_or_default().to_string(),
                authorization: header("authorization"),
                body,
            })
        })();
        let _ = tx.send(result);
    });
    Ok((base_url, rx))
}

fn offered(name: &str, entry: ProviderEntry) -> ProvidersConfig {
    let mut config = ProvidersConfig {
        offered: vec![name.into()],
        ..ProvidersConfig::default()
    };
    config.providers.insert(name.into(), entry);
    config
}

#[tokio::test]
async fn hosted_provider_sends_bearer_chat_without_strict_tools() -> anyhow::Result<()> {
    let (base_url, rx) = capture_one_request()?;
    let config = offered("together", ProviderEntry {
        api_key: Some(Secret::new("tg-test".into())),
        base_url: Some(base_url),
        models: vec!["zai-org/GLM-5.3".into()],
        ..ProviderEntry::default()
    });
    let mut registry = ProviderRegistry::empty();
    registry.register_openai_compatible_providers(&config, &HashMap::new(), &HashMap::new());
    let provider = registry
        .get("together::zai-org/GLM-5.3")
        .context("registered together model")?;
    provider
        .complete(&[ChatMessage::user("hi")], &[serde_json::json!({
            "name": "lookup",
            "description": "Look something up",
            "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}
        })])
        .await
        .context("completion")?;

    let captured = rx.recv().context("captured")??;
    assert!(
        captured.request_line.starts_with("POST /chat/completions"),
        "{}",
        captured.request_line
    );
    assert_eq!(captured.authorization.as_deref(), Some("Bearer tg-test"));
    assert_eq!(captured.body["model"], "zai-org/GLM-5.3");
    let tool = &captured.body["tools"][0]["function"];
    assert_ne!(tool.get("strict"), Some(&Value::Bool(true)));
    Ok(())
}

#[test]
fn cloudflare_base_url_takes_the_account_id_from_env() {
    let config = offered("cloudflare", ProviderEntry {
        api_key: Some(Secret::new("cf-test".into())),
        models: vec!["@cf/openai/gpt-oss-120b".into()],
        ..ProviderEntry::default()
    });
    let env = HashMap::from([("CLOUDFLARE_ACCOUNT_ID".to_string(), "acct123".to_string())]);
    let mut registry = ProviderRegistry::empty();
    registry.register_openai_compatible_providers(&config, &env, &HashMap::new());
    assert!(
        registry
            .list_models()
            .iter()
            .any(|m| m.provider == "cloudflare" && m.id == "cloudflare::@cf/openai/gpt-oss-120b")
    );
}

#[test]
fn cloudflare_without_account_id_is_skipped() {
    if std::env::var_os("CLOUDFLARE_ACCOUNT_ID").is_some() {
        return;
    }
    let config = offered("cloudflare", ProviderEntry {
        api_key: Some(Secret::new("cf-test".into())),
        models: vec!["@cf/openai/gpt-oss-120b".into()],
        ..ProviderEntry::default()
    });
    let mut registry = ProviderRegistry::empty();
    registry.register_openai_compatible_providers(&config, &HashMap::new(), &HashMap::new());
    assert!(
        registry
            .list_models()
            .iter()
            .all(|m| m.provider != "cloudflare")
    );
}

#[test]
fn cloudflare_without_models_falls_back_to_the_catalog() {
    let config = offered("cloudflare", ProviderEntry {
        api_key: Some(Secret::new("cf-test".into())),
        ..ProviderEntry::default()
    });
    let env = HashMap::from([("CLOUDFLARE_ACCOUNT_ID".to_string(), "acct123".to_string())]);
    let mut registry = ProviderRegistry::empty();
    registry.register_openai_compatible_providers(&config, &env, &HashMap::new());
    let models: Vec<_> = registry
        .list_models()
        .iter()
        .filter(|m| m.provider == "cloudflare")
        .collect();
    assert!(!models.is_empty(), "catalog fallback registered nothing");
}

#[test]
fn azure_registers_only_configured_deployments() {
    let env = HashMap::from([("AZURE_RESOURCE_NAME".to_string(), "contoso".to_string())]);
    let empty = offered("azure", ProviderEntry {
        api_key: Some(Secret::new("az-test".into())),
        ..ProviderEntry::default()
    });
    let mut registry = ProviderRegistry::empty();
    registry.register_openai_compatible_providers(&empty, &env, &HashMap::new());
    assert!(registry.list_models().iter().all(|m| m.provider != "azure"));

    let configured = offered("azure", ProviderEntry {
        api_key: Some(Secret::new("az-test".into())),
        models: vec!["my-gpt-deployment".into()],
        ..ProviderEntry::default()
    });
    let mut registry = ProviderRegistry::empty();
    registry.register_openai_compatible_providers(&configured, &env, &HashMap::new());
    assert!(
        registry
            .list_models()
            .iter()
            .any(|m| m.provider == "azure" && m.id == "azure::my-gpt-deployment")
    );
}
