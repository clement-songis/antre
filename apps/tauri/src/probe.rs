//! HTTP probes against a daemon: is it up, does it accept our API key.

use std::time::Duration;

use {
    anyhow::Context,
    secrecy::{ExposeSecret, SecretString},
    serde::Deserialize,
    url::Url,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Deserialize)]
pub struct Health {
    pub version: String,
    pub protocol: u32,
}

/// What a daemon answered when asked to trade the API key for a session.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionTicket {
    /// Navigate the WebView here: it sets the session cookie and redirects
    /// to the app.
    Redeem(Url),
    /// Valid key without `operator.admin`: the user logs in in the WebView.
    NotAdmin,
    /// Unknown or revoked key.
    Rejected,
}

#[derive(Debug, Deserialize)]
struct TicketResponse {
    redeem_path: String,
}

/// Only this path is ever navigated to: a hostile daemon must not steer the
/// WebView elsewhere through the response.
const REDEEM_PATH_PREFIX: &str = "/api/auth/session-ticket/redeem?ticket=";

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(PROBE_TIMEOUT).build()?)
}

/// `GET /health`, which needs no authentication.
pub async fn health(base: &Url) -> anyhow::Result<Health> {
    let response = client()?
        .get(base.join("health")?)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

/// `POST /api/auth/session-ticket` with the API key as Bearer token.
pub async fn session_ticket(base: &Url, api_key: &SecretString) -> anyhow::Result<SessionTicket> {
    let response = client()?
        .post(base.join("api/auth/session-ticket")?)
        .bearer_auth(api_key.expose_secret())
        .send()
        .await?;
    match response.status() {
        reqwest::StatusCode::UNAUTHORIZED => return Ok(SessionTicket::Rejected),
        reqwest::StatusCode::FORBIDDEN => return Ok(SessionTicket::NotAdmin),
        _ => {},
    }
    let ticket: TicketResponse = response.error_for_status()?.json().await?;
    if !ticket.redeem_path.starts_with(REDEEM_PATH_PREFIX) {
        anyhow::bail!("unexpected redeem path from {base}");
    }
    Ok(SessionTicket::Redeem(base.join(&ticket.redeem_path)?))
}

/// Poll `/health` until it answers or `timeout` elapses.
pub async fn wait_until_healthy(base: &Url, timeout: Duration) -> anyhow::Result<Health> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_error = None;
    while tokio::time::Instant::now() < deadline {
        match health(base).await {
            Ok(health) => return Ok(health),
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no answer")))
        .with_context(|| format!("daemon at {base} did not become healthy within {timeout:?}"))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        tokio::io::{AsyncReadExt, AsyncWriteExt},
    };

    /// A one-route HTTP server answering every request with `body`.
    async fn serve(body: &'static str) -> Url {
        serve_status("200 OK", body).await
    }

    async fn serve_status(status: &'static str, body: &'static str) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0_u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: \
                     {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()
    }

    #[tokio::test]
    async fn reads_health() {
        let base = serve(r#"{"status":"ok","version":"1.2.3","protocol":4,"connections":0}"#).await;
        let health = wait_until_healthy(&base, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(health.version, "1.2.3");
        assert_eq!(health.protocol, 4);
    }

    fn key() -> SecretString {
        SecretString::new("mk_x".into())
    }

    #[tokio::test]
    async fn a_ticket_becomes_a_same_origin_redeem_url() {
        let base = serve(
            r#"{"ticket":"t1","expires_in":60,"redeem_path":"/api/auth/session-ticket/redeem?ticket=t1"}"#,
        )
        .await;
        let SessionTicket::Redeem(url) = session_ticket(&base, &key()).await.unwrap() else {
            panic!("expected a redeem URL");
        };
        assert_eq!(url.origin(), base.origin());
        assert_eq!(url.path(), "/api/auth/session-ticket/redeem");
        assert_eq!(url.query(), Some("ticket=t1"));
    }

    #[tokio::test]
    async fn a_hostile_redeem_path_is_refused() {
        let base =
            serve(r#"{"ticket":"t","expires_in":60,"redeem_path":"//evil.example/x"}"#).await;
        assert!(session_ticket(&base, &key()).await.is_err());
        let base = serve(r#"{"ticket":"t","expires_in":60,"redeem_path":"/elsewhere"}"#).await;
        assert!(session_ticket(&base, &key()).await.is_err());
    }

    #[tokio::test]
    async fn key_verdicts_map_to_outcomes() {
        let rejected = serve_status("401 Unauthorized", "{}").await;
        assert_eq!(
            session_ticket(&rejected, &key()).await.unwrap(),
            SessionTicket::Rejected
        );
        let reader = serve_status("403 Forbidden", "{}").await;
        assert_eq!(
            session_ticket(&reader, &key()).await.unwrap(),
            SessionTicket::NotAdmin
        );
    }

    #[tokio::test]
    async fn times_out_when_nothing_listens() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let base = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert!(
            wait_until_healthy(&base, Duration::from_millis(400))
                .await
                .is_err()
        );
    }
}
