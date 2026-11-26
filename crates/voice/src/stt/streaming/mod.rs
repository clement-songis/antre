//! Live (streaming) speech-to-text.

mod gemini_live;
mod mock;
mod openai_realtime;
mod types;

pub use {
    gemini_live::{DEFAULT_REALTIME_MODEL as GEMINI_DEFAULT_REALTIME_MODEL, GeminiLiveStt},
    mock::MockStreamingStt,
    openai_realtime::{DEFAULT_REALTIME_MODEL as OPENAI_DEFAULT_REALTIME_MODEL, OpenAiRealtimeStt},
    types::{
        StreamingSttEvent, StreamingSttInput, StreamingSttProvider, StreamingSttRequest,
        StreamingSttSession,
    },
};
