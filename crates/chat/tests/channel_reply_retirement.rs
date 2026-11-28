#![allow(clippy::unwrap_used, clippy::expect_used)]

use {
    antre_agents::model::{
        ChatMessage, CompletionResponse, LlmProvider, StreamEvent, Usage, UserContent,
    },
    antre_channels::{ChannelOutbound, ChannelReplyTarget, ChannelType},
    antre_chat::{ChatRuntime, DisabledModelsStore, LiveChatService},
    antre_providers::{ModelCapabilities, ModelInfo, ProviderRegistry},
    antre_service_traits::{ChatService, NoopMcpService, NoopProjectService, NoopTtsService},
    antre_sessions::{metadata::SqliteSessionMetadata, store::SessionStore},
    async_trait::async_trait,
    serde_json::{Value, json},
    std::{
        collections::HashMap,
        pin::Pin,
        sync::{Arc, OnceLock, Weak},
        time::Duration,
    },
    tokio::sync::{Mutex, MutexGuard, Notify, RwLock, Semaphore},
    tokio_stream::Stream,
};

const SESSION: &str = "reply-target-regression";
// This integration-test binary owns the process-wide config/data overrides.
static TEST_DIRECTORY: Mutex<Option<tempfile::TempDir>> = Mutex::const_new(None);

#[derive(Default)]
struct RecordingOutbound {
    replies: RwLock<Vec<(String, Option<String>)>>,
    changed: Notify,
}

#[async_trait]
impl ChannelOutbound for RecordingOutbound {
    async fn send_text(
        &self,
        _account_id: &str,
        _to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<()> {
        self.replies
            .write()
            .await
            .push((text.to_string(), reply_to.map(str::to_string)));
        self.changed.notify_one();
        Ok(())
    }

    async fn send_media(
        &self,
        _account: &str,
        _to: &str,
        _payload: &antre_common::types::ReplyPayload,
        _reply_to: Option<&str>,
    ) -> antre_channels::Result<()> {
        panic!("text-only regression tests must not send media")
    }
}

struct CleanupGate {
    entered: Semaphore,
    release: Semaphore,
}

struct TestRuntime {
    targets: RwLock<HashMap<String, Vec<ChannelReplyTarget>>>,
    outbound: Arc<RecordingOutbound>,
    tts: NoopTtsService,
    project: NoopProjectService,
    mcp: NoopMcpService,
    chat: OnceLock<Weak<LiveChatService>>,
    cleanup_gate: RwLock<Option<Arc<CleanupGate>>>,
}

impl Default for TestRuntime {
    fn default() -> Self {
        Self {
            targets: RwLock::new(HashMap::new()),
            outbound: Arc::default(),
            tts: NoopTtsService,
            project: NoopProjectService,
            mcp: NoopMcpService,
            chat: OnceLock::new(),
            cleanup_gate: RwLock::new(None),
        }
    }
}

#[async_trait]
impl ChatRuntime for TestRuntime {
    async fn broadcast(&self, _topic: &str, _payload: Value) {}

    async fn push_channel_reply(&self, session: &str, target: ChannelReplyTarget) {
        self.targets
            .write()
            .await
            .entry(session.into())
            .or_default()
            .push(target);
    }

    async fn drain_channel_replies(&self, session: &str) -> Vec<ChannelReplyTarget> {
        let gate = self.cleanup_gate.write().await.take();
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
        self.targets
            .write()
            .await
            .remove(session)
            .unwrap_or_default()
    }

    async fn peek_channel_replies(&self, session: &str) -> Vec<ChannelReplyTarget> {
        self.targets
            .read()
            .await
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    async fn push_channel_status_log(&self, _session: &str, _message: String) {}

    async fn drain_channel_status_log(&self, _session: &str) -> Vec<String> {
        Vec::new()
    }

    async fn set_run_error(&self, _run_id: &str, _error: String) {}

    async fn active_session_key(&self, _conn_id: &str) -> Option<String> {
        None
    }

    async fn active_project_id(&self, _conn_id: &str) -> Option<String> {
        None
    }

    fn hostname(&self) -> &str {
        "reply-target-test"
    }

    fn sandbox_router(&self) -> Option<&Arc<antre_tools::sandbox::SandboxRouter>> {
        None
    }

    fn memory_manager(&self) -> Option<&antre_memory::runtime::DynMemoryRuntime> {
        None
    }

    async fn cached_location(&self) -> Option<antre_config::GeoLocation> {
        None
    }

    async fn tts_overrides(
        &self,
        _session: &str,
        _channel: &str,
    ) -> (
        Option<antre_chat::TtsOverride>,
        Option<antre_chat::TtsOverride>,
    ) {
        (None, None)
    }

    fn channel_outbound(&self) -> Option<Arc<dyn ChannelOutbound>> {
        Some(self.outbound.clone())
    }

    fn channel_stream_outbound(&self) -> Option<Arc<dyn antre_channels::ChannelStreamOutbound>> {
        None
    }

    fn tts_service(&self) -> &dyn antre_service_traits::TtsService {
        &self.tts
    }

    fn project_service(&self) -> &dyn antre_service_traits::ProjectService {
        &self.project
    }

    fn mcp_service(&self) -> &dyn antre_service_traits::McpService {
        &self.mcp
    }

    async fn chat_service(&self) -> Arc<dyn ChatService> {
        self.chat
            .get()
            .and_then(Weak::upgrade)
            .expect("fixture chat service is alive")
    }

    async fn last_run_error(&self, _run_id: &str) -> Option<String> {
        None
    }

    async fn send_push_notification(
        &self,
        _title: &str,
        _body: &str,
        _url: Option<&str>,
        _session: Option<&str>,
    ) -> antre_chat::error::Result<usize> {
        Ok(0)
    }

    async fn ensure_local_model_cached(&self, _model: &str) -> antre_chat::error::Result<bool> {
        Ok(false)
    }

    async fn connected_nodes(&self) -> Vec<antre_chat::runtime::ConnectedNodeSummary> {
        Vec::new()
    }
}

struct TestProvider {
    started: Arc<Semaphore>,
}

#[async_trait]
impl LlmProvider for TestProvider {
    fn name(&self) -> &str {
        "test"
    }

    fn id(&self) -> &str {
        "reply-target"
    }

    async fn complete(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
    ) -> anyhow::Result<CompletionResponse> {
        anyhow::bail!("reply-target tests must use streaming")
    }

    fn stream(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        let Some(ChatMessage::User {
            content: UserContent::Text(marker),
            ..
        }) = messages.last()
        else {
            panic!("regression request must end with a text user message")
        };
        if marker.contains("HOLD") {
            let started = self.started.clone();
            return Box::pin(async_stream::stream! {
                started.add_permits(1);
                std::future::pending::<()>().await;
                yield StreamEvent::Done(Usage::default());
            });
        }
        if marker.contains("EOF") {
            return Box::pin(tokio_stream::empty());
        }
        let usage = Usage {
            output_tokens: 1,
            ..Default::default()
        };
        let events = if marker.contains("SILENT") {
            vec![StreamEvent::Done(usage)]
        } else {
            vec![
                StreamEvent::Delta("VISIBLE-ANSWER".into()),
                StreamEvent::Done(usage),
            ]
        };
        Box::pin(tokio_stream::iter(events))
    }
}

struct Fixture {
    directory: MutexGuard<'static, Option<tempfile::TempDir>>,
    service: Arc<LiveChatService>,
    runtime: Arc<TestRuntime>,
    started: Arc<Semaphore>,
    metadata: Arc<SqliteSessionMetadata>,
}

impl Fixture {
    async fn new() -> Self {
        let mut directory = TEST_DIRECTORY.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let session_path = temporary.path().join("sessions");
        antre_config::set_config_dir(temporary.path().join("config"));
        antre_config::set_data_dir(temporary.path().join("data"));
        *directory = Some(temporary);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Session rows reference projects; the stand-in table lets them exist.
        sqlx::query("CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        SqliteSessionMetadata::init(&pool).await.unwrap();
        let started = Arc::new(Semaphore::new(0));
        let mut registry = ProviderRegistry::empty();
        registry.register(
            ModelInfo {
                id: "reply-target".into(),
                provider: "test".into(),
                display_name: "Reply target test".into(),
                created_at: None,
                recommended: false,
                capabilities: ModelCapabilities::infer("reply-target"),
            },
            Arc::new(TestProvider {
                started: started.clone(),
            }),
        );
        let runtime = Arc::new(TestRuntime::default());
        let metadata = Arc::new(SqliteSessionMetadata::new(pool));
        let service = Arc::new(
            LiveChatService::new(
                Arc::new(RwLock::new(registry)),
                Arc::new(RwLock::new(DisabledModelsStore::default())),
                runtime.clone(),
                Arc::new(SessionStore::new(session_path)),
                Arc::clone(&metadata),
            )
            .with_config(antre_config::AntreConfig::default()),
        );
        assert!(runtime.chat.set(Arc::downgrade(&service)).is_ok());
        Self {
            directory,
            service,
            runtime,
            started,
            metadata,
        }
    }

    fn params(marker: &str, anchor: &str) -> Value {
        json!({"sessionKey": SESSION, "model": "test::reply-target", "message": marker,
        "stream_only": true, "_channel_reply_target": ChannelReplyTarget {
            channel_type: ChannelType::Slack, account_id: "test".into(), chat_id: "room".into(),
            message_id: Some(anchor.into()), thread_id: None, ack_message_id: None, direct_chat: false,
        }})
    }

    async fn assert_only_next_answer(&self) {
        self.service
            .send_sync(Self::params("VISIBLE", "new-anchor"))
            .await
            .unwrap();
        assert_eq!(*self.runtime.outbound.replies.read().await, vec![(
            "VISIBLE-ANSWER".to_string(),
            Some("new-anchor".to_string())
        )]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        antre_config::clear_config_dir();
        antre_config::clear_data_dir();
        self.directory.take();
    }
}

#[tokio::test]
async fn silent_completion_retires_channel_reply_target() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .send_sync(Fixture::params("SILENT", "old-anchor"))
        .await
        .unwrap();
    assert!(
        fixture
            .runtime
            .peek_channel_replies(SESSION)
            .await
            .is_empty()
    );
    assert!(fixture.runtime.outbound.replies.read().await.is_empty());
    fixture.assert_only_next_answer().await;
}

#[tokio::test]
async fn eof_completion_retires_channel_reply_target() {
    let fixture = Fixture::new().await;
    assert!(
        fixture
            .service
            .send_sync(Fixture::params("EOF", "old-anchor"))
            .await
            .is_err()
    );
    assert!(
        fixture
            .runtime
            .peek_channel_replies(SESSION)
            .await
            .is_empty()
    );
    fixture.assert_only_next_answer().await;
}

#[tokio::test]
async fn visible_completion_preserves_each_channel_reply() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .send_sync(Fixture::params("VISIBLE", "old-anchor"))
        .await
        .unwrap();
    fixture
        .service
        .send_sync(Fixture::params("VISIBLE", "new-anchor"))
        .await
        .unwrap();
    assert_eq!(*fixture.runtime.outbound.replies.read().await, vec![
        ("VISIBLE-ANSWER".to_string(), Some("old-anchor".to_string())),
        ("VISIBLE-ANSWER".to_string(), Some("new-anchor".to_string())),
    ]);
}

#[tokio::test]
async fn abort_cleanup_preserves_prequeued_and_new_arrival_targets() {
    let fixture = Fixture::new().await;
    let head = fixture
        .service
        .send(Fixture::params("HOLD", "old-anchor"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let queued = fixture
        .service
        .send(Fixture::params("VISIBLE", "queued-anchor"))
        .await
        .unwrap();
    assert_eq!(queued["queued"], true);
    let gate = Arc::new(CleanupGate {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    *fixture.runtime.cleanup_gate.write().await = Some(gate.clone());
    let service = fixture.service.clone();
    let abort = tokio::spawn(async move {
        service
            .abort(json!({"runId": head["runId"], "sessionKey": SESSION}))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let arrival = fixture
        .service
        .send(Fixture::params("VISIBLE", "arrival-anchor"))
        .await
        .unwrap();
    assert_eq!(arrival["queued"], true);
    let pending = fixture.runtime.peek_channel_replies(SESSION).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id.as_deref(), Some("old-anchor"));
    gate.release.add_permits(1);
    assert_eq!(abort.await.unwrap().unwrap()["aborted"], true);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let changed = fixture.runtime.outbound.changed.notified();
            if fixture.runtime.outbound.replies.read().await.len() >= 2 {
                break;
            }
            changed.await;
        }
        while fixture
            .service
            .peek(json!({"sessionKey": SESSION}))
            .await
            .unwrap()["active"]
            == true
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*fixture.runtime.outbound.replies.read().await, vec![
        (
            "VISIBLE-ANSWER".to_string(),
            Some("queued-anchor".to_string())
        ),
        (
            "VISIBLE-ANSWER".to_string(),
            Some("arrival-anchor".to_string())
        ),
    ]);
    assert!(
        fixture
            .runtime
            .peek_channel_replies(SESSION)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn abort_retires_channel_reply_target() {
    let fixture = Fixture::new().await;
    let started = fixture
        .service
        .send(Fixture::params("HOLD", "old-anchor"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let aborted = fixture
        .service
        .abort(json!({"runId": started["runId"], "sessionKey": SESSION}))
        .await
        .unwrap();
    assert_eq!(aborted["aborted"], true);
    assert!(
        fixture
            .runtime
            .peek_channel_replies(SESSION)
            .await
            .is_empty()
    );
    fixture.assert_only_next_answer().await;
}

/// `send_sync` on a busy session (cron, webhooks, `sessions_send` with a
/// wait) used to fail with "session already has an active turn". It now
/// queues behind the active turn and returns its own answer once that turn
/// ends, without the two runs overlapping.
#[tokio::test]
async fn send_sync_waits_for_the_active_turn_instead_of_failing() {
    let fixture = Fixture::new().await;
    let held = fixture
        .service
        .send(Fixture::params("HOLD", "held-anchor"))
        .await
        .unwrap();
    assert!(held.get("runId").is_some());
    fixture.started.acquire().await.unwrap().forget();

    let waiting = {
        let service = Arc::clone(&fixture.service);
        tokio::spawn(async move {
            service
                .send_sync(Fixture::params("VISIBLE", "queued-anchor"))
                .await
        })
    };
    // Give the queued turn every chance to run early: it must not.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiting.is_finished(), "ran beside the active turn");
    assert!(fixture.runtime.outbound.replies.read().await.is_empty());

    fixture
        .service
        .abort(json!({ "sessionKey": SESSION }))
        .await
        .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("queued send_sync finished")
        .unwrap()
        .unwrap();
    assert_eq!(answer["text"], "VISIBLE-ANSWER");
    assert_eq!(*fixture.runtime.outbound.replies.read().await, vec![(
        "VISIBLE-ANSWER".to_string(),
        Some("queued-anchor".to_string())
    )]);
}

/// Cancelling the queue ends a waiting `send_sync` with an error rather
/// than leaving its caller hanging.
#[tokio::test]
async fn cancelling_the_queue_ends_a_waiting_send_sync() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .send(Fixture::params("HOLD", "held-anchor"))
        .await
        .unwrap();
    fixture.started.acquire().await.unwrap().forget();

    let waiting = {
        let service = Arc::clone(&fixture.service);
        tokio::spawn(async move {
            service
                .send_sync(Fixture::params("VISIBLE", "queued-anchor"))
                .await
        })
    };
    let mut cleared = 0;
    for _ in 0..200 {
        let result = fixture
            .service
            .cancel_queued(json!({ "sessionKey": SESSION }))
            .await
            .unwrap();
        cleared = result["cleared"].as_u64().unwrap_or_default();
        if cleared > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(cleared, 1);
    let error = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("cancelled send_sync finished")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");

    fixture
        .service
        .abort(json!({ "sessionKey": SESSION }))
        .await
        .unwrap();
    assert!(fixture.runtime.outbound.replies.read().await.is_empty());
}

async fn history_texts(fixture: &Fixture) -> Vec<String> {
    fixture
        .service
        .history(json!({ "sessionKey": SESSION }))
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str().map(String::from))
        .collect()
}

/// `chat.inject` waits for the active turn, so the injected message lands
/// after that turn's history rather than in the middle of it.
#[tokio::test]
async fn inject_waits_for_the_active_turn() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .send(Fixture::params("HOLD", "held-anchor"))
        .await
        .unwrap();
    fixture.started.acquire().await.unwrap().forget();

    let injecting = {
        let service = Arc::clone(&fixture.service);
        tokio::spawn(async move {
            service
                .inject(json!({ "sessionKey": SESSION, "text": "injected note" }))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!injecting.is_finished(), "injected during the active turn");

    fixture
        .service
        .abort(json!({ "sessionKey": SESSION }))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), injecting)
        .await
        .expect("inject finished")
        .unwrap()
        .unwrap();
    assert_eq!(
        history_texts(&fixture).await.last().map(String::as_str),
        Some("injected note")
    );
    assert!(
        fixture
            .service
            .inject(json!({ "sessionKey": SESSION }))
            .await
            .is_err(),
        "text is required"
    );
}

/// A background sub-agent's result reaches the parent session and its
/// channel once, even though the parent's turn already ended.
#[tokio::test]
async fn background_task_outcome_is_posted_once_to_the_parent_and_its_channel() {
    let fixture = Fixture::new().await;
    fixture.metadata.upsert(SESSION, None).await.unwrap();
    let binding = ChannelReplyTarget {
        channel_type: ChannelType::Slack,
        account_id: "test".into(),
        chat_id: "room".into(),
        message_id: None,
        thread_id: None,
        ack_message_id: None,
        direct_chat: false,
    };
    fixture
        .metadata
        .set_channel_binding(SESSION, Some(serde_json::to_string(&binding).unwrap()))
        .await;

    let store = antre_tools::spawn_agent_tasks::SpawnTaskStore::default();
    let (abort, _) = futures::future::AbortHandle::new_pair();
    let task = store
        .insert_running(
            "check the build".into(),
            Some(SESSION.into()),
            "m".into(),
            None,
            abort,
        )
        .await;
    store
        .complete(&task.id, antre_tools::spawn_agent_tasks::SpawnTaskUpdate {
            text: Some("build is green".into()),
            iterations: 1,
            tool_calls_made: 0,
            error: None,
        })
        .await;

    assert!(
        fixture
            .service
            .announce_background_task(SESSION, &task.id, &store)
            .await
            .unwrap()
    );
    assert!(
        !fixture
            .service
            .announce_background_task(SESSION, &task.id, &store)
            .await
            .unwrap(),
        "announced twice"
    );
    let expected = "**Background task finished:** check the build\n\nbuild is green";
    assert_eq!(
        history_texts(&fixture).await.last().map(String::as_str),
        Some(expected)
    );
    assert_eq!(*fixture.runtime.outbound.replies.read().await, vec![(
        expected.to_string(),
        None
    )]);
}
