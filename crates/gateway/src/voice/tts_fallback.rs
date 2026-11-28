//! Ordered TTS provider fallback chain.

use {
    antre_voice::{AudioOutput, SynthesizeRequest, TtsProvider, TtsProviderId},
    tracing::{debug, info, warn},
};

pub(crate) type BoxedTtsProvider = Box<dyn TtsProvider + Send + Sync>;

/// Build the ordered chain: `primary` first, then `fallback`, each provider
/// at most once. An empty `fallback` means every other configured provider,
/// in listing order.
#[must_use]
pub(crate) fn build_chain(
    primary: TtsProviderId,
    fallback: &[TtsProviderId],
    configured: &[(TtsProviderId, bool)],
) -> Vec<TtsProviderId> {
    let rest: Vec<TtsProviderId> = if fallback.is_empty() {
        configured
            .iter()
            .filter(|(_, is_configured)| *is_configured)
            .map(|(id, _)| *id)
            .collect()
    } else {
        fallback.to_vec()
    };
    let mut chain = vec![primary];
    for provider in rest {
        if !chain.contains(&provider) {
            chain.push(provider);
        }
    }
    chain
}

/// A successful synthesis and every provider that was tried.
pub(crate) struct ChainOutput {
    pub(crate) provider: TtsProviderId,
    pub(crate) output: AudioOutput,
    pub(crate) attempted: Vec<TtsProviderId>,
}

/// Synthesize with each configured provider of `chain` in order until one
/// succeeds. `request_for` builds the request per provider (persona voice
/// bindings differ by provider). Every error falls through: providers differ
/// in voices, formats and text limits.
pub(crate) async fn synthesize_with_fallback<F, R>(
    chain: &[TtsProviderId],
    create: F,
    request_for: R,
) -> Result<ChainOutput, String>
where
    F: Fn(TtsProviderId) -> Option<BoxedTtsProvider>,
    R: Fn(TtsProviderId) -> SynthesizeRequest,
{
    let mut attempted = Vec::new();
    let mut failures: Vec<(TtsProviderId, String)> = Vec::new();
    for (position, provider_id) in chain.iter().copied().enumerate() {
        let Some(provider) = create(provider_id) else {
            debug!(provider = %provider_id, "TTS provider not configured, skipping");
            continue;
        };
        attempted.push(provider_id);
        match provider.synthesize(request_for(provider_id)).await {
            Ok(output) => {
                if position > 0 {
                    info!(provider = %provider_id, "TTS fallback succeeded");
                }
                return Ok(ChainOutput {
                    provider: provider_id,
                    output,
                    attempted,
                });
            },
            Err(error) => {
                warn!(
                    provider = %provider_id,
                    error = %error,
                    fallback_remaining = position + 1 < chain.len(),
                    "TTS provider failed"
                );
                failures.push((provider_id, error.to_string()));
            },
        }
    }
    Err(match failures.as_slice() {
        [] => format!(
            "provider '{}' not configured",
            chain.first().copied().unwrap_or_default()
        ),
        [(_, error)] => format!("TTS synthesis failed: {error}"),
        _ => format!(
            "TTS synthesis failed, all TTS providers failed: {}",
            failures
                .iter()
                .map(|(id, error)| format!("{id}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    })
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        antre_voice::AudioFormat,
        async_trait::async_trait,
        bytes::Bytes,
        std::sync::{Arc, Mutex},
    };

    struct ScriptedTts {
        outcome: Result<&'static str, &'static str>,
        seen_voices: Arc<Mutex<Vec<Option<String>>>>,
    }

    #[async_trait]
    impl TtsProvider for ScriptedTts {
        fn id(&self) -> &'static str {
            "scripted"
        }

        fn name(&self) -> &'static str {
            "Scripted"
        }

        fn is_configured(&self) -> bool {
            true
        }

        async fn voices(&self) -> anyhow::Result<Vec<antre_voice::Voice>> {
            Ok(Vec::new())
        }

        async fn synthesize(&self, request: SynthesizeRequest) -> anyhow::Result<AudioOutput> {
            self.seen_voices.lock().unwrap().push(request.voice_id);
            let data = self.outcome.map_err(|message| anyhow::anyhow!(message))?;
            Ok(AudioOutput {
                data: Bytes::from_static(data.as_bytes()),
                format: AudioFormat::Mp3,
                duration_ms: None,
            })
        }
    }

    fn request(voice: &str) -> SynthesizeRequest {
        SynthesizeRequest {
            text: "hello".into(),
            voice_id: Some(voice.into()),
            ..SynthesizeRequest::default()
        }
    }

    async fn run(
        chain: &[TtsProviderId],
        script: &[(TtsProviderId, Option<Result<&'static str, &'static str>>)],
    ) -> (Result<ChainOutput, String>, Vec<Option<String>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in_factory = Arc::clone(&seen);
        let factory = move |id: TtsProviderId| -> Option<BoxedTtsProvider> {
            let (_, outcome) = script.iter().find(|(scripted, _)| *scripted == id)?;
            outcome.map(|outcome| {
                Box::new(ScriptedTts {
                    outcome,
                    seen_voices: Arc::clone(&seen_in_factory),
                }) as BoxedTtsProvider
            })
        };
        let result =
            synthesize_with_fallback(chain, factory, |id| request(&format!("voice-{id}"))).await;
        let seen = seen.lock().unwrap().clone();
        (result, seen)
    }

    #[test]
    fn explicit_fallback_order_is_kept_and_deduped() {
        let chain = build_chain(
            TtsProviderId::OpenAi,
            &[
                TtsProviderId::Piper,
                TtsProviderId::OpenAi,
                TtsProviderId::Google,
                TtsProviderId::Piper,
            ],
            &[(TtsProviderId::ElevenLabs, true)],
        );
        assert_eq!(chain, [
            TtsProviderId::OpenAi,
            TtsProviderId::Piper,
            TtsProviderId::Google,
        ]);
    }

    #[test]
    fn empty_fallback_uses_other_configured_providers() {
        let chain = build_chain(TtsProviderId::Google, &[], &[
            (TtsProviderId::ElevenLabs, false),
            (TtsProviderId::OpenAi, true),
            (TtsProviderId::Google, true),
            (TtsProviderId::Piper, true),
        ]);
        assert_eq!(chain, [
            TtsProviderId::Google,
            TtsProviderId::OpenAi,
            TtsProviderId::Piper,
        ]);
    }

    #[tokio::test]
    async fn primary_success_tries_nothing_else() {
        let (result, seen) = run(&[TtsProviderId::OpenAi, TtsProviderId::Google], &[
            (TtsProviderId::OpenAi, Some(Ok("a"))),
            (TtsProviderId::Google, Some(Ok("b"))),
        ])
        .await;
        let output = result.unwrap();
        assert_eq!(output.provider, TtsProviderId::OpenAi);
        assert_eq!(output.attempted, [TtsProviderId::OpenAi]);
        assert_eq!(seen, [Some("voice-openai".to_string())]);
    }

    #[tokio::test]
    async fn failure_falls_through_with_a_per_provider_request() {
        let (result, seen) = run(
            &[
                TtsProviderId::OpenAi,
                TtsProviderId::ElevenLabs,
                TtsProviderId::Google,
            ],
            &[
                (TtsProviderId::OpenAi, Some(Err("HTTP 429"))),
                (TtsProviderId::ElevenLabs, None),
                (TtsProviderId::Google, Some(Ok("b"))),
            ],
        )
        .await;
        let output = result.unwrap();
        assert_eq!(output.provider, TtsProviderId::Google);
        assert_eq!(output.attempted, [
            TtsProviderId::OpenAi,
            TtsProviderId::Google
        ]);
        assert_eq!(seen, [
            Some("voice-openai".to_string()),
            Some("voice-google".to_string())
        ]);
    }

    #[tokio::test]
    async fn errors_name_every_failed_provider() {
        let (single, _) = run(&[TtsProviderId::OpenAi], &[(
            TtsProviderId::OpenAi,
            Some(Err("HTTP 500")),
        )])
        .await;
        assert_eq!(single.err().unwrap(), "TTS synthesis failed: HTTP 500");

        let (all, _) = run(&[TtsProviderId::OpenAi, TtsProviderId::Piper], &[
            (TtsProviderId::OpenAi, Some(Err("HTTP 500"))),
            (TtsProviderId::Piper, Some(Err("model missing"))),
        ])
        .await;
        assert_eq!(
            all.err().unwrap(),
            "TTS synthesis failed, all TTS providers failed: openai: HTTP 500; piper: model missing"
        );

        let (none, _) = run(&[TtsProviderId::Coqui], &[]).await;
        assert_eq!(none.err().unwrap(), "provider 'coqui' not configured");
    }
}
