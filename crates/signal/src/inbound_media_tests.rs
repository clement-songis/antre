//! End-to-end voice-note and attachment handling against a fake signal-cli HTTP daemon.

use std::sync::{Arc, Mutex, RwLock};

use {
    antre_channels::{
        ChannelAttachment, ChannelEvent, ChannelEventSink, ChannelMessageKind, ChannelMessageMeta,
        ChannelReplyTarget, Result, SavedChannelFile, gating::DmPolicy, otp::OtpState,
    },
    async_trait::async_trait,
    base64::{Engine as _, engine::general_purpose::STANDARD as BASE64},
    serde_json::{Value, json},
    tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    },
};

use crate::{
    client::SignalClient,
    config::SignalAccountConfig,
    inbound::handle_event,
    voice::{VOICE_REPLY_STT_SETUP_HINT, VOICE_REPLY_TRANSCRIPTION_FAILED},
};

const SENDER: &str = "+15550001111";
const AUDIO: &[u8] = b"fake-m4a-bytes";

/// Answers every JSON-RPC call and records `(method, params)`.
async fn spawn_fake_daemon() -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("bind fake daemon: {e}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("fake daemon addr: {e}"));
    let recorded = Arc::clone(&calls);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let Some(request) = read_json_body(&mut stream).await else {
                    return;
                };
                let method = request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let params = request.get("params").cloned().unwrap_or(Value::Null);
                recorded
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((method.clone(), params));
                let result = match method.as_str() {
                    "getAttachment" => json!({ "data": BASE64.encode(AUDIO) }),
                    _ => json!({ "timestamp": 1 }),
                };
                let body = json!({ "jsonrpc": "2.0", "id": request.get("id"), "result": result })
                    .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}"), calls)
}

async fn read_json_body(stream: &mut tokio::net::TcpStream) -> Option<Value> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&buffer);
        let Some(header_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let content_length = text[..header_end]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        if buffer.len() >= body_start + content_length {
            return serde_json::from_slice(&buffer[body_start..body_start + content_length]).ok();
        }
    }
}

#[derive(Default)]
struct RecordingSink {
    stt_available: bool,
    transcript: Option<&'static str>,
    dispatched: Mutex<Vec<(String, ChannelMessageMeta)>>,
    dispatched_images: Mutex<Vec<ChannelAttachment>>,
    saved_files: Mutex<Vec<String>>,
    saved_voice: Mutex<Vec<(Vec<u8>, String)>>,
    transcribed: Mutex<Vec<(Vec<u8>, String)>>,
}

#[async_trait]
impl ChannelEventSink for RecordingSink {
    async fn emit(&self, _event: ChannelEvent) {}

    async fn dispatch_to_chat(
        &self,
        text: &str,
        _reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        self.dispatched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((text.to_string(), meta));
    }

    async fn dispatch_to_chat_with_attachments(
        &self,
        text: &str,
        attachments: Vec<ChannelAttachment>,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        self.dispatched_images
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(attachments);
        self.dispatch_to_chat(text, reply_to, meta).await;
    }

    async fn dispatch_command(
        &self,
        _command: &str,
        _reply_to: ChannelReplyTarget,
        _sender_id: Option<&str>,
    ) -> Result<String> {
        Ok(String::new())
    }

    async fn request_disable_account(&self, _channel_type: &str, _account_id: &str, _reason: &str) {
    }

    async fn save_channel_attachment(
        &self,
        _file_data: &[u8],
        filename: &str,
        _reply_to: &ChannelReplyTarget,
    ) -> Option<SavedChannelFile> {
        self.saved_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(filename.to_string());
        Some(SavedChannelFile {
            filename: filename.to_string(),
            media_ref: format!("media/main/{filename}"),
            absolute_path: format!("/tmp/media/{filename}"),
        })
    }

    async fn save_channel_voice(
        &self,
        audio_data: &[u8],
        filename: &str,
        _reply_to: &ChannelReplyTarget,
    ) -> Option<String> {
        self.saved_voice
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((audio_data.to_vec(), filename.to_string()));
        Some(filename.to_string())
    }

    async fn transcribe_voice(&self, audio_data: &[u8], format: &str) -> Result<String> {
        self.transcribed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((audio_data.to_vec(), format.to_string()));
        self.transcript
            .map(ToString::to_string)
            .ok_or_else(|| antre_channels::Error::unavailable("stt down"))
    }

    async fn voice_stt_available(&self) -> bool {
        self.stt_available
    }
}

fn config(http_url: String) -> Arc<RwLock<SignalAccountConfig>> {
    Arc::new(RwLock::new(SignalAccountConfig {
        account: Some("+15559990000".to_string()),
        http_url,
        dm_policy: DmPolicy::Allowlist,
        allowlist: vec![SENDER.to_string()],
        ..Default::default()
    }))
}

fn voice_event(attachments: Value) -> Value {
    message_event(Value::Null, attachments)
}

fn message_event(message: Value, attachments: Value) -> Value {
    json!({
        "method": "receive",
        "params": {
            "envelope": {
                "sourceNumber": SENDER,
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": { "message": message, "attachments": attachments }
            }
        }
    })
}

async fn run(sink: Arc<RecordingSink>, attachments: Value) -> Vec<(String, Value)> {
    run_event(sink, voice_event(attachments)).await
}

async fn run_event(sink: Arc<RecordingSink>, event: Value) -> Vec<(String, Value)> {
    let (url, calls) = spawn_fake_daemon().await;
    let event_sink: Arc<dyn ChannelEventSink> = sink;
    let otp = Arc::new(Mutex::new(OtpState::new(300)));
    handle_event(
        &event,
        &SignalClient::new(),
        &config(url),
        &otp,
        "acct",
        &event_sink,
    )
    .await;
    calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn sent_messages(calls: &[(String, Value)]) -> Vec<String> {
    calls
        .iter()
        .filter(|(method, _)| method == "send")
        .filter_map(|(_, params)| params.get("message").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn voice_attachment() -> Value {
    json!([{ "contentType": "audio/aac", "id": "att-42", "size": AUDIO.len() }])
}

#[tokio::test]
async fn voice_note_is_downloaded_transcribed_and_dispatched() {
    let sink = Arc::new(RecordingSink {
        stt_available: true,
        transcript: Some("bonjour antre"),
        ..Default::default()
    });
    let calls = run(Arc::clone(&sink), voice_attachment()).await;

    let download = calls
        .iter()
        .find(|(method, _)| method == "getAttachment")
        .map(|(_, params)| params.clone());
    assert_eq!(
        download,
        Some(json!({ "id": "att-42", "recipient": SENDER, "account": "+15559990000" }))
    );
    assert!(sent_messages(&calls).is_empty());

    let transcribed = sink.transcribed.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(*transcribed, vec![(AUDIO.to_vec(), "aac".to_string())]);
    let saved = sink.saved_voice.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(*saved, vec![(
        AUDIO.to_vec(),
        "voice-signal-1700000000000.aac".to_string()
    )]);
    let dispatched = sink.dispatched.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].0, "bonjour antre");
    assert!(matches!(
        dispatched[0].1.message_kind,
        Some(ChannelMessageKind::Voice)
    ));
    assert_eq!(
        dispatched[0].1.audio_filename.as_deref(),
        Some("voice-signal-1700000000000.aac")
    );
}

#[tokio::test]
async fn voice_note_without_stt_replies_with_setup_hint() {
    let sink = Arc::new(RecordingSink::default());
    let calls = run(Arc::clone(&sink), voice_attachment()).await;

    assert!(calls.iter().all(|(method, _)| method != "getAttachment"));
    assert_eq!(sent_messages(&calls), vec![VOICE_REPLY_STT_SETUP_HINT]);
    assert!(
        sink.dispatched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    );
}

#[tokio::test]
async fn failed_transcription_replies_to_sender() {
    let sink = Arc::new(RecordingSink {
        stt_available: true,
        ..Default::default()
    });
    let calls = run(Arc::clone(&sink), voice_attachment()).await;

    assert_eq!(sent_messages(&calls), vec![
        VOICE_REPLY_TRANSCRIPTION_FAILED
    ]);
    assert!(
        sink.dispatched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    );
}

#[tokio::test]
async fn image_and_document_reach_the_agent_with_caption() {
    let sink = Arc::new(RecordingSink {
        stt_available: true,
        ..Default::default()
    });
    let attachments = json!([
        { "contentType": "image/png", "id": "img-1", "filename": "shot.png", "size": 10 },
        { "contentType": "text/plain", "id": "doc-1", "filename": "notes.txt", "size": 14 }
    ]);
    let calls = run_event(
        Arc::clone(&sink),
        message_event(json!("look at these"), attachments),
    )
    .await;

    let downloaded: Vec<_> = calls
        .iter()
        .filter(|(method, _)| method == "getAttachment")
        .filter_map(|(_, params)| params.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(downloaded.len(), 2);
    assert!(sent_messages(&calls).is_empty());

    let images = sink
        .dispatched_images
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].media_type, "image/png");
    let saved = sink.saved_files.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(*saved, vec!["doc-1_notes.txt".to_string()]);

    let dispatched = sink.dispatched.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(dispatched.len(), 1);
    assert_eq!(
        dispatched[0].0,
        "look at these\n\n[Document: notes.txt (text/plain)]\n\nfake-m4a-bytes"
    );
    let meta = &dispatched[0].1;
    assert!(matches!(meta.message_kind, Some(ChannelMessageKind::Photo)));
    let documents = meta.documents.as_deref().unwrap_or_default();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].stored_filename, "doc-1_notes.txt");
}

#[tokio::test]
async fn failed_voice_note_still_delivers_other_attachments() {
    let sink = Arc::new(RecordingSink::default());
    let attachments = json!([
        { "contentType": "audio/aac", "id": "att-42", "size": 10 },
        { "contentType": "application/zip", "id": "zip-1", "filename": "a.zip", "size": 10 }
    ]);
    let calls = run(Arc::clone(&sink), attachments).await;

    assert_eq!(sent_messages(&calls), vec![VOICE_REPLY_STT_SETUP_HINT]);
    let dispatched = sink.dispatched.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].0, "[Document: a.zip (application/zip)]");
    assert!(matches!(
        dispatched[0].1.message_kind,
        Some(ChannelMessageKind::Document)
    ));
}
