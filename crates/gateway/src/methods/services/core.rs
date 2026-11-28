use super::*;

/// Strip gateway-owned routing and trust markers from RPC-supplied params.
///
/// The list is owned by `moltis-chat` (the crate that reads these) so it cannot
/// drift from what the chat service treats as a gateway assertion.
use moltis_chat::request_params::strip_gateway_owned_params as strip_internal_channel_fields;

/// Prepare client-supplied `chat.send`/`chat.send_sync` params for the chat
/// service: drop anything the client must not be able to claim, then inject the
/// connection's own context.
///
/// The `_`-prefixed params are internal plumbing, and some of them are set
/// legitimately by the web UI (`_session_key`, `_audio_filename`,
/// `_document_files`), so this is a targeted removal rather than a blanket
/// strip. See [`moltis_chat::params::SERVER_ONLY`] and
/// [`moltis_chat::request_params::GATEWAY_OWNED_REQUEST_PARAMS`] for which keys
/// are dropped and why; the server re-derives each of them from state it trusts.
///
/// Only the channel dispatch path sets those keys, and it calls the chat service
/// directly rather than through this registry, so stripping here cannot affect
/// it.
async fn prepare_chat_send_params(ctx: &MethodContext) -> serde_json::Value {
    let mut params = ctx.params.clone();
    moltis_chat::params::strip_server_only(&mut params);
    strip_internal_channel_fields(&mut params);
    params["_conn_id"] = serde_json::json!(ctx.client_conn_id);

    // Forward client Accept-Language, public remote IP, and timezone.
    let registry = ctx.state.client_registry.read().await;
    if let Some(client) = registry.clients.get(&ctx.client_conn_id) {
        if let Some(ref lang) = client.accept_language {
            params["_accept_language"] = serde_json::json!(lang);
        }
        if let Some(ref ip) = client.remote_ip {
            params["_remote_ip"] = serde_json::json!(ip);
        }
        if let Some(ref tz) = client.timezone {
            params["_timezone"] = serde_json::json!(tz);
        }
    }
    drop(registry);

    // Ensure the target session has a persisted agent_id (runtime default)
    // before chat.send builds the prompt — never leave NULL → hardcoded "main".
    // Ephemeral/throwaway questions deliberately skip session writes in chat;
    // do not create or bind saved sessions for them here either.
    let ephemeral = params
        .get("_ephemeral")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !ephemeral {
        let session_key = params
            .get("_session_key")
            .or_else(|| params.get("sessionKey"))
            .or_else(|| params.get("session_key"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let session_key = match session_key {
            Some(key) => Some(key),
            None => active_session_key_for_ctx(ctx).await,
        };
        if let Some(session_key) = session_key
            && let Some(ref metadata) = ctx.state.services.session_metadata
        {
            let _ = metadata.upsert(&session_key, None).await;
            let needs_bind = metadata
                .get(&session_key)
                .await
                .map(|entry| {
                    entry
                        .agent_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_none()
                })
                .unwrap_or(true);
            if needs_bind {
                let default_id = default_agent_id_for_ctx(ctx).await;
                let _ = metadata.set_agent_id(&session_key, Some(&default_id)).await;
            }
        }
    }

    params
}

pub(super) fn register(reg: &mut MethodRegistry) {
    // Config
    reg.register(
        "config.get",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .config
                    .get(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "config.set",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .config
                    .set(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "config.apply",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .config
                    .apply(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "config.patch",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .config
                    .patch(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "config.schema",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .config
                    .schema()
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    automation::register(reg);

    // Chat (uses chat_override if set, otherwise falls back to services.chat).
    // See `prepare_chat_send_params` for what is stripped and injected.
    reg.register(
        "chat.send",
        Box::new(|ctx| {
            Box::pin(async move {
                let params = prepare_chat_send_params(&ctx).await;
                ctx.state
                    .chat()
                    .send(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.send_sync",
        Box::new(|ctx| {
            Box::pin(async move {
                let params = prepare_chat_send_params(&ctx).await;
                ctx.state
                    .chat()
                    .send_sync(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.abort",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .chat()
                    .abort(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    reg.register(
        "external_agents.list",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .external_agent
                    .list()
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "external_agents.bind",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .external_agent
                    .bind(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "external_agents.unbind",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .external_agent
                    .unbind(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "external_agents.status",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .services
                    .external_agent
                    .status(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.peek",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .chat()
                    .peek(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.cancel_queued",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .chat()
                    .cancel_queued(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.steer",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .chat()
                    .steer(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.history",
        Box::new(|ctx| {
            Box::pin(async move {
                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                ctx.state
                    .chat()
                    .history(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.inject",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .chat()
                    .inject(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.clear",
        Box::new(|ctx| {
            Box::pin(async move {
                // Export the session before the clear destroys its history.
                if let Some(session_key) = active_session_key_for_ctx(&ctx).await {
                    let hooks = ctx.state.inner.read().await.hook_registry.clone();
                    if let Some(ref hooks) = hooks {
                        crate::session::dispatch_command_hook(hooks, &session_key, "reset", None)
                            .await;
                    }
                }

                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                ctx.state
                    .chat()
                    .clear(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );
    reg.register(
        "chat.compact",
        Box::new(|ctx| {
            Box::pin(async move {
                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                ctx.state
                    .chat()
                    .compact(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    reg.register(
        "chat.cancel_quota_wait",
        Box::new(|ctx| {
            Box::pin(async move {
                ctx.state
                    .chat()
                    .cancel_quota_wait(ctx.params.clone())
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    reg.register(
        "chat.context",
        Box::new(|ctx| {
            Box::pin(async move {
                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                ctx.state
                    .chat()
                    .context(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    reg.register(
        "chat.raw_prompt",
        Box::new(|ctx| {
            Box::pin(async move {
                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                // Forward client Accept-Language, public remote IP, and timezone.
                {
                    let registry = ctx.state.client_registry.read().await;
                    if let Some(client) = registry.clients.get(&ctx.client_conn_id) {
                        if let Some(ref lang) = client.accept_language {
                            params["_accept_language"] = serde_json::json!(lang);
                        }
                        if let Some(ref ip) = client.remote_ip {
                            params["_remote_ip"] = serde_json::json!(ip);
                        }
                        if let Some(ref tz) = client.timezone {
                            params["_timezone"] = serde_json::json!(tz);
                        }
                    }
                }
                ctx.state
                    .chat()
                    .raw_prompt(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    reg.register(
        "chat.full_context",
        Box::new(|ctx| {
            Box::pin(async move {
                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                // Forward client Accept-Language, public remote IP, and timezone.
                {
                    let registry = ctx.state.client_registry.read().await;
                    if let Some(client) = registry.clients.get(&ctx.client_conn_id) {
                        if let Some(ref lang) = client.accept_language {
                            params["_accept_language"] = serde_json::json!(lang);
                        }
                        if let Some(ref ip) = client.remote_ip {
                            params["_remote_ip"] = serde_json::json!(ip);
                        }
                        if let Some(ref tz) = client.timezone {
                            params["_timezone"] = serde_json::json!(tz);
                        }
                    }
                }
                ctx.state
                    .chat()
                    .full_context(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    reg.register(
        "chat.prompt_memory.refresh",
        Box::new(|ctx| {
            Box::pin(async move {
                let mut params = ctx.params.clone();
                params["_conn_id"] = serde_json::json!(ctx.client_conn_id);
                ctx.state
                    .chat()
                    .refresh_prompt_memory(params)
                    .await
                    .map_err(ErrorShape::from)
            })
        }),
    );

    // Session switching
    reg.register(
        "sessions.switch",
        Box::new(|ctx| {
            Box::pin(async move {
                let key = ctx
                    .params
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ErrorShape::new(error_codes::INVALID_REQUEST, "missing 'key' parameter")
                    })?;
                let include_history = ctx
                    .params
                    .get("include_history")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let previous_active_key = {
                    let registry = ctx.state.client_registry.read().await;
                    registry.active_sessions.get(&ctx.client_conn_id).cloned()
                };
                let was_existing_session =
                    if let Some(ref metadata) = ctx.state.services.session_metadata {
                        metadata.get(key).await.is_some()
                    } else {
                        false
                    };

                // Store the active session (and project if provided) for this connection.
                {
                    let mut registry = ctx.state.client_registry.write().await;
                    registry
                        .active_sessions
                        .insert(ctx.client_conn_id.clone(), key.to_string());

                    if let Some(project_id) = ctx.params.get("project_id").and_then(|v| v.as_str())
                    {
                        if project_id.is_empty() {
                            registry.active_projects.remove(&ctx.client_conn_id);
                        } else {
                            registry
                                .active_projects
                                .insert(ctx.client_conn_id.clone(), project_id.to_string());
                        }
                    }
                }

                // Resolve first (auto-creates session if needed), then
                // persist project_id so the entry exists when we patch.
                // New Chat must bind the runtime default agent, not inherit the
                // previous session's agent. Explicit fork (`sessions.fork`) and
                // callers that pass `inherit_agent_from` keep inheritance.
                let mut resolve_params = serde_json::json!({
                    "key": key,
                    "include_history": include_history,
                });
                if let Some(inherit_from) = ctx
                    .params
                    .get("inherit_agent_from")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    resolve_params["inherit_agent_from"] = serde_json::json!(inherit_from);
                }
                let result = ctx
                    .state
                    .services
                    .session
                    .resolve(resolve_params)
                    .await
                    .map_err(|e| {
                        tracing::error!("session resolve failed: {e}");
                        ErrorShape::new(
                            error_codes::UNAVAILABLE,
                            format!("session resolve failed: {e}"),
                        )
                    })?;

                // Mark the session as seen so unread state clears.
                ctx.state.services.session.mark_seen(key).await;

                // Export the previous session when the user creates a brand-new
                // session (e.g. "+" button or /new).  Switching between two
                // *existing* sessions intentionally skips export — only new-
                // session creation signals the end of the previous conversation.
                if !was_existing_session
                    && let Some(prev_key) = previous_active_key.as_deref().filter(|pk| *pk != key)
                {
                    let hooks = ctx.state.inner.read().await.hook_registry.clone();
                    if let Some(ref hooks) = hooks {
                        crate::session::dispatch_command_hook(hooks, prev_key, "new", None).await;
                    }
                }

                if let Some(pid) = ctx.params.get("project_id").and_then(|v| v.as_str()) {
                    let _ = ctx
                        .state
                        .services
                        .session
                        .patch(serde_json::json!({ "key": key, "project_id": pid }))
                        .await;

                    // Auto-create worktree if project has auto_worktree enabled.
                    if let Ok(proj_val) = ctx
                        .state
                        .services
                        .project
                        .get(serde_json::json!({"id": pid}))
                        .await
                        && proj_val
                            .get("auto_worktree")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        && let Some(dir) = proj_val.get("directory").and_then(|v| v.as_str())
                    {
                        let project_dir = Path::new(dir);
                        let create_result =
                            match moltis_projects::WorktreeManager::resolve_base_branch(project_dir)
                                .await
                            {
                                Ok(base) => {
                                    moltis_projects::WorktreeManager::create_from_base(
                                        project_dir,
                                        key,
                                        &base,
                                    )
                                    .await
                                },
                                Err(_) => {
                                    moltis_projects::WorktreeManager::create(project_dir, key).await
                                },
                            };
                        match create_result {
                            Ok(wt_dir) => {
                                let prefix = proj_val
                                    .get("branch_prefix")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("moltis");
                                let branch = format!("{prefix}/{key}");
                                let _ = ctx
                                    .state
                                    .services
                                    .session
                                    .patch(serde_json::json!({
                                        "key": key,
                                        "worktree_branch": branch,
                                    }))
                                    .await;

                                if let Err(e) = moltis_projects::worktree::copy_project_config(
                                    project_dir,
                                    &wt_dir,
                                ) {
                                    tracing::warn!("failed to copy project config: {e}");
                                }

                                if let Some(cmd) = proj_val
                                    .get("setup_command")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    && let Err(e) = moltis_projects::WorktreeManager::run_setup(
                                        &wt_dir,
                                        cmd,
                                        project_dir,
                                        key,
                                    )
                                    .await
                                {
                                    tracing::warn!("worktree setup failed: {e}");
                                }
                            },
                            Err(e) => {
                                tracing::warn!("auto-create worktree failed: {e}");
                            },
                        }
                    }
                }

                // If the client already has a cached history with the same
                // message count, skip sending the full history to avoid
                // transferring megabytes of data on every session switch.
                let cached_count = ctx
                    .params
                    .get("cached_message_count")
                    .and_then(|v| v.as_u64());
                let mut result = result;
                if !include_history && let Some(obj) = result.as_object_mut() {
                    obj.insert("history".to_string(), serde_json::Value::Array(Vec::new()));
                    obj.insert("historyOmitted".to_string(), serde_json::Value::Bool(true));
                    obj.remove("historyTruncated");
                    obj.remove("historyDroppedCount");
                }
                if let Some(cached) = cached_count
                    && include_history
                    && let Some(obj) = result.as_object_mut()
                    && let Some(entry_obj) = obj.get("entry").and_then(|e| e.as_object())
                    && let Some(server_count) =
                        entry_obj.get("messageCount").and_then(|v| v.as_u64())
                    && cached == server_count
                {
                    obj.insert("history".to_string(), serde_json::Value::Array(Vec::new()));
                    obj.insert("historyCacheHit".to_string(), serde_json::Value::Bool(true));
                    obj.remove("historyTruncated");
                    obj.remove("historyDroppedCount");
                }

                // Inject replying state so frontend restores thinking
                // indicator and voice-pending state after page reload.
                let chat = ctx.state.chat();
                let active_keys = chat.active_session_keys().await;
                let replying = active_keys.iter().any(|k| k == key);
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("replying".to_string(), serde_json::Value::Bool(replying));
                    if replying {
                        if let Some(text) = chat.active_thinking_text(key).await {
                            obj.insert("thinkingText".to_string(), serde_json::Value::String(text));
                        }
                        if let Some(text) = chat.active_partial_text(key).await {
                            obj.insert("streamText".to_string(), serde_json::Value::String(text));
                        }
                        if chat.active_voice_pending(key).await {
                            obj.insert("voicePending".to_string(), serde_json::Value::Bool(true));
                        }
                    }
                }

                Ok(result)
            })
        }),
    );

    // TTS and STT (voice feature)
    #[cfg(feature = "voice")]
    {
        reg.register(
            "tts.status",
            Box::new(|ctx| {
                Box::pin(async move {
                    let mut status = ctx
                        .state
                        .services
                        .tts
                        .status()
                        .await
                        .map_err(ErrorShape::from)?;

                    // Enrich with active persona info.
                    if let Some(ref store) = ctx.state.services.voice_persona_store
                        && let Ok(Some(active)) = store.get_active().await
                    {
                        status["persona"] = serde_json::json!({
                            "id": active.persona.id,
                            "label": active.persona.label,
                        });
                    }

                    Ok(status)
                })
            }),
        );
        reg.register(
            "tts.providers",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .tts
                        .providers()
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "tts.enable",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .tts
                        .enable(ctx.params.clone())
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "tts.disable",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .tts
                        .disable()
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "tts.convert",
            Box::new(|ctx| {
                Box::pin(async move {
                    let mut params = ctx.params.clone();

                    // Resolve voice persona through the full chain:
                    // explicit personaId → session agent's voice_persona_id → global active.
                    if params.get("persona").is_none()
                        && let Some(ref vp_store) = ctx.state.services.voice_persona_store
                    {
                        let explicit_id = params.get("personaId").and_then(|v| v.as_str());
                        let session_key = params
                            .get("_session_key")
                            .and_then(|v| v.as_str())
                            .map(String::from);

                        let persona = crate::voice_persona::resolve_persona(
                            vp_store,
                            ctx.state.services.agent_persona_store.as_deref(),
                            explicit_id,
                            session_key.as_deref(),
                            ctx.state.services.session_metadata.as_deref(),
                        )
                        .await;

                        if let Some(persona) = persona
                            && let Ok(v) = serde_json::to_value(&persona)
                        {
                            params["persona"] = v;
                        }
                    }

                    ctx.state
                        .services
                        .tts
                        .convert(params)
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
                "tts.generate_phrase",
                Box::new(|ctx| {
                    Box::pin(async move {
                        let context = ctx
                            .params
                            .get("context")
                            .and_then(|v| v.as_str())
                            .unwrap_or("settings");

                        let identity = moltis_config::resolve_identity();
                        let user = identity
                            .user_name
                            .unwrap_or_else(|| "friend".into());
                        let bot = identity.name;

                        // Try LLM generation with a 3-second timeout.
                        // Clone the Arc out so we don't hold the outer RwLock across awaits.
                        let providers = ctx.state.inner.read().await.llm_providers.clone();
                        if let Some(providers) = providers {
                            let provider = providers.read().await.first();
                            if let Some(provider) = provider {
                                let system_prompt = format!(
                                    "You generate short, funny TTS test phrases for a voice assistant.\n\
                                     The user's name is {user}. The bot's name is {bot}.\n\
                                     Include SSML <break time=\"0.5s\"/> tags for natural pauses.\n\
                                     Reply with ONLY the phrase text — no quotes, no markdown. Under 200 chars."
                                );
                                let messages = vec![
                                    moltis_agents::model::ChatMessage::system(system_prompt),
                                    moltis_agents::model::ChatMessage::user(format!(
                                        "Generate a {context} TTS test phrase."
                                    )),
                                ];
                                let result = tokio::time::timeout(
                                    std::time::Duration::from_secs(3),
                                    provider.complete(&messages, &[]),
                                )
                                .await;

                                if let Ok(Ok(response)) = result
                                    && let Some(text) = response.text
                                {
                                    let text = text.trim().to_string();
                                    if !text.is_empty() {
                                        return Ok(serde_json::json!({
                                            "phrase": text,
                                            "source": "llm",
                                        }));
                                    }
                                }
                            }
                        }

                        // Fall back to static phrases with sequential picking.
                        let phrases =
                            crate::tts_phrases::static_phrases(&user, &bot, context);
                        let idx = ctx.state.next_tts_phrase_index(phrases.len());
                        let phrase = phrases
                            .into_iter()
                            .nth(idx)
                            .unwrap_or_default();

                        Ok(serde_json::json!({
                            "phrase": phrase,
                            "source": "static",
                        }))
                    })
                }),
            );
        reg.register(
            "tts.setProvider",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .tts
                        .set_provider(ctx.params.clone())
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "stt.status",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .stt
                        .status()
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "stt.providers",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .stt
                        .providers()
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "stt.transcribe",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .stt
                        .transcribe(ctx.params.clone())
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
        reg.register(
            "stt.setProvider",
            Box::new(|ctx| {
                Box::pin(async move {
                    ctx.state
                        .services
                        .stt
                        .set_provider(ctx.params.clone())
                        .await
                        .map_err(ErrorShape::from)
                })
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        auth::{AuthMode, ResolvedAuth},
        services::GatewayServices,
        state::GatewayState,
    };

    fn context(params: serde_json::Value) -> MethodContext {
        MethodContext {
            request_id: "test".into(),
            method: "chat.send".into(),
            params,
            client_conn_id: "conn-1".into(),
            client_role: "operator".into(),
            client_scopes: vec!["operator.write".to_string()],
            state: GatewayState::new(
                ResolvedAuth {
                    mode: AuthMode::Token,
                    token: None,
                    password: None,
                },
                GatewayServices::noop(),
            ),
            channel: None,
        }
    }

    #[tokio::test]
    async fn client_supplied_server_only_params_are_stripped() {
        // A WebSocket client must not be able to claim the acknowledgment slot
        // of an inbound channel message, nor name the chat a reply is sent to.
        // Both are re-derived server-side from state the server trusts.
        let params = prepare_chat_send_params(&context(serde_json::json!({
            "text": "hi",
            "channel": {"sender_id": "forged"},
            "_native_channel_request": true,
            moltis_chat::params::ACK_KEYS: ["slack-acct:C123:1700000000.1"],
            moltis_chat::params::CHANNEL_REPLY_TARGET: {
                "channel_type": "slack",
                "account_id": "victim-workspace",
                "chat_id": "C999",
            },
        })))
        .await;

        for key in moltis_chat::params::SERVER_ONLY {
            assert!(params.get(*key).is_none(), "{key} was not stripped");
        }
        for key in moltis_chat::request_params::GATEWAY_OWNED_REQUEST_PARAMS {
            assert!(params.get(key).is_none(), "{key} was not stripped");
        }
        assert_eq!(params["text"], "hi");
    }

    #[tokio::test]
    async fn connection_context_is_injected_and_not_client_controlled() {
        let params = prepare_chat_send_params(&context(serde_json::json!({
            "text": "hi",
            "_conn_id": "someone-elses-connection",
        })))
        .await;

        assert_eq!(params["_conn_id"], "conn-1");
    }

    #[tokio::test]
    async fn legitimate_internal_params_survive() {
        // The web UI sets these itself; stripping them would break uploads and
        // explicit session targeting.
        let params = prepare_chat_send_params(&context(serde_json::json!({
            "text": "hi",
            "_session_key": "session:42",
            "_audio_filename": "voice.ogg",
            "_document_files": [{ "name": "a.pdf" }],
            "_tool_audience": "public",
            "_tool_policy": {"deny": ["*"]},
            "_private_context": false,
        })))
        .await;

        assert_eq!(params["_session_key"], "session:42");
        assert_eq!(params["_audio_filename"], "voice.ogg");
        assert_eq!(params["_document_files"][0]["name"], "a.pdf");
        assert_eq!(params["_tool_audience"], "public");
        assert_eq!(params["_tool_policy"]["deny"][0], "*");
        assert_eq!(params["_private_context"], false);
    }
}
