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

#[derive(Debug, Deserialize)]
struct AuthStatus {
    authenticated: bool,
}

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

/// Whether the daemon accepts `api_key` (`GET /api/auth/status` with a Bearer
/// token reports `authenticated`).
pub async fn api_key_accepted(base: &Url, api_key: &SecretString) -> anyhow::Result<bool> {
    let response = client()?
        .get(base.join("api/auth/status")?)
        .bearer_auth(api_key.expose_secret())
        .send()
        .await?
        .error_for_status()?;
    let status: AuthStatus = response.json().await?;
    Ok(status.authenticated)
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0_u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: \
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

    #[tokio::test]
    async fn reads_api_key_verdict() {
        let base = serve(r#"{"authenticated":false,"setup_required":false}"#).await;
        let key = SecretString::new("mk_x".into());
        assert!(!api_key_accepted(&base, &key).await.unwrap());
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
