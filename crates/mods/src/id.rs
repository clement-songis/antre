//! Validated identifiers: mod ids (`publisher.name`), local ids and the
//! namespaced contribution ids built from them (`owner/local`).

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

const MAX_SEGMENT_LEN: usize = 48;
const MAX_LOCAL_LEN: usize = 64;

/// Publishers that packages cannot claim. `core` names built-in
/// contributions, `local` names compatibility mods the host creates itself
/// (for example migrated shell hooks).
const RESERVED_PUBLISHERS: &[&str] = &["core", "local", "moltis", "antre"];

fn invalid(value: &str, reason: &'static str) -> Error {
    Error::InvalidId {
        value: value.to_string(),
        reason,
    }
}

fn validate_segment(value: &str, whole: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_SEGMENT_LEN {
        return Err(invalid(whole, "each segment must be 1-48 characters"));
    }
    let mut chars = value.chars();
    if !chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
    {
        return Err(invalid(whole, "segments must start with [a-z0-9]"));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(invalid(whole, "segments may only contain [a-z0-9-]"));
    }
    if value.ends_with('-') {
        return Err(invalid(whole, "segments must not end with '-'"));
    }
    Ok(())
}

/// A mod identifier: `<publisher>.<name>`, lowercase.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModId {
    publisher: String,
    name: String,
}

impl ModId {
    /// Parse an id coming from a package manifest. Reserved publishers are
    /// rejected so a package can never impersonate core or host-created mods.
    pub fn parse(value: &str) -> Result<Self> {
        let id = Self::parse_any(value)?;
        if RESERVED_PUBLISHERS.contains(&id.publisher.as_str()) {
            return Err(invalid(value, "publisher is reserved"));
        }
        Ok(id)
    }

    /// Build an id for a host-created mod under a reserved publisher.
    pub fn host_owned(publisher: &'static str, name: &str) -> Result<Self> {
        let whole = format!("{publisher}.{name}");
        if !RESERVED_PUBLISHERS.contains(&publisher) {
            return Err(invalid(&whole, "host-owned ids need a reserved publisher"));
        }
        validate_segment(name, &whole)?;
        Ok(Self {
            publisher: publisher.to_string(),
            name: name.to_string(),
        })
    }

    fn parse_any(value: &str) -> Result<Self> {
        let Some((publisher, name)) = value.split_once('.') else {
            return Err(invalid(value, "expected `<publisher>.<name>`"));
        };
        if name.contains('.') {
            return Err(invalid(value, "expected exactly one '.'"));
        }
        validate_segment(publisher, value)?;
        validate_segment(name, value)?;
        Ok(Self {
            publisher: publisher.to_string(),
            name: name.to_string(),
        })
    }

    #[must_use]
    pub fn publisher(&self) -> &str {
        &self.publisher
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True for ids under a reserved publisher (`local.*`, `core.*`).
    #[must_use]
    pub fn is_host_owned(&self) -> bool {
        RESERVED_PUBLISHERS.contains(&self.publisher.as_str())
    }
}

impl fmt::Display for ModId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.publisher, self.name)
    }
}

impl Serialize for ModId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ModId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse_any(&raw).map_err(serde::de::Error::custom)
    }
}

/// The local part of a contribution id, unique within its owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct LocalId(String);

impl LocalId {
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty() || value.len() > MAX_LOCAL_LEN {
            return Err(invalid(value, "local ids must be 1-64 characters"));
        }
        if !value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        {
            return Err(invalid(value, "local ids must start with [a-z0-9]"));
        }
        if !value.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_' | '.')
        }) {
            return Err(invalid(value, "local ids may only contain [a-z0-9._-]"));
        }
        Ok(Self(value.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for LocalId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Who owns a contribution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    /// Built into the binary.
    Core,
    /// Contributed by an installed mod.
    Mod(ModId),
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core => f.write_str("core"),
            Self::Mod(id) => id.fmt(f),
        }
    }
}

/// A namespaced contribution id: `core/<local>` or `<mod-id>/<local>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContributionId {
    owner: Owner,
    local: LocalId,
}

impl ContributionId {
    #[must_use]
    pub fn new(owner: Owner, local: LocalId) -> Self {
        Self { owner, local }
    }

    pub fn core(local: &str) -> Result<Self> {
        Ok(Self::new(Owner::Core, LocalId::parse(local)?))
    }

    #[must_use]
    pub fn owner(&self) -> &Owner {
        &self.owner
    }

    #[must_use]
    pub fn local(&self) -> &LocalId {
        &self.local
    }
}

impl fmt::Display for ContributionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.local)
    }
}

impl FromStr for ContributionId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let Some((owner, local)) = value.split_once('/') else {
            return Err(invalid(value, "expected `<owner>/<local>`"));
        };
        let owner = if owner == "core" {
            Owner::Core
        } else {
            Owner::Mod(ModId::parse_any(owner)?)
        };
        Ok(Self::new(owner, LocalId::parse(local)?))
    }
}

impl Serialize for ContributionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_mod_ids() {
        let id = ModId::parse("acme.github-prs").unwrap();
        assert_eq!(id.publisher(), "acme");
        assert_eq!(id.name(), "github-prs");
        assert_eq!(id.to_string(), "acme.github-prs");
        assert!(!id.is_host_owned());
    }

    #[test]
    fn rejects_malformed_mod_ids() {
        for bad in [
            "",
            "acme",
            "acme.",
            ".name",
            "Acme.tool",
            "acme.tool.extra",
            "acme.to ol",
            "acme.-tool",
            "acme.tool-",
            "acme/tool",
            "../etc.passwd",
            "acme.tööl",
        ] {
            assert!(ModId::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        let long = format!("acme.{}", "a".repeat(49));
        assert!(ModId::parse(&long).is_err());
    }

    #[test]
    fn packages_cannot_claim_reserved_publishers() {
        for reserved in ["core.calc", "local.hook-x", "moltis.web", "antre.ui"] {
            assert!(ModId::parse(reserved).is_err(), "{reserved} accepted");
        }
        let host = ModId::host_owned("local", "hook-x").unwrap();
        assert!(host.is_host_owned());
        assert!(ModId::host_owned("acme", "x").is_err());
    }

    #[test]
    fn contribution_ids_round_trip() {
        let id: ContributionId = "acme.prs/refresh".parse().unwrap();
        assert_eq!(id.to_string(), "acme.prs/refresh");
        assert!(matches!(id.owner(), Owner::Mod(_)));
        let core = ContributionId::core("hook.command-logger").unwrap();
        assert_eq!(core.to_string(), "core/hook.command-logger");
        assert_eq!(
            "core/hook.command-logger"
                .parse::<ContributionId>()
                .unwrap(),
            core
        );
        assert!("acme.prs".parse::<ContributionId>().is_err());
        assert!("acme.prs/Bad".parse::<ContributionId>().is_err());
        assert!(LocalId::parse("").is_err());
        assert!(LocalId::parse("a/b").is_err());
    }

    #[test]
    fn mod_id_serde_round_trip() {
        let id = ModId::parse("acme.prs").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"acme.prs\"");
        let back: ModId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_str::<ModId>("\"bad\"").is_err());
    }
}
