//! [`ScheduleSpec`]: the cadence half of a `schedules:` entry, plus the
//! interval and time-of-day string parsers it is built from.

/// A parsed schedule specification: the cadence and its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleSpec {
    /// `every "5m"` -- an interval, normalised to seconds.
    Every {
        /// The interval in seconds.
        seconds: u64,
    },
    /// `daily "03:00"` -- a time of day, UTC.
    Daily {
        /// The hour, 0-23.
        hour: u8,
        /// The minute, 0-59.
        minute: u8,
    },
}

/// Parses an interval string like `"5m"`, `"1h"`, `"30s"`, or `"1d"` into
/// seconds. Returns `None` for an unrecognised format.
pub fn parse_interval(s: &str) -> Option<u64> {
    let (number, unit) = s.split_at_checked(s.len().checked_sub(1)?)?;
    let n: u64 = number.parse().ok()?;
    match unit {
        "s" => Some(n),
        "m" => n.checked_mul(60),
        "h" => n.checked_mul(3600),
        "d" => n.checked_mul(86400),
        _ => None,
    }
}

/// Parses a time string `"HH:MM"` into `(hour, minute)`. Returns `None` for
/// an unrecognised format or an out-of-range value.
pub fn parse_time(s: &str) -> Option<(u8, u8)> {
    let mut parts = s.split(':');
    let hour: u8 = parts.next()?.parse().ok()?;
    let minute: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_units_normalise_to_seconds() {
        assert_eq!(parse_interval("30s"), Some(30));
        assert_eq!(parse_interval("5m"), Some(300));
        assert_eq!(parse_interval("2h"), Some(7200));
        assert_eq!(parse_interval("1d"), Some(86_400));
    }

    #[test]
    fn interval_rejects_unknown_units_and_junk() {
        assert_eq!(parse_interval("5w"), None);
        assert_eq!(parse_interval("m"), None);
        assert_eq!(parse_interval(""), None);
        assert_eq!(parse_interval("abc"), None);
    }

    #[test]
    fn time_of_day_parses_hour_and_minute() {
        assert_eq!(parse_time("03:00"), Some((3, 0)));
        assert_eq!(parse_time("23:59"), Some((23, 59)));
    }

    #[test]
    fn time_of_day_rejects_out_of_range_and_malformed_values() {
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("12:60"), None);
        assert_eq!(parse_time("12"), None);
        assert_eq!(parse_time("1:2:3"), None);
    }
}
