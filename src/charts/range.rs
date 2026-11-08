use chrono::{DateTime, Duration, Utc};

pub const MAX_SPAN: i64 = 10 * 24 * 3600;
pub const LOOKBACK: i64 = 20 * 24 * 3600;

/// UI policy, not a claim about the HA recorder's actual retention.
pub fn validate(start: i64, end: i64, now: DateTime<Utc>) -> Result<(), &'static str> {
    let span = end.checked_sub(start).ok_or("order")?;
    if span < 60 {
        return Err("order");
    }
    if span > MAX_SPAN {
        return Err("span");
    }
    if end > now.timestamp() {
        return Err("future");
    }
    if start < (now - Duration::seconds(LOOKBACK)).timestamp() {
        return Err("past");
    }
    if DateTime::from_timestamp(start, 0).is_none() || DateTime::from_timestamp(end, 0).is_none() {
        return Err("order");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_minute_ranges_without_overflow_or_future_dates() {
        let now = DateTime::parse_from_rfc3339("2026-11-07T08:00:00+03:00")
            .unwrap()
            .with_timezone(&Utc);
        let end = now.timestamp();
        assert_eq!(validate(end - 97 * 60, end, now), Ok(()));
        assert_eq!(validate(end - 60, end, now), Ok(()));
        for (s, e) in [
            (end, end),
            (end, end + 60),
            (end - MAX_SPAN - 60, end),
            (end - LOOKBACK - 60, end - LOOKBACK),
            (i64::MIN, i64::MAX),
        ] {
            assert!(validate(s, e, now).is_err());
        }
    }
}
