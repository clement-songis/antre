//! Ordered STT provider fallback chain.

use {
    moltis_agents::provider_chain::classify_error,
    moltis_voice::{SttProvider, SttProviderId, TranscribeRequest, Transcript},
    tracing::{debug, warn},
};

pub(crate) type BoxedSttProvider = Box<dyn SttProvider + Send + Sync>;

/// Why a transcription chain produced no transcript.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SttChainError {
    #[error("STT provider '{0}' not configured")]
    NotConfigured(SttProviderId),
    #[error("transcription failed: {0}")]
    Failed(String),
    #[error("transcription failed, all STT providers failed: {0}")]
    AllFailed(String),
}

/// Build the ordered chain: `primary` first, then `fallback`, each provider
/// at most once.
#[must_use]
pub(crate) fn build_chain(
    primary: SttProviderId,
    fallback: &[SttProviderId],
) -> Vec<SttProviderId> {
    let mut chain = vec![primary];
    for provider in fallback {
        if !chain.contains(provider) {
            chain.push(*provider);
        }
    }
    chain
}

/// Transcribe with each configured provider of `chain` in order until one
/// succeeds. Unconfigured providers (`create` returns `None`) are skipped.
///
/// Every error falls through, unlike the LLM chain which stops on 400s:
/// STT providers accept different audio formats and size limits, so a
/// rejection by one says little about the next.
pub(crate) async fn transcribe_with_fallback<F>(
    chain: &[SttProviderId],
    create: F,
    request: TranscribeRequest,
) -> Result<(SttProviderId, Transcript), SttChainError>
where
    F: Fn(SttProviderId) -> Option<BoxedSttProvider>,
{
    let mut failures: Vec<(SttProviderId, String)> = Vec::new();

    for (position, provider_id) in chain.iter().copied().enumerate() {
        let Some(provider) = create(provider_id) else {
            debug!(provider = %provider_id, "STT provider not configured, skipping");
            continue;
        };
        if position > 0 {
            debug!(provider = %provider_id, "trying STT fallback provider");
        }

        match provider.transcribe(request.clone()).await {
            Ok(transcript) => return Ok((provider_id, transcript)),
            Err(error) => {
                warn!(
                    provider = %provider_id,
                    error = %error,
                    kind = ?classify_error(&error),
                    audio_bytes = request.audio.len(),
                    fallback_remaining = position + 1 < chain.len(),
                    "STT provider failed"
                );
                failures.push((provider_id, error.to_string()));
            },
        }
    }

    Err(match failures.as_slice() {
        [] => SttChainError::NotConfigured(chain.first().copied().unwrap_or_default()),
        [(_, error)] => SttChainError::Failed(error.clone()),
        _ => SttChainError::AllFailed(
            failures
                .iter()
                .map(|(provider_id, error)| format!("{provider_id}: {error}"))
                .collect::<Vec<_>>()
                .join("; "),
        ),
    })
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        async_trait::async_trait,
        bytes::Bytes,
        moltis_voice::AudioFormat,
        std::sync::{Arc, Mutex},
    };

    struct ScriptedStt {
        outcome: Result<&'static str, &'static str>,
    }

    #[async_trait]
    impl SttProvider for ScriptedStt {
        fn id(&self) -> &'static str {
            "scripted"
        }

        fn name(&self) -> &'static str {
            "Scripted"
        }

        fn is_configured(&self) -> bool {
            true
        }

        async fn transcribe(&self, _request: TranscribeRequest) -> anyhow::Result<Transcript> {
            let text = self.outcome.map_err(|message| anyhow::anyhow!(message))?;
            Ok(Transcript {
                text: text.into(),
                language: None,
                confidence: None,
                duration_seconds: None,
                words: None,
            })
        }
    }

    enum Behaviour {
        Succeed(&'static str),
        Fail(&'static str),
        Unconfigured,
    }

    fn request() -> TranscribeRequest {
        TranscribeRequest {
            audio: Bytes::from_static(b"fake-audio"),
            format: AudioFormat::Opus,
            language: None,
            prompt: None,
        }
    }

    /// Runs the chain against scripted providers and returns the result plus
    /// the providers that were instantiated, in order.
    async fn run(
        chain: &[SttProviderId],
        script: &[(SttProviderId, Behaviour)],
    ) -> (
        Result<(SttProviderId, Transcript), SttChainError>,
        Vec<SttProviderId>,
    ) {
        let created = Arc::new(Mutex::new(Vec::new()));
        let created_in_factory = Arc::clone(&created);
        let factory = move |id: SttProviderId| -> Option<BoxedSttProvider> {
            created_in_factory.lock().unwrap().push(id);
            let (_, behaviour) = script.iter().find(|(scripted, _)| *scripted == id)?;
            match behaviour {
                Behaviour::Succeed(text) => Some(Box::new(ScriptedStt { outcome: Ok(*text) })),
                Behaviour::Fail(message) => Some(Box::new(ScriptedStt {
                    outcome: Err(*message),
                })),
                Behaviour::Unconfigured => None,
            }
        };
        let result = transcribe_with_fallback(chain, factory, request()).await;
        let created = created.lock().unwrap().clone();
        (result, created)
    }

    #[test]
    fn build_chain_puts_primary_first_and_dedups() {
        let chain = build_chain(SttProviderId::Google, &[
            SttProviderId::Whisper,
            SttProviderId::Google,
            SttProviderId::Groq,
            SttProviderId::Whisper,
        ]);
        assert_eq!(chain, [
            SttProviderId::Google,
            SttProviderId::Whisper,
            SttProviderId::Groq,
        ]);
    }

    #[tokio::test]
    async fn primary_succeeds_without_trying_fallback() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, created) = run(&chain, &[
            (SttProviderId::Google, Behaviour::Succeed("from google")),
            (SttProviderId::Whisper, Behaviour::Succeed("from whisper")),
        ])
        .await;

        let (served_by, transcript) = result.unwrap();
        assert_eq!(served_by, SttProviderId::Google);
        assert_eq!(transcript.text, "from google");
        assert_eq!(created, [SttProviderId::Google]);
    }

    #[tokio::test]
    async fn primary_fails_then_fallback_succeeds() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, created) = run(&chain, &[
            (
                SttProviderId::Google,
                Behaviour::Fail("Gemini API error (HTTP 429): quota exceeded"),
            ),
            (SttProviderId::Whisper, Behaviour::Succeed("from whisper")),
        ])
        .await;

        let (served_by, transcript) = result.unwrap();
        assert_eq!(served_by, SttProviderId::Whisper);
        assert_eq!(transcript.text, "from whisper");
        assert_eq!(created, [SttProviderId::Google, SttProviderId::Whisper]);
    }

    #[tokio::test]
    async fn fallback_runs_after_invalid_request_error() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, _) = run(&chain, &[
            (
                SttProviderId::Google,
                Behaviour::Fail("HTTP 400 bad request: unsupported audio format"),
            ),
            (SttProviderId::Whisper, Behaviour::Succeed("from whisper")),
        ])
        .await;

        assert_eq!(result.unwrap().0, SttProviderId::Whisper);
    }

    #[tokio::test]
    async fn all_fail_reports_every_failure_ending_with_the_last() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, created) = run(&chain, &[
            (
                SttProviderId::Google,
                Behaviour::Fail("HTTP 503 overloaded"),
            ),
            (
                SttProviderId::Whisper,
                Behaviour::Fail("connection refused"),
            ),
        ])
        .await;

        let error = result.unwrap_err();
        assert_eq!(
            error,
            SttChainError::AllFailed(
                "google: HTTP 503 overloaded; whisper: connection refused".into()
            )
        );
        assert!(error.to_string().ends_with("whisper: connection refused"));
        assert_eq!(created, [SttProviderId::Google, SttProviderId::Whisper]);
    }

    #[tokio::test]
    async fn unconfigured_fallback_is_skipped() {
        let chain = [
            SttProviderId::Google,
            SttProviderId::Groq,
            SttProviderId::Whisper,
        ];
        let (result, created) = run(&chain, &[
            (SttProviderId::Google, Behaviour::Fail("timed out")),
            (SttProviderId::Groq, Behaviour::Unconfigured),
            (SttProviderId::Whisper, Behaviour::Succeed("from whisper")),
        ])
        .await;

        assert_eq!(result.unwrap().0, SttProviderId::Whisper);
        assert_eq!(created, [
            SttProviderId::Google,
            SttProviderId::Groq,
            SttProviderId::Whisper,
        ]);
    }

    #[tokio::test]
    async fn unconfigured_primary_falls_through_to_fallback() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, _) = run(&chain, &[
            (SttProviderId::Google, Behaviour::Unconfigured),
            (SttProviderId::Whisper, Behaviour::Succeed("from whisper")),
        ])
        .await;

        assert_eq!(result.unwrap().0, SttProviderId::Whisper);
    }

    #[tokio::test]
    async fn single_failure_keeps_plain_error_message() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, _) = run(&chain, &[
            (SttProviderId::Google, Behaviour::Fail("HTTP 429")),
            (SttProviderId::Whisper, Behaviour::Unconfigured),
        ])
        .await;

        assert_eq!(
            result.unwrap_err().to_string(),
            "transcription failed: HTTP 429"
        );
    }

    #[tokio::test]
    async fn nothing_configured_names_the_primary() {
        let chain = [SttProviderId::Google, SttProviderId::Whisper];
        let (result, _) = run(&chain, &[]).await;

        assert_eq!(
            result.unwrap_err(),
            SttChainError::NotConfigured(SttProviderId::Google)
        );
    }
}
