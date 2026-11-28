use super::{Diagnostic, Severity};

use crate::schema::DeliveryConfig;

fn warn(diagnostics: &mut Vec<Diagnostic>, field: &str, message: &str) {
    diagnostics.push(Diagnostic {
        severity: Severity::Warning,
        category: "invalid-value",
        path: format!("chat.delivery.{field}"),
        message: message.into(),
    });
}

pub(crate) fn check_delivery(delivery: &DeliveryConfig, diagnostics: &mut Vec<Diagnostic>) {
    if delivery.max_attempts == 0 {
        warn(
            diagnostics,
            "max_attempts",
            "chat.delivery.max_attempts must be at least 1; one attempt is used",
        );
    }
    if delivery.initial_retry_secs == 0 {
        warn(
            diagnostics,
            "initial_retry_secs",
            "chat.delivery.initial_retry_secs must be at least 1; 1 second is used",
        );
    }
    if delivery.max_retry_secs < delivery.initial_retry_secs {
        warn(
            diagnostics,
            "max_retry_secs",
            "chat.delivery.max_retry_secs is below initial_retry_secs; the initial delay is used as the ceiling",
        );
    }
    if delivery.attempt_timeout_secs == 0 {
        warn(
            diagnostics,
            "attempt_timeout_secs",
            "chat.delivery.attempt_timeout_secs must be at least 1; 1 second is used",
        );
    }
}
