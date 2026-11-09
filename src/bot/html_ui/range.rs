//! Calendar drafts belong to one admitted viewer and never execute HA services.
use super::presentation::{Button, Page, Prepared};
use crate::bot::{
    models::View,
    router::{ChartRangeEdit as Edit, ControlPayload, DeviceCmd, Payload, RenderContext},
};
use crate::{
    charts::range as limits,
    db,
    i18n::{t, Language},
};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use std::collections::HashMap;
use teloxide::types::{
    InlineKeyboardButton as Key, InlineKeyboardButtonKind, InlineKeyboardMarkup,
};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Overview,
    Calendar,
    Time,
}
#[derive(Clone)]
pub(super) struct Draft {
    id: u64,
    generation: String,
    room: i64,
    device: i64,
    original: View,
    bounds: [i64; 2],
    earliest: i64,
    latest: i64,
    mode: Mode,
    end: bool,
    month: NaiveDate,
    error: Option<String>,
}
fn words(lang: Language, ru: &str, en: &str) -> String {
    if lang == Language::Ru { ru } else { en }.into()
}
fn at(value: i64) -> DateTime<Local> {
    DateTime::from_timestamp(value, 0)
        .expect("validated range")
        .with_timezone(&Local)
}
fn payload(room: i64, device: i64, cmd: DeviceCmd) -> Payload {
    Payload::Control(ControlPayload::QuickAction { room, device, cmd })
}
fn key(label: impl Into<String>, p: Payload) -> Key {
    Key::callback(label, p.to_string())
}
fn month_name(month: u32, lang: Language) -> &'static str {
    let names = if lang == Language::Ru {
        [
            "Январь",
            "Февраль",
            "Март",
            "Апрель",
            "Май",
            "Июнь",
            "Июль",
            "Август",
            "Сентябрь",
            "Октябрь",
            "Ноябрь",
            "Декабрь",
        ]
    } else {
        [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ]
    };
    names[(month - 1) as usize]
}
fn date(value: i64) -> String {
    at(value).format("%d.%m.%Y").to_string()
}
fn time(value: i64) -> String {
    at(value).format("%H:%M").to_string()
}
fn message(code: &str, lang: Language) -> String {
    let (ru, en) = match code {
        "order" => (
            "Конец должен быть позже начала минимум на минуту.",
            "End must be at least a minute after start.",
        ),
        "span" => (
            "Выберите период не длиннее 10 суток.",
            "Choose a range no longer than 10 days.",
        ),
        "future" => (
            "Конец периода не может быть в будущем.",
            "The range cannot end in the future.",
        ),
        "past" => (
            "Выберите начало в пределах последних 20 суток.",
            "Choose a start within the last 20 days.",
        ),
        "clock" => (
            "Это время отсутствует или неоднозначно при переводе часов. Выберите другое.",
            "This time is missing or ambiguous across a clock change. Choose another.",
        ),
        _ => (
            "Значение недоступно. Выберите другую дату или время.",
            "Value unavailable. Choose another date or time.",
        ),
    };
    words(lang, ru, en)
}
impl Draft {
    fn selected(&self) -> i64 {
        self.bounds[usize::from(self.end)]
    }
    fn command(&self, edit: Edit) -> Payload {
        payload(
            self.room,
            self.device,
            DeviceCmd::EditChartRange {
                draft: self.id,
                edit,
            },
        )
    }
    fn button(&self, label: impl Into<String>, edit: Edit) -> Key {
        key(label, self.command(edit))
    }
    fn change_local(&mut self, value: NaiveDateTime) -> Result<(), &'static str> {
        let instant = Local
            .from_local_datetime(&value)
            .single()
            .ok_or("clock")?
            .timestamp();
        self.bounds[usize::from(self.end)] = instant;
        Ok(())
    }
    fn month_allowed(&self, month: NaiveDate) -> bool {
        month >= at(self.earliest).date_naive().with_day(1).unwrap()
            && month <= at(self.latest).date_naive().with_day(1).unwrap()
    }
    fn days(&self) -> [u8; 5] {
        let next = self
            .month
            .checked_add_months(chrono::Months::new(1))
            .unwrap();
        let count = (next - self.month).num_days() as u8;
        let selected = at(self.selected()).date_naive();
        let first = at(self.earliest).date_naive();
        let last = at(self.latest).date_naive();
        [
            self.month.weekday().num_days_from_monday() as u8,
            count,
            if selected.with_day(1) == Some(self.month) {
                selected.day() as u8
            } else {
                0
            },
            if first.with_day(1) == Some(self.month) {
                first.day() as u8
            } else {
                1
            },
            if last.with_day(1) == Some(self.month) {
                last.day() as u8
            } else {
                count
            },
        ]
    }
    fn edit(&mut self, edit: &Edit) -> Result<(), &'static str> {
        let local = at(self.selected());
        match *edit {
            Edit::Read => return Ok(()),
            Edit::Overview => self.mode = Mode::Overview,
            Edit::Date(end) => {
                self.end = end;
                self.month = at(self.selected()).date_naive().with_day(1).unwrap();
                self.mode = Mode::Calendar;
            }
            Edit::Time(end) => {
                self.end = end;
                self.mode = Mode::Time;
            }
            Edit::Month(delta) if self.mode == Mode::Calendar && [-1, 1].contains(&delta) => {
                let next = if delta > 0 {
                    self.month.checked_add_months(chrono::Months::new(1))
                } else {
                    self.month.checked_sub_months(chrono::Months::new(1))
                }
                .ok_or("value")?;
                if !self.month_allowed(next) {
                    return Err("value");
                };
                self.month = next;
            }
            Edit::Day(day) if self.mode == Mode::Calendar => {
                let info = self.days();
                if day < info[3] || day > info[4] {
                    return Err("value");
                };
                let date = self.month.with_day(day.into()).ok_or("value")?;
                self.change_local(date.and_time(local.time()))?;
                self.mode = Mode::Overview;
            }
            Edit::Hour(hour) if self.mode == Mode::Time && hour < 24 => {
                self.change_local(local.naive_local().with_hour(hour.into()).ok_or("value")?)?;
            }
            Edit::Minute(minute) if self.mode == Mode::Time && minute < 60 => {
                self.change_local(
                    local
                        .naive_local()
                        .with_minute(minute.into())
                        .ok_or("value")?,
                )?;
            }
            Edit::Nudge(delta) if self.mode == Mode::Time && [-1, 1].contains(&delta) => {
                let minute = local.minute() as i32 + i32::from(delta);
                if !(0..60).contains(&minute) {
                    return Err("value");
                };
                self.change_local(
                    local
                        .naive_local()
                        .with_minute(minute as u32)
                        .ok_or("value")?,
                )?;
            }
            _ => return Err("value"),
        }
        self.error = None;
        Ok(())
    }
    fn view(&self, lang: Language) -> View {
        let mut rows = Vec::new();
        let mut calendar = None;
        let bound = words(
            lang,
            if self.end {
                "Конец"
            } else {
                "Начало"
            },
            if self.end { "End" } else { "Start" },
        );
        let local = at(self.selected());
        let zone = words(lang, "Время бота", "Bot time");
        let title = match self.mode {
            Mode::Overview => {
                for (end, label) in [
                    (false, words(lang, "Начало", "Start")),
                    (true, words(lang, "Конец", "End")),
                ] {
                    let value = self.bounds[usize::from(end)];
                    rows.push(vec![
                        self.button(format!("{label}\n{}", date(value)), Edit::Date(end)),
                        self.button(time(value), Edit::Time(end)),
                    ]);
                }
                rows.push(vec![self.button(
                    words(lang, "Показать график", "Show chart"),
                    Edit::Apply,
                )]);
                words(lang, "Период графика", "Chart range")
            }
            Mode::Calendar => {
                let mut nav = Vec::new();
                for delta in [-1, 1] {
                    let candidate = if delta < 0 {
                        self.month.checked_sub_months(chrono::Months::new(1))
                    } else {
                        self.month.checked_add_months(chrono::Months::new(1))
                    };
                    if let Some(candidate) = candidate.filter(|m| self.month_allowed(*m)) {
                        nav.push(self.button(
                            format!(
                                "{} {}",
                                if delta < 0 { "‹" } else { "›" },
                                month_name(candidate.month(), lang)
                            ),
                            Edit::Month(delta),
                        ));
                    }
                }
                if !nav.is_empty() {
                    rows.push(nav);
                }
                let info = self.days();
                calendar = Some(info);
                // These commands are transmitted as a compact calendar descriptor + dN keys.
                let days = (info[3]..=info[4])
                    .map(|day| self.button(day.to_string(), Edit::Day(day)))
                    .collect::<Vec<_>>();
                rows.extend(days.chunks(7).map(|row| row.to_vec()));
                format!(
                    "{} {}",
                    month_name(self.month.month(), lang),
                    self.month.year()
                )
            }
            Mode::Time => {
                // Classic fallback keeps bounded rows; HTML transmits just h/m values
                // and registers all exact selections as issued opaque keys.
                for (count, hour) in [(24, true), (60, false)] {
                    let choices = (0..count)
                        .map(|n| {
                            self.button(
                                format!("{n:02}"),
                                if hour { Edit::Hour(n) } else { Edit::Minute(n) },
                            )
                        })
                        .collect::<Vec<_>>();
                    rows.extend(choices.chunks(6).map(|r| r.to_vec()));
                }
                rows.push(vec![
                    self.button(words(lang, "Готово", "Done"), Edit::Overview)
                ]);
                words(lang, "Выберите время", "Choose time")
            }
        };
        let back = match self.mode {
            Mode::Overview => Edit::Cancel,
            _ => Edit::Overview,
        };
        rows.push(vec![self.button(t(lang, "common.back"), back)]);
        let body = if self.mode == Mode::Overview {
            let a = at(self.bounds[0]).format("%:z").to_string();
            let b = at(self.bounds[1]).format("%:z").to_string();
            format!(
                "{zone}: UTC{a}{}",
                if a == b {
                    String::new()
                } else {
                    format!(" / UTC{b}")
                }
            )
        } else if self.mode == Mode::Time {
            format!(
                "{}\n{zone}: UTC{}",
                date(self.selected()),
                local.format("%:z")
            )
        } else {
            format!(
                "{} · {}\n{zone}: UTC{}",
                date(self.selected()),
                time(self.selected()),
                local.format("%:z")
            )
        };
        View {
            html_title: Some(title),
            html_context: Some(if self.mode == Mode::Overview {
                String::new()
            } else {
                bound
            }),
            html_kind: Some(
                match self.mode {
                    Mode::Overview => "range",
                    Mode::Calendar => "calendar",
                    Mode::Time => "time",
                }
                .into(),
            ),
            html_range: Some(self.bounds),
            html_calendar: calendar,
            html_time: (self.mode == Mode::Time)
                .then_some([local.hour() as u8, local.minute() as u8]),
            alert: self.error.clone(),
            text: body,
            kb: InlineKeyboardMarkup::new(rows),
            payload: self.command(Edit::Read),
            lang,
            displayed_resources: self.original.displayed_resources.clone(),
            ..View::default()
        }
    }
}

pub async fn render(ctx: RenderContext, room: i64, device: i64, cmd: DeviceCmd) -> Result<View> {
    let dev = db::devices::get_device_by_id(device, &ctx.config.db)
        .await?
        .context("Device missing")?;
    ctx.track_device(device, &dev.entity_id);
    ctx.track_room(room);
    let mut draft = if cmd == DeviceCmd::OpenChartRange {
        let view = ctx
            .config
            .html_ui
            .sessions
            .get(&ctx.user_id)
            .filter(|s| s.generation == ctx.admission_generation)
            .map(|s| s.view.clone())
            .context("Open the graph again")?;
        ensure!(
            matches!(&view.payload,Payload::Control(ControlPayload::QuickAction {device:d,..}) if *d==device),
            "Different chart"
        );
        let mut bounds = view.html_range.context("No displayed chart range")?;
        bounds.iter_mut().for_each(|v| *v = v.div_euclid(60) * 60);
        let now = Utc::now().timestamp().div_euclid(60) * 60;
        Draft {
            id: super::sequence(),
            generation: ctx.admission_generation.clone(),
            room,
            device,
            original: view,
            bounds,
            earliest: now - limits::LOOKBACK,
            latest: now,
            mode: Mode::Overview,
            end: false,
            month: at(bounds[0]).date_naive().with_day(1).unwrap(),
            error: None,
        }
    } else {
        ctx.config
            .html_ui
            .ranges
            .get(&ctx.user_id)
            .filter(|d| {
                d.device == device && d.room == room && d.generation == ctx.admission_generation
            })
            .map(|d| d.clone())
            .context("Range editor expired; reopen the graph")?
    };
    if let DeviceCmd::EditChartRange { draft: id, edit } = cmd {
        ensure!(id == draft.id, "Range editor replaced");
        match edit {
            Edit::Cancel => {
                ctx.config.html_ui.ranges.remove(&ctx.user_id);
                return Ok(draft.original);
            }
            Edit::Apply => {
                if let Err(code) = limits::validate(draft.bounds[0], draft.bounds[1], Utc::now()) {
                    draft.error = Some(message(code, ctx.lang));
                    draft.mode = Mode::Overview;
                } else {
                    let next = DeviceCmd::ShowChartRange {
                        start: draft.bounds[0],
                        end: draft.bounds[1],
                    };
                    match crate::bot::screens::control::device_control::render(
                        ctx.clone(),
                        room,
                        device,
                        next,
                    )
                    .await
                    {
                        Ok(view) => {
                            ctx.config.html_ui.ranges.remove(&ctx.user_id);
                            return Ok(view);
                        }
                        Err(error) => {
                            log::warn!("Chart range read failed: {error:#}");
                            draft.error=Some(words(ctx.lang,"Не удалось получить историю. Выбранный период сохранён в редакторе.","Could not load history. Your selection remains in the editor."));
                            draft.mode = Mode::Overview;
                        }
                    }
                }
            }
            _ => {
                if let Err(code) = draft.edit(&edit) {
                    draft.error = Some(message(code, ctx.lang));
                    draft.mode = Mode::Overview;
                }
            }
        }
    }
    let view = draft.view(ctx.lang);
    ctx.config.html_ui.ranges.insert(ctx.user_id, draft);
    Ok(view)
}

/// Chart/editor pages are already intentionally sized, not an adaptation of the
/// classic preset keyboard. Chart controls occupy two rows, including PNG export.
pub(super) fn prepare(view: &View, rev: String, seq: u64) -> Result<Prepared> {
    let mut actions = HashMap::new();
    let mut rows = Vec::new();
    let mut back = None;
    let graph = view.html_chart.is_some();
    let keyboard = if graph {
        let (room, device) = match view.payload {
            Payload::Control(ControlPayload::QuickAction { room, device, .. }) => (room, device),
            _ => anyhow::bail!("Chart route missing"),
        };
        let mut keyboard = vec![vec![
            key(
                words(view.lang, "Период", "Date range"),
                payload(room, device, DeviceCmd::OpenChartRange),
            ),
            key(
                words(view.lang, "Сведения", "Details"),
                Payload::DeviceDetails { room, device },
            ),
        ]];
        if view.image.as_ref().is_some_and(|image| !image.is_empty()) {
            keyboard.push(vec![key(
                words(view.lang, "Отправить PNG", "Send PNG"),
                payload(room, device, DeviceCmd::ExportChart),
            )]);
        }
        keyboard.push(vec![key(
            t(view.lang, "common.back"),
            Payload::Control(ControlPayload::RoomDetail { room }),
        )]);
        keyboard
    } else {
        view.kb.inline_keyboard.clone()
    };
    for row in keyboard {
        let mut buttons = Vec::new();
        for button in row {
            let InlineKeyboardButtonKind::CallbackData(data) = button.kind else {
                anyhow::bail!("Not a callback")
            };
            let p = Payload::from_string(&data)?;
            let compact = match &p {
                Payload::Control(ControlPayload::QuickAction {
                    cmd: DeviceCmd::EditChartRange { edit, .. },
                    ..
                }) => match edit {
                    Edit::Day(n) => Some(format!("d{n}")),
                    Edit::Hour(n) => Some(format!("h{n}")),
                    Edit::Minute(n) => Some(format!("m{n}")),
                    _ => None,
                },
                _ => None,
            };
            let id = compact
                .clone()
                .unwrap_or_else(|| format!("a{}", actions.len()));
            actions.insert(id.clone(), p);
            if button.text == t(view.lang, "common.back") {
                back = Some(id);
            } else if compact.is_none() {
                let selected = button.text.starts_with("✓ ");
                buttons.push(Button {
                    text: button.text.trim_start_matches("✓ ").into(),
                    key: id,
                    kind: if selected { "selected" } else { "nav" }.into(),
                });
            }
        }
        if !buttons.is_empty() {
            rows.push(buttons);
        }
    }
    let mut body = if graph {
        view.html_summary.clone().unwrap_or_default()
    } else {
        view.text.clone()
    };
    if graph {
        let [start, end] = view.html_range.context("Missing chart bounds")?;
        body.push_str(&format!(
            "\n{} {} — {} {}\nUTC{}",
            date(start),
            time(start),
            date(end),
            time(end),
            at(start).format("%:z")
        ));
        if at(start).offset() != at(end).offset() {
            body.push_str(&format!(" / UTC{}", at(end).format("%:z")));
        }
    }
    let page = Page {
        v: 1,
        rev,
        seq,
        s: view.payload.for_refresh().to_string(),
        lang: if view.lang == Language::Ru {
            "ru"
        } else {
            "en"
        }
        .into(),
        title: view.html_title.clone().unwrap_or_default(),
        context: view.html_context.clone().unwrap_or_default(),
        kind: if graph {
            "chart".into()
        } else {
            view.html_kind.clone().unwrap_or_default()
        },
        hint: String::new(),
        body,
        notice: view.notice.clone().unwrap_or_default(),
        alert: view.alert.clone().unwrap_or_default(),
        status: Vec::new(),
        slider: None,
        graph: view.html_chart.clone(),
        calendar: view.html_calendar,
        list: None,
        bounded: false,
        time: view.html_time,
        rows,
        read: "read".into(),
        back,
        prev: None,
        next: None,
        page: 1,
        pages: 1,
    };
    page.json()?;
    Ok(Prepared {
        pages: vec![page],
        actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn draft(year: i32, month: u32, day: u32) -> Draft {
        let end = Local
            .with_ymd_and_hms(year, month, day, 10, 37, 0)
            .single()
            .unwrap();
        Draft {
            id: 17,
            generation: "test".into(),
            room: 2,
            device: 7,
            original: View::default(),
            bounds: [
                (end - chrono::Duration::days(2)).timestamp(),
                end.timestamp(),
            ],
            earliest: (end - chrono::Duration::days(20)).timestamp(),
            latest: end.timestamp(),
            mode: Mode::Overview,
            end: false,
            month: end.date_naive().with_day(1).unwrap(),
            error: None,
        }
    }
    #[test]
    fn calendar_limits_leap_years_month_rollover_and_every_minute() -> Result<()> {
        let mut d = draft(2024, 3, 2);
        d.edit(&Edit::Date(false)).unwrap();
        assert_eq!(d.days(), [3, 29, 29, 11, 29]);
        assert!(d.edit(&Edit::Day(10)).is_err());
        assert!(d.edit(&Edit::Day(30)).is_err());
        d.edit(&Edit::Month(1)).unwrap();
        assert_eq!(d.days(), [4, 31, 0, 1, 2]);
        assert!(d.edit(&Edit::Day(3)).is_err());
        assert!(d.edit(&Edit::Month(1)).is_err());
        d.edit(&Edit::Day(1)).unwrap();
        assert_eq!(date(d.bounds[0]), "01.03.2024");
        d.edit(&Edit::Time(false)).unwrap();
        d.edit(&Edit::Hour(23)).unwrap();
        for minute in 0..60 {
            d.edit(&Edit::Minute(minute)).unwrap();
            assert_eq!(at(d.bounds[0]).minute(), minute as u32);
        }
        assert!(d.edit(&Edit::Nudge(1)).is_err());
        for minute in (0..59).rev() {
            d.edit(&Edit::Nudge(-1)).unwrap();
            assert_eq!(at(d.bounds[0]).minute(), minute);
        }
        assert!(d.edit(&Edit::Nudge(-1)).is_err());
        assert!(d.edit(&Edit::Minute(60)).is_err());
        assert!(d.edit(&Edit::Hour(24)).is_err());
        let mut d = draft(2027, 1, 2);
        d.edit(&Edit::Date(false)).unwrap();
        assert_eq!(d.month.year(), 2026);
        d.edit(&Edit::Month(1)).unwrap();
        assert_eq!(d.month.year(), 2027);
        Ok(())
    }
    #[test]
    fn calendar_pages_fit_transport_and_export_preview() -> Result<()> {
        let mut fixtures: Vec<Value> = Vec::new();
        for lang in [Language::Ru, Language::En] {
            let suffix = if lang == Language::En { "_en" } else { "" };
            let mut d = draft(2026, 11, 7);
            let mut states = Vec::new();
            states.push(("range", d.clone()));
            d.edit(&Edit::Date(false)).unwrap();
            states.push(("calendar", d.clone()));
            d.edit(&Edit::Month(-1)).unwrap();
            states.push(("calendar_previous", d.clone()));
            d.edit(&Edit::Time(false)).unwrap();
            states.push(("time", d.clone()));
            d.edit(&Edit::Hour(8)).unwrap();
            states.push(("time_changed", d.clone()));
            d.mode = Mode::Overview;
            d.error = Some(message("order", lang));
            states.push(("range_error", d.clone()));
            d.error = Some(words(
                lang,
                "Не удалось получить историю. Выбранный период сохранён в редакторе.",
                "Could not load history. Your selection remains in the editor.",
            ));
            states.push(("range_retry", d.clone()));
            let mut leap = draft(2024, 3, 2);
            leap.edit(&Edit::Date(false)).unwrap();
            states.push(("calendar_leap", leap));
            let mut six = draft(2026, 9, 2);
            six.edit(&Edit::Date(false)).unwrap();
            states.push(("calendar_six_weeks", six));
            for (id, draft) in states {
                let view = draft.view(lang);
                let prepared = prepare(&view, "a".repeat(32), 1)?;
                let page = &prepared.pages[0];
                assert!(page.json()?.len() <= 1000, "{id}");
                assert!(
                    serde_json::to_vec(&json!({"page":page}))?.len() <= 1024,
                    "{id}"
                );
                assert!(page.rows.len() <= 8 && page.rows.iter().all(|r| r.len() <= 4));
                assert!(view.kb.inline_keyboard.iter().all(|r| r.len() <= 7));
                let mut links = serde_json::Map::new();
                for (key, payload) in &prepared.actions {
                    assert!(payload.to_string().len() <= 64);
                    assert_eq!(Payload::from_string(&payload.to_string())?, *payload);
                    if let Payload::Control(ControlPayload::QuickAction {
                        cmd: DeviceCmd::EditChartRange { edit, .. },
                        ..
                    }) = payload
                    {
                        let target = match edit {
                            Edit::Date(_) => "calendar",
                            Edit::Time(_) => "time",
                            Edit::Hour(_) | Edit::Minute(_) | Edit::Nudge(_) => "time_changed",
                            Edit::Month(-1) => "calendar_previous",
                            Edit::Month(_) => "calendar",
                            Edit::Apply | Edit::Cancel => "numeric",
                            _ => "range",
                        };
                        links.insert(key.clone(), json!(format!("{target}{suffix}")));
                    }
                }
                if let Some(time) = page.time {
                    assert_eq!(
                        time,
                        [
                            at(draft.selected()).hour() as u8,
                            at(draft.selected()).minute() as u8
                        ]
                    );
                    for (prefix, max) in [("h", 24), ("m", 60)] {
                        for n in 0..max {
                            assert!(prepared.actions.contains_key(&format!("{prefix}{n}")));
                        }
                        assert!(!prepared.actions.contains_key(&format!("{prefix}{max}")));
                    }
                    assert_eq!(page.rows.len(), 1, "Only Done is transmitted as a row");
                }
                if let Some(c) = page.calendar {
                    for day in 1..=c[1] {
                        assert_eq!(
                            prepared.actions.contains_key(&format!("d{day}")),
                            day >= c[3] && day <= c[4]
                        );
                    }
                }
                fixtures.push(json!({"id":format!("{id}{suffix}"),"label":format!("{id}{suffix}"),"page":page,"links":links}));
            }
        }
        if let Ok(path) = std::env::var("HA_WIDGET_EXPORT_CALENDAR") {
            std::fs::write(path, serde_json::to_vec_pretty(&fixtures)?)?;
        }
        Ok(())
    }
}
