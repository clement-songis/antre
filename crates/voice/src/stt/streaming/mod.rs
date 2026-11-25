//! Live (streaming) speech-to-text.

mod mock;
mod openai_realtime;
mod types;

pub use {
    mock::MockStreamingStt,
    openai_realtime::{DEFAULT_REALTIME_MODEL as OPENAI_DEFAULT_REALTIME_MODEL, OpenAiRealtimeStt},
    types::{
        StreamingSttEvent, StreamingSttInput, StreamingSttProvider, StreamingSttRequest,
        StreamingSttSession,
    },
};
