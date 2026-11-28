//! Canvas events pushed to connected clients over the gateway event bus.

use serde::Serialize;

use crate::{Design, DesignId, DesignVersion};

/// Visibility of a published design link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
}

/// Something changed on a design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", untagged)]
pub enum CanvasEvent {
    DesignCreated {
        design: Design,
    },
    VersionCreated {
        #[serde(rename = "designId")]
        design_id: DesignId,
        version: DesignVersion,
        /// Version this one restored, for reverts.
        #[serde(rename = "revertedFrom", skip_serializing_if = "Option::is_none")]
        reverted_from: Option<u32>,
    },
    Published {
        #[serde(rename = "designId")]
        design_id: DesignId,
        #[serde(rename = "shareId")]
        share_id: String,
        version: u32,
        visibility: Visibility,
    },
    Unpublished {
        #[serde(rename = "designId")]
        design_id: Option<DesignId>,
        #[serde(rename = "shareId")]
        share_id: String,
    },
    CommentsChanged {
        #[serde(rename = "designId")]
        design_id: DesignId,
        /// Version whose pins changed; `None` when several may have.
        #[serde(rename = "commentsVersion", skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
    },
}

impl CanvasEvent {
    /// Event name on the WebSocket bus.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::DesignCreated { .. } => "canvas.design.created",
            Self::VersionCreated { .. } => "canvas.version.created",
            Self::Published { .. } => "canvas.design.published",
            Self::Unpublished { .. } => "canvas.design.unpublished",
            Self::CommentsChanged { .. } => "canvas.comments.changed",
        }
    }
}

/// Receives canvas events. Implemented by the gateway on top of its
/// WebSocket broadcaster; must not block.
pub trait CanvasEventSink: Send + Sync {
    fn emit(&self, event: CanvasEvent);
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Collects events for assertions.
    #[derive(Default)]
    pub(crate) struct RecordingSink {
        pub(crate) events: Mutex<Vec<CanvasEvent>>,
    }

    impl RecordingSink {
        pub(crate) fn names(&self) -> Vec<&'static str> {
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(CanvasEvent::name)
                .collect()
        }
    }

    impl CanvasEventSink for RecordingSink {
        fn emit(&self, event: CanvasEvent) {
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event);
        }
    }

    #[test]
    fn events_serialize_with_camel_case_ids() {
        let id: DesignId = "0123456789abcdef0123456789abcdef"
            .parse()
            .unwrap_or_else(|_| DesignId::generate());
        let event = CanvasEvent::Published {
            design_id: id.clone(),
            share_id: "s".into(),
            version: 2,
            visibility: Visibility::Private,
        };
        assert_eq!(event.name(), "canvas.design.published");
        let json = serde_json::to_value(&event).unwrap_or_default();
        assert_eq!(json["designId"], id.as_str());
        assert_eq!(json["shareId"], "s");
        assert_eq!(json["visibility"], "private");
    }
}
