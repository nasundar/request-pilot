/// Duration parsing utilities for auto-run intervals.
///
/// Supported formats: `30s`, `5m`, `1h`, `2d` (seconds, minutes, hours, days).

/// Parse a human-readable duration string into seconds.
///
/// Accepts formats like "30s", "5m", "1h", "2d".
/// Returns `None` for invalid input.
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, suffix) = s.split_at(s.len() - 1);
    let multiplier = match suffix {
        "s" => 1u64,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };

    let n: u64 = num_str.parse().ok()?;
    if n == 0 {
        return None;
    }

    n.checked_mul(multiplier)
}

/// Format seconds back into a human-readable duration string.
///
/// Picks the largest clean unit (d > h > m > s).
pub fn format_duration(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

/// Format a remaining-seconds count as "M:SS" or "H:MM:SS".
pub fn format_countdown(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        let m = secs / 60;
        let s = secs % 60;
        format!("{}:{:02}", m, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_seconds() {
        assert_eq!(parse_duration_secs("30s"), Some(30));
        assert_eq!(parse_duration_secs("1s"), Some(1));
        assert_eq!(parse_duration_secs("120s"), Some(120));
    }

    #[test]
    fn test_parse_minutes() {
        assert_eq!(parse_duration_secs("5m"), Some(300));
        assert_eq!(parse_duration_secs("1m"), Some(60));
        assert_eq!(parse_duration_secs("15m"), Some(900));
        assert_eq!(parse_duration_secs("30m"), Some(1800));
    }

    #[test]
    fn test_parse_hours() {
        assert_eq!(parse_duration_secs("1h"), Some(3600));
        assert_eq!(parse_duration_secs("2h"), Some(7200));
        assert_eq!(parse_duration_secs("4h"), Some(14400));
    }

    #[test]
    fn test_parse_days() {
        assert_eq!(parse_duration_secs("1d"), Some(86400));
        assert_eq!(parse_duration_secs("7d"), Some(604800));
    }

    #[test]
    fn test_parse_invalid() {
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("abc"), None);
        assert_eq!(parse_duration_secs("m"), None);
        assert_eq!(parse_duration_secs("0m"), None);
        assert_eq!(parse_duration_secs("5x"), None);
        assert_eq!(parse_duration_secs("-1m"), None);
        assert_eq!(parse_duration_secs("5"), None);
        // Overflow: huge values should return None, not panic
        assert_eq!(parse_duration_secs("999999999999999999m"), None);
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(parse_duration_secs("  5m  "), Some(300));
        assert_eq!(parse_duration_secs(" 1h "), Some(3600));
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(300), "5m");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(7200), "2h");
        assert_eq!(format_duration(86400), "1d");
        assert_eq!(format_duration(90), "90s"); // not cleanly minutes
        assert_eq!(format_duration(0), "0s");
    }

    #[test]
    fn test_format_countdown() {
        assert_eq!(format_countdown(0), "0:00");
        assert_eq!(format_countdown(5), "0:05");
        assert_eq!(format_countdown(65), "1:05");
        assert_eq!(format_countdown(3661), "1:01:01");
        assert_eq!(format_countdown(900), "15:00");
    }

    #[test]
    fn test_roundtrip() {
        for input in &["30s", "5m", "15m", "1h", "2h", "4h", "1d"] {
            let secs = parse_duration_secs(input).unwrap();
            assert_eq!(format_duration(secs), *input);
        }
    }
}
