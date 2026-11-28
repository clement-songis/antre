//! Delivery records and their state machine.

use std::{fmt, str::FromStr};

use {
    antre_channels::ChannelReplyTarget,
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::error::Error;

/// Where a delivery is in its life.
///
/// ```text
/// queued ──claim──▶ sending ──ok──▶ delivered
///   ▲                  │
///   └──retry (backoff)─┤
///                      └──out of attempts / rejected──▶ failed ──redeliver──▶ queued
/// ```
///
/// A `sending` row whose lease expired (the process died or the sender was
/// cancelled mid-send) goes back to `queued`, so delivery is at least once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    Queued,
    Sending,
    Delivered,
    Failed,
}

impl DeliveryState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }

    /// `true` once nothing more will happen without an explicit redelivery.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Delivered | Self::Failed)
    }
}

impl fmt::Display for DeliveryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeliveryState {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "sending" => Ok(Self::Sending),
            "delivered" => Ok(Self::Delivered),
            "failed" => Ok(Self::Failed),
            other => Err(Error::Corrupt(format!("unknown delivery state {other:?}"))),
        }
    }
}

/// What the delivery carries, for the UI and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryKind {
    /// The final answer of a run.
    Reply,
    /// The terminal error of a run.
    Error,
    /// The activity logbook sent after an answer that went out another way.
    Logbook,
    /// A message not produced by a run (background task result, notice).
    Notice,
}

impl DeliveryKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Error => "error",
            Self::Logbook => "logbook",
            Self::Notice => "notice",
        }
    }
}

impl fmt::Display for DeliveryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeliveryKind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "reply" => Ok(Self::Reply),
            "error" => Ok(Self::Error),
            "logbook" => Ok(Self::Logbook),
            "notice" => Ok(Self::Notice),
            other => Err(Error::Corrupt(format!("unknown delivery kind {other:?}"))),
        }
    }
}

/// The message to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum DeliveryBody {
    /// Plain or markdown text, optionally followed by an HTML suffix (the
    /// activity logbook).
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suffix_html: Option<String>,
    },
    /// Pre-rendered HTML.
    Html { html: String },
}

impl DeliveryBody {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            suffix_html: None,
        }
    }

    /// A short plain-text preview for logs and the UI.
    #[must_use]
    pub fn preview(&self, max_chars: usize) -> String {
        let source = match self {
            Self::Text { text, .. } => text,
            Self::Html { html } => html,
        };
        let mut preview: String = source.chars().take(max_chars).collect();
        if source.chars().count() > max_chars {
            preview.push('…');
        }
        preview
    }
}

/// A delivery to enqueue.
#[derive(Debug, Clone)]
pub struct NewDelivery {
    pub session_key: String,
    pub run_id: Option<String>,
    pub kind: DeliveryKind,
    pub target: ChannelReplyTarget,
    pub body: DeliveryBody,
    /// Platform message id to reply to (threads the answer under the
    /// user's message).
    pub reply_to: Option<String>,
}

/// A persisted delivery.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub id: i64,
    pub session_key: String,
    pub run_id: Option<String>,
    pub kind: DeliveryKind,
    pub target: ChannelReplyTarget,
    pub body: DeliveryBody,
    pub reply_to: Option<String>,
    pub state: DeliveryState,
    /// Send attempts started so far, including the one in flight.
    pub attempts: u32,
    /// Set when a failed delivery was queued again; the sink says so to the
    /// recipient.
    pub redelivered: bool,
    pub next_attempt_at: OffsetDateTime,
    pub last_error: Option<String>,
    pub message_ids: Vec<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Deliveries to the same key are sent in order: a later one waits while an
/// earlier one is still queued or sending.
#[must_use]
pub fn target_key(target: &ChannelReplyTarget) -> String {
    format!(
        "{}:{}:{}:{}",
        target.channel_type.as_str(),
        target.account_id,
        target.chat_id,
        target.thread_id.as_deref().unwrap_or("")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, antre_channels::ChannelType};

    fn target(thread: Option<&str>) -> ChannelReplyTarget {
        ChannelReplyTarget {
            channel_type: ChannelType::Telegram,
            account_id: "bot".into(),
            chat_id: "42".into(),
            message_id: Some("7".into()),
            thread_id: thread.map(String::from),
            ack_message_id: None,
            direct_chat: false,
        }
    }

    #[test]
    fn states_and_kinds_round_trip_through_their_names() {
        for state in [
            DeliveryState::Queued,
            DeliveryState::Sending,
            DeliveryState::Delivered,
            DeliveryState::Failed,
        ] {
            assert_eq!(state.as_str().parse::<DeliveryState>().unwrap(), state);
        }
        for kind in [
            DeliveryKind::Reply,
            DeliveryKind::Error,
            DeliveryKind::Logbook,
            DeliveryKind::Notice,
        ] {
            assert_eq!(kind.as_str().parse::<DeliveryKind>().unwrap(), kind);
        }
        assert!("lost".parse::<DeliveryState>().is_err());
        assert!("lost".parse::<DeliveryKind>().is_err());
    }

    #[test]
    fn only_delivered_and_failed_are_terminal() {
        assert!(!DeliveryState::Queued.is_terminal());
        assert!(!DeliveryState::Sending.is_terminal());
        assert!(DeliveryState::Delivered.is_terminal());
        assert!(DeliveryState::Failed.is_terminal());
    }

    #[test]
    fn target_key_separates_threads_but_ignores_the_reply_anchor() {
        let mut other_anchor = target(None);
        other_anchor.message_id = Some("8".into());
        assert_eq!(target_key(&target(None)), target_key(&other_anchor));
        assert_ne!(target_key(&target(None)), target_key(&target(Some("t1"))));
        assert_eq!(target_key(&target(Some("t1"))), "telegram:bot:42:t1");
    }

    #[test]
    fn body_serializes_with_a_format_tag_and_previews_by_chars() {
        let body = DeliveryBody::text("héllo wörld");
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["format"], "text");
        assert!(json.get("suffix_html").is_none());
        assert_eq!(body.preview(5), "héllo…");
        assert_eq!(body.preview(50), "héllo wörld");
        let html = DeliveryBody::Html {
            html: "<b>x</b>".into(),
        };
        let back: DeliveryBody =
            serde_json::from_value(serde_json::to_value(&html).unwrap()).unwrap();
        assert_eq!(back, html);
    }
}
