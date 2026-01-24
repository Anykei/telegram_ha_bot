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

#[derive(Clone, Copy)]
pub enum ChartStyle {
    Numeric,
    Binary,
}

/// Точка входа для отрисовки.
/// Вычисляет временные границы один раз для всех под-систем.
pub fn draw_ha_chart(
    data: &[(DateTime<Utc>, String)],
    title: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    style: ChartStyle,
) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Err(anyhow!("Данные отсутствуют"));
    }

    let width = 1000;
    let height = if matches!(style, ChartStyle::Binary) { 400 } else { 600 };
    let mut buffer = vec![0u8; width * height * 3];

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (width as u32, height as u32)).into_drawing_area();
        root.fill(&HA_BG)?;

        match style {
            ChartStyle::Numeric => render_numeric(&root, data, title, start_time, end_time)?,
            ChartStyle::Binary => render_binary(&root, data, title, start_time, end_time)?,
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
) -> Result<()> where B::ErrorType: 'static {
    // Безопасный парсинг данных
    let parsed_data: Vec<(DateTime<Utc>, f64)> = data.iter()
        .filter_map(|(t, s)| s.parse::<f64>().ok().map(|v| (*t, v)))
        .collect();

    if parsed_data.is_empty() { return Err(anyhow!("Ошибка парсинга числовых данных")); }

    // Расчет Y-оси
    let min_val = parsed_data.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
    let max_val = parsed_data.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max);
    let range = (max_val - min_val).max(1.0);
    let y_min = min_val - range * 0.2;
    let y_max = max_val + range * 0.2;

    let mut chart = ChartBuilder::on(root)
        .caption(title, ("sans-serif", 25).into_font().color(&HA_TEXT))
        .margin(30).x_label_area_size(80).y_label_area_size(60)
        .build_cartesian_2d(start_time..end_time, y_min..y_max)?;

    chart.configure_mesh()
        .x_labels(8).y_labels(6).disable_x_mesh()
        .axis_style(HA_GRID).label_style(("sans-serif", 15).into_font().color(&HA_TEXT))
        .x_label_formatter(&|x| x.with_timezone(&Local).format("%H:%M").to_string())
        .draw()?;

    draw_date_separators(&mut chart, start_time, end_time, y_min, y_max)?;

    // Отрисовка "ступенчатого" графика (HA Style)
    let mut stepped = Vec::new();
    if !parsed_data.is_empty() {
        for i in 0..parsed_data.len() - 1 {
            stepped.push((parsed_data[i].0, parsed_data[i].1));
            stepped.push((parsed_data[i+1].0, parsed_data[i].1));
        }
        if let Some(&last) = parsed_data.last() {
            stepped.push(last);
            stepped.push((end_time, last.1));
        }
    }

    chart.draw_series(LineSeries::new(stepped, HA_BLUE.stroke_width(2)))?;
    Ok(())
}

fn render_binary<B: DrawingBackend>(
    root: &DrawingArea<B, plotters::coord::Shift>,
    data: &[(DateTime<Utc>, String)],
    title: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
) -> Result<()> where B::ErrorType: 'static {
    let (y_min, y_max) = (0i32, 100i32);

    let mut chart = ChartBuilder::on(root)
        .caption(title, ("sans-serif", 20).into_font().color(&HA_TEXT))
        .margin(20).x_label_area_size(80)
        .build_cartesian_2d(start_time..end_time, y_min..y_max)?;

    chart.configure_mesh()
        .disable_y_axis().disable_y_mesh().axis_style(HA_GRID).x_labels(8)
        .label_style(("sans-serif", 14).into_font().color(&HA_TEXT))
        .x_label_formatter(&|x| x.with_timezone(&Local).format("%H:%M").to_string())
        .draw()?;

    draw_date_separators(&mut chart, start_time, end_time, y_min, y_max)?;

    for i in 0..data.len() {
        let t_start = data[i].0;
        let is_on = is_state_on(&data[i].1);
        let t_end = if i + 1 < data.len() { data[i+1].0 } else { end_time };

        let actual_ts = t_start.max(start_time);
        let actual_te = t_end.min(end_time);

        if actual_te > actual_ts {
            let color = if is_on { HA_BIN_ON } else { HA_BIN_OFF };
            chart.draw_series(std::iter::once(Rectangle::new(
                [(actual_ts, 30), (actual_te, 70)],
                color.filled(),
            )))?;

            if is_on {
                chart.draw_series(std::iter::once(PathElement::new(
                    vec![(actual_ts, 30), (actual_te, 30)],
                    HA_BLUE.stroke_width(2),
                )))?;
            }
        }
    }
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
        let midnight = curr.with_hour(0).unwrap().with_minute(0).unwrap().with_second(0).unwrap().with_timezone(&Utc);

        if midnight > start && midnight < end {
            chart.draw_series(std::iter::once(PathElement::new(
                vec![(midnight, y_min.clone()), (midnight, y_max.clone())],
                HA_GRID.stroke_width(1).color.mix(0.3),
            )))?;

            let label_time = midnight + Duration::hours(12);
            if label_time >= start && label_time <= end {
                chart.draw_series(std::iter::once(Text::new(
                    curr.format("%d %b").to_string(),
                    (label_time, y_min.clone()),
                    ("sans-serif", 15).into_font().color(&HA_TEXT).pos(Pos::new(HPos::Center, VPos::Top)),
                )))?;
            }
        }
        curr = curr + Duration::days(1);
    }
    Ok(())
}

fn is_state_on(state: &str) -> bool {
    let s = state.to_lowercase();
    matches!(s.as_str(), "on" | "open" | "detected" | "unlocked" | "home")
}

fn encode_png(buffer: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png_bytes)
        .write_image(buffer, w as u32, h as u32, ExtendedColorType::Rgb8)
        .map_err(|e| anyhow!("PNG Error: {}", e))?;
    Ok(png_bytes)
}