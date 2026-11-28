//! Capabilities a mod can request, their canonical form and the consent hash
//! computed over a normalized set.

use std::{cmp::Ordering, collections::BTreeSet, fmt, str::FromStr};

use {
    antre_common::hooks::HookEvent,
    serde::{Deserialize, Deserializer, Serialize, Serializer},
    sha2::{Digest, Sha256},
};

use crate::error::{Error, Result};

const MAX_HOST_LEN: usize = 253;

/// How a hook may act on an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookMode {
    /// Read the payload; any block or modify answer is ignored.
    Observe,
    /// Block the event or replace its payload.
    Intercept,
}

impl HookMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Intercept => "intercept",
        }
    }
}

/// How much a capability exposes, shown at consent time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

/// A host name a mod may reach over HTTP: an exact name, or `*.` followed by
/// a registrable-looking suffix of at least two labels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NetHost {
    wildcard: bool,
    suffix: String,
}

impl NetHost {
    pub fn parse(value: &str) -> Result<Self> {
        let lowered = value.trim().to_ascii_lowercase();
        let (wildcard, suffix) = match lowered.strip_prefix("*.") {
            Some(rest) => (true, rest.to_string()),
            None => (false, lowered),
        };
        let reject =
            |reason: &str| Error::Manifest(format!("invalid net host `{value}`: {reason}"));
        if suffix.is_empty() || suffix.len() > MAX_HOST_LEN {
            return Err(reject("length"));
        }
        let labels: Vec<&str> = suffix.split('.').collect();
        if labels.len() < 2 {
            return Err(reject("needs at least two labels"));
        }
        for label in &labels {
            let valid = !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
            if !valid {
                return Err(reject(
                    "labels must be [a-z0-9-], not starting or ending with '-'",
                ));
            }
        }
        if labels
            .iter()
            .all(|label| label.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Err(reject("IP addresses are not allowed"));
        }
        if labels
            .last()
            .is_some_and(|tld| tld.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Err(reject("top-level label cannot be numeric"));
        }
        if matches!(suffix.as_str(), "localhost" | "localhost.localdomain")
            || suffix.ends_with(".localhost")
            || suffix.ends_with(".local")
            || suffix.ends_with(".internal")
        {
            return Err(reject("local names are not allowed"));
        }
        Ok(Self { wildcard, suffix })
    }

    /// Whether a request to `host` is allowed by this entry. Exact entries
    /// match only themselves; wildcard entries match strict subdomains.
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.wildcard {
            host.len() > self.suffix.len()
                && host.ends_with(&self.suffix)
                && host.as_bytes()[host.len() - self.suffix.len() - 1] == b'.'
        } else {
            host == self.suffix
        }
    }

    /// Whether every host matched by `other` is also matched by `self`.
    #[must_use]
    pub fn covers(&self, other: &Self) -> bool {
        match (self.wildcard, other.wildcard) {
            (false, false) => self.suffix == other.suffix,
            (true, false) => self.matches(&other.suffix),
            (true, true) => self.suffix == other.suffix || self.matches(&other.suffix),
            (false, true) => false,
        }
    }

    /// The bare host (wildcard stripped), used for the HTTP client allowlist.
    #[must_use]
    pub fn suffix(&self) -> &str {
        &self.suffix
    }
}

impl Serialize for NetHost {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl fmt::Display for NetHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.wildcard {
            write!(f, "*.{}", self.suffix)
        } else {
            f.write_str(&self.suffix)
        }
    }
}

/// One capability. The canonical string form (`Display`) is what the consent
/// hash is computed over, so it must stay stable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Offer tools to the agent (trusted turns only).
    Tool,
    /// Per-mod key/value storage up to `bytes`.
    Storage { bytes: u64 },
    /// Outbound HTTP to a host.
    Net(NetHost),
    /// Subscribe to a hook event.
    Hook { event: HookEvent, mode: HookMode },
    /// Run native processes with the user's authority (shell hooks).
    Exec,
}

impl Capability {
    #[must_use]
    pub fn risk(&self) -> Risk {
        match self {
            Self::Storage { .. } => Risk::Low,
            Self::Tool | Self::Net(_) => Risk::Medium,
            Self::Hook {
                mode: HookMode::Observe,
                ..
            } => Risk::Medium,
            Self::Hook {
                mode: HookMode::Intercept,
                ..
            } => Risk::High,
            Self::Exec => Risk::Critical,
        }
    }

    /// Whether holding `self` implies holding `other`.
    #[must_use]
    pub fn covers(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Storage { bytes: have }, Self::Storage { bytes: want }) => want <= have,
            (Self::Net(have), Self::Net(want)) => have.covers(want),
            (
                Self::Hook {
                    event: have_event,
                    mode: have_mode,
                },
                Self::Hook {
                    event: want_event,
                    mode: want_mode,
                },
            ) => have_event == want_event && want_mode <= have_mode,
            (Self::Tool, Self::Tool) | (Self::Exec, Self::Exec) => true,
            _ => false,
        }
    }

    /// Short human description for consent screens.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Tool => "Offer tools to the agent".to_string(),
            Self::Storage { bytes } => format!("Store up to {bytes} bytes of data"),
            Self::Net(host) => format!("Make HTTP requests to {host}"),
            Self::Hook { event, mode } => match mode {
                HookMode::Observe => format!("Observe {event} events"),
                HookMode::Intercept => format!("Block or modify {event} events"),
            },
            Self::Exec => "Run programs on this machine with your permissions".to_string(),
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tool => f.write_str("tool"),
            Self::Storage { bytes } => write!(f, "storage:{bytes}"),
            Self::Net(host) => write!(f, "net:{host}"),
            Self::Hook { event, mode } => write!(f, "hook:{event}:{}", mode.as_str()),
            Self::Exec => f.write_str("exec"),
        }
    }
}

impl FromStr for Capability {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let bad = || Error::Manifest(format!("unknown capability `{value}`"));
        match value {
            "tool" => return Ok(Self::Tool),
            "exec" => return Ok(Self::Exec),
            _ => {},
        }
        let (kind, rest) = value.split_once(':').ok_or_else(bad)?;
        match kind {
            "storage" => rest
                .parse()
                .map(|bytes| Self::Storage { bytes })
                .map_err(|_| bad()),
            "net" => NetHost::parse(rest).map(Self::Net),
            "hook" => {
                let (event, mode) = rest.rsplit_once(':').ok_or_else(bad)?;
                let event = event.parse::<HookEvent>().map_err(|_| bad())?;
                let mode = match mode {
                    "observe" => HookMode::Observe,
                    "intercept" => HookMode::Intercept,
                    _ => return Err(bad()),
                };
                Ok(Self::Hook { event, mode })
            },
            _ => Err(bad()),
        }
    }
}

impl PartialOrd for Capability {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Capability {
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl Serialize for Capability {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Capability {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// SHA-256 over the canonical capability list, hex encoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConsentHash(String);

impl ConsentHash {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConsentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A normalized set of capabilities: one storage entry (the largest), and an
/// intercept hook supersedes an observe hook for the same event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySet(BTreeSet<Capability>);

impl CapabilitySet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, capability: Capability) {
        if self.0.iter().any(|existing| existing.covers(&capability)) {
            return;
        }
        self.0.retain(|existing| !capability.covers(existing));
        self.0.insert(capability);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.0.iter()
    }

    /// Whether some capability in the set implies `capability`.
    #[must_use]
    pub fn allows(&self, capability: &Capability) -> bool {
        self.0.iter().any(|have| have.covers(capability))
    }

    /// Capabilities in `self` that `granted` does not cover: the widening an
    /// update asks for.
    #[must_use]
    pub fn missing_from(&self, granted: &Self) -> Vec<Capability> {
        self.0
            .iter()
            .filter(|want| !granted.allows(want))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn is_covered_by(&self, granted: &Self) -> bool {
        self.missing_from(granted).is_empty()
    }

    #[must_use]
    pub fn consent_hash(&self) -> ConsentHash {
        let mut hasher = Sha256::new();
        hasher.update(b"antre-mod-capabilities-v1\n");
        for capability in &self.0 {
            hasher.update(capability.to_string().as_bytes());
            hasher.update(b"\n");
        }
        ConsentHash(hex::encode(hasher.finalize()))
    }

    #[must_use]
    pub fn net_hosts(&self) -> Vec<&NetHost> {
        self.0
            .iter()
            .filter_map(|capability| match capability {
                Capability::Net(host) => Some(host),
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn storage_quota(&self) -> Option<u64> {
        self.0.iter().find_map(|capability| match capability {
            Capability::Storage { bytes } => Some(*bytes),
            _ => None,
        })
    }

    #[must_use]
    pub fn max_risk(&self) -> Option<Risk> {
        self.0.iter().map(Capability::risk).max()
    }
}

impl FromIterator<Capability> for CapabilitySet {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        let mut set = Self::new();
        for capability in iter {
            set.insert(capability);
        }
        set
    }
}

impl Serialize for CapabilitySet {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter())
    }
}

impl<'de> Deserialize<'de> for CapabilitySet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let items = Vec::<Capability>::deserialize(deserializer)?;
        Ok(items.into_iter().collect())
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn net(host: &str) -> Capability {
        Capability::Net(NetHost::parse(host).unwrap())
    }

    #[test]
    fn net_host_validation() {
        for good in [
            "api.github.com",
            "*.example.org",
            "API.GitHub.com",
            "a-b.c1.io",
        ] {
            assert!(NetHost::parse(good).is_ok(), "{good} rejected");
        }
        for bad in [
            "",
            "com",
            "*.com",
            "localhost",
            "foo.localhost",
            "printer.local",
            "metadata.google.internal",
            "127.0.0.1",
            "10.0.0.1",
            "1.2.3.4",
            "-bad.com",
            "bad-.com",
            "a..b",
            "exa mple.com",
            "example.com/path",
            "user@example.com",
            "*.*.example.com",
            "example.123",
        ] {
            assert!(NetHost::parse(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn exact_host_does_not_match_lookalikes() {
        let host = NetHost::parse("api.github.com").unwrap();
        assert!(host.matches("api.github.com"));
        assert!(host.matches("API.GITHUB.COM"));
        assert!(host.matches("api.github.com."));
        assert!(!host.matches("evil.api.github.com"));
        assert!(!host.matches("api.github.com.attacker.tld"));
        assert!(!host.matches("xapi.github.com"));
        assert!(!host.matches("github.com"));
    }

    #[test]
    fn wildcard_matches_only_strict_subdomains() {
        let host = NetHost::parse("*.example.org").unwrap();
        assert!(host.matches("a.example.org"));
        assert!(host.matches("a.b.example.org"));
        assert!(!host.matches("example.org"));
        assert!(!host.matches("badexample.org"));
        assert!(!host.matches("example.org.evil.com"));
    }

    #[test]
    fn coverage_rules() {
        assert!(net("*.example.org").covers(&net("api.example.org")));
        assert!(net("*.example.org").covers(&net("*.api.example.org")));
        assert!(!net("api.example.org").covers(&net("*.example.org")));
        assert!(!net("example.org").covers(&net("api.example.org")));
        assert!(Capability::Storage { bytes: 100 }.covers(&Capability::Storage { bytes: 10 }));
        assert!(!Capability::Storage { bytes: 10 }.covers(&Capability::Storage { bytes: 100 }));
        let intercept = Capability::Hook {
            event: HookEvent::BeforeToolCall,
            mode: HookMode::Intercept,
        };
        let observe = Capability::Hook {
            event: HookEvent::BeforeToolCall,
            mode: HookMode::Observe,
        };
        assert!(intercept.covers(&observe));
        assert!(!observe.covers(&intercept));
        assert!(!Capability::Tool.covers(&Capability::Exec));
    }

    #[test]
    fn canonical_round_trip() {
        for raw in [
            "tool",
            "exec",
            "storage:65536",
            "net:api.github.com",
            "net:*.example.org",
            "hook:BeforeToolCall:intercept",
            "hook:SessionStart:observe",
        ] {
            let capability: Capability = raw.parse().unwrap();
            assert_eq!(capability.to_string(), raw);
        }
        for bad in [
            "",
            "net",
            "net:",
            "hook:Nope:observe",
            "hook:SessionStart",
            "fs:/",
            "storage:x",
        ] {
            assert!(bad.parse::<Capability>().is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn set_normalizes() {
        let set: CapabilitySet = [
            Capability::Storage { bytes: 10 },
            Capability::Storage { bytes: 100 },
            net("api.example.org"),
            net("*.example.org"),
            Capability::Hook {
                event: HookEvent::BeforeToolCall,
                mode: HookMode::Observe,
            },
            Capability::Hook {
                event: HookEvent::BeforeToolCall,
                mode: HookMode::Intercept,
            },
        ]
        .into_iter()
        .collect();
        assert_eq!(set.len(), 3);
        assert_eq!(set.storage_quota(), Some(100));
        assert_eq!(set.max_risk(), Some(Risk::High));
    }

    #[test]
    fn consent_hash_is_order_independent_and_sensitive() {
        let a: CapabilitySet = [Capability::Tool, net("api.github.com")]
            .into_iter()
            .collect();
        let b: CapabilitySet = [net("api.github.com"), Capability::Tool]
            .into_iter()
            .collect();
        assert_eq!(a.consent_hash(), b.consent_hash());
        let c: CapabilitySet = [Capability::Tool, net("api.gitlab.com")]
            .into_iter()
            .collect();
        assert_ne!(a.consent_hash(), c.consent_hash());
        assert_ne!(CapabilitySet::new().consent_hash(), a.consent_hash());
        assert_eq!(a.consent_hash().as_str().len(), 64);
    }

    #[test]
    fn widening_is_detected() {
        let granted: CapabilitySet = [Capability::Tool, net("api.github.com")]
            .into_iter()
            .collect();
        let same: CapabilitySet = [net("api.github.com")].into_iter().collect();
        assert!(same.is_covered_by(&granted));
        let wider: CapabilitySet = [Capability::Tool, net("api.github.com"), net("gitlab.com")]
            .into_iter()
            .collect();
        assert_eq!(wider.missing_from(&granted), vec![net("gitlab.com")]);
        let exec: CapabilitySet = [Capability::Exec].into_iter().collect();
        assert!(!exec.is_covered_by(&granted));
    }

    #[test]
    fn serde_round_trip() {
        let set: CapabilitySet = [Capability::Tool, net("api.github.com"), Capability::Exec]
            .into_iter()
            .collect();
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(json, r#"["exec","net:api.github.com","tool"]"#);
        let back: CapabilitySet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, set);
        assert!(serde_json::from_str::<CapabilitySet>(r#"["fs:/"]"#).is_err());
    }
}
