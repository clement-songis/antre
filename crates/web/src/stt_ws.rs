//! Live transcription WebSocket (`/api/stt/ws`).
//!
//! Bridges the browser socket to [`antre_gateway::voice::stt_stream`], which
//! relays audio to the configured streaming STT provider.

use std::{net::SocketAddr, sync::LazyLock};

use {
    antre_gateway::voice::stt_stream::{
        MAX_STREAMS_PER_IP, SttStreamFrame, SttStreamLimits, SttStreamSlot, SttStreamSlots,
        configured_limits, resolve_live_streaming_provider, run_stt_stream,
    },
    antre_httpd::AppState,
    axum::{
        Json,
        extract::{
            ConnectInfo, State, WebSocketUpgrade,
            ws::{Message, WebSocket},
        },
        http::{HeaderMap, StatusCode, header},
        response::{IntoResponse, Response},
    },
    futures::{SinkExt, StreamExt},
    std::sync::Arc,
    tokio::sync::mpsc,
    tracing::warn,
};

use crate::terminal::auth::{is_local_connection, is_same_origin, websocket_header_authenticated};

static STREAM_SLOTS: LazyLock<Arc<SttStreamSlots>> =
    LazyLock::new(|| SttStreamSlots::new(MAX_STREAMS_PER_IP));

fn reject(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "code": code, "error": message })),
    )
        .into_response()
}

/// Upgrade to the live transcription socket after the same-origin and auth
/// checks used by the terminal socket.
pub async fn api_stt_ws_upgrade_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !is_same_origin(origin, host) {
            warn!(origin, host, remote = %addr, "rejected cross-origin STT WebSocket upgrade");
            return reject(
                StatusCode::FORBIDDEN,
                "CROSS_ORIGIN",
                "cross-origin WebSocket connections are not allowed",
            );
        }
    }

    let is_local = is_local_connection(
        &headers,
        addr,
        state.gateway.behind_proxy,
        state.gateway.trust_docker_loopback,
    );
    if !websocket_header_authenticated(&headers, state.gateway.credential_store.as_ref(), is_local)
        .await
    {
        return reject(
            StatusCode::UNAUTHORIZED,
            "AUTH_NOT_AUTHENTICATED",
            "not authenticated",
        );
    }

    let Some(slot) = STREAM_SLOTS.try_acquire(addr.ip()) else {
        return reject(
            StatusCode::TOO_MANY_REQUESTS,
            "STT_STREAM_LIMIT",
            "too many live transcription streams",
        );
    };
    let limits = configured_limits();
    ws.max_message_size(256 * 1024)
        .on_upgrade(move |socket| handle_stt_socket(socket, slot, limits))
}

async fn handle_stt_socket(socket: WebSocket, slot: SttStreamSlot, limits: SttStreamLimits) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (frame_tx, frame_rx) = mpsc::channel(32);
    let (message_tx, mut message_rx) = mpsc::channel(32);

    let reader = tokio::spawn(async move {
        while let Some(Ok(message)) = ws_rx.next().await {
            let frame = match message {
                Message::Text(text) => SttStreamFrame::Text(text.to_string()),
                Message::Binary(audio) => SttStreamFrame::Binary(audio),
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => continue,
            };
            if frame_tx.send(frame).await.is_err() {
                break;
            }
        }
    });

    let writer = async move {
        while let Some(message) = message_rx.recv().await {
            let Ok(text) = serde_json::to_string(&message) else {
                continue;
            };
            if ws_tx.send(Message::Text(text.into())).await.is_err() {
                return;
            }
        }
        let _ = ws_tx.close().await;
    };

    tokio::join!(
        run_stt_stream(
            frame_rx,
            message_tx,
            resolve_live_streaming_provider,
            limits
        ),
        writer
    );
    reader.abort();
    drop(slot);
}
