//! Bounded overview of HA change history for Telegram's 1 KiB widget state.
//! Each time bucket retains its minimum, maximum, and final value. A bucket
//! overlapping an unknown interval stays a gap, never an interpolated value.
use super::{binary_value, history_intervals, numeric_value, ChartLabels, ChartStyle};
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

const BUCKETS: usize = 32;
const LEVELS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidgetChart {
    pub plot: String,
    pub top: String,
    pub bottom: String,
    pub start: String,
    pub end: String,
}

pub fn overview(
    points: &[(DateTime<Utc>, String)],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    style: ChartStyle,
    labels: &ChartLabels,
) -> WidgetChart {
    let intervals = history_intervals(points, start, end, |state| match style {
        ChartStyle::Numeric => numeric_value(state),
        ChartStyle::Binary => binary_value(state).map(|value| f64::from(u8::from(value))),
    });
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for value in intervals.iter().filter_map(|interval| interval.value) {
        min = min.min(value);
        max = max.max(value);
    }
    let has_data = min.is_finite() && max.is_finite();
    if matches!(style, ChartStyle::Binary) {
        min = 0.0;
        max = 1.0;
    } else if has_data && min == max {
        let pad = (min.abs() * 0.05).max(0.5);
        min = (min - pad).max(-f64::MAX);
        max = (max + pad).min(f64::MAX);
    }
    // Scale first so even widely separated finite values cannot overflow.
    let scale = min.abs().max(max.abs()).max(1.0);
    let encode = |value: f64| {
        let y = ((value / scale - min / scale) / (max / scale - min / scale) * 63.0)
            .round()
            .clamp(0.0, 63.0) as usize;
        LEVELS[y] as char
    };
    let mut plot = String::with_capacity(BUCKETS * 3);
    let span = end - start;
    for bucket in 0..BUCKETS {
        let left = start + span * bucket as i32 / BUCKETS as i32;
        let right = start + span * (bucket + 1) as i32 / BUCKETS as i32;
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        let mut last = None;
        let mut unknown = false;
        for interval in intervals.iter().filter(|i| i.end > left && i.start < right) {
            if let Some(value) = interval.value {
                low = low.min(value);
                high = high.max(value);
                last = Some(value);
            } else {
                unknown = true;
            }
        }
        match last.filter(|_| !unknown) {
            Some(value) => {
                plot.push(encode(low));
                plot.push(encode(high));
                plot.push(encode(value));
            }
            None => plot.push_str("..."),
        }
    }
    let (top, bottom) = if !has_data {
        (String::new(), String::new())
    } else if matches!(style, ChartStyle::Binary) {
        (labels.on.clone(), labels.off.clone())
    } else {
        (axis_label(max), axis_label(min))
    };
    WidgetChart {
        plot,
        top,
        bottom,
        start: start
            .with_timezone(&Local)
            .format("%d.%m %H:%M")
            .to_string(),
        end: end.with_timezone(&Local).format("%d.%m %H:%M").to_string(),
    }
}

fn axis_label(value: f64) -> String {
    if value != 0.0 && (value.abs() < 0.001 || value.abs() >= 1_000_000.0) {
        format!("{value:.3e}")
    } else {
        format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn chart(events: &[(i64, &str)], style: ChartStyle) -> WidgetChart {
        let start = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        overview(
            &events
                .iter()
                .map(|(seconds, state)| (start + Duration::seconds(*seconds), (*state).into()))
                .collect::<Vec<_>>(),
            start,
            start + Duration::seconds(320),
            style,
            &ChartLabels {
                on: "Открыто".into(),
                off: "Закрыто".into(),
                unknown: "Нет данных".into(),
            },
        )
    }

    #[test]
    fn preserves_extrema_inside_a_bucket_and_ignores_future_events() {
        let c = chart(
            &[(0, "10"), (2, "100"), (4, "20"), (320, "999")],
            ChartStyle::Numeric,
        );
        assert_eq!(c.plot.len(), 96);
        assert_eq!(&c.plot[..2], "A_");
        assert_eq!(c.plot.as_bytes()[2], LEVELS[7]);
        assert_eq!(c.top, "100");
        assert_eq!(c.bottom, "10");
        assert!(serde_json::to_string(&c).unwrap().len() < 250);
    }

    #[test]
    fn unknown_partial_buckets_are_gaps_and_a_real_event_restores_data() {
        let c = chart(
            &[(5, "20"), (12, "unavailable"), (25, "30")],
            ChartStyle::Numeric,
        );
        assert_eq!(&c.plot[..9], ".........");
        assert_eq!(&c.plot[9..12], "___");
    }

    #[test]
    fn binary_history_retains_mixed_bucket_and_localized_axis() {
        let c = chart(&[(-1, "off"), (4, "on"), (8, "off")], ChartStyle::Binary);
        assert_eq!(&c.plot[..3], "A_A");
        assert_eq!(&c.plot[3..], "A".repeat(93));
        assert_eq!((c.top.as_str(), c.bottom.as_str()), ("Открыто", "Закрыто"));
    }

    #[test]
    fn nonfinite_and_absent_history_never_become_zero() {
        for events in [vec![], vec![(0, "NaN"), (20, "inf"), (30, "unknown")]] {
            let c = chart(&events, ChartStyle::Numeric);
            assert_eq!(c.plot, ".".repeat(96));
            assert!(c.top.is_empty() && c.bottom.is_empty());
        }
        let c = chart(&[(0, "21.5")], ChartStyle::Numeric);
        assert!(!c.plot.contains('.'));
        assert_ne!(c.top, c.bottom);
    }
}
