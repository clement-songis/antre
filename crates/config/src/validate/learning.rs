//! Semantic checks for `[learning]`.

use super::{Diagnostic, Severity};

fn push(diagnostics: &mut Vec<Diagnostic>, severity: Severity, path: &str, message: String) {
    diagnostics.push(Diagnostic {
        severity,
        category: "invalid-value",
        path: path.into(),
        message,
    });
}

pub(super) fn check_learning(
    learning: &crate::schema::LearningConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let curator = &learning.curator;
    for (path, days) in [
        (
            "learning.curator.stale_after_days",
            curator.stale_after_days,
        ),
        (
            "learning.curator.archive_after_days",
            curator.archive_after_days,
        ),
    ] {
        if days == 0 {
            push(
                diagnostics,
                Severity::Error,
                path,
                format!("{path} must be at least 1; 0 would age every skill on the next run"),
            );
        }
    }
    if curator.archive_after_days != 0 && curator.archive_after_days <= curator.stale_after_days {
        push(
            diagnostics,
            Severity::Warning,
            "learning.curator.archive_after_days",
            "learning.curator.archive_after_days should be greater than stale_after_days; skills are archived without a stale period".into(),
        );
    }
    if curator.interval_hours == 0 {
        push(
            diagnostics,
            Severity::Warning,
            "learning.curator.interval_hours",
            "learning.curator.interval_hours = 0 runs the curator on every idle check; use at least 1".into(),
        );
    }
    if !curator.min_idle_hours.is_finite() || curator.min_idle_hours < 0.0 {
        push(
            diagnostics,
            Severity::Error,
            "learning.curator.min_idle_hours",
            "learning.curator.min_idle_hours must be a finite number of hours, 0 or more".into(),
        );
    }

    let review = &learning.review;
    if review.enabled && review.every_user_turns == 0 && review.every_tool_calls == 0 {
        push(
            diagnostics,
            Severity::Warning,
            "learning.review",
            "learning.review.every_user_turns and every_tool_calls are both 0, so the review never runs automatically".into(),
        );
    }
    if review.max_changes == 0 {
        push(
            diagnostics,
            Severity::Warning,
            "learning.review.max_changes",
            "learning.review.max_changes = 0 discards every proposed change".into(),
        );
    }
    if !review.skills && !review.memory {
        push(
            diagnostics,
            Severity::Warning,
            "learning.review",
            "learning.review.skills and memory are both off, so the review has nothing to propose"
                .into(),
        );
    }
    if learning.enabled && !learning.write_approval {
        push(
            diagnostics,
            Severity::Info,
            "learning.write_approval",
            "learning.write_approval is off: learned changes are written directly (roll back with learning.ledger.rollback)".into(),
        );
    }
    if let Some(model) = learning.model.as_deref()
        && model.trim().is_empty()
    {
        push(
            diagnostics,
            Severity::Warning,
            "learning.model",
            "learning.model is empty; remove it to use the catalog's utility model".into(),
        );
    }
}
