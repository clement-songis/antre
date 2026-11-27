//! End-to-end tests against an in-process mock MCP server speaking
//! newline-delimited JSON-RPC, the stdio wire format.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use {
    async_trait::async_trait,
    moltis_mcp::{
        ClientSetup, ElicitationRequest, HostContext, McpClient, McpClientTrait, McpHost,
        SamplingRequest, SamplingResponse, ServerPolicy, TrustTier,
        protocol::{CreateMessageResult, ElicitResult, Root, ServerNotification, UiResource},
        sampling::{CallContext, SamplingConfig},
        transport::StdioTransport,
        types::ToolContent,
    },
    serde_json::{Value, json},
    tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
        sync::{Mutex as AsyncMutex, mpsc},
    },
};

/// Mock server state shared with the test.
#[derive(Default)]
struct ServerLog {
    received: Vec<Value>,
}

type Writer = Arc<AsyncMutex<tokio::io::WriteHalf<DuplexStream>>>;

async fn send(writer: &Writer, message: Value) {
    let mut line = message.to_string();
    line.push('\n');
    let mut writer = writer.lock().await;
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Run the mock server on `stream`. Server-to-client requests use ids
/// starting at 1000; their responses are routed through `responses`.
fn spawn_server(stream: DuplexStream, log: Arc<Mutex<ServerLog>>) {
    let (read, write) = tokio::io::split(stream);
    let writer: Writer = Arc::new(AsyncMutex::new(write));
    let pending: Arc<AsyncMutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>> =
        Arc::default();
    tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        let mut next_id = 1000i64;
        while let Ok(Some(line)) = lines.next_line().await {
            let message: Value = serde_json::from_str(&line).unwrap();
            log.lock().unwrap().received.push(message.clone());
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                // A response to one of our requests.
                let id = message["id"].as_i64().unwrap();
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(message);
                }
                continue;
            };
            let Some(id) = message.get("id").cloned() else {
                continue;
            };
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let writer = Arc::clone(&writer);
            let response = match method {
                "initialize" => reply(
                    &id,
                    json!({
                        "protocolVersion": "2025-06-18",
                        "capabilities": {
                            "tools": {"listChanged": true},
                            "resources": {"subscribe": true, "listChanged": true},
                            "prompts": {"listChanged": true}
                        },
                        "serverInfo": {"name": "mock", "version": "1"}
                    }),
                ),
                "tools/list" => {
                    if params.get("cursor").is_some() {
                        reply(
                            &id,
                            json!({"tools": [
                                {"name": "ask", "inputSchema": {"type": "object"}},
                                {"name": "widget", "inputSchema": {"type": "object"},
                                 "_meta": {"ui": {"resourceUri": "ui://widget", "visibility": ["app"]}}}
                            ]}),
                        )
                    } else {
                        reply(
                            &id,
                            json!({"tools": [{"name": "echo", "inputSchema": {"type": "object"}}], "nextCursor": "p2"}),
                        )
                    }
                },
                "resources/list" => {
                    if params.get("cursor").is_some() {
                        reply(
                            &id,
                            json!({"resources": [{"uri": "ui://widget", "name": "widget", "mimeType": "text/html;profile=mcp-app"}]}),
                        )
                    } else {
                        reply(
                            &id,
                            json!({"resources": [{"uri": "file:///readme", "name": "readme", "mimeType": "text/plain"}], "nextCursor": "r2"}),
                        )
                    }
                },
                "resources/templates/list" => reply(
                    &id,
                    json!({"resourceTemplates": [{"uriTemplate": "file:///{path}", "name": "files"}]}),
                ),
                "resources/read" => {
                    let uri = params["uri"].as_str().unwrap().to_string();
                    let contents = if uri == "ui://widget" {
                        json!([{"uri": uri, "mimeType": "text/html;profile=mcp-app", "text": "<div>widget</div>"}])
                    } else {
                        json!([{"uri": uri, "mimeType": "text/plain", "text": "hello"}])
                    };
                    reply(&id, json!({"contents": contents}))
                },
                "resources/subscribe" => {
                    let uri = params["uri"].clone();
                    let notify = Arc::clone(&writer);
                    tokio::spawn(async move {
                        send(&notify, json!({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}})).await;
                    });
                    reply(&id, json!({}))
                },
                "resources/unsubscribe" => reply(&id, json!({})),
                "prompts/list" => reply(
                    &id,
                    json!({"prompts": [{"name": "review", "description": "Review code", "arguments": [{"name": "code", "required": true}]}]}),
                ),
                "prompts/get" => reply(
                    &id,
                    json!({
                        "description": "Review",
                        "messages": [{"role": "user", "content": {"type": "text", "text": format!("Review: {}", params["arguments"]["code"].as_str().unwrap_or(""))}}]
                    }),
                ),
                "tools/call" => {
                    // Ask the client three things, then answer with what came back.
                    let mut answers = Vec::new();
                    for (method, params) in [
                        (
                            "elicitation/create",
                            json!({"message": "Project?", "requestedSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}}),
                        ),
                        (
                            "sampling/createMessage",
                            json!({"messages": [{"role": "user", "content": {"type": "text", "text": "Summarize"}}], "maxTokens": 9999}),
                        ),
                        ("roots/list", json!({})),
                    ] {
                        let request_id = next_id;
                        next_id += 1;
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        pending.lock().await.insert(request_id, tx);
                        send(&writer, json!({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})).await;
                        let pending_wait = rx;
                        // Answered by the reader loop above, which keeps running
                        // because this branch runs in its own task below.
                        answers.push((method, pending_wait));
                    }
                    let writer = Arc::clone(&writer);
                    tokio::spawn(async move {
                        let mut texts = Vec::new();
                        for (method, rx) in answers {
                            let answer = rx.await.unwrap();
                            texts.push(json!({"method": method, "answer": answer}).to_string());
                        }
                        send(&writer, reply(&id, json!({"content": [{"type": "text", "text": serde_json::to_string(&texts).unwrap()}], "isError": false}))).await;
                    });
                    continue;
                },
                other => {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": other}})
                },
            };
            send(&writer, response).await;
        }
    });
}

#[derive(Default)]
struct TestHost {
    elicitations: Mutex<Vec<ElicitationRequest>>,
    sampling: Mutex<Vec<SamplingRequest>>,
    notifications: Mutex<Vec<ServerNotification>>,
    notified: Mutex<Option<mpsc::UnboundedSender<ServerNotification>>>,
}

#[async_trait]
impl McpHost for TestHost {
    async fn create_message(
        &self,
        request: SamplingRequest,
    ) -> moltis_mcp::Result<SamplingResponse> {
        let max = request.params.max_tokens;
        self.sampling.lock().unwrap().push(request);
        Ok(SamplingResponse {
            result: CreateMessageResult::text("mock-model", format!("summary (max {max})")),
            tokens_used: 30,
        })
    }

    async fn elicit(&self, request: ElicitationRequest) -> moltis_mcp::Result<ElicitResult> {
        self.elicitations.lock().unwrap().push(request);
        // Answer after the request timeout would have fired.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut content = serde_json::Map::new();
        content.insert("project".into(), json!("antre"));
        Ok(ElicitResult::accept(content))
    }

    async fn on_notification(&self, _server: &str, notification: ServerNotification) {
        self.notifications
            .lock()
            .unwrap()
            .push(notification.clone());
        if let Some(tx) = self.notified.lock().unwrap().as_ref() {
            let _ = tx.send(notification);
        }
    }
}

async fn connect(
    trust: TrustTier,
    sampling: bool,
) -> (McpClient, Arc<TestHost>, HostContext, Arc<Mutex<ServerLog>>) {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let log = Arc::new(Mutex::new(ServerLog::default()));
    spawn_server(server_side, Arc::clone(&log));
    let (read, write) = tokio::io::split(client_side);
    let transport = StdioTransport::from_streams(read, write, Duration::from_millis(200)).await;

    let host = Arc::new(TestHost::default());
    let context = HostContext::default();
    context.host.set(host.clone());
    context.roots.set(vec![
        Root::from_path(
            std::path::Path::new("/work/project"),
            Some("project".into()),
        )
        .unwrap(),
    ]);
    let setup = ClientSetup {
        policy: ServerPolicy {
            trust,
            sampling: SamplingConfig {
                enabled: sampling,
                max_tokens_per_request: 256,
                ..SamplingConfig::default()
            },
        },
        apps_enabled: true,
        host: Some(context.clone()),
    };
    let client = McpClient::from_transport("mock", transport, &setup)
        .await
        .unwrap();
    (client, host, context, log)
}

#[tokio::test]
async fn handshake_advertises_capabilities_and_lists_paginated_tools() {
    let (mut client, _host, _context, log) = connect(TrustTier::Trusted, true).await;
    let init = log.lock().unwrap().received[0].clone();
    assert_eq!(init["params"]["protocolVersion"], "2025-11-25");
    let caps = &init["params"]["capabilities"];
    assert_eq!(caps["roots"]["listChanged"], true);
    assert!(caps["sampling"].is_object());
    assert!(caps["elicitation"].is_object());
    assert!(caps["extensions"]["io.modelcontextprotocol/ui"].is_object());

    let names: Vec<String> = client
        .list_tools()
        .await
        .unwrap()
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert_eq!(names, vec!["ask", "echo", "widget"]);
    assert!(!client.tools()[2].is_model_visible());
    assert!(
        client
            .server_capabilities()
            .unwrap()
            .resources
            .as_ref()
            .unwrap()
            .subscribe
    );
}

#[tokio::test]
async fn resources_templates_read_and_subscribe() {
    let (client, host, _context, _log) = connect(TrustTier::Trusted, false).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    *host.notified.lock().unwrap() = Some(tx);

    let resources = McpClientTrait::list_resources(&client).await.unwrap();
    assert_eq!(resources.len(), 2, "both pages are fetched");
    let templates = McpClientTrait::list_resource_templates(&client)
        .await
        .unwrap();
    assert_eq!(templates[0].uri_template, "file:///{path}");

    let read = McpClientTrait::read_resource(&client, "file:///readme")
        .await
        .unwrap();
    assert_eq!(read.contents[0].text.as_deref(), Some("hello"));

    let ui = McpClientTrait::read_resource(&client, "ui://widget")
        .await
        .unwrap();
    let ui = UiResource::from_read("mock", "ui://widget", ui).unwrap();
    assert_eq!(ui.html, "<div>widget</div>");

    McpClientTrait::subscribe_resource(&client, "file:///readme")
        .await
        .unwrap();
    let notification = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notification, ServerNotification::ResourceUpdated {
        uri: "file:///readme".into()
    });
    McpClientTrait::unsubscribe_resource(&client, "file:///readme")
        .await
        .unwrap();
}

#[tokio::test]
async fn prompts_list_and_get() {
    let (client, _host, _context, _log) = connect(TrustTier::Trusted, false).await;
    let prompts = McpClientTrait::list_prompts(&client).await.unwrap();
    assert_eq!(prompts[0].name, "review");
    let rendered = McpClientTrait::get_prompt(
        &client,
        "review",
        HashMap::from([("code".to_string(), "fn main() {}".to_string())]),
    )
    .await
    .unwrap();
    match &rendered.messages[0].content {
        ToolContent::Text { text } => assert_eq!(text, "Review: fn main() {}"),
        other => panic!("unexpected content {other:?}"),
    }
}

#[tokio::test]
async fn tool_call_answers_elicitation_sampling_and_roots_without_timing_out() {
    let (client, host, context, log) = connect(TrustTier::Trusted, true).await;
    let _call = context.contexts.enter("mock", CallContext {
        session_key: Some("session:42".into()),
    });

    // The elicitation answer takes 400 ms while the request timeout is 200 ms.
    let result = client.call_tool("ask", json!({})).await.unwrap();
    let ToolContent::Text { text } = &result.content[0] else {
        panic!("expected text");
    };
    let answers: Vec<String> = serde_json::from_str(text).unwrap();
    let answers: Vec<Value> = answers
        .iter()
        .map(|a| serde_json::from_str(a).unwrap())
        .collect();

    assert_eq!(
        answers[0]["answer"]["result"],
        json!({"action": "accept", "content": {"project": "antre"}})
    );
    assert_eq!(
        answers[1]["answer"]["result"]["content"]["text"],
        "summary (max 256)"
    );
    assert_eq!(
        answers[2]["answer"]["result"]["roots"][0]["uri"],
        "file:///work/project/"
    );

    assert_eq!(
        host.elicitations.lock().unwrap()[0].session_key.as_deref(),
        Some("session:42")
    );
    assert_eq!(
        host.sampling.lock().unwrap()[0].session_key.as_deref(),
        Some("session:42")
    );
    assert_eq!(context.budget.usage("mock", Some("session:42")).tokens, 30);

    // Responses to server requests were written back on the same stream.
    let received = log.lock().unwrap().received.clone();
    assert!(
        received
            .iter()
            .any(|message| message["id"] == 1000 && message.get("result").is_some())
    );
}

#[tokio::test]
async fn untrusted_server_gets_no_sampling_roots_or_elicitation() {
    let (client, host, _context, log) = connect(TrustTier::Untrusted, true).await;
    let init = log.lock().unwrap().received[0].clone();
    assert!(init["params"]["capabilities"].get("sampling").is_none());
    assert!(init["params"]["capabilities"].get("elicitation").is_none());

    let result = client.call_tool("ask", json!({})).await.unwrap();
    let ToolContent::Text { text } = &result.content[0] else {
        panic!("expected text");
    };
    let answers: Vec<String> = serde_json::from_str(text).unwrap();
    let answers: Vec<Value> = answers
        .iter()
        .map(|a| serde_json::from_str(a).unwrap())
        .collect();
    assert_eq!(answers[0]["answer"]["result"]["action"], "decline");
    assert_eq!(answers[1]["answer"]["error"]["code"], -1);
    assert_eq!(answers[2]["answer"]["result"]["roots"], json!([]));
    assert!(host.elicitations.lock().unwrap().is_empty());
    assert!(host.sampling.lock().unwrap().is_empty());
}

#[tokio::test]
async fn roots_change_notifies_server() {
    let (client, _host, _context, log) = connect(TrustTier::Trusted, false).await;
    McpClientTrait::notify_roots_changed(&client).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let seen = log
                .lock()
                .unwrap()
                .received
                .iter()
                .any(|message| message["method"] == "notifications/roots/list_changed");
            if seen {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
