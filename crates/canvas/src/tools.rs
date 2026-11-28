//! Agent tools: `design_create`, `design_open`, `design_screenshot`.
//!
//! Tool names use underscores because provider APIs only accept
//! `[a-zA-Z0-9_-]` in function names. Edits go through the regular
//! Read/Edit/MultiEdit/Write tools on the directory these tools return.

use std::sync::Arc;

use {
    antre_agents::tool_registry::AgentTool,
    antre_browser::RenderFormat,
    async_trait::async_trait,
    base64::{Engine, engine::general_purpose::STANDARD},
    serde::Deserialize,
    serde_json::{Value, json},
};

use crate::{
    CanvasService, DesignId,
    service::{CreateDesign, DesignDetails, Viewport},
};

/// Rules every design follows, returned with `design_create`/`design_open`.
pub const DESIGN_RULES: &str = "Edit the design with Read/Edit/MultiEdit/Write using absolute \
paths inside `dir`; the entry file is index.html. Files are versioned automatically at the end \
of your turn. The page runs in a sandbox with no network: no fetch/XHR/WebSocket, no forms, no \
navigation of the top window, no remote images, fonts or scripts. Inline your CSS and JS or put \
them in files next to index.html; the only shared libraries are the vendored scripts listed in \
`vendor` (reference them as /_vendor/<name>). Put data-mid=\"<stable-id>\" on every semantic \
block (header, section, card, nav item) and keep those ids stable across edits. Call \
design_screenshot to check the result visually.";

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> anyhow::Result<T> {
    serde_json::from_value(params).map_err(|error| anyhow::anyhow!("invalid parameters: {error}"))
}

fn details_json(details: &DesignDetails) -> Value {
    json!({
        "design_id": details.design.id,
        "title": details.design.title,
        "dir": details.dir,
        "entry": details.entry,
        "current_version": details.design.current_version,
        "design_system": details.design.design_system,
        "files": details.files,
        "versions": details.versions.iter().map(|version| json!({
            "version": version.version,
            "reason": version.reason,
            "created_at": version.created_at,
        })).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize)]
struct CreateParams {
    title: String,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    design_system: Option<String>,
    #[serde(default, rename = "_session_key")]
    session_key: Option<String>,
    #[serde(default, rename = "_run_id")]
    run_id: Option<String>,
}

/// Creates a design directory with an `index.html`.
pub struct DesignCreateTool {
    service: Arc<CanvasService>,
}

impl DesignCreateTool {
    #[must_use]
    pub fn new(service: Arc<CanvasService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AgentTool for DesignCreateTool {
    fn name(&self) -> &str {
        "design_create"
    }

    fn description(&self) -> &str {
        "Create a new visual design (an HTML page with its assets) that the user sees in a live \
         preview. Returns the directory to edit with Read/Edit/Write. Use for mockups, landing \
         pages, UI screens, slides or one-pagers. Optionally apply a design system."
    }

    fn parameters_schema(&self) -> Value {
        let systems: Vec<String> = self
            .service
            .catalog()
            .list()
            .into_iter()
            .map(|info| info.name)
            .collect();
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short human-readable name of the design."},
                "html": {"type": "string", "description": "Optional full initial index.html. When omitted, a blank page is created."},
                "design_system": {
                    "type": "string",
                    "description": format!(
                        "Optional design system to apply (copies tokens.css and returns its DESIGN.md). Available: {}.",
                        if systems.is_empty() { "none".to_string() } else { systems.join(", ") }
                    ),
                },
            },
            "required": ["title"],
        })
    }

    async fn execute(&self, params: Value) -> anyhow::Result<Value> {
        let params: CreateParams = parse(params)?;
        let created = self
            .service
            .create(CreateDesign {
                title: params.title,
                html: params.html,
                design_system: params.design_system,
                session_key: params.session_key,
                project_id: None,
                run_id: params.run_id,
            })
            .await?;
        let vendor = self.service.vendor.list().await;
        Ok(json!({
            "design_id": created.design.id,
            "title": created.design.title,
            "dir": created.dir,
            "entry": created.dir.join(crate::export::ENTRY_FILE),
            "version": created.version.version,
            "design_system": created.design_system.as_ref().map(|system| json!({
                "name": system.info.name,
                "tokens_file": created.dir.join(crate::design_systems::TOKENS_FILE),
                "guide": system.guide,
            })),
            "vendor": vendor,
            "rules": DESIGN_RULES,
        }))
    }
}

#[derive(Deserialize)]
struct OpenParams {
    design_id: String,
    #[serde(default, rename = "_session_key")]
    session_key: Option<String>,
    #[serde(default, rename = "_run_id")]
    run_id: Option<String>,
}

/// Opens an existing design for editing.
pub struct DesignOpenTool {
    service: Arc<CanvasService>,
}

impl DesignOpenTool {
    #[must_use]
    pub fn new(service: Arc<CanvasService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AgentTool for DesignOpenTool {
    fn name(&self) -> &str {
        "design_open"
    }

    fn description(&self) -> &str {
        "Open an existing design: returns its directory, files, recent versions and design \
         system guidance so you can continue editing it with Read/Edit/Write."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "design_id": {"type": "string", "description": "Id returned by design_create."},
            },
            "required": ["design_id"],
        })
    }

    async fn execute(&self, params: Value) -> anyhow::Result<Value> {
        let params: OpenParams = parse(params)?;
        let id: DesignId = params.design_id.parse()?;
        let details = self.service.open(&id).await?;
        if let Some(session_key) = params.session_key.as_deref()
            && let Some(previous) = self
                .service
                .touch(session_key, params.run_id.as_deref(), &id)
        {
            self.service.commit_turn(previous).await;
        }
        let guide = match details.design.design_system.as_deref() {
            Some(name) => self
                .service
                .catalog()
                .get(name)
                .ok()
                .map(|system| system.guide),
            None => None,
        };
        let mut result = details_json(&details);
        result["design_system_guide"] = json!(guide);
        result["vendor"] = json!(self.service.vendor.list().await);
        result["rules"] = json!(DESIGN_RULES);
        Ok(result)
    }
}

#[derive(Deserialize)]
struct ScreenshotParams {
    design_id: String,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    full_page: Option<bool>,
}

/// Renders a design to PNG so the agent can check its work.
pub struct DesignScreenshotTool {
    service: Arc<CanvasService>,
}

impl DesignScreenshotTool {
    #[must_use]
    pub fn new(service: Arc<CanvasService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AgentTool for DesignScreenshotTool {
    fn name(&self) -> &str {
        "design_screenshot"
    }

    fn description(&self) -> &str {
        "Render a design to a PNG screenshot (current files, or a past version) to check layout, \
         spacing and visual quality. Use width 390 for mobile, 768 for tablet, 1280 for desktop."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "design_id": {"type": "string"},
                "version": {"type": "integer", "minimum": 1, "description": "Past version to render. Default: current files."},
                "width": {"type": "integer", "minimum": 200, "maximum": 3840, "description": "Viewport width in CSS pixels (default 1280)."},
                "height": {"type": "integer", "minimum": 200, "maximum": 4000, "description": "Viewport height (default 800)."},
                "full_page": {"type": "boolean", "description": "Capture the whole page instead of the viewport (default true)."},
            },
            "required": ["design_id"],
        })
    }

    async fn execute(&self, params: Value) -> anyhow::Result<Value> {
        let params: ScreenshotParams = parse(params)?;
        let id: DesignId = params.design_id.parse()?;
        let viewport = Viewport {
            width: params.width.unwrap_or(1280).clamp(200, 3840),
            height: params.height.unwrap_or(800).clamp(200, 4000),
            full_page: params.full_page.unwrap_or(true),
        };
        let png = self
            .service
            .render(&id, params.version, RenderFormat::Png, viewport)
            .await?;
        Ok(json!({
            "design_id": id,
            "version": params.version,
            "width": viewport.width,
            "height": viewport.height,
            "screenshot": format!("data:image/png;base64,{}", STANDARD.encode(png)),
        }))
    }
}

/// The three design tools, ready to register.
#[must_use]
pub fn design_tools(service: &Arc<CanvasService>) -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(DesignCreateTool::new(Arc::clone(service))),
        Box::new(DesignOpenTool::new(Arc::clone(service))),
        Box::new(DesignScreenshotTool::new(Arc::clone(service))),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, crate::service::tests::service_with};

    #[tokio::test]
    async fn create_open_and_screenshot() {
        let (_tmp, service, _sink) = service_with(None).await;
        let tools = design_tools(&service);
        let names: Vec<_> = tools.iter().map(|tool| tool.name().to_string()).collect();
        assert_eq!(names, vec![
            "design_create",
            "design_open",
            "design_screenshot"
        ]);
        for tool in &tools {
            assert!(
                tool.name()
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "provider-safe tool name"
            );
            assert_eq!(tool.parameters_schema()["type"], "object");
        }

        let created = tools[0]
            .execute(json!({
                "title": "Pricing",
                "design_system": "minimal",
                "_session_key": "main",
                "_run_id": "r1",
            }))
            .await
            .unwrap();
        let id = created["design_id"].as_str().unwrap().to_string();
        assert_eq!(created["version"], 1);
        assert!(
            created["design_system"]["guide"]
                .as_str()
                .unwrap()
                .contains("Minimal")
        );
        assert!(created["rules"].as_str().unwrap().contains("data-mid"));
        let dir = created["dir"].as_str().unwrap();
        assert!(std::path::Path::new(dir).join("tokens.css").exists());
        assert!(
            tools[0].parameters_schema()["properties"]["design_system"]["description"]
                .as_str()
                .unwrap()
                .contains("minimal")
        );

        let opened = tools[1]
            .execute(json!({"design_id": id, "_session_key": "main"}))
            .await
            .unwrap();
        assert_eq!(opened["current_version"], 1);
        assert_eq!(opened["design_system"], "minimal");
        assert!(opened["design_system_guide"].as_str().is_some());
        assert_eq!(opened["files"].as_array().unwrap().len(), 2);

        let shot = tools[2]
            .execute(json!({"design_id": id, "width": 390}))
            .await
            .unwrap();
        assert!(
            shot["screenshot"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(shot["width"], 390);
    }

    #[tokio::test]
    async fn screenshot_requests_offline_document() {
        let renderer = Arc::new(crate::render::tests::FakeRenderer::default());
        let (_tmp, service, _sink) =
            service_with(Some(Arc::clone(&renderer) as Arc<dyn crate::DesignRenderer>)).await;
        let tools = design_tools(&service);
        let created = tools[0].execute(json!({"title": "x"})).await.unwrap();
        tools[2]
            .execute(json!({"design_id": created["design_id"], "width": 10}))
            .await
            .unwrap();
        let requests = renderer.requests.lock().unwrap();
        assert_eq!(requests[0].width, 200, "width is clamped");
        assert!(requests[0].html.contains("connect-src 'none'"));
    }

    #[tokio::test]
    async fn rejects_bad_parameters() {
        let (_tmp, service, _sink) = service_with(None).await;
        let tools = design_tools(&service);
        assert!(tools[0].execute(json!({})).await.is_err());
        assert!(
            tools[0]
                .execute(json!({"title": "x", "design_system": "nope"}))
                .await
                .is_err()
        );
        assert!(
            tools[1]
                .execute(json!({"design_id": "../etc"}))
                .await
                .is_err()
        );
        assert!(
            tools[1]
                .execute(json!({"design_id": DesignId::generate()}))
                .await
                .is_err()
        );
    }
}
