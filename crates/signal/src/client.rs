//! HTTP JSON-RPC client for signal-cli daemon mode.

use {
    base64::{Engine as _, engine::general_purpose::STANDARD as BASE64},
    reqwest::{Client, Response},
    serde::{Deserialize, Serialize, de::DeserializeOwned},
    serde_json::{Value, json},
    std::time::Duration,
    uuid::Uuid,
};

#[derive(Clone, Debug)]
pub struct SignalClient {
    /// Short-lived RPC requests (connect 10s, overall 30s).
    http: Client,
    /// Long-lived SSE stream (connect 10s, no overall timeout).
    sse: Client,
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: Option<i64>,
    message: Option<String>,
}

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: Value,
    id: String,
}

impl Default for SignalClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            sse: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    pub async fn check(&self, base_url: &str) -> antre_channels::Result<Value> {
        let url = format!("{}/api/v1/check", base_url.trim_end_matches('/'));
        let res = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| antre_channels::Error::external("signal-cli check", e))?;
        let status = res.status();
        if !status.is_success() {
            return Err(antre_channels::Error::unavailable(format!(
                "signal-cli check failed with HTTP {status}"
            )));
        }
        let body = res
            .bytes()
            .await
            .map_err(|e| antre_channels::Error::external("signal-cli check body", e))?;
        parse_check_body(&body)
    }

    pub async fn stream_events(
        &self,
        base_url: &str,
        account: Option<&str>,
    ) -> antre_channels::Result<Response> {
        let mut url = url::Url::parse(&format!("{}/api/v1/events", base_url.trim_end_matches('/')))
            .map_err(|e| {
                antre_channels::Error::invalid_input(format!("invalid Signal daemon URL: {e}"))
            })?;
        if let Some(account) = account {
            url.query_pairs_mut().append_pair("account", account);
        }
        self.sse
            .get(url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|e| antre_channels::Error::external("signal-cli events", e))
    }

    pub async fn rpc_value(
        &self,
        base_url: &str,
        method: &str,
        params: Value,
    ) -> antre_channels::Result<Value> {
        self.rpc(base_url, method, params).await
    }

    /// Fetch an inbound attachment through signal-cli's `getAttachment` RPC.
    ///
    /// `chat_id` is the conversation the attachment arrived in: a sender
    /// identifier for DMs or `group:<id>` for groups.
    pub async fn get_attachment(
        &self,
        base_url: &str,
        account: Option<&str>,
        attachment_id: &str,
        chat_id: &str,
        max_bytes: usize,
    ) -> antre_channels::Result<Vec<u8>> {
        let mut params = json!({ "id": attachment_id });
        if let Some(obj) = params.as_object_mut() {
            match chat_id.strip_prefix("group:") {
                Some(group_id) => obj.insert("groupId".to_string(), json!(group_id)),
                None => obj.insert("recipient".to_string(), json!(chat_id)),
            };
            if let Some(account) = account {
                obj.insert("account".to_string(), json!(account));
            }
        }
        let result = self.rpc_value(base_url, "getAttachment", params).await?;
        decode_attachment_payload(&result, max_bytes)
    }

    pub async fn rpc<T>(
        &self,
        base_url: &str,
        method: &str,
        params: Value,
    ) -> antre_channels::Result<T>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}/api/v1/rpc", base_url.trim_end_matches('/'));
        let body = RpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id: Uuid::new_v4().to_string(),
        };

        let res = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| antre_channels::Error::external("signal-cli rpc", e))?;
        let status = res.status();
        if !status.is_success() {
            return Err(antre_channels::Error::unavailable(format!(
                "signal-cli RPC {method} failed with HTTP {status}"
            )));
        }

        let text = res
            .text()
            .await
            .map_err(|e| antre_channels::Error::external("signal-cli rpc body", e))?;
        if text.trim().is_empty() {
            return serde_json::from_value(json!({}))
                .map_err(|e| antre_channels::Error::external("signal-cli empty rpc response", e));
        }

        let parsed: RpcResponse<T> = serde_json::from_str(&text)
            .map_err(|e| antre_channels::Error::external("signal-cli malformed rpc response", e))?;
        if let Some(error) = parsed.error {
            let code = error
                .code
                .map_or_else(|| "unknown".to_string(), |v| v.to_string());
            let message = error
                .message
                .unwrap_or_else(|| "Signal RPC error".to_string());
            return Err(antre_channels::Error::unavailable(format!(
                "signal-cli RPC {method} error {code}: {message}"
            )));
        }
        parsed.result.ok_or_else(|| {
            antre_channels::Error::unavailable(format!(
                "signal-cli RPC {method} returned no result"
            ))
        })
    }
}

/// Decode the base64 payload returned by `getAttachment`, either
/// `{"data": "<base64>"}` or a bare base64 string, enforcing `max_bytes`.
pub(crate) fn decode_attachment_payload(
    result: &Value,
    max_bytes: usize,
) -> antre_channels::Result<Vec<u8>> {
    let encoded = result
        .get("data")
        .and_then(Value::as_str)
        .or_else(|| result.as_str())
        .ok_or_else(|| {
            antre_channels::Error::invalid_input("signal-cli getAttachment returned no data")
        })?;
    // Reject before decoding: base64 inflates the payload by 4/3.
    if encoded.len() / 4 * 3 > max_bytes.saturating_add(3) {
        return Err(antre_channels::Error::invalid_input(format!(
            "Signal attachment exceeds {max_bytes} bytes"
        )));
    }
    let bytes = BASE64
        .decode(encoded.trim())
        .map_err(|e| antre_channels::Error::external("signal-cli attachment base64", e))?;
    if bytes.len() > max_bytes {
        return Err(antre_channels::Error::invalid_input(format!(
            "Signal attachment exceeds {max_bytes} bytes"
        )));
    }
    Ok(bytes)
}

/// signal-cli's `/api/v1/check` answers 200 with an empty body, so only a
/// non-empty body is decoded.
fn parse_check_body(body: &[u8]) -> antre_channels::Result<Value> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body)
        .map_err(|e| antre_channels::Error::external("signal-cli check json", e))
}

#[cfg(test)]
mod tests {
    use {
        super::{decode_attachment_payload, parse_check_body},
        base64::{Engine as _, engine::general_purpose::STANDARD as BASE64},
        serde_json::{Value, json},
    };

    #[test]
    fn decodes_data_object_and_bare_string() {
        let encoded = BASE64.encode(b"voice-bytes");
        let from_object = decode_attachment_payload(&json!({ "data": encoded }), 1024);
        assert_eq!(from_object.ok(), Some(b"voice-bytes".to_vec()));
        let from_string = decode_attachment_payload(&json!(encoded), 1024);
        assert_eq!(from_string.ok(), Some(b"voice-bytes".to_vec()));
    }

    #[test]
    fn rejects_missing_data_invalid_base64_and_oversize() {
        assert!(decode_attachment_payload(&json!({}), 1024).is_err());
        assert!(decode_attachment_payload(&json!({ "data": "%%%" }), 1024).is_err());
        let encoded = BASE64.encode(vec![0_u8; 64]);
        assert!(decode_attachment_payload(&json!({ "data": encoded }), 63).is_err());
        assert!(decode_attachment_payload(&json!({ "data": encoded }), 64).is_ok());
        let huge = BASE64.encode(vec![0_u8; 4096]);
        assert!(decode_attachment_payload(&json!({ "data": huge }), 16).is_err());
    }

    #[test]
    fn empty_check_body_is_healthy() {
        assert!(matches!(parse_check_body(b""), Ok(Value::Null)));
        assert!(matches!(parse_check_body(b" \n"), Ok(Value::Null)));
    }

    #[test]
    fn json_check_body_is_decoded() {
        assert_eq!(
            parse_check_body(br#"{"version":"0.13"}"#).ok(),
            Some(json!({ "version": "0.13" }))
        );
    }

    #[test]
    fn malformed_check_body_is_an_error() {
        assert!(parse_check_body(b"not json").is_err());
    }
}
