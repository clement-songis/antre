//! Helpers for turning provider `/models` payloads into catalog metadata.

/// A display name the API returned, if it is more than the id echoed back.
pub(crate) fn api_name(model_id: &str, display_name: Option<&str>) -> Option<String> {
    display_name
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != model_id)
        .map(str::to_string)
}

/// Parse `YYYY-MM-DD` or an RFC 3339 timestamp into a date.
pub(crate) fn parse_date(value: &str) -> Option<time::Date> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    let date_part = value.get(..10).unwrap_or(value);
    time::Date::parse(date_part, &format).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_name_ignores_echoed_ids() {
        assert_eq!(api_name("m", Some("m")), None);
        assert_eq!(api_name("m", Some("  ")), None);
        assert_eq!(api_name("m", None), None);
        assert_eq!(api_name("m", Some(" Model M ")), Some("Model M".into()));
    }

    #[test]
    fn parse_date_accepts_dates_and_timestamps() {
        assert_eq!(
            parse_date("2026-12-11"),
            Some(time::macros::date!(2026 - 12 - 11))
        );
        assert_eq!(
            parse_date("2027-01-22T00:00:00Z"),
            Some(time::macros::date!(2027 - 01 - 22))
        );
        assert_eq!(parse_date("soon"), None);
    }
}
