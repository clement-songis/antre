//! `moltis-mod.toml`: parsing and validation into concrete types.

use std::path::{Component, Path, PathBuf};

use {
    moltis_common::hooks::HookEvent,
    serde::{Deserialize, Serialize},
};

use crate::{
    capability::{Capability, CapabilitySet, HookMode, NetHost},
    error::{Error, Result},
    id::{LocalId, ModId},
};

/// File name of the manifest at the root of a mod package.
pub const MANIFEST_FILE: &str = "moltis-mod.toml";

/// SDK major/minor this host implements. Mods built for another `0.x` minor
/// are refused because `0.x` minors may break.
pub const SDK_VERSION: &str = "0.1";

const MAX_TOOLS: usize = 64;
const MAX_HOOKS: usize = 32;
const MAX_NET_HOSTS: usize = 32;
const MAX_TEXT: usize = 1024;
const MAX_NAME: usize = 80;
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
const MAX_STORAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema: u32,
    id: String,
    version: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    license: Option<String>,
    sdk: String,
    #[serde(default)]
    server: Option<RawServer>,
    #[serde(default)]
    capabilities: RawCapabilities,
    #[serde(default)]
    contributes: RawContributes,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    component: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCapabilities {
    #[serde(default)]
    net: Vec<String>,
    #[serde(default)]
    storage: Option<StorageSize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContributes {
    #[serde(default)]
    tools: Vec<RawTool>,
    #[serde(default)]
    hooks: Vec<RawHook>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTool {
    name: String,
    description: String,
    #[serde(default)]
    parameters: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHook {
    id: String,
    events: Vec<String>,
    #[serde(default)]
    mode: Option<HookMode>,
    #[serde(default)]
    priority: i32,
}

/// A storage quota written as bytes (`65536`) or with a unit (`"64KiB"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StorageSize(u64);

impl<'de> Deserialize<'de> for StorageSize {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bytes(u64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Bytes(bytes) => Ok(Self(bytes)),
            Raw::Text(text) => parse_size(&text)
                .map(Self)
                .ok_or_else(|| serde::de::Error::custom(format!("invalid size `{text}`"))),
        }
    }
}

fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let value: u64 = digits.parse().ok()?;
    let multiplier = match unit.trim() {
        "" | "B" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        _ => return None,
    };
    value.checked_mul(multiplier)
}

/// A tool a mod offers, declared statically so it is visible before any mod
/// code runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDecl {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A hook handler a mod registers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HookDecl {
    pub id: LocalId,
    pub events: Vec<HookEvent>,
    pub mode: HookMode,
    pub priority: i32,
}

/// A validated manifest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModManifest {
    pub id: ModId,
    pub version: String,
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    /// Component path relative to the package root.
    pub component: Option<PathBuf>,
    pub tools: Vec<ToolDecl>,
    pub hooks: Vec<HookDecl>,
    pub net: Vec<NetHost>,
    pub storage: Option<u64>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Manifest(message.into())
}

fn check_text(field: &str, value: &str, max: usize, required: bool) -> Result<()> {
    if required && value.trim().is_empty() {
        return Err(invalid(format!("`{field}` must not be empty")));
    }
    if value.chars().count() > max {
        return Err(invalid(format!(
            "`{field}` is longer than {max} characters"
        )));
    }
    if value.chars().any(|ch| ch.is_control() && ch != '\n') {
        return Err(invalid(format!("`{field}` contains control characters")));
    }
    Ok(())
}

fn check_version(version: &str) -> Result<()> {
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let numeric = parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 9 && part.chars().all(|c| c.is_ascii_digit())
        });
    let pre_ok = pre.is_none_or(|pre| {
        !pre.is_empty()
            && pre.len() <= 32
            && pre
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
    });
    if numeric && pre_ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "`version` `{version}` is not `MAJOR.MINOR.PATCH`"
        )))
    }
}

/// A relative path that stays inside the package: no root, no `..`.
fn check_relative_path(field: &str, value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty() || value.contains('\\') || value.contains('\0') {
        return Err(invalid(format!("`{field}` is not a valid relative path")));
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(invalid(format!("`{field}` must stay inside the package")));
    }
    Ok(path.to_path_buf())
}

fn check_tool_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|ch: char| ch.is_ascii_lowercase())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "tool name `{name}` must match [a-z][a-z0-9_]{{0,63}}"
        )))
    }
}

fn check_parameters(
    tool: &str,
    parameters: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let parameters =
        parameters.unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    if parameters.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(invalid(format!(
            "tool `{tool}` parameters must be a JSON schema with \"type\" = \"object\""
        )));
    }
    if serde_json::to_vec(&parameters)?.len() > MAX_SCHEMA_BYTES {
        return Err(invalid(format!(
            "tool `{tool}` parameters schema is too large"
        )));
    }
    Ok(parameters)
}

impl ModManifest {
    /// Parse and validate manifest text.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawManifest = toml::from_str(text)?;
        if raw.schema != 1 {
            return Err(invalid(format!(
                "unsupported manifest schema {}",
                raw.schema
            )));
        }
        if raw.sdk != SDK_VERSION {
            return Err(invalid(format!(
                "mod targets SDK {} but this host implements {SDK_VERSION}",
                raw.sdk
            )));
        }
        let id = ModId::parse(&raw.id)?;
        check_version(&raw.version)?;
        check_text("name", &raw.name, MAX_NAME, true)?;
        check_text("description", &raw.description, MAX_TEXT, false)?;
        if let Some(license) = &raw.license {
            check_text("license", license, MAX_NAME, true)?;
        }

        let component = raw
            .server
            .map(|server| check_relative_path("server.component", &server.component))
            .transpose()?;
        if let Some(path) = &component
            && path.extension().and_then(|ext| ext.to_str()) != Some("wasm")
        {
            return Err(invalid("`server.component` must be a .wasm file"));
        }

        if raw.contributes.tools.len() > MAX_TOOLS {
            return Err(invalid(format!("more than {MAX_TOOLS} tools")));
        }
        if raw.contributes.hooks.len() > MAX_HOOKS {
            return Err(invalid(format!("more than {MAX_HOOKS} hooks")));
        }
        let needs_component =
            !raw.contributes.tools.is_empty() || !raw.contributes.hooks.is_empty();
        if needs_component && component.is_none() {
            return Err(invalid("tools and hooks need a `[server] component`"));
        }

        let mut tools = Vec::with_capacity(raw.contributes.tools.len());
        for tool in raw.contributes.tools {
            check_tool_name(&tool.name)?;
            check_text("tool description", &tool.description, MAX_TEXT, true)?;
            if tools
                .iter()
                .any(|existing: &ToolDecl| existing.name == tool.name)
            {
                return Err(invalid(format!("duplicate tool `{}`", tool.name)));
            }
            let parameters = check_parameters(&tool.name, tool.parameters)?;
            tools.push(ToolDecl {
                name: tool.name,
                description: tool.description,
                parameters,
            });
        }

        let mut hooks = Vec::with_capacity(raw.contributes.hooks.len());
        for hook in raw.contributes.hooks {
            let id = LocalId::parse(&hook.id)?;
            if hooks.iter().any(|existing: &HookDecl| existing.id == id) {
                return Err(invalid(format!("duplicate hook id `{id}`")));
            }
            if hook.events.is_empty() {
                return Err(invalid(format!("hook `{id}` has no events")));
            }
            let mut events = Vec::with_capacity(hook.events.len());
            for name in &hook.events {
                let event: HookEvent = name
                    .parse()
                    .map_err(|_| invalid(format!("hook `{id}`: unknown event `{name}`")))?;
                if !events.contains(&event) {
                    events.push(event);
                }
            }
            hooks.push(HookDecl {
                id,
                events,
                mode: hook.mode.unwrap_or(HookMode::Observe),
                priority: hook.priority.clamp(-1000, 1000),
            });
        }

        if raw.capabilities.net.len() > MAX_NET_HOSTS {
            return Err(invalid(format!("more than {MAX_NET_HOSTS} net hosts")));
        }
        let net = raw
            .capabilities
            .net
            .iter()
            .map(|host| NetHost::parse(host))
            .collect::<Result<Vec<_>>>()?;
        let storage = raw.capabilities.storage.map(|size| size.0);
        if storage.is_some_and(|bytes| bytes == 0 || bytes > MAX_STORAGE_BYTES) {
            return Err(invalid(
                "`capabilities.storage` must be between 1 B and 64 MiB",
            ));
        }

        Ok(Self {
            id,
            version: raw.version,
            name: raw.name,
            description: raw.description,
            license: raw.license,
            component,
            tools,
            hooks,
            net,
            storage,
        })
    }

    /// The full capability set: declared capabilities plus those implied by
    /// contributions. Read-only events are always observe-only.
    #[must_use]
    pub fn capabilities(&self) -> CapabilitySet {
        let mut set = CapabilitySet::new();
        if !self.tools.is_empty() {
            set.insert(Capability::Tool);
        }
        for hook in &self.hooks {
            for event in &hook.events {
                let mode = if event.is_read_only() {
                    HookMode::Observe
                } else {
                    hook.mode
                };
                set.insert(Capability::Hook {
                    event: *event,
                    mode,
                });
            }
        }
        for host in &self.net {
            set.insert(Capability::Net(host.clone()));
        }
        if let Some(bytes) = self.storage {
            set.insert(Capability::Storage { bytes });
        }
        set
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const DEMO: &str = r#"
schema = 1
id = "acme.echo"
version = "0.1.0"
name = "Echo"
description = "Echoes text back."
license = "MIT"
sdk = "0.1"

[server]
component = "server/echo.wasm"

[capabilities]
net = ["api.github.com"]
storage = "64KiB"

[[contributes.tools]]
name = "echo"
description = "Echo the input"
parameters = { type = "object", properties = { text = { type = "string" } }, required = ["text"] }

[[contributes.hooks]]
id = "audit"
events = ["PreToolUse", "SessionStart"]
mode = "intercept"
"#;

    fn with(replace: &str, by: &str) -> String {
        assert!(DEMO.contains(replace), "{replace}");
        DEMO.replacen(replace, by, 1)
    }

    #[test]
    fn parses_demo_manifest() {
        let manifest = ModManifest::parse(DEMO).unwrap();
        assert_eq!(manifest.id.to_string(), "acme.echo");
        assert_eq!(manifest.component, Some(PathBuf::from("server/echo.wasm")));
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].parameters["required"][0], "text");
        assert_eq!(manifest.hooks[0].events, vec![
            HookEvent::BeforeToolCall,
            HookEvent::SessionStart
        ]);
        assert_eq!(manifest.storage, Some(64 * 1024));
        let caps: Vec<String> = manifest
            .capabilities()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(caps, vec![
            "hook:BeforeToolCall:intercept",
            "hook:SessionStart:observe",
            "net:api.github.com",
            "storage:65536",
            "tool",
        ]);
    }

    #[test]
    fn rejects_unknown_keys_and_exec() {
        assert!(ModManifest::parse(&format!("{DEMO}\nunknown = 1\n")).is_err());
        let exec = with("storage = \"64KiB\"", "storage = \"64KiB\"\nexec = true");
        assert!(ModManifest::parse(&exec).is_err());
    }

    #[test]
    fn rejects_bad_fields() {
        let cases = [
            ("schema = 1", "schema = 2"),
            ("sdk = \"0.1\"", "sdk = \"0.2\""),
            ("id = \"acme.echo\"", "id = \"core.echo\""),
            ("id = \"acme.echo\"", "id = \"Acme\""),
            ("version = \"0.1.0\"", "version = \"1.0\""),
            ("version = \"0.1.0\"", "version = \"1.0.0-\""),
            ("name = \"Echo\"", "name = \"\""),
            (
                "component = \"server/echo.wasm\"",
                "component = \"../evil.wasm\"",
            ),
            (
                "component = \"server/echo.wasm\"",
                "component = \"/etc/evil.wasm\"",
            ),
            (
                "component = \"server/echo.wasm\"",
                "component = \"server\\\\echo.wasm\"",
            ),
            (
                "component = \"server/echo.wasm\"",
                "component = \"server/echo.so\"",
            ),
            ("name = \"echo\"", "name = \"Echo-Tool\""),
            ("net = [\"api.github.com\"]", "net = [\"127.0.0.1\"]"),
            ("storage = \"64KiB\"", "storage = \"1GiB\""),
            ("storage = \"64KiB\"", "storage = 0"),
            ("events = [\"PreToolUse\", \"SessionStart\"]", "events = []"),
            (
                "events = [\"PreToolUse\", \"SessionStart\"]",
                "events = [\"Nope\"]",
            ),
            ("id = \"audit\"", "id = \"Audit Hook\""),
            (
                "parameters = { type = \"object\", properties = { text = { type = \"string\" } }, required = [\"text\"] }",
                "parameters = { type = \"string\" }",
            ),
        ];
        for (from, to) in cases {
            assert!(
                ModManifest::parse(&with(from, to)).is_err(),
                "accepted {to}"
            );
        }
    }

    #[test]
    fn contributions_need_a_component() {
        let text = DEMO.replace("[server]\ncomponent = \"server/echo.wasm\"\n", "");
        let err = ModManifest::parse(&text).unwrap_err().to_string();
        assert!(err.contains("need a `[server] component`"), "{err}");
    }

    #[test]
    fn duplicate_tools_and_hooks_are_rejected() {
        let dup_tool =
            format!("{DEMO}\n[[contributes.tools]]\nname = \"echo\"\ndescription = \"again\"\n");
        assert!(ModManifest::parse(&dup_tool).is_err());
        let dup_hook =
            format!("{DEMO}\n[[contributes.hooks]]\nid = \"audit\"\nevents = [\"Stop\"]\n");
        assert!(ModManifest::parse(&dup_hook).is_err());
    }

    #[test]
    fn read_only_events_are_observe_only() {
        let text = with(
            "events = [\"PreToolUse\", \"SessionStart\"]",
            "events = [\"AfterToolCall\"]",
        );
        let manifest = ModManifest::parse(&text).unwrap();
        assert!(
            manifest
                .capabilities()
                .iter()
                .any(|cap| cap.to_string() == "hook:AfterToolCall:observe")
        );
    }

    #[test]
    fn minimal_manifest_without_server() {
        let manifest = ModManifest::parse(
            "schema = 1\nid = \"acme.theme\"\nversion = \"1.0.0-beta.1\"\nname = \"Theme\"\nsdk = \"0.1\"\n",
        )
        .unwrap();
        assert!(manifest.capabilities().is_empty());
        assert!(manifest.component.is_none());
    }

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("10"), Some(10));
        assert_eq!(parse_size("2KiB"), Some(2048));
        assert_eq!(parse_size("1MiB"), Some(1024 * 1024));
        assert_eq!(parse_size("1GB"), None);
        assert_eq!(parse_size("KiB"), None);
    }
}
