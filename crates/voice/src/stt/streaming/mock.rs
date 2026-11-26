//! Mock live transcription provider for tests.
//!
//! Emits one partial per audio chunk and a canned final transcript on
//! [`StreamingSttInput::Finish`], without network calls.

use {anyhow::Result, async_trait::async_trait, tokio::sync::mpsc};

use {
    super::types::{
        STREAM_CHANNEL_CAPACITY, StreamingSttEvent, StreamingSttInput, StreamingSttProvider,
        StreamingSttRequest, StreamingSttSession,
    },
    crate::config::SttProviderId,
};

/// Mock live transcription provider.
#[derive(Debug, Clone)]
pub struct MockStreamingStt {
    final_text: String,
    sample_rate: u32,
}

impl MockStreamingStt {
    #[must_use]
    pub fn new(final_text: impl Into<String>) -> Self {
        Self {
            final_text: final_text.into(),
            sample_rate: 16_000,
        }
    }
}

#[async_trait]
impl StreamingSttProvider for MockStreamingStt {
    fn id(&self) -> SttProviderId {
        SttProviderId::Whisper
    }

    fn model(&self) -> &str {
        "mock-live"
    }

    fn input_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    async fn open(&self, _request: StreamingSttRequest) -> Result<StreamingSttSession> {
        let (input_tx, mut input_rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let final_text = self.final_text.clone();
        tokio::spawn(async move {
            let mut chunks = 0usize;
            while let Some(input) = input_rx.recv().await {
                match input {
                    StreamingSttInput::Audio(_) => {
                        chunks += 1;
                        let partial = StreamingSttEvent::Partial {
                            segment: "mock".into(),
                            text: format!("chunk {chunks}"),
                        };
                        if event_tx.send(partial).await.is_err() {
                            return;
                        }
                    },
                    StreamingSttInput::Finish => {
                        let _ = event_tx
                            .send(StreamingSttEvent::Final {
                                segment: "mock".into(),
                                text: final_text,
                            })
                            .await;
                        break;
                    },
                }
            }
            let _ = event_tx.send(StreamingSttEvent::Closed).await;
        });
        Ok(StreamingSttSession {
            input: input_tx,
            events: event_rx,
        })
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, bytes::Bytes};

    #[tokio::test]
    async fn emits_partials_then_final() {
        let mut session = MockStreamingStt::new("done")
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        session
            .input
            .send(StreamingSttInput::Audio(Bytes::from_static(&[0, 0])))
            .await
            .unwrap();
        session.input.send(StreamingSttInput::Finish).await.unwrap();
        let mut events = Vec::new();
        while let Some(event) = session.events.recv().await {
            events.push(event);
        }
        assert_eq!(events, vec![
            StreamingSttEvent::Partial {
                segment: "mock".into(),
                text: "chunk 1".into()
            },
            StreamingSttEvent::Final {
                segment: "mock".into(),
                text: "done".into()
            },
            StreamingSttEvent::Closed,
        ]);
    }
}
