//! Consent: what a human approved, and whether a capability set is covered.

use {
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::capability::{Capability, CapabilitySet, ConsentHash};

/// Whether a human is present to approve capabilities. Anything started by
/// the agent, a channel, a webhook, a schedule or a script is non-interactive
/// and can never grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    Interactive,
    NonInteractive,
}

/// Where a grant came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantOrigin {
    /// A person approved the exact consent hash.
    Interactive,
    /// Recorded when existing shell hooks were migrated to mods; they ran
    /// before mods existed, so they keep running until their content changes.
    Migrated,
}

/// An approved capability set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub consent_hash: ConsentHash,
    pub capabilities: CapabilitySet,
    pub origin: GrantOrigin,
    #[serde(with = "time::serde::rfc3339")]
    pub granted_at: OffsetDateTime,
}

impl Grant {
    #[must_use]
    pub fn new(capabilities: CapabilitySet, origin: GrantOrigin) -> Self {
        Self {
            consent_hash: capabilities.consent_hash(),
            capabilities,
            origin,
            granted_at: OffsetDateTime::now_utc(),
        }
    }
}

/// Result of checking a requested set against a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConsentState {
    /// Everything requested is covered by the grant.
    Granted,
    /// Approval needed; `missing` lists what the grant does not cover
    /// (everything, for a mod that was never granted).
    Pending {
        consent_hash: ConsentHash,
        missing: Vec<Capability>,
    },
}

impl ConsentState {
    #[must_use]
    pub fn evaluate(requested: &CapabilitySet, grant: Option<&Grant>) -> Self {
        let missing = match grant {
            Some(grant) => requested.missing_from(&grant.capabilities),
            None => requested.iter().cloned().collect(),
        };
        if grant.is_some() && missing.is_empty() {
            Self::Granted
        } else {
            Self::Pending {
                consent_hash: requested.consent_hash(),
                missing,
            }
        }
    }

    #[must_use]
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted)
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> CapabilitySet {
        items.iter().map(|item| item.parse().unwrap()).collect()
    }

    #[test]
    fn never_granted_is_pending_even_when_empty() {
        let state = ConsentState::evaluate(&CapabilitySet::new(), None);
        assert!(!state.is_granted());
        let state = ConsentState::evaluate(&set(&["tool"]), None);
        match state {
            ConsentState::Pending { missing, .. } => assert_eq!(missing.len(), 1),
            ConsentState::Granted => panic!("granted without a grant"),
        }
    }

    #[test]
    fn subset_update_stays_granted() {
        let grant = Grant::new(
            set(&["tool", "net:api.github.com"]),
            GrantOrigin::Interactive,
        );
        assert!(ConsentState::evaluate(&set(&["tool"]), Some(&grant)).is_granted());
        assert!(
            ConsentState::evaluate(&set(&["tool", "net:api.github.com"]), Some(&grant))
                .is_granted()
        );
    }

    #[test]
    fn widening_update_reprompts_with_delta() {
        let grant = Grant::new(
            set(&["tool", "net:api.github.com"]),
            GrantOrigin::Interactive,
        );
        let wider = set(&["tool", "net:api.github.com", "net:gitlab.com"]);
        match ConsentState::evaluate(&wider, Some(&grant)) {
            ConsentState::Pending {
                consent_hash,
                missing,
            } => {
                assert_eq!(consent_hash, wider.consent_hash());
                assert_eq!(missing.len(), 1);
                assert_eq!(missing[0].to_string(), "net:gitlab.com");
            },
            ConsentState::Granted => panic!("widening accepted silently"),
        }
    }

    #[test]
    fn grant_serde_round_trip() {
        let grant = Grant::new(set(&["exec"]), GrantOrigin::Migrated);
        let json = serde_json::to_string(&grant).unwrap();
        let back: Grant = serde_json::from_str(&json).unwrap();
        assert_eq!(back.consent_hash, grant.consent_hash);
        assert_eq!(back.origin, GrantOrigin::Migrated);
    }
}
