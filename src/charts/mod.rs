use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Local, Timelike, Utc};
use image::{ExtendedColorType, ImageEncoder};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

// --- ЦВЕТОВАЯ ПАЛИТРА (HA DARK THEME) ---
const HA_BG: RGBColor = RGBColor(28, 28, 28);
const HA_GRID: RGBColor = RGBColor(50, 50, 50);
const HA_TEXT: RGBColor = RGBColor(180, 180, 180);
const HA_BLUE: RGBColor = RGBColor(93, 175, 243);
const HA_BIN_ON: RGBColor = RGBColor(93, 175, 243);
const HA_BIN_OFF: RGBColor = RGBColor(70, 70, 70);
const HA_UNKNOWN: RGBColor = RGBColor(145, 145, 145);

#[derive(Clone, Copy)]
pub enum ChartStyle {
    Numeric,
    Binary,
}

pub struct ChartLabels {
    pub on: String,
    pub off: String,
    pub unknown: String,
}

#[derive(Debug, PartialEq)]
struct HistoryInterval<T> {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    value: Option<T>,
}

/// HA records changes, not periodic samples. Only recorded events may establish a
/// value; a missing event at the left boundary means an unknown initial interval.
fn history_intervals<T: Copy>(
    data: &[(DateTime<Utc>, String)],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    parse: impl Fn(&str) -> Option<T>,
) -> Vec<HistoryInterval<T>> {
    if start >= end {
        return Vec::new();
    }
    let mut events = data.iter().collect::<Vec<_>>();
    events.sort_by_key(|(time, _)| *time);
    let mut cursor = start;
    let mut value = None;
    let mut intervals = Vec::new();
    for (time, state) in events {
        if *time >= end {
            break;
        }
        if *time > cursor {
            intervals.push(HistoryInterval {
                start: cursor,
                end: *time,
                value,
            });
            cursor = *time;
        }
        value = parse(state);
    }
    intervals.push(HistoryInterval {
        start: cursor,
        end,
        value,
    });
    intervals
}

fn numeric_value(state: &str) -> Option<f64> {
    state.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn binary_value(state: &str) -> Option<bool> {
    match state {
        "on" | "open" | "detected" | "unlocked" | "home" | "true" => Some(true),
        "off" | "closed" | "clear" | "locked" | "not_home" | "false" => Some(false),
        _ => None,
    }
}

pub fn has_history_data(
    data: &[(DateTime<Utc>, String)],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    style: ChartStyle,
) -> bool {
    match style {
        ChartStyle::Numeric => history_intervals(data, start, end, numeric_value)
            .iter()
            .any(|interval| interval.value.is_some()),
        ChartStyle::Binary => history_intervals(data, start, end, binary_value)
            .iter()
            .any(|interval| interval.value.is_some()),
    }
}

/// Точка входа для отрисовки.
/// Вычисляет временные границы один раз для всех под-систем.
pub fn draw_ha_chart(
    data: &[(DateTime<Utc>, String)],
    title: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    style: ChartStyle,
    labels: &ChartLabels,
) -> Result<Vec<u8>> {
    if !has_history_data(data, start_time, end_time, style) {
        return Err(anyhow!("Данные отсутствуют"));
    }

    let width = 1000;
    let height = if matches!(style, ChartStyle::Binary) {
        400
    } else {
        600
    };
    let mut buffer = vec![0u8; width * height * 3];

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (width as u32, height as u32))
            .into_drawing_area();
        root.fill(&HA_BG)?;

        match style {
            ChartStyle::Numeric => {
                render_numeric(&root, data, title, start_time, end_time, labels)?
            }
            ChartStyle::Binary => render_binary(&root, data, title, start_time, end_time, labels)?,
        }
        root.present()?;
    }
    encode_png(&buffer, width, height)
}

fn render_numeric<B: DrawingBackend>(
    root: &DrawingArea<B, plotters::coord::Shift>,
    data: &[(DateTime<Utc>, String)],
    title: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    labels: &ChartLabels,
) -> Result<()>
where
    B::ErrorType: 'static,
{
    let intervals = history_intervals(data, start_time, end_time, numeric_value);
    let parsed_data = intervals
        .iter()
        .filter_map(|interval| interval.value)
        .collect::<Vec<_>>();
    if parsed_data.is_empty() {
        return Err(anyhow!("No numeric history data"));
    }

    // Расчет Y-оси
    let min_val = parsed_data.iter().copied().fold(f64::INFINITY, f64::min);
    let max_val = parsed_data
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let range = (max_val - min_val).max(1.0);
    let y_min = min_val - range * 0.2;
    let y_max = max_val + range * 0.2;

    let mut chart = ChartBuilder::on(root)
        .caption(title, ("sans-serif", 25).into_font().color(&HA_TEXT))
        .margin(30)
        .x_label_area_size(80)
        .y_label_area_size(60)
        .build_cartesian_2d(start_time..end_time, y_min..y_max)?;

    chart
        .configure_mesh()
        .x_labels(8)
        .y_labels(6)
        .disable_x_mesh()
        .axis_style(HA_GRID)
        .label_style(("sans-serif", 15).into_font().color(&HA_TEXT))
        .x_label_formatter(&|x| x.with_timezone(&Local).format("%H:%M").to_string())
        .draw()?;

    draw_date_separators(&mut chart, start_time, end_time, y_min, y_max)?;

    let mut stepped = Vec::new();
    for interval in intervals {
        if let Some(value) = interval.value {
            stepped.push((interval.start, value));
            stepped.push((interval.end, value));
        } else {
            if !stepped.is_empty() {
                chart.draw_series(LineSeries::new(
                    std::mem::take(&mut stepped),
                    HA_BLUE.stroke_width(2),
                ))?;
            }
            chart.draw_series(std::iter::once(Rectangle::new(
                [(interval.start, y_min), (interval.end, y_max)],
                HA_UNKNOWN.mix(0.08).filled(),
            )))?;
            let step = (end_time - start_time) / 80;
            if step > Duration::zero() {
                let mut time = interval.start;
                while time < interval.end {
                    chart.draw_series(std::iter::once(PathElement::new(
                        vec![(time, y_min), ((time + step).min(interval.end), y_max)],
                        HA_UNKNOWN.mix(0.25).stroke_width(1),
                    )))?;
                    time += step;
                }
            }
        }
    }
    if !stepped.is_empty() {
        chart.draw_series(LineSeries::new(stepped, HA_BLUE.stroke_width(2)))?;
    }
    root.draw(&Text::new(
        format!("/// {}", labels.unknown),
        (70, 560),
        ("sans-serif", 16).into_font().color(&HA_TEXT),
    ))?;
    Ok(())
}

fn render_binary<B: DrawingBackend>(
    root: &DrawingArea<B, plotters::coord::Shift>,
    data: &[(DateTime<Utc>, String)],
    title: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    labels: &ChartLabels,
) -> Result<()>
where
    B::ErrorType: 'static,
{
    let (y_min, y_max) = (0i32, 100i32);

    let mut chart = ChartBuilder::on(root)
        .caption(title, ("sans-serif", 20).into_font().color(&HA_TEXT))
        .margin(20)
        .x_label_area_size(80)
        .build_cartesian_2d(start_time..end_time, y_min..y_max)?;

    chart
        .configure_mesh()
        .disable_y_axis()
        .disable_y_mesh()
        .axis_style(HA_GRID)
        .x_labels(8)
        .label_style(("sans-serif", 14).into_font().color(&HA_TEXT))
        .x_label_formatter(&|x| x.with_timezone(&Local).format("%H:%M").to_string())
        .draw()?;

    draw_date_separators(&mut chart, start_time, end_time, y_min, y_max)?;

    for interval in history_intervals(data, start_time, end_time, binary_value) {
        let (low, high, color) = match interval.value {
            Some(true) => (50, 80, HA_BIN_ON),
            Some(false) => (20, 50, HA_BIN_OFF),
            None => (20, 80, HA_BG),
        };
        chart.draw_series(std::iter::once(Rectangle::new(
            [(interval.start, low), (interval.end, high)],
            color.filled(),
        )))?;
        if interval.value.is_none() {
            let step = (end_time - start_time) / 80;
            if step > Duration::zero() {
                let mut time = interval.start;
                while time < interval.end {
                    chart.draw_series(std::iter::once(PathElement::new(
                        vec![(time, low), ((time + step).min(interval.end), high)],
                        HA_UNKNOWN.stroke_width(1),
                    )))?;
                    time += step;
                }
            }
        }
    }
    root.draw(&Text::new(
        format!(
            "↑ {}    ↓ {}    /// {}",
            labels.on, labels.off, labels.unknown
        ),
        (30, 360),
        ("sans-serif", 16).into_font().color(&HA_TEXT),
    ))?;
    Ok(())
}

fn draw_date_separators<B, X, Y>(
    chart: &mut ChartContext<B, Cartesian2d<X, Y>>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    y_min: Y::ValueType,
    y_max: Y::ValueType,
) -> Result<()>
where
    B: DrawingBackend,
    X: Ranged<ValueType = DateTime<Utc>>,
    Y: Ranged,
    Y::ValueType: Clone + 'static,
    B::ErrorType: 'static,
{
    let mut curr = start.with_timezone(&Local);
    while curr <= end.with_timezone(&Local) {
        let midnight = curr
            .with_hour(0)
            .unwrap()
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap()
            .with_timezone(&Utc);

        if midnight > start && midnight < end {
            chart.draw_series(std::iter::once(PathElement::new(
                vec![(midnight, y_min.clone()), (midnight, y_max.clone())],
                HA_GRID.stroke_width(1).color.mix(0.3),
            )))?;

            let label_time = midnight + Duration::hours(12);
            if label_time >= start && label_time <= end {
                chart.draw_series(std::iter::once(Text::new(
                    curr.format("%d.%m").to_string(),
                    (label_time, y_min.clone()),
                    ("sans-serif", 15)
                        .into_font()
                        .color(&HA_TEXT)
                        .pos(Pos::new(HPos::Center, VPos::Top)),
                )))?;
            }
        }
        curr += Duration::days(1);
    }
    Ok(())
}

fn encode_png(buffer: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png_bytes)
        .write_image(buffer, w as u32, h as u32, ExtendedColorType::Rgb8)
        .map_err(|e| anyhow!("PNG Error: {}", e))?;
    Ok(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(hour: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(hour * 3600, 0).unwrap()
    }

    fn data(values: &[&str]) -> Vec<(DateTime<Utc>, String)> {
        values
            .iter()
            .enumerate()
            .map(|(n, value)| (time(n as i64), (*value).into()))
            .collect()
    }

    #[test]
    fn numeric_unknown_events_break_the_history_without_backfilling() {
        let intervals = history_intervals(
            &data(&["20", "unavailable", "22"]),
            time(0),
            time(3),
            numeric_value,
        );
        assert_eq!(
            intervals,
            vec![
                HistoryInterval {
                    start: time(0),
                    end: time(1),
                    value: Some(20.0)
                },
                HistoryInterval {
                    start: time(1),
                    end: time(2),
                    value: None
                },
                HistoryInterval {
                    start: time(2),
                    end: time(3),
                    value: Some(22.0)
                },
            ]
        );
        for state in [
            "unknown",
            "unavailable",
            "text",
            "NaN",
            "inf",
            "-inf",
            "Infinity",
        ] {
            assert_eq!(numeric_value(state), None, "{state}");
            assert!(!has_history_data(
                &data(&[state]),
                time(0),
                time(5),
                ChartStyle::Numeric
            ));
        }
    }

    #[test]
    fn binary_history_has_three_states_and_unknown_is_not_inverted() {
        let events = data(&["off", "unavailable", "on"]);
        let values = |events: &[(DateTime<Utc>, String)]| {
            history_intervals(events, time(0), time(3), binary_value)
                .into_iter()
                .map(|interval| interval.value)
                .collect::<Vec<_>>()
        };
        assert_eq!(values(&events), [Some(false), None, Some(true)]);
        let inverted = events
            .into_iter()
            .map(|(time, state)| {
                (
                    time,
                    crate::core::presentation::StateFormatter::logical_state(&state, true),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(values(&inverted), [Some(true), None, Some(false)]);
        assert_eq!(binary_value("anything else"), None);
    }

    #[test]
    fn event_boundaries_and_last_known_state_keep_step_semantics() {
        let start = time(0);
        let end = time(5);
        assert_eq!(
            history_intervals(&[(time(2), "20".into())], start, end, numeric_value),
            vec![
                HistoryInterval {
                    start,
                    end: time(2),
                    value: None
                },
                HistoryInterval {
                    start: time(2),
                    end,
                    value: Some(20.0)
                },
            ]
        );
        assert_eq!(
            history_intervals(
                &[(time(-1), "20".into()), (end, "30".into())],
                start,
                end,
                numeric_value
            ),
            vec![HistoryInterval {
                start,
                end,
                value: Some(20.0)
            }]
        );
        assert!(!has_history_data(
            &[(end, "30".into())],
            start,
            end,
            ChartStyle::Numeric
        ));
        assert!(!has_history_data(&[], start, end, ChartStyle::Binary));
        let unordered = vec![
            (time(2), "25".into()),
            (start, "20".into()),
            (time(2), "unavailable".into()),
        ];
        let intervals = history_intervals(&unordered, start, end, numeric_value);
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[1].value, None);
    }

    #[test]
    fn charts_with_unknown_segments_render_as_png() -> Result<()> {
        let labels = ChartLabels {
            on: "Open".into(),
            off: "Closed".into(),
            unknown: "No data".into(),
        };
        for (values, style) in [
            (["20", "unavailable", "22"], ChartStyle::Numeric),
            (["off", "unknown", "on"], ChartStyle::Binary),
        ] {
            let png = draw_ha_chart(&data(&values), "History", time(0), time(3), style, &labels)?;
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
        Ok(())
    }
}
