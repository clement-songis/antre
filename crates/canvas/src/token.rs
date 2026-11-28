//! Short-lived signed links for the render origin.
//!
//! The render origin never reads cookies: a cookie set for the app host is
//! also sent to every other port of that host, so access is granted only by
//! an HMAC-signed token carried in the URL path (so relative asset URLs keep
//! it). The key is random per process; links die on restart.

use {
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    hmac::{Hmac, Mac},
    rand::Rng,
    sha2::Sha256,
    time::OffsetDateTime,
};

use crate::{DesignId, Error, Result};

type HmacSha256 = Hmac<Sha256>;

/// What a valid token grants access to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenClaims {
    pub design_id: DesignId,
    /// `None` serves the live directory, `Some(n)` the snapshot of version n.
    pub version: Option<u32>,
    pub expires_at: i64,
}

/// What a valid frame token grants access to: one staged document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameClaims {
    pub frame_id: DesignId,
    pub expires_at: i64,
}

/// Prefix of frame token payloads, covered by the MAC so a design token can
/// never be replayed as a frame token or the reverse.
const FRAME_PREFIX: &str = "f";

fn expiry(ttl_secs: u64) -> i64 {
    let ttl = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
    OffsetDateTime::now_utc()
        .unix_timestamp()
        .saturating_add(ttl)
}

/// Signs and verifies render-origin tokens.
pub struct TokenSigner {
    key: [u8; 32],
}

impl std::fmt::Debug for TokenSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSigner")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl TokenSigner {
    #[must_use]
    pub fn random() -> Self {
        let mut key = [0_u8; 32];
        rand::rng().fill_bytes(&mut key);
        Self { key }
    }

    #[cfg(test)]
    pub(crate) fn with_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    fn mac(&self, payload: &str) -> HmacSha256 {
        // HMAC accepts keys of any length, so this cannot fail.
        let mut mac = match HmacSha256::new_from_slice(&self.key) {
            Ok(mac) => mac,
            Err(_) => unreachable!("HMAC-SHA256 accepts any key length"),
        };
        mac.update(payload.as_bytes());
        mac
    }

    /// Token for `design_id` valid for `ttl_secs` from now.
    #[must_use]
    pub fn sign(&self, design_id: &DesignId, version: Option<u32>, ttl_secs: u64) -> String {
        self.sign_until(design_id, version, expiry(ttl_secs))
    }

    fn seal(&self, payload: String) -> String {
        let tag = URL_SAFE_NO_PAD.encode(self.mac(&payload).finalize().into_bytes());
        format!("{payload}.{tag}")
    }

    /// Payload of a well-formed token whose MAC matches.
    fn open<'a>(&self, token: &'a str) -> Result<&'a str> {
        let invalid = || Error::Invalid("invalid or expired link".into());
        if token.len() > 256 {
            return Err(invalid());
        }
        let (payload, tag) = token.rsplit_once('.').ok_or_else(invalid)?;
        let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| invalid())?;
        self.mac(payload)
            .verify_slice(&tag)
            .map_err(|_| invalid())?;
        Ok(payload)
    }

    /// Token for staged document `frame_id`, valid for `ttl_secs` from now.
    #[must_use]
    pub fn sign_frame(&self, frame_id: &DesignId, ttl_secs: u64) -> String {
        self.seal(format!("{FRAME_PREFIX}.{frame_id}.{}", expiry(ttl_secs)))
    }

    /// Check a frame token's signature and expiry.
    pub fn verify_frame(&self, token: &str) -> Result<FrameClaims> {
        self.verify_frame_at(token, OffsetDateTime::now_utc().unix_timestamp())
    }

    fn verify_frame_at(&self, token: &str, now: i64) -> Result<FrameClaims> {
        let invalid = || Error::Invalid("invalid or expired link".into());
        let payload = self.open(token)?;
        let mut parts = payload.split('.');
        let (Some(FRAME_PREFIX), Some(id), Some(expires_at), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let expires_at: i64 = expires_at.parse().map_err(|_| invalid())?;
        if expires_at < now {
            return Err(invalid());
        }
        Ok(FrameClaims {
            frame_id: id.parse().map_err(|_| invalid())?,
            expires_at,
        })
    }

    fn sign_until(&self, design_id: &DesignId, version: Option<u32>, expires_at: i64) -> String {
        let version = version.map_or_else(|| "live".to_string(), |v| v.to_string());
        self.seal(format!("{design_id}.{version}.{expires_at}"))
    }

    /// Check signature and expiry.
    pub fn verify(&self, token: &str) -> Result<TokenClaims> {
        self.verify_at(token, OffsetDateTime::now_utc().unix_timestamp())
    }

    fn verify_at(&self, token: &str, now: i64) -> Result<TokenClaims> {
        let invalid = || Error::Invalid("invalid or expired link".into());
        let payload = self.open(token)?;
        let mut parts = payload.split('.');
        let (Some(id), Some(version), Some(expires_at), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let expires_at: i64 = expires_at.parse().map_err(|_| invalid())?;
        if expires_at < now {
            return Err(invalid());
        }
        let version = match version {
            "live" => None,
            number => Some(number.parse().map_err(|_| invalid())?),
        };
        Ok(TokenClaims {
            design_id: id.parse().map_err(|_| invalid())?,
            version,
            expires_at,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_live_and_versioned() {
        let signer = TokenSigner::random();
        let id = DesignId::generate();
        let live = signer.verify(&signer.sign(&id, None, 60)).unwrap();
        assert_eq!(live.design_id, id);
        assert_eq!(live.version, None);
        let pinned = signer.verify(&signer.sign(&id, Some(3), 60)).unwrap();
        assert_eq!(pinned.version, Some(3));
    }

    #[test]
    fn rejects_expired_tokens() {
        let signer = TokenSigner::with_key([7; 32]);
        let id = DesignId::generate();
        let token = signer.sign_until(&id, None, 1_000);
        assert!(signer.verify_at(&token, 999).is_ok());
        assert!(signer.verify_at(&token, 1_000).is_ok());
        assert!(signer.verify_at(&token, 1_001).is_err());
    }

    #[test]
    fn rejects_tampering_and_foreign_keys() {
        let signer = TokenSigner::with_key([1; 32]);
        let other = TokenSigner::with_key([2; 32]);
        let id = DesignId::generate();
        let token = signer.sign(&id, Some(1), 60);
        assert!(other.verify(&token).is_err());

        let forged_version = token.replacen(".1.", ".2.", 1);
        assert!(signer.verify(&forged_version).is_err());
        let other_id = DesignId::generate();
        let forged_id = token.replacen(id.as_str(), other_id.as_str(), 1);
        assert!(signer.verify(&forged_id).is_err());

        for garbage in ["", "abc", "a.b.c.d", "x".repeat(300).as_str()] {
            assert!(signer.verify(garbage).is_err());
        }
    }

    #[test]
    fn frame_tokens_are_separate_from_design_tokens() {
        let signer = TokenSigner::with_key([3; 32]);
        let id = DesignId::generate();
        let frame = signer.sign_frame(&id, 60);
        assert_eq!(signer.verify_frame(&frame).unwrap().frame_id, id);
        assert!(
            signer.verify(&frame).is_err(),
            "frame token is not a design token"
        );
        let design = signer.sign(&id, None, 60);
        assert!(signer.verify_frame(&design).is_err());
        assert!(TokenSigner::with_key([4; 32]).verify_frame(&frame).is_err());

        let expired = signer.seal(format!("f.{id}.1000"));
        assert!(signer.verify_frame_at(&expired, 1_000).is_ok());
        assert!(signer.verify_frame_at(&expired, 1_001).is_err());
        let other = DesignId::generate();
        assert!(
            signer
                .verify_frame(&frame.replacen(id.as_str(), other.as_str(), 1))
                .is_err()
        );
    }

    #[test]
    fn debug_redacts_key() {
        let rendered = format!("{:?}", TokenSigner::with_key([9; 32]));
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains('9'));
    }
}
