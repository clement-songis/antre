use url::{Host, Url};

use crate::{Error, Result};

/// Parse an OAuth endpoint URL, refusing plain HTTP except on loopback.
///
/// Tokens and client secrets travel in these requests, so a remote endpoint
/// must use TLS. Loopback stays allowed for local MCP servers and test mocks,
/// as RFC 8252 does for redirect URIs.
pub(crate) fn secure_endpoint_url(raw: &str, endpoint: &str) -> Result<Url> {
    let url = Url::parse(raw)?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        _ => Err(Error::message(format!(
            "OAuth {endpoint} endpoint must use https (got {})",
            url.scheme()
        ))),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https() {
        assert!(secure_endpoint_url("https://auth.example.com/token", "token").is_ok());
    }

    #[test]
    fn accepts_loopback_http() {
        for raw in [
            "http://127.0.0.1:8080/token",
            "http://localhost/token",
            "http://[::1]:9000/token",
        ] {
            assert!(secure_endpoint_url(raw, "token").is_ok(), "{raw}");
        }
    }

    #[test]
    fn rejects_remote_http_and_other_schemes() {
        for raw in [
            "http://auth.example.com/token",
            "http://192.168.1.10/token",
            "http://localhost.example.com/token",
            "ftp://auth.example.com/token",
        ] {
            assert!(secure_endpoint_url(raw, "token").is_err(), "{raw}");
        }
    }

    #[test]
    fn rejects_unparseable_url() {
        assert!(secure_endpoint_url("not a url", "token").is_err());
    }
}
