use super::{heartbeat_patch::apply_heartbeat_patch, *};

/// Cron, webhook and heartbeat methods.
pub(super) fn register(reg: &mut MethodRegistry) {
    // Cron
    reg.register(
        "cron.list",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .list()
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "cron.status",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .status()
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "cron.add",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .add(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "cron.update",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .update(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "cron.remove",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .remove(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "cron.run",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .run(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "cron.runs",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .cron
                    .runs(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    // Webhooks
    reg.register(
        "webhooks.list",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .list()
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.get",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .get(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.create",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .create(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.update",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .update(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.delete",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .delete(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.deliveries",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .deliveries(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.delivery.get",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .delivery_get(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.delivery.payload",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .delivery_payload(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.delivery.actions",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .delivery_actions(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "webhooks.profiles",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .webhooks
                    .profiles()
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    // Heartbeat
    reg.register(
        "heartbeat.status",
        Box::new(|ctx| {
            Box::pin(async move {
                let config = ctx.state.inner.read().await.heartbeat_config.clone();
                let heartbeat_path = antre_config::heartbeat_path();
                let heartbeat_file_exists = heartbeat_path.exists();
                let heartbeat_md = antre_config::load_heartbeat_md();
                let (_, prompt_source) = antre_cron::heartbeat::resolve_heartbeat_prompt(
                    config.prompt.as_deref(),
                    heartbeat_md.as_deref(),
                );
                // No meaningful prompt → heartbeat won't execute.
                let has_prompt =
                    prompt_source != antre_cron::heartbeat::HeartbeatPromptSource::Default;
                // Find the heartbeat job to get its state.
                let jobs_val = ctx
                    .state
                    .services
                    .cron
                    .list()
                    .await
                    .map_err(ErrorShape::from)?;
                let jobs: Vec<antre_cron::types::CronJob> =
                    serde_json::from_value(jobs_val).unwrap_or_default();
                let hb_job = jobs.iter().find(|j| j.name == "__heartbeat__");
                Ok(serde_json::json!({
                    "config": config,
                    "job": hb_job,
                    "promptSource": prompt_source.as_str(),
                    "heartbeatFileExists": heartbeat_file_exists,
                    "hasPrompt": has_prompt,
                }))
            })
        }),
    );
    reg.register(
            "heartbeat.update",
            Box::new(|ctx| {
                Box::pin(async move {
                    let patch = {
                        let mut state = ctx.state.inner.write().await;
                        let patch = apply_heartbeat_patch(&state.heartbeat_config, &ctx.params)
                            .map_err(|e| {
                                ErrorShape::new(
                                    error_codes::INVALID_REQUEST,
                                    format!("invalid heartbeat config: {e}"),
                                )
                            })?;
                        state.heartbeat_config = patch.clone();
                        patch
                    };

                    // Persist to antre.toml so the config survives restarts.
                    if let Err(e) = antre_config::update_config(|cfg| {
                        cfg.heartbeat = patch.clone();
                    }) {
                        tracing::warn!(error = %e, "failed to persist heartbeat config");
                    }

                    // Update the heartbeat cron job in-place.
                    let jobs_val = ctx
                        .state
                        .services
                        .cron
                        .list()
                        .await
                        .map_err(ErrorShape::from)?;
                    let jobs: Vec<antre_cron::types::CronJob> =
                        serde_json::from_value(jobs_val).unwrap_or_default();
                    let interval_ms = antre_cron::heartbeat::parse_interval_ms(&patch.every)
                        .unwrap_or(antre_cron::heartbeat::DEFAULT_INTERVAL_MS);
                    let heartbeat_md = antre_config::load_heartbeat_md();
                    let (prompt, prompt_source) =
                        antre_cron::heartbeat::resolve_heartbeat_prompt(
                            patch.prompt.as_deref(),
                            heartbeat_md.as_deref(),
                        );
                    if prompt_source
                        == antre_cron::heartbeat::HeartbeatPromptSource::HeartbeatMd
                    {
                        tracing::info!("loaded heartbeat prompt from HEARTBEAT.md");
                    }
                    if patch.prompt.as_deref().is_some_and(|p| !p.trim().is_empty())
                        && heartbeat_md.as_deref().is_some_and(|p| !p.trim().is_empty())
                        && prompt_source
                            == antre_cron::heartbeat::HeartbeatPromptSource::Config
                    {
                        tracing::warn!(
                            "heartbeat prompt source conflict: config heartbeat.prompt overrides HEARTBEAT.md"
                        );
                    }
                    // Disable the job when there is no meaningful prompt,
                    // even if the user toggled enabled=true.
                    let has_prompt = prompt_source
                        != antre_cron::heartbeat::HeartbeatPromptSource::Default;
                    let effective_enabled = patch.enabled && has_prompt;

                    if let Some(hb_job) = jobs.iter().find(|j| j.id == "__heartbeat__") {
                        let job_patch = antre_cron::types::CronJobPatch {
                            schedule: Some(antre_cron::types::CronSchedule::Every {
                                every_ms: interval_ms,
                                anchor_ms: None,
                            }),
                            payload: Some(antre_cron::types::CronPayload::AgentTurn {
                                message: prompt,
                                model: patch.model.clone(),
                                agent_id: patch.agent_id.clone(),
                                timeout_secs: None,
                                tool_controls: Default::default(),
                                deliver: patch.deliver,
                                channel: patch.channel.clone(),
                                to: patch.to.clone(),
                            }),
                            enabled: Some(effective_enabled),
                            sandbox: Some(antre_cron::types::CronSandboxConfig {
                                enabled: patch.sandbox_enabled,
                                image: patch.sandbox_image.clone(),
                                auto_prune_container: None,
                            }),
                            ..Default::default()
                        };
                        ctx.state
                            .services
                            .cron
                            .update(serde_json::json!({
                                "id": hb_job.id,
                                "patch": job_patch,
                            }))
                            .await
                            .map_err(ErrorShape::from)?;
                    } else if effective_enabled {
                        // Create the heartbeat job only when enabled with a valid prompt.
                        let create = antre_cron::types::CronJobCreate {
                            id: Some("__heartbeat__".into()),
                            name: "__heartbeat__".into(),
                            schedule: antre_cron::types::CronSchedule::Every {
                                every_ms: interval_ms,
                                anchor_ms: None,
                            },
                            payload: antre_cron::types::CronPayload::AgentTurn {
                                message: prompt,
                                model: patch.model.clone(),
                                agent_id: patch.agent_id.clone(),
                                timeout_secs: None,
                                tool_controls: Default::default(),
                                deliver: patch.deliver,
                                channel: patch.channel.clone(),
                                to: patch.to.clone(),
                            },
                            session_target: antre_cron::types::SessionTarget::Named("heartbeat".into()),
                            delete_after_run: false,
                            enabled: effective_enabled,
                            system: true,
                            sandbox: antre_cron::types::CronSandboxConfig {
                                enabled: patch.sandbox_enabled,
                                image: patch.sandbox_image.clone(),
                                auto_prune_container: None,
                            },
                            wake_mode: antre_cron::types::CronWakeMode::default(),
                        };
                        let create_json = serde_json::to_value(create)
                            .map_err(|e| ErrorShape::new(error_codes::INVALID_REQUEST, format!("failed to serialize job: {e}")))?;
                        ctx.state
                            .services
                            .cron
                            .add(create_json)
                            .await
                            .map_err(ErrorShape::from)?;
                    }
                    Ok(serde_json::json!({ "updated": true }))
                })
            }),
        );
    reg.register(
        "heartbeat.run",
        Box::new(|ctx| {
            Box::pin(async move {
                let jobs_val = ctx
                    .state
                    .services
                    .cron
                    .list()
                    .await
                    .map_err(ErrorShape::from)?;
                let jobs: Vec<antre_cron::types::CronJob> =
                    serde_json::from_value(jobs_val).unwrap_or_default();
                let hb_job = jobs
                    .iter()
                    .find(|j| j.name == "__heartbeat__")
                    .ok_or_else(|| {
                        ErrorShape::new(error_codes::INVALID_REQUEST, "heartbeat job not found")
                    })?;
                ctx.state
                    .services
                    .cron
                    .run(serde_json::json!({
                        "id": hb_job.id,
                        "force": true,
                    }))
                    .await
                    .map_err(ErrorShape::from)?;
                Ok(serde_json::json!({ "triggered": true }))
            })
        }),
    );
    reg.register(
        "heartbeat.runs",
        Box::new(|ctx| {
            Box::pin(async move {
                let jobs_val = ctx
                    .state
                    .services
                    .cron
                    .list()
                    .await
                    .map_err(ErrorShape::from)?;
                let jobs: Vec<antre_cron::types::CronJob> =
                    serde_json::from_value(jobs_val).unwrap_or_default();
                let hb_job = jobs
                    .iter()
                    .find(|j| j.name == "__heartbeat__")
                    .ok_or_else(|| {
                        ErrorShape::new(error_codes::INVALID_REQUEST, "heartbeat job not found")
                    })?;
                let limit = ctx
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20);
                ctx.state
                    .services
                    .cron
                    .runs(serde_json::json!({
                        "id": hb_job.id,
                        "limit": limit,
                    }))
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
}
