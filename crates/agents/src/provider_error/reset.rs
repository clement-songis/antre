//! Build a [`ProviderFailure`] from a failed HTTP response and parse the
//! reset instant each provider reports.
//!
//! | Source | Format |
//! |---|---|
//! | `Retry-After` | seconds or HTTP-date |
//! | Anthropic `anthropic-ratelimit-*-reset` | RFC 3339 with offset |
//! | Claude subscription `anthropic-ratelimit-unified-reset` | epoch seconds |
//! | OpenAI `x-ratelimit-reset-requests` / `-tokens` | Go durations (`6m0s`, `1.5s`) |
//! | Codex `x-codex-{primary,secondary}-reset-after-seconds` | seconds, window in `-window-minutes` |
//! | Codex body `usage_limit_reached` | `resets_at` epoch or `resets_in_seconds` |
//! | Z.AI / GLM body (codes 1308, 1310) | `YYYY-MM-DD HH:MM:SS` without offset, China time |
//!
//! Every instant is stored in UTC. Offset-less times use the provider's
//! timezone from the model catalog (`rate_limit_tz`), never the server's
//! local zone. A reset in the past or more than [`MAX_RESET_HORIZON`] ahead
//! is treated as unknown.

use std::time::Duration;

use {
    reqwest::header::HeaderMap,
    serde::Deserialize,
    time::{OffsetDateTime, PrimitiveDateTime, UtcOffset},
};

use super::{LimitScope, ProviderError, ProviderFailure, QuotaWindow};

/// Resets further ahead than this are treated as parsing mistakes.
pub const MAX_RESET_HORIZON: time::Duration = time::Duration::days(8);

/// Phrases providers use for context-window overflow in error bodies.
const CONTEXT_WINDOW_PATTERNS: &[&str] = &[
    "context_length_exceeded",
    "context_window_exceeded",
    "model_context_window_exceeded",
    "maximum context length",
    "context window",
    "prompt is too long",
    "input too long",
    "input_too_long",
    "too many tokens",
    "request_too_large",
    "content_too_large",
];

const BILLING_PATTERNS: &[&str] = &[
    "insufficient_quota",
    "billing",
    "credit balance",
    "payment required",
    "exceeded your current quota",
];

/// Keep a parsed reset only when it is plausible.
#[must_use]
pub fn sane_reset(at: OffsetDateTime, now: OffsetDateTime) -> Option<OffsetDateTime> {
    if at <= now || at - now > MAX_RESET_HORIZON {
        tracing::debug!(%at, %now, "ignoring implausible provider reset time");
        return None;
    }
    Some(at)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

/// `Retry-After`: delay in seconds (fractions tolerated) or an HTTP-date.
#[must_use]
pub fn parse_retry_after(value: &str, now: OffsetDateTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<f64>() {
        return (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds));
    }
    let format = time::macros::format_description!(
        "[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT"
    );
    let at = PrimitiveDateTime::parse(value, &format).ok()?.assume_utc();
    let at = sane_reset(at, now)?;
    Duration::try_from(at - now).ok()
}

/// Go-style durations as OpenAI prints them: `6m0s`, `1.5s`, `20ms`, `1h2m3s`.
#[must_use]
pub fn parse_go_duration(value: &str) -> Option<Duration> {
    let mut rest = value.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0_f64;
    while !rest.is_empty() {
        let number_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if number_len == 0 {
            return None;
        }
        let number: f64 = rest[..number_len].parse().ok()?;
        rest = &rest[number_len..];
        let (factor, unit_len) = if rest.starts_with("ms") {
            (0.001, 2)
        } else if rest.starts_with('h') {
            (3600.0, 1)
        } else if rest.starts_with('m') {
            (60.0, 1)
        } else if rest.starts_with('s') {
            (1.0, 1)
        } else {
            return None;
        };
        total += number * factor;
        rest = &rest[unit_len..];
    }
    Some(Duration::from_secs_f64(total))
}

/// Seconds or milliseconds since the epoch (values past year 2286 in
/// seconds are taken as milliseconds).
#[must_use]
pub fn parse_epoch(value: i64) -> Option<OffsetDateTime> {
    let seconds = if value > 9_999_999_999 {
        value / 1000
    } else {
        value
    };
    OffsetDateTime::from_unix_timestamp(seconds).ok()
}

/// The first `YYYY-MM-DD HH:MM:SS` in `text`, read in `tz` and converted to
/// UTC (Z.AI prints China time without an offset).
#[must_use]
pub fn local_time_in_text(text: &str, tz: UtcOffset) -> Option<OffsetDateTime> {
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(18)).find_map(|start| {
        let candidate = text.get(start..start + 19)?;
        if !candidate.as_bytes()[0].is_ascii_digit() {
            return None;
        }
        PrimitiveDateTime::parse(candidate, &format)
            .ok()
            .map(|local| local.assume_offset(tz).to_offset(UtcOffset::UTC))
    })
}

/// Anthropic API key limits: the reset of the exhausted bucket.
fn anthropic_reset(
    headers: &HeaderMap,
    now: OffsetDateTime,
) -> Option<(OffsetDateTime, LimitScope)> {
    let buckets = [
        ("requests", LimitScope::Requests),
        ("input-tokens", LimitScope::InputTokens),
        ("output-tokens", LimitScope::OutputTokens),
        ("tokens", LimitScope::Tokens),
    ];
    let parse = |bucket: &str| {
        let value = header(headers, &format!("anthropic-ratelimit-{bucket}-reset"))?;
        OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
    };
    let exhausted = buckets.iter().find_map(|(bucket, scope)| {
        let remaining = header(headers, &format!("anthropic-ratelimit-{bucket}-remaining"))?;
        (remaining == "0")
            .then(|| parse(bucket).map(|at| (at, *scope)))
            .flatten()
    });
    exhausted
        .or_else(|| {
            buckets
                .iter()
                .find_map(|(bucket, scope)| parse(bucket).map(|at| (at, *scope)))
        })
        .and_then(|(at, scope)| sane_reset(at, now).map(|at| (at, scope)))
}

/// Claude subscription (setup-token) limits.
fn claude_unified_reset(
    headers: &HeaderMap,
    now: OffsetDateTime,
) -> Option<(OffsetDateTime, QuotaWindow)> {
    let at = header(headers, "anthropic-ratelimit-unified-reset")?
        .parse::<i64>()
        .ok()
        .and_then(parse_epoch)
        .and_then(|at| sane_reset(at, now))?;
    let window = match header(headers, "anthropic-ratelimit-unified-representative-claim") {
        Some(claim) if claim.contains("five_hour") => QuotaWindow::Rolling5h,
        Some(claim) if claim.contains("seven_day") => QuotaWindow::Weekly,
        _ => QuotaWindow::Unknown,
    };
    Some((at, window))
}

/// OpenAI per-minute limits.
fn openai_reset(headers: &HeaderMap) -> Option<(Duration, LimitScope)> {
    let requests = header(headers, "x-ratelimit-reset-requests").and_then(parse_go_duration);
    let tokens = header(headers, "x-ratelimit-reset-tokens").and_then(parse_go_duration);
    let exhausted = |name: &str| header(headers, name) == Some("0");
    match (requests, tokens) {
        (Some(r), _) if exhausted("x-ratelimit-remaining-requests") => {
            Some((r, LimitScope::Requests))
        },
        (_, Some(t)) if exhausted("x-ratelimit-remaining-tokens") => Some((t, LimitScope::Tokens)),
        (Some(r), Some(t)) => Some(if r >= t {
            (r, LimitScope::Requests)
        } else {
            (t, LimitScope::Tokens)
        }),
        (Some(r), None) => Some((r, LimitScope::Requests)),
        (None, Some(t)) => Some((t, LimitScope::Tokens)),
        (None, None) => None,
    }
}

/// Codex subscription windows: the exhausted one (used percent >= 100),
/// else the later reset.
fn codex_reset(headers: &HeaderMap, now: OffsetDateTime) -> Option<(OffsetDateTime, QuotaWindow)> {
    let window = |name: &str| {
        let seconds: u64 = header(headers, &format!("x-codex-{name}-reset-after-seconds"))?
            .parse()
            .ok()?;
        let minutes: u64 = header(headers, &format!("x-codex-{name}-window-minutes"))
            .and_then(|m| m.parse().ok())
            .unwrap_or_default();
        let used: f64 = header(headers, &format!("x-codex-{name}-used-percent"))
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let at = now + time::Duration::seconds(i64::try_from(seconds).ok()?);
        Some((at, QuotaWindow::from_minutes(minutes), used >= 100.0))
    };
    let windows: Vec<_> = ["primary", "secondary"]
        .into_iter()
        .filter_map(window)
        .collect();
    let pick = windows
        .iter()
        .filter(|(_, _, exhausted)| *exhausted)
        .max_by_key(|(at, ..)| *at)
        .or_else(|| windows.iter().max_by_key(|(at, ..)| *at))?;
    sane_reset(pick.0, now).map(|at| (at, pick.1))
}

#[derive(Debug, Default, Deserialize)]
struct Envelope {
    #[serde(default)]
    error: Option<ErrorField>,
    #[serde(flatten)]
    top: ErrorBody,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ErrorField {
    Body(ErrorBody),
    Text(String),
}

#[derive(Debug, Default, Deserialize)]
struct ErrorBody {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    code: Option<Code>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    resets_at: Option<i64>,
    #[serde(default)]
    resets_in_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Code {
    Text(String),
    Number(i64),
}

impl Code {
    fn as_string(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Number(number) => number.to_string(),
        }
    }
}

struct ParsedBody {
    kind: String,
    code: String,
    message: String,
    resets_at: Option<i64>,
    resets_in_seconds: Option<u64>,
}

fn parse_body(body: &str) -> ParsedBody {
    let envelope: Envelope = body
        .find('{')
        .and_then(|start| serde_json::from_str(&body[start..]).ok())
        .unwrap_or_default();
    let (inner, text) = match envelope.error {
        Some(ErrorField::Body(inner)) => (inner, None),
        Some(ErrorField::Text(text)) => (ErrorBody::default(), Some(text)),
        None => (ErrorBody::default(), None),
    };
    let top = envelope.top;
    ParsedBody {
        kind: inner
            .kind
            .or(top.kind)
            .unwrap_or_default()
            .to_ascii_lowercase(),
        code: inner
            .code
            .or(top.code)
            .map(|c| c.as_string())
            .unwrap_or_default(),
        message: inner
            .message
            .or(top.message)
            .or(text)
            .unwrap_or_else(|| body.to_string()),
        resets_at: inner.resets_at.or(top.resets_at),
        resets_in_seconds: inner.resets_in_seconds.or(top.resets_in_seconds),
    }
}

/// The window a provider states when rejecting an oversized request.
#[must_use]
pub fn context_limit(message: &str) -> Option<u32> {
    // "maximum context length is 131072 tokens" / "maximum context length of 32768"
    let lower = message.to_ascii_lowercase();
    let index = lower.find("context length")?;
    lower[index..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|chunk| chunk.len() >= 3)
        .and_then(|digits| digits.parse().ok())
}

/// Inputs of [`classify_http`].
pub struct HttpFailure<'a> {
    pub provider: &'a str,
    pub status: u16,
    pub headers: &'a HeaderMap,
    pub body: &'a str,
    /// Timezone of offset-less reset times (`rate_limit_tz` in the catalog).
    pub tz: UtcOffset,
    /// The message to keep on the failure (defaults to the body).
    pub message: Option<String>,
}

/// Classify a failed HTTP response into a typed failure.
#[must_use]
pub fn classify_http(failure: HttpFailure<'_>, now: OffsetDateTime) -> ProviderFailure {
    let parsed = parse_body(failure.body);
    let lower = parsed.message.to_ascii_lowercase();
    let body_lower = failure.body.to_ascii_lowercase();
    let message = failure.message.unwrap_or_else(|| failure.body.to_string());
    let build = |error| ProviderFailure::new(error, failure.provider, message.clone());

    if failure.status == 413
        || CONTEXT_WINDOW_PATTERNS
            .iter()
            .any(|pattern| lower.contains(pattern) || body_lower.contains(pattern))
    {
        return build(ProviderError::ContextWindowExceeded {
            limit: context_limit(&parsed.message),
        });
    }

    let headers = failure.headers;
    let zai_quota = matches!(parsed.code.as_str(), "1308" | "1310");
    let codex_quota = parsed.kind == "usage_limit_reached" || parsed.code == "usage_limit_reached";
    let unified_rejected =
        header(headers, "anthropic-ratelimit-unified-status") == Some("rejected");
    let quota_wording = lower.contains("usage limit") || lower.contains("quota");
    let billing = BILLING_PATTERNS.iter().any(|pattern| {
        lower.contains(pattern) || parsed.code.contains(pattern) || parsed.kind.contains(pattern)
    });

    match failure.status {
        401 | 403 if !(zai_quota || codex_quota) => build(ProviderError::Auth),
        402 => build(ProviderError::Billing),
        _ if billing && !zai_quota && !codex_quota => build(ProviderError::Billing),
        429 | 403 | 400
            if zai_quota
                || codex_quota
                || unified_rejected
                || (failure.status == 429 && quota_wording) =>
        {
            let window_from_text = if lower.contains("5 hour") || lower.contains("5-hour") {
                QuotaWindow::Rolling5h
            } else if lower.contains("week") {
                QuotaWindow::Weekly
            } else if lower.contains("month") {
                QuotaWindow::Monthly
            } else if lower.contains("day") {
                QuotaWindow::Daily
            } else {
                QuotaWindow::Unknown
            };
            let from_codex_body = parsed.resets_at.and_then(parse_epoch).or_else(|| {
                parsed
                    .resets_in_seconds
                    .and_then(|s| i64::try_from(s).ok())
                    .map(|s| now + time::Duration::seconds(s))
            });
            let (resets_at, window) = codex_reset(headers, now)
                .map(|(at, window)| (Some(at), window))
                .or_else(|| {
                    claude_unified_reset(headers, now).map(|(at, window)| (Some(at), window))
                })
                .unwrap_or_else(|| {
                    let at = from_codex_body
                        .or_else(|| local_time_in_text(&parsed.message, failure.tz))
                        .or_else(|| {
                            header(headers, "retry-after")
                                .and_then(|v| parse_retry_after(v, now))
                                .and_then(|d| time::Duration::try_from(d).ok())
                                .map(|d| now + d)
                        });
                    (at, window_from_text)
                });
            let window = if window == QuotaWindow::Unknown {
                window_from_text
            } else {
                window
            };
            build(ProviderError::QuotaExhausted {
                resets_at: resets_at.and_then(|at| sane_reset(at, now)),
                window,
            })
        },
        429 => {
            let retry_after =
                header(headers, "retry-after").and_then(|v| parse_retry_after(v, now));
            let (retry_after, scope) = match (
                retry_after,
                openai_reset(headers),
                anthropic_reset(headers, now),
            ) {
                (Some(delay), Some((_, scope)), _) => (Some(delay), scope),
                (Some(delay), None, Some((_, scope))) => (Some(delay), scope),
                (Some(delay), None, None) => (Some(delay), LimitScope::Unknown),
                (None, Some((delay, scope)), _) => (Some(delay), scope),
                (None, None, Some((at, scope))) => (Duration::try_from(at - now).ok(), scope),
                (None, None, None) => (None, LimitScope::Unknown),
            };
            build(ProviderError::RateLimited { retry_after, scope })
        },
        400 | 404 | 422 => build(ProviderError::InvalidRequest),
        status @ 500..=599 => build(ProviderError::Server { status }),
        // Anthropic's "overloaded" is 529, inside the 5xx range above;
        // anything else is unexpected.
        _ => build(ProviderError::Other),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, reqwest::header::HeaderValue, time::macros::datetime};

    const NOW: OffsetDateTime = datetime!(2026-11-27 12:00 UTC);
    const CHINA: UtcOffset = time::macros::offset!(+8);

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn classify(status: u16, headers: &HeaderMap, body: &str, tz: UtcOffset) -> ProviderError {
        classify_http(
            HttpFailure {
                provider: "p",
                status,
                headers,
                body,
                tz,
                message: None,
            },
            NOW,
        )
        .error
    }

    #[test]
    fn retry_after_seconds_and_http_date() {
        assert_eq!(parse_retry_after("30", NOW), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_retry_after("1.5", NOW),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            parse_retry_after("Fri, 27 Nov 2026 12:05:00 GMT", NOW),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            parse_retry_after("Fri, 27 Nov 2026 11:00:00 GMT", NOW),
            None
        );
        assert_eq!(parse_retry_after("-1", NOW), None);
        assert_eq!(parse_retry_after("soon", NOW), None);
    }

    #[test]
    fn go_durations() {
        assert_eq!(parse_go_duration("6m0s"), Some(Duration::from_secs(360)));
        assert_eq!(parse_go_duration("1.5s"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_go_duration("20ms"), Some(Duration::from_millis(20)));
        assert_eq!(parse_go_duration("1h2m3s"), Some(Duration::from_secs(3723)));
        assert_eq!(parse_go_duration(""), None);
        assert_eq!(parse_go_duration("5x"), None);
        assert_eq!(parse_go_duration("s"), None);
    }

    #[test]
    fn epoch_seconds_and_millis() {
        assert_eq!(parse_epoch(1_796_367_894), parse_epoch(1_796_367_894_999));
        assert!(parse_epoch(1_796_367_894).is_some());
    }

    #[test]
    fn sanity_bounds() {
        assert_eq!(sane_reset(NOW - time::Duration::minutes(1), NOW), None);
        assert_eq!(sane_reset(NOW + time::Duration::days(9), NOW), None);
        assert!(sane_reset(NOW + time::Duration::days(7), NOW).is_some());
    }

    #[test]
    fn glm_offsetless_china_time_across_utc_date_boundary() {
        // 03:15 on Nov 28 in Beijing is 19:15 on Nov 27 UTC.
        let body = r#"{"error":{"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at 2026-11-28 03:15:42"}}"#;
        assert_eq!(
            classify(429, &HeaderMap::new(), body, CHINA),
            ProviderError::QuotaExhausted {
                resets_at: Some(datetime!(2026-11-27 19:15:42 UTC)),
                window: QuotaWindow::Rolling5h,
            }
        );
        // Read in UTC by mistake it would land eight hours later.
        assert_eq!(
            local_time_in_text("reset at 2026-11-28 03:15:42", UtcOffset::UTC),
            Some(datetime!(2026-11-28 03:15:42 UTC))
        );
    }

    #[test]
    fn glm_weekly_limit_without_time_falls_back_to_retry_after() {
        let body = r#"{"error":{"code":1310,"message":"Weekly usage limit reached"}}"#;
        let h = headers(&[("retry-after", "3600")]);
        assert_eq!(
            classify(429, &h, body, CHINA),
            ProviderError::QuotaExhausted {
                resets_at: Some(datetime!(2026-11-27 13:00 UTC)),
                window: QuotaWindow::Weekly,
            }
        );
    }

    #[test]
    fn codex_usage_limit_from_headers_and_body() {
        let body = r#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_in_seconds":7200}}"#;
        let h = headers(&[
            ("x-codex-primary-used-percent", "100.0"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-after-seconds", "3600"),
            ("x-codex-secondary-used-percent", "40.0"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-after-seconds", "400000"),
        ]);
        assert_eq!(
            classify(429, &h, body, UtcOffset::UTC),
            ProviderError::QuotaExhausted {
                resets_at: Some(datetime!(2026-11-27 13:00 UTC)),
                window: QuotaWindow::Rolling5h,
            }
        );
        assert_eq!(
            classify(429, &HeaderMap::new(), body, UtcOffset::UTC),
            ProviderError::QuotaExhausted {
                resets_at: Some(datetime!(2026-11-27 14:00 UTC)),
                window: QuotaWindow::Unknown,
            }
        );
    }

    #[test]
    fn claude_subscription_unified_limit() {
        let reset = (NOW + time::Duration::hours(2))
            .unix_timestamp()
            .to_string();
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-reset", &reset),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
        ]);
        let body =
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"}}"#;
        assert_eq!(
            classify(429, &h, body, UtcOffset::UTC),
            ProviderError::QuotaExhausted {
                resets_at: Some(datetime!(2026-11-27 14:00 UTC)),
                window: QuotaWindow::Rolling5h,
            }
        );
    }

    #[test]
    fn anthropic_api_rate_limit_uses_exhausted_bucket() {
        let h = headers(&[
            ("anthropic-ratelimit-requests-remaining", "10"),
            ("anthropic-ratelimit-requests-reset", "2026-11-27T12:00:30Z"),
            ("anthropic-ratelimit-output-tokens-remaining", "0"),
            (
                "anthropic-ratelimit-output-tokens-reset",
                "2026-11-27T13:00:20+01:00",
            ),
        ]);
        let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
        assert_eq!(
            classify(429, &h, body, UtcOffset::UTC),
            ProviderError::RateLimited {
                retry_after: Some(Duration::from_secs(20)),
                scope: LimitScope::OutputTokens,
            }
        );
    }

    #[test]
    fn openai_rate_limit_durations_and_retry_after() {
        let h = headers(&[
            ("x-ratelimit-remaining-requests", "5"),
            ("x-ratelimit-reset-requests", "1s"),
            ("x-ratelimit-remaining-tokens", "0"),
            ("x-ratelimit-reset-tokens", "6m0s"),
        ]);
        let body = r#"{"error":{"message":"Rate limit reached for tokens","type":"tokens","code":"rate_limit_exceeded"}}"#;
        assert_eq!(
            classify(429, &h, body, UtcOffset::UTC),
            ProviderError::RateLimited {
                retry_after: Some(Duration::from_secs(360)),
                scope: LimitScope::Tokens,
            }
        );
        let with_retry_after = headers(&[("retry-after", "2")]);
        assert_eq!(
            classify(429, &with_retry_after, "Too Many Requests", UtcOffset::UTC),
            ProviderError::RateLimited {
                retry_after: Some(Duration::from_secs(2)),
                scope: LimitScope::Unknown,
            }
        );
    }

    #[test]
    fn billing_auth_context_and_server() {
        let quota = r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#;
        assert_eq!(
            classify(429, &HeaderMap::new(), quota, UtcOffset::UTC),
            ProviderError::Billing
        );
        assert_eq!(
            classify(402, &HeaderMap::new(), "", UtcOffset::UTC),
            ProviderError::Billing
        );
        assert_eq!(
            classify(401, &HeaderMap::new(), "bad key", UtcOffset::UTC),
            ProviderError::Auth
        );
        let context = r#"{"error":{"message":"This model's maximum context length is 131072 tokens. However, you requested 140000 tokens."}}"#;
        assert_eq!(
            classify(400, &HeaderMap::new(), context, UtcOffset::UTC),
            ProviderError::ContextWindowExceeded {
                limit: Some(131_072)
            }
        );
        assert_eq!(
            classify(413, &HeaderMap::new(), "too big", UtcOffset::UTC),
            ProviderError::ContextWindowExceeded { limit: None }
        );
        assert_eq!(
            classify(529, &HeaderMap::new(), "overloaded", UtcOffset::UTC),
            ProviderError::Server { status: 529 }
        );
        assert_eq!(
            classify(400, &HeaderMap::new(), "bad", UtcOffset::UTC),
            ProviderError::InvalidRequest
        );
        assert_eq!(
            classify(302, &HeaderMap::new(), "", UtcOffset::UTC),
            ProviderError::Other
        );
    }

    #[test]
    fn implausible_resets_become_unknown() {
        let body = r#"{"error":{"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at 2020-01-01 00:00:00"}}"#;
        assert_eq!(
            classify(429, &HeaderMap::new(), body, CHINA),
            ProviderError::QuotaExhausted {
                resets_at: None,
                window: QuotaWindow::Rolling5h,
            }
        );
    }

    #[test]
    fn plain_text_error_field() {
        let body = r#"{"error":"usage limit exceeded, quota resets 2026-11-27 21:00:00"}"#;
        assert_eq!(
            classify(429, &HeaderMap::new(), body, CHINA),
            ProviderError::QuotaExhausted {
                resets_at: Some(datetime!(2026-11-27 13:00 UTC)),
                window: QuotaWindow::Unknown,
            }
        );
    }
}
