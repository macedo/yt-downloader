//! Clip times: parsing user input and formatting for file names.

/// Parses "90", "1:30" or "1:02:03" as seconds. Empty = `Some(None)`; invalid = `None`.
pub fn parse_time(s: &str) -> Option<Option<u64>> {
    let s = s.trim();
    if s.is_empty() {
        return Some(None);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let nums: Vec<u64> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    // Minutes and seconds after the first field must be below 60.
    if nums.iter().skip(1).any(|&n| n >= 60) {
        return None;
    }
    Some(Some(nums.iter().fold(0, |acc, n| acc * 60 + n)))
}

/// Time for file names (no ":", which Windows rejects): 1m30s, 1h02m03s.
pub fn format_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m{s:02}s"),
        _ => format!("{h}h{m:02}m{s:02}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_time, parse_time};

    #[test]
    fn parses_clip_times() {
        assert_eq!(parse_time(""), Some(None));
        assert_eq!(parse_time("  "), Some(None));
        assert_eq!(parse_time("90"), Some(Some(90)));
        assert_eq!(parse_time("1:30"), Some(Some(90)));
        assert_eq!(parse_time("01:02:03"), Some(Some(3723)));
        assert_eq!(parse_time("75:00"), Some(Some(4500)));
        assert_eq!(parse_time("1:60"), None);
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time("1:"), None);
        assert_eq!(parse_time("-5"), None);
        assert_eq!(parse_time("1.5"), None);
        assert_eq!(parse_time("abc"), None);
    }

    #[test]
    fn formats_time_for_file_names() {
        assert_eq!(format_time(0), "0s");
        assert_eq!(format_time(45), "45s");
        assert_eq!(format_time(90), "1m30s");
        assert_eq!(format_time(3723), "1h02m03s");
    }
}
