use super::*;

pub(super) fn validate_instrumentation(
    config: &crate::schema::InstrumentationConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !config.sample_rate.is_finite() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            category: "invalid-value",
            path: "instrumentation.sample_rate".into(),
            message: "instrumentation.sample_rate must be finite and between 0.0 and 1.0".into(),
        });
    } else if !(0.0..=1.0).contains(&config.sample_rate) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            category: "invalid-value",
            path: "instrumentation.sample_rate".into(),
            message: format!(
                "instrumentation.sample_rate must be between 0.0 and 1.0 (got {})",
                config.sample_rate
            ),
        });
    }

    let zero_values = [
        (config.queue_capacity == 0, "instrumentation.queue_capacity"),
        (
            config.flush_interval_ms == 0,
            "instrumentation.flush_interval_ms",
        ),
        (
            config.max_batch_bytes == 0,
            "instrumentation.max_batch_bytes",
        ),
        (
            config.langfuse.timeout_secs == 0,
            "instrumentation.langfuse.timeout_secs",
        ),
        (
            config.otlp.timeout_secs == 0,
            "instrumentation.otlp.timeout_secs",
        ),
        (
            config.datadog.timeout_secs == 0,
            "instrumentation.datadog.timeout_secs",
        ),
        (
            config.feedback.link_retention_days == 0,
            "instrumentation.feedback.link_retention_days",
        ),
    ];
    diagnostics.extend(
        zero_values
            .into_iter()
            .filter(|(is_zero, _)| *is_zero)
            .map(|(_, path)| Diagnostic {
                severity: Severity::Error,
                category: "invalid-value",
                path: path.into(),
                message: format!("{path} must be at least 1"),
            }),
    );

    let unusually_large = [
        (
            config.queue_capacity > 1_000_000,
            "instrumentation.queue_capacity",
            "a queue above 1,000,000 events can consume excessive memory",
        ),
        (
            config.flush_interval_ms > 300_000,
            "instrumentation.flush_interval_ms",
            "an interval above 5 minutes delays delivery and increases shutdown loss risk",
        ),
        (
            config.max_batch_bytes > 100_000_000,
            "instrumentation.max_batch_bytes",
            "a batch above 100 MB can exceed collector and proxy request limits",
        ),
        (
            config.langfuse.timeout_secs > 300,
            "instrumentation.langfuse.timeout_secs",
            "a request timeout above 5 minutes can delay retries and shutdown",
        ),
        (
            config.otlp.timeout_secs > 300,
            "instrumentation.otlp.timeout_secs",
            "a request timeout above 5 minutes can delay retries and shutdown",
        ),
        (
            config.datadog.timeout_secs > 300,
            "instrumentation.datadog.timeout_secs",
            "a request timeout above 5 minutes can delay retries and shutdown",
        ),
    ];
    diagnostics.extend(
        unusually_large
            .into_iter()
            .filter(|(is_large, ..)| *is_large)
            .map(|(_, path, message)| Diagnostic {
                severity: Severity::Warning,
                category: "invalid-value",
                path: path.into(),
                message: message.into(),
            }),
    );
}

/// Validate a `context_window` override value (optional field).
pub(super) fn validate_context_window(
    value: Option<u32>,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(cw) = value else {
        return;
    };
    if cw == 0 {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            category: "invalid-value",
            path: path.into(),
            message: "context_window must be at least 1".into(),
        });
    } else if cw > 10_000_000 {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            category: "invalid-value",
            path: path.into(),
            message: format!("context_window is {cw}, which is unusually large (> 10M)"),
        });
    }
}

/// Warn about literal (non-env-var) API keys stored directly in the config.
///
/// API keys should be supplied via environment variables (e.g. `${{ANTHROPIC_API_KEY}}`)
/// or stored in the credential store (`provider_keys.json`), not hard-coded in
/// `antre.toml`.  The config file may be backed up, synced, or accidentally
/// committed to version control.
pub(super) fn check_catalog(
    catalog: &crate::schema::CatalogConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, url) in catalog.refresh_urls.iter().enumerate() {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                category: "invalid-value",
                path: format!("catalog.refresh_urls[{index}]"),
                message: format!(
                    "catalog.refresh_urls entries must be http(s) URLs (got \"{url}\")"
                ),
            });
        }
    }
    if catalog.refresh && catalog.refresh_urls.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            category: "invalid-value",
            path: "catalog.refresh_urls".into(),
            message: "catalog.refresh is on but catalog.refresh_urls is empty; set refresh = false to keep the bundled snapshot".into(),
        });
    }
    if catalog.refresh && catalog.refresh_interval_hours == 0 {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            category: "invalid-value",
            path: "catalog.refresh_interval_hours".into(),
            message: "catalog.refresh_interval_hours = 0 downloads the catalog on every check; use at least 1".into(),
        });
    }
}

pub(super) fn check_mcp_sampling(
    name: &str,
    server: &crate::schema::McpServerEntry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let sampling = &server.sampling;
    if !sampling.enabled {
        return;
    }
    match server.trust {
        crate::schema::McpTrustTier::Untrusted => diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            category: "invalid-value",
            path: format!("mcp.servers.{name}.sampling.enabled"),
            message:
                "sampling is never allowed for untrusted MCP servers; this setting has no effect"
                    .into(),
        }),
        crate::schema::McpTrustTier::Trusted | crate::schema::McpTrustTier::Ask => {},
    }
    let zero_limits = [
        (
            "max_tokens_per_request",
            u64::from(sampling.max_tokens_per_request),
        ),
        (
            "max_requests_per_session",
            u64::from(sampling.max_requests_per_session),
        ),
        ("max_tokens_per_session", sampling.max_tokens_per_session),
    ];
    for (field, value) in zero_limits {
        if value == 0 {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                category: "invalid-value",
                path: format!("mcp.servers.{name}.sampling.{field}"),
                message: format!("{field} = 0 rejects every sampling request"),
            });
        }
    }
}
