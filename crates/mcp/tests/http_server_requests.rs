//! Streamable HTTP: a server request sent inside a POST's SSE response is
//! answered with a separate POST before the original response arrives.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use {
    moltis_mcp::{
        ClientSetup, HostContext, McpClient, McpClientTrait, McpServerConfig, ServerPolicy,
        protocol::Root, registry::TransportType, remote::ResolvedRemoteConfig,
        sse_transport::SseTransport, types::ToolContent,
    },
    secrecy::Secret,
    serde_json::{Value, json},
    tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::{Mutex, oneshot},
    },
};

struct Request {
    method: String,
    headers: String,
    body: Value,
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let method = headers.split_whitespace().next()?.to_string();
    let body =
        serde_json::from_slice(&buffer[header_end..header_end + length]).unwrap_or(Value::Null);
    Some(Request {
        method,
        headers,
        body,
    })
}

async fn respond_json(stream: &mut TcpStream, status: &str, body: &Value) {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nMcp-Session-Id: s-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

#[derive(Default)]
struct Shared {
    reply: Option<oneshot::Sender<Value>>,
    headers: Vec<String>,
}

async fn handle(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    shared.lock().await.headers.push(request.headers.clone());
    if request.method == "GET" {
        let _ = stream
            .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    let body = request.body;
    let Some(method) = body.get("method").and_then(Value::as_str) else {
        // The client's answer to our server request.
        if let Some(tx) = shared.lock().await.reply.take() {
            let _ = tx.send(body);
        }
        let _ = stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    let id = body.get("id").cloned();
    match (method, id) {
        ("initialize", Some(id)) => {
            respond_json(&mut stream, "200 OK", &json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "http-mock"}}
            }))
            .await;
        },
        ("tools/list", Some(id)) => {
            respond_json(&mut stream, "200 OK", &json!({
                "jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "where", "inputSchema": {"type": "object"}}]}
            }))
            .await;
        },
        ("tools/call", Some(id)) => {
            let (tx, rx) = oneshot::channel();
            shared.lock().await.reply = Some(tx);
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let request = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"});
            let _ = stream
                .write_all(format!("{head}event: message\ndata: {request}\n\n").as_bytes())
                .await;
            let answer = tokio::time::timeout(Duration::from_secs(5), rx)
                .await
                .unwrap()
                .unwrap();
            let roots = answer["result"]["roots"].to_string();
            let response = json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": roots}]}});
            let _ = stream
                .write_all(format!("data: {response}\n\n").as_bytes())
                .await;
        },
        _ => {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        },
    }
}

#[tokio::test]
async fn server_request_inside_sse_response_is_answered() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shared = Arc::new(Mutex::new(Shared::default()));
    let server_shared = Arc::clone(&shared);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle(stream, Arc::clone(&server_shared)));
        }
    });

    let config = McpServerConfig {
        transport: TransportType::StreamableHttp,
        url: Some(Secret::new(format!("http://{address}/mcp"))),
        ..Default::default()
    };
    let remote = ResolvedRemoteConfig::from_server_config(&config, &Default::default()).unwrap();
    let transport = SseTransport::new_with_remote(remote, Duration::from_secs(5)).unwrap();
    let context = HostContext::default();
    context.roots.set(vec![
        Root::from_path(std::path::Path::new("/srv/data"), None).unwrap(),
    ]);
    let setup = ClientSetup {
        policy: ServerPolicy::default(),
        apps_enabled: false,
        host: Some(context),
    };
    let mut client = McpClient::from_transport("http-mock", transport, &setup)
        .await
        .unwrap();
    client.list_tools().await.unwrap();

    let result = client.call_tool("where", json!({})).await.unwrap();
    let ToolContent::Text { text } = &result.content[0] else {
        panic!("expected text");
    };
    assert_eq!(text, r#"[{"uri":"file:///srv/data/"}]"#);

    let headers = shared.lock().await.headers.clone();
    let initialize = &headers[0];
    assert!(
        !initialize
            .to_ascii_lowercase()
            .contains("mcp-protocol-version"),
        "no version header before negotiation"
    );
    assert!(
        headers.iter().skip(1).any(|h| h
            .to_ascii_lowercase()
            .contains("mcp-protocol-version: 2025-06-18")),
        "negotiated version is sent afterwards"
    );
    assert!(
        headers
            .iter()
            .skip(1)
            .any(|h| h.to_ascii_lowercase().contains("mcp-session-id: s-1"))
    );
}
