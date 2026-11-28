//! `mcp.*` RPC methods for resources, prompts, elicitation and MCP Apps.

use antre_protocol::ErrorShape;

use super::MethodRegistry;

/// Register one `mcp.*` method that forwards its params to an `McpService` call.
macro_rules! forward {
    ($reg:expr, $name:literal, $method:ident) => {
        $reg.register(
            $name,
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .mcp
                        .$method(ctx.params.clone())
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
    };
}

/// Method names, also used by the scope tables in `dispatch.rs`.
pub(crate) const MCP_FEATURE_READ_METHODS: &[&str] = &[
    "mcp.features",
    "mcp.resources.list",
    "mcp.resources.read",
    "mcp.prompts.list",
    "mcp.prompts.get",
    "mcp.apps.resource",
];

pub(crate) const MCP_FEATURE_WRITE_METHODS: &[&str] =
    &["mcp.resources.subscribe", "mcp.resources.unsubscribe"];

pub(super) fn register(reg: &mut MethodRegistry) {
    forward!(reg, "mcp.features", features);
    forward!(reg, "mcp.resources.list", resources_list);
    forward!(reg, "mcp.resources.read", resource_read);
    forward!(reg, "mcp.resources.subscribe", resource_subscribe);
    forward!(reg, "mcp.resources.unsubscribe", resource_unsubscribe);
    forward!(reg, "mcp.prompts.list", prompts_list);
    forward!(reg, "mcp.prompts.get", prompt_get);
    forward!(reg, "mcp.elicitation.list", elicitation_list);
    forward!(reg, "mcp.elicitation.resolve", elicitation_resolve);
    forward!(reg, "mcp.apps.resource", ui_resource_read);
}
