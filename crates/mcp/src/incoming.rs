//! Routing of server-to-client messages (requests and notifications) shared
//! by the transports.

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use {
    async_trait::async_trait,
    serde_json::Value,
    tokio::task::AbortHandle,
    tracing::{debug, trace},
};

use crate::{
    error::{Error, Result},
    types::{JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
};

/// Handles requests and notifications a server sends to the client.
#[async_trait]
pub trait IncomingDispatcher: Send + Sync {
    async fn on_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> std::result::Result<Value, JsonRpcError>;

    async fn on_notification(&self, method: &str, params: Option<Value>);
}

/// A JSON-RPC message received from a server.
#[derive(Debug)]
pub(crate) enum IncomingMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
}

impl IncomingMessage {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        let has_method = value.get("method").is_some_and(Value::is_string);
        let has_id = value.get("id").is_some_and(|id| !id.is_null());
        Ok(match (has_method, has_id) {
            (true, true) => Self::Request(serde_json::from_value(value)?),
            (true, false) => Self::Notification(serde_json::from_value(value)?),
            (false, true) => Self::Response(serde_json::from_value(value)?),
            (false, false) => {
                return Err(Error::message("JSON-RPC message has neither method nor id"));
            },
        })
    }
}

/// Dispatches incoming messages and tracks activity for request timeouts.
pub(crate) struct IncomingRouter {
    dispatcher: RwLock<Option<Arc<dyn IncomingDispatcher>>>,
    in_flight: Mutex<HashMap<String, AbortHandle>>,
    in_flight_count: AtomicUsize,
    last_activity: Mutex<Instant>,
}

impl Default for IncomingRouter {
    fn default() -> Self {
        Self {
            dispatcher: RwLock::new(None),
            in_flight: Mutex::new(HashMap::new()),
            in_flight_count: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
        }
    }
}

struct InFlightGuard {
    router: Arc<IncomingRouter>,
    key: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.router
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
        self.router.in_flight_count.fetch_sub(1, Ordering::AcqRel);
        self.router.touch();
    }
}

impl IncomingRouter {
    pub(crate) fn set_dispatcher(&self, dispatcher: Arc<dyn IncomingDispatcher>) {
        *self.dispatcher.write().unwrap_or_else(|e| e.into_inner()) = Some(dispatcher);
    }

    fn dispatcher(&self) -> Option<Arc<dyn IncomingDispatcher>> {
        self.dispatcher
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn touch(&self) {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    fn is_busy(&self) -> bool {
        self.in_flight_count.load(Ordering::Acquire) > 0
    }

    /// Handle a server request in its own task and pass the response to
    /// `respond`. `ping` is answered here; other methods need a dispatcher.
    pub(crate) fn route_request<F, Fut>(self: &Arc<Self>, request: JsonRpcRequest, respond: F)
    where
        F: FnOnce(JsonRpcResponse) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.touch();
        let id = request.id.clone();
        let key = id.to_string();
        debug!(method = %request.method, id = %key, "MCP server -> client request");

        if request.method == "ping" {
            tokio::spawn(respond(JsonRpcResponse::success(id, serde_json::json!({}))));
            return;
        }
        let Some(dispatcher) = self.dispatcher() else {
            let error = JsonRpcError::method_not_found(&request.method);
            tokio::spawn(respond(JsonRpcResponse::failure(id, error)));
            return;
        };

        self.in_flight_count.fetch_add(1, Ordering::AcqRel);
        let guard = InFlightGuard {
            router: Arc::clone(self),
            key: key.clone(),
        };
        let mut in_flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let task = tokio::spawn(async move {
            let response = match dispatcher.on_request(&request.method, request.params).await {
                Ok(result) => JsonRpcResponse::success(id, result),
                Err(error) => JsonRpcResponse::failure(id, error),
            };
            drop(guard);
            respond(response).await;
        });
        in_flight.insert(key, task.abort_handle());
    }

    /// Handle a server notification. `notifications/cancelled` aborts the
    /// matching in-flight request handler; no response is sent for it.
    pub(crate) fn route_notification(self: &Arc<Self>, notification: JsonRpcNotification) {
        self.touch();
        trace!(method = %notification.method, "MCP server -> client notification");
        if notification.method == "notifications/cancelled" {
            let request_id = notification
                .params
                .as_ref()
                .and_then(|params| params.get("requestId"))
                .map(Value::to_string);
            if let Some(key) = request_id {
                let handle = self
                    .in_flight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
                if let Some(handle) = handle {
                    debug!(id = %key, "MCP server cancelled its request");
                    handle.abort();
                }
            }
            return;
        }
        let Some(dispatcher) = self.dispatcher() else {
            return;
        };
        tokio::spawn(async move {
            dispatcher
                .on_notification(&notification.method, notification.params)
                .await;
        });
    }

    /// Await `future` for at most `timeout` of inactivity. Time spent while
    /// a server request (elicitation, sampling) is being answered, or since
    /// the last progress notification, does not count.
    pub(crate) async fn await_active<T>(
        &self,
        future: impl Future<Output = T>,
        timeout: Duration,
    ) -> Option<T> {
        tokio::pin!(future);
        let mut wait = timeout;
        loop {
            match tokio::time::timeout(wait, &mut future).await {
                Ok(value) => return Some(value),
                Err(_) if self.is_busy() => wait = timeout,
                Err(_) => {
                    let idle = self.idle_for();
                    if idle >= timeout {
                        return None;
                    }
                    wait = timeout - idle;
                },
            }
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, serde_json::json, tokio::sync::mpsc};

    struct EchoDispatcher {
        notifications: mpsc::UnboundedSender<String>,
        delay: Duration,
    }

    #[async_trait]
    impl IncomingDispatcher for EchoDispatcher {
        async fn on_request(
            &self,
            method: &str,
            params: Option<Value>,
        ) -> std::result::Result<Value, JsonRpcError> {
            tokio::time::sleep(self.delay).await;
            match method {
                "echo" => Ok(params.unwrap_or(Value::Null)),
                other => Err(JsonRpcError::method_not_found(other)),
            }
        }

        async fn on_notification(&self, method: &str, _params: Option<Value>) {
            let _ = self.notifications.send(method.to_string());
        }
    }

    fn request(id: u64, method: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(id, method, Some(json!({"x": id})))
    }

    fn router(delay: Duration) -> (Arc<IncomingRouter>, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let router = Arc::new(IncomingRouter::default());
        router.set_dispatcher(Arc::new(EchoDispatcher {
            notifications: tx,
            delay,
        }));
        (router, rx)
    }

    async fn respond_once(
        router: &Arc<IncomingRouter>,
        request: JsonRpcRequest,
    ) -> JsonRpcResponse {
        let (tx, mut rx) = mpsc::unbounded_channel();
        router.route_request(request, move |response| async move {
            let _ = tx.send(response);
        });
        rx.recv().await.unwrap()
    }

    #[test]
    fn parse_classifies_messages() {
        assert!(matches!(
            IncomingMessage::parse(r#"{"jsonrpc":"2.0","id":1,"method":"roots/list"}"#).unwrap(),
            IncomingMessage::Request(_)
        ));
        assert!(matches!(
            IncomingMessage::parse(r#"{"jsonrpc":"2.0","method":"notifications/x"}"#).unwrap(),
            IncomingMessage::Notification(_)
        ));
        assert!(matches!(
            IncomingMessage::parse(r#"{"jsonrpc":"2.0","id":"a","result":{}}"#).unwrap(),
            IncomingMessage::Response(_)
        ));
        assert!(IncomingMessage::parse(r#"{"jsonrpc":"2.0"}"#).is_err());
        assert!(IncomingMessage::parse("not json").is_err());
    }

    #[tokio::test]
    async fn requests_are_dispatched_and_answered() {
        let (router, _rx) = router(Duration::ZERO);
        let response = respond_once(&router, request(7, "echo")).await;
        assert_eq!(response.id, json!(7));
        assert_eq!(response.result, Some(json!({"x": 7})));

        let response = respond_once(&router, request(8, "nope")).await;
        assert_eq!(response.error.unwrap().code, JsonRpcError::METHOD_NOT_FOUND);

        let ping = respond_once(&router, request(9, "ping")).await;
        assert_eq!(ping.result, Some(json!({})));
    }

    #[tokio::test]
    async fn missing_dispatcher_answers_method_not_found() {
        let router = Arc::new(IncomingRouter::default());
        let response = respond_once(&router, request(1, "roots/list")).await;
        assert_eq!(response.error.unwrap().code, JsonRpcError::METHOD_NOT_FOUND);
        router.route_notification(JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "notifications/tools/list_changed".into(),
            params: None,
        });
    }

    #[tokio::test]
    async fn notifications_reach_dispatcher() {
        let (router, mut rx) = router(Duration::ZERO);
        router.route_notification(JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "notifications/resources/updated".into(),
            params: Some(json!({"uri": "x"})),
        });
        assert_eq!(rx.recv().await.unwrap(), "notifications/resources/updated");
    }

    #[tokio::test]
    async fn cancelled_notification_aborts_handler() {
        let (router, _rx) = router(Duration::from_secs(30));
        let (tx, mut responses) = mpsc::unbounded_channel();
        router.route_request(request(3, "echo"), move |response| async move {
            let _ = tx.send(response);
        });
        assert!(router.is_busy());
        router.route_notification(JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "notifications/cancelled".into(),
            params: Some(json!({"requestId": 3})),
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while router.is_busy() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(responses.try_recv().is_err());
    }

    #[tokio::test]
    async fn await_active_times_out_when_idle() {
        let router = IncomingRouter::default();
        let result = router
            .await_active(std::future::pending::<()>(), Duration::from_millis(30))
            .await;
        assert!(result.is_none());
        let ready = router
            .await_active(async { 5 }, Duration::from_millis(30))
            .await;
        assert_eq!(ready, Some(5));
    }

    #[tokio::test]
    async fn await_active_waits_while_server_request_in_flight() {
        let (router, _rx) = router(Duration::from_millis(120));
        let (tx, mut responses) = mpsc::unbounded_channel();
        router.route_request(request(1, "echo"), move |response| async move {
            let _ = tx.send(response);
        });
        // The handler takes 120 ms, longer than the 40 ms timeout; the wait
        // must survive until the handler finishes.
        let received = router
            .await_active(responses.recv(), Duration::from_millis(40))
            .await;
        assert!(received.flatten().is_some());
    }
}
