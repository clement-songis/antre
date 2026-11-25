//! End-to-end voice-note handling against a fake signal-cli HTTP daemon.

use std::sync::{Arc, Mutex, RwLock};

use {
    async_trait::async_trait,
    base64::{Engine as _, engine::general_purpose::STANDARD as BASE64},
    moltis_channels::{
        ChannelEvent, ChannelEventSink, ChannelMessageKind, ChannelMessageMeta, ChannelReplyTarget,
        Result, gating::DmPolicy, otp::OtpState,
    },
    serde_json::{Value, json},
    tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    },
};

use crate::{
    client::SignalClient,
    config::SignalAccountConfig,
    inbound::{ATTACHMENT_PLACEHOLDER, handle_event},
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
            .ok_or_else(|| moltis_channels::Error::unavailable("stt down"))
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
    json!({
        "method": "receive",
        "params": {
            "envelope": {
                "sourceNumber": SENDER,
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": { "message": null, "attachments": attachments }
            }
        }
    })
}

async fn run(sink: Arc<RecordingSink>, attachments: Value) -> Vec<(String, Value)> {
    let (url, calls) = spawn_fake_daemon().await;
    let event_sink: Arc<dyn ChannelEventSink> = sink;
    let otp = Arc::new(Mutex::new(OtpState::new(300)));
    handle_event(
        &voice_event(attachments),
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
        transcript: Some("bonjour moltis"),
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
    assert_eq!(dispatched[0].0, "bonjour moltis");
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
async fn non_audio_attachment_keeps_placeholder() {
    let sink = Arc::new(RecordingSink {
        stt_available: true,
        transcript: Some("unused"),
        ..Default::default()
    });
    let attachments = json!([{ "contentType": "image/jpeg", "id": "img-1", "size": 10 }]);
    let calls = run(Arc::clone(&sink), attachments).await;

    assert!(calls.iter().all(|(method, _)| method != "getAttachment"));
    let dispatched = sink.dispatched.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].0, ATTACHMENT_PLACEHOLDER);
    assert!(matches!(
        dispatched[0].1.message_kind,
        Some(ChannelMessageKind::Text)
    ));
}
