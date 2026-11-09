use super::super::{
    models::View,
    personal::PersonalPayload,
    router::{
        AdminPayload, CameraPayload, CommandPayload, ControlPayload, Payload, SettingsPayload,
    },
};
use crate::i18n::{t, Language};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use teloxide::types::InlineKeyboardButtonKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Button {
    pub text: String,
    pub key: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub v: u8,
    pub rev: String,
    pub seq: u64,
    /// Stable screen identity; a new revision does not imply navigation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub s: String,
    pub lang: String,
    pub title: String,
    pub context: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub body: String,
    /// Home statuses or device metrics: label, value, detail. Plain text only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<[String; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slider: Option<super::temperature::Slider>,
    pub notice: String,
    pub alert: String,
    #[serde(rename = "b", alias = "rows", with = "button_rows")]
    pub rows: Vec<Vec<Button>>,
    pub read: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<String>,
    /// Source-list position and opaque previous/next callbacks; shared by wire pages.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "q")]
    pub list: Option<(String, Option<String>, Option<String>)>,
    /// Ordinary controls follow the two-line layout; media keeps its existing layout.
    #[serde(default, skip_serializing_if = "std::ops::Not::not", rename = "l")]
    pub bounded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::charts::widget::WidgetChart>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "c")]
    pub calendar: Option<[u8; 5]>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "t")]
    pub time: Option<[u8; 2]>,
    pub prev: Option<String>,
    pub next: Option<String>,
    pub page: usize,
    pub pages: usize,
}

mod button_rows {
    use super::Button;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        rows: &[Vec<Button>],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|b| (&b.text, &b.key, &b.kind))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<Button>>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireButton {
            Compact((String, String, String)),
            Legacy(Button),
        }
        Ok(Vec::<Vec<WireButton>>::deserialize(deserializer)?
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|b| match b {
                        WireButton::Compact((text, key, kind)) => Button { text, key, kind },
                        WireButton::Legacy(button) => button,
                    })
                    .collect()
            })
            .collect())
    }
}
impl Page {
    pub fn json(&self) -> Result<String> {
        let json = serde_json::to_string(self)?;
        // All command phases reserve the same wire space; changing an action
        // to wait/uncertain must not repartition an otherwise unchanged page.
        let phase_reserve: usize = self
            .rows
            .iter()
            .flatten()
            .filter(|b| matches!(b.kind.as_str(), "action" | "wait" | "uncertain"))
            .map(|b| "uncertain".len() - b.kind.len())
            .sum();
        ensure!(
            json.len() + phase_reserve <= 1000 && !json.contains("\\u0000"),
            "HTML page exceeds Telegram data budget"
        );
        Ok(json)
    }
}

pub fn supported(view: &View) -> bool {
    if view.html_chart.is_some() && view.html_range.is_none() {
        return false;
    }
    if view.image.is_some() && view.html_chart.is_none() && !view.html_media_controls {
        return false;
    }
    // Every text screen uses the same server-issued callback contract. Access
    // control and command semantics belong to the existing router, not a UI whitelist.
    view.kb
        .inline_keyboard
        .iter()
        .flatten()
        .all(|button| matches!(button.kind, InlineKeyboardButtonKind::CallbackData(_)))
}

pub struct Prepared {
    pub pages: Vec<Page>,
    pub actions: HashMap<String, Payload>,
}

/// Match the route and its owner/filter, not translated arrow labels.
fn list_target(payload: &Payload) -> Option<(Payload, u16)> {
    let mut base = payload.clone();
    let page = match &mut base {
        Payload::Control(
            ControlPayload::RoomsPage { page }
            | ControlPayload::RoomPage { page, .. }
            | ControlPayload::ActionGroupsPage { page },
        )
        | Payload::Settings(
            SettingsPayload::RoomsPage { page } | SettingsPayload::RoomPage { page, .. },
        )
        | Payload::AdminSettings(
            SettingsPayload::RoomsPage { page } | SettingsPayload::RoomPage { page, .. },
        )
        | Payload::Personal(
            PersonalPayload::Favorites { page } | PersonalPayload::Notifications { page },
        )
        | Payload::GroupDevices { page, .. }
        | Payload::Admin(
            AdminPayload::ListUsersPage { page }
            | AdminPayload::ActionGroupsPage { page }
            | AdminPayload::ActionGroupItems { page, .. }
            | AdminPayload::RecordingRuleGroupRules { page, .. }
            | AdminPayload::RecordingRuleGroupRulesForRoom { page, .. },
        ) => page,
        _ => return None,
    };
    let index = *page;
    *page = 0;
    Some((base, index))
}

pub(crate) fn list_identity(payload: &Payload) -> Payload {
    match payload {
        Payload::Control(ControlPayload::RoomsPage { .. }) => {
            Payload::Control(ControlPayload::ListRooms)
        }
        Payload::Control(ControlPayload::RoomPage { room, .. }) => {
            Payload::Control(ControlPayload::RoomDetail { room: *room })
        }
        Payload::Control(ControlPayload::ActionGroupsPage { .. }) => {
            Payload::Control(ControlPayload::ActionGroups)
        }
        Payload::Settings(SettingsPayload::RoomsPage { .. }) => {
            Payload::Settings(SettingsPayload::ListRooms)
        }
        Payload::AdminSettings(SettingsPayload::RoomsPage { .. }) => {
            Payload::AdminSettings(SettingsPayload::ListRooms)
        }
        Payload::Settings(SettingsPayload::RoomPage { room, .. }) => {
            Payload::Settings(SettingsPayload::RoomDetail { room: *room })
        }
        Payload::AdminSettings(SettingsPayload::RoomPage { room, .. }) => {
            Payload::AdminSettings(SettingsPayload::RoomDetail { room: *room })
        }
        Payload::Admin(AdminPayload::ListUsersPage { .. }) => {
            Payload::Admin(AdminPayload::ListUsers)
        }
        Payload::Admin(AdminPayload::ActionGroupsPage { .. }) => {
            Payload::Admin(AdminPayload::ActionGroups)
        }
        _ => list_target(payload)
            .map(|(base, _)| base)
            .unwrap_or_else(|| payload.clone()),
    }
}

struct SourcePager {
    row: usize,
    position: (String, Option<String>, Option<String>),
}

fn source_pager(view: &View) -> Option<SourcePager> {
    let mut offset = 0;
    for (index, row) in view.kb.inline_keyboard.iter().enumerate() {
        let start = offset;
        offset += row.len();
        let targets = row
            .iter()
            .map(|button| {
                let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                    return None;
                };
                list_target(&Payload::from_string(data).ok()?)
            })
            .collect::<Option<Vec<_>>>();
        let Some(targets) = targets.filter(|t| !t.is_empty() && t.len() <= 3) else {
            continue;
        };
        if !targets.iter().all(|(base, _)| base == &targets[0].0) {
            continue;
        }
        for (button, (_, page)) in row.iter().zip(&targets) {
            let Some((left, right)) = button.text.split_once('/') else {
                continue;
            };
            let (Ok(current), Ok(pages)) =
                (left.trim().parse::<u16>(), right.trim().parse::<u16>())
            else {
                continue;
            };
            if current != page + 1 || current > pages || pages < 2 {
                continue;
            }
            let previous = targets
                .iter()
                .position(|(_, p)| p < page)
                .map(|i| format!("a{}", start + i));
            let next = targets
                .iter()
                .position(|(_, p)| p > page)
                .map(|i| format!("a{}", start + i));
            return Some(SourcePager {
                row: index,
                position: (
                    format!(
                        "{} {current}/{pages}",
                        if view.lang == Language::Ru {
                            "Список"
                        } else {
                            "List"
                        }
                    ),
                    previous,
                    next,
                ),
            });
        }
    }
    None
}

#[cfg(test)]
pub(super) fn is_source_pager_button(view: &View, index: usize) -> bool {
    source_pager(view).is_some_and(|pager| {
        let start: usize = view.kb.inline_keyboard[..pager.row]
            .iter()
            .map(Vec::len)
            .sum();
        (start..start + view.kb.inline_keyboard[pager.row].len()).contains(&index)
    })
}

pub(super) fn bounded_label(label: &str) -> String {
    let mut lines = label.lines();
    let first = lines.next().unwrap_or_default();
    let rest = lines.collect::<Vec<_>>().join(" ");
    if rest.is_empty() {
        first.into()
    } else {
        format!("{first}\n{rest}")
    }
}
fn kind(payload: &Payload) -> &'static str {
    match payload {
        Payload::Control(ControlPayload::ConfirmAction { .. })
        | Payload::Command(CommandPayload::Confirm { .. })
        | Payload::Camera(CameraPayload::DeleteRecording { .. })
        | Payload::Admin(
            AdminPayload::DeleteCamera { .. }
            | AdminPayload::DeleteUser { .. }
            | AdminPayload::DeleteRecordingRule { .. }
            | AdminPayload::DeleteRecordingRuleGroup { .. }
            | AdminPayload::DeleteRecordingRuleGroupForRoom { .. }
            | AdminPayload::DeleteActionGroup { .. }
            | AdminPayload::DeleteActionSchedule { .. },
        ) => "danger",
        Payload::Control(ControlPayload::CancelAction { .. })
        | Payload::Command(CommandPayload::Cancel { .. } | CommandPayload::CancelChoice { .. }) => {
            "nav"
        }
        Payload::Control(ControlPayload::QuickAction { cmd, .. })
            if super::super::router::action_requires_control(cmd) =>
        {
            "action"
        }
        Payload::Personal(PersonalPayload::SetInterface { .. })
        | Payload::Admin(
            AdminPayload::RoomCameraSnapshot { .. } | AdminPayload::RoomCameraClip { .. },
        ) => "action",
        _ if payload == &payload.for_refresh() => "nav",
        _ => "action",
    }
}

fn screen_kind(view: &View) -> &str {
    if view.next_state.is_some() {
        return "form";
    }
    if view.kb.inline_keyboard.iter().flatten().any(|button| {
        matches!(&button.kind, InlineKeyboardButtonKind::CallbackData(data)
            if matches!(Payload::from_string(data), Ok(Payload::Control(ControlPayload::ConfirmAction { .. }) | Payload::Command(CommandPayload::Confirm { .. }))))
    }) {
        return "confirmation";
    }
    if let Some(
        kind @ ("menu" | "device" | "climate" | "temperature" | "confirmation" | "list" | "room"
        | "camera"),
    ) = view.html_kind.as_deref()
    {
        return kind;
    }
    match view.payload {
        Payload::Home | Payload::Personal(_) | Payload::Admin(AdminPayload::ListActions) => "menu",
        Payload::DeviceDetails { .. } | Payload::Control(ControlPayload::DeviceControl { .. }) => {
            "device"
        }
        Payload::Control(ControlPayload::ReviewAction { .. }) => "confirmation",
        _ => "list",
    }
}
fn split_text(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    for ch in text.chars() {
        if part.len() + ch.len_utf8() > limit {
            parts.push(std::mem::take(&mut part));
        }
        part.push(ch);
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

fn home_status_tile(item: &crate::core::HeaderItem, preview: bool) -> [String; 3] {
    fn short(value: &str, limit: usize) -> String {
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if value.chars().count() <= limit {
            value
        } else {
            format!("{}…", value.chars().take(limit - 1).collect::<String>())
        }
    }
    let age = crate::core::presentation::StateFormatter::format_last_update(item.last_update);
    // The existing header producer appends a canonical event counter. Keep
    // arbitrary device values intact if they do not have that exact suffix.
    let (value, meta) = item
        .value
        .rsplit_once(" [x")
        .and_then(|(value, count)| {
            let count = count.strip_suffix(']')?;
            (count.bytes().all(|c| c.is_ascii_digit()) && count.parse::<u64>().ok()? > 0)
                .then(|| (value, format!("×{count} · {age}")))
        })
        .unwrap_or((&item.value, age));
    [
        format!(
            "{} {}",
            item.icon,
            if preview {
                short(&item.label, 26)
            } else {
                item.label.clone()
            }
        ),
        if preview {
            short(value, 24)
        } else {
            value.into()
        },
        meta,
    ]
}

fn append_text(
    mut current: Page,
    empty: &Page,
    parts: &mut Vec<Page>,
    field: &str,
    text: &str,
) -> Result<Page> {
    // Ordinary widgets paginate by measured client height. Do not impose a
    // second 200-character page limit or leave room for an entire 180-byte
    // fragment on the server. Fill the Telegram wire budget at UTF-8 boundaries.
    // Media templates keep their existing text/controls layout contract.
    let fragments = if empty.bounded {
        text.chars().map(|ch| ch.to_string()).collect()
    } else {
        split_text(text, 180)
    };
    for part in fragments {
        let mut candidate = current.clone();
        let target = match field {
            "alert" => &mut candidate.alert,
            "notice" => &mut candidate.notice,
            _ => &mut candidate.body,
        };
        target.push_str(&part);
        let characters = candidate.body.chars().count()
            + candidate.alert.chars().count()
            + candidate.notice.chars().count()
            + candidate.hint.chars().count();
        if candidate.json().is_err() || (!empty.bounded && characters > 200) {
            if !current.hint.is_empty()
                || !current.body.is_empty()
                || !current.alert.is_empty()
                || !current.notice.is_empty()
                || current.graph.is_some()
                || !current.rows.is_empty()
            {
                parts.push(current);
            }
            current = empty.clone();
            match field {
                "alert" => current.alert = part,
                "notice" => current.notice = part,
                _ => current.body = part,
            };
            current.json()?;
        } else {
            current = candidate;
        }
    }
    Ok(current)
}

pub fn prepare(view: &View, rev: String, seq: u64) -> Result<Prepared> {
    prepare_with_pending(view, rev, seq, &HashMap::new())
}

pub fn prepare_with_pending(
    view: &View,
    rev: String,
    seq: u64,
    pending: &HashMap<i64, &'static str>,
) -> Result<Prepared> {
    ensure!(supported(view), "Screen uses classic presentation");
    if view.html_range.is_some() {
        return super::range::prepare(view, rev, seq);
    }
    let mut structured = view.clone();
    super::super::screen_structure::apply(&mut structured);
    super::temperature::controls(&mut structured);
    if structured.html_temperature.is_some() && structured.alert.is_none() {
        if let Payload::Control(
            ControlPayload::QuickAction { device, .. }
            | ControlPayload::DeviceControl { device, .. },
        ) = &structured.payload
        {
            if pending.get(device) == Some(&"uncertain") {
                structured.alert = Some(
                    crate::bot::personal::words(
                        structured.lang,
                        "HA не подтвердил температуру. Обновите состояние.",
                        "HA has not confirmed the temperature. Refresh the state.",
                    )
                    .into(),
                );
            }
        }
    }
    let view = &structured;
    let bounded = !view.html_media_controls
        && view.html_chart.is_none()
        && view.html_temperature.is_none()
        && !matches!(view.payload, Payload::Camera(_));
    let pager = bounded.then(|| source_pager(view)).flatten();
    let parent = super::super::screen_structure::parent(view);
    let back = parent.as_ref().map(|parent| {
        view.kb
            .inline_keyboard
            .iter()
            .flatten()
            .enumerate()
            .find_map(|(index, button)| match &button.kind {
                InlineKeyboardButtonKind::CallbackData(data)
                    if Payload::from_string(data).ok().as_ref() == Some(parent) =>
                {
                    Some(format!("a{index}"))
                }
                _ => None,
            })
            .unwrap_or_else(|| "back".into())
    });
    let english = view.lang == Language::En;
    let title = view
        .html_title
        .clone()
        .or_else(|| view.header.clone())
        .unwrap_or_else(|| {
            match &view.payload {
                Payload::Home => {
                    if english {
                        "My home"
                    } else {
                        "Мой дом"
                    }
                }
                Payload::Help => {
                    if english {
                        "Help"
                    } else {
                        "Помощь"
                    }
                }
                Payload::Personal(PersonalPayload::Interface) => {
                    if english {
                        "Interface"
                    } else {
                        "Вид интерфейса"
                    }
                }
                Payload::Personal(_) => {
                    if english {
                        "My settings"
                    } else {
                        "Мои настройки"
                    }
                }
                _ => {
                    if english {
                        "Home"
                    } else {
                        "Дом"
                    }
                }
            }
            .into()
        });
    let empty = Page {
        v: 1,
        rev,
        seq,
        s: view.payload.for_refresh().to_string(),
        lang: if english { "en" } else { "ru" }.into(),
        title,
        context: view.html_context.clone().unwrap_or_default(),
        kind: screen_kind(view).into(),
        hint: String::new(),
        body: String::new(),
        status: Vec::new(),
        slider: None,
        notice: String::new(),
        alert: String::new(),
        rows: Vec::new(),
        read: "read".into(),
        back,
        list: pager.as_ref().map(|p| p.position.clone()),
        bounded,
        prev: Some("p9999".into()),
        next: Some("p9999".into()),
        graph: None,
        calendar: None,
        time: None,
        page: 9999,
        pages: 9999,
    };
    // Reserve room for non-ASCII labels and pagination; oversized content falls back as a whole.
    ensure!(empty.json()?.len() <= 600, "Header is too large for HTML");
    let mut parts: Vec<Page> = Vec::new();
    let mut current = empty.clone();
    current.graph = view.html_chart.clone();
    current.status = view.html_metrics.clone();
    current.slider = view.html_temperature.clone();
    current.json()?;
    if view.next_state.is_some() {
        current.hint = if english {
            "Reply with a message in this chat. /cancel to cancel."
        } else {
            "Ответьте сообщением в этот чат. /cancel — отмена."
        }
        .into();
    }
    let mut body = Vec::new();
    if view.text_page.is_some() {
        // Switching from a classic long screen preserves its source page and pager.
        body.push(view.get_plain_text());
    } else {
        // The graph renderer keeps its header compact; preserve the full room
        // context in the detail pages, including names too long for the header.
        if view.html_chart.is_some() {
            if let Some(context) = &view.html_context {
                body.push(context.clone());
            }
        }
        for item in view.visible_notifications() {
            body.push(format!(
                "{} {}{}{} · {}",
                item.icon,
                item.label,
                if matches!(view.payload, Payload::Home) {
                    "\n"
                } else {
                    ": "
                },
                item.value,
                crate::core::presentation::StateFormatter::format_last_update(item.last_update)
            ));
        }
        if !view.text.is_empty() {
            body.push(view.text.clone());
        }
    }
    let body = body.join("\n\n");
    let summary = view
        .html_summary
        .as_deref()
        .filter(|_| view.text_page.is_none() && view.next_state.is_none());
    // Keep receipts with the room overview; commands update the device rows
    // without adding a block above them or changing their pagination.
    let room_notice = matches!(
        view.payload,
        Payload::Control(ControlPayload::RoomDetail { .. } | ControlPayload::RoomPage { .. })
    ) && summary.is_some();
    for (field, text) in [
        (
            "alert",
            if view.text_page.is_some() {
                ""
            } else {
                view.alert.as_deref().unwrap_or("")
            },
        ),
        (
            "notice",
            if view.text_page.is_some() || room_notice {
                ""
            } else {
                view.notice.as_deref().unwrap_or("")
            },
        ),
        ("body", summary.unwrap_or(&body)),
    ] {
        current = append_text(current, &empty, &mut parts, field, text)?;
    }
    let root = empty.clone();
    let mut empty = empty;
    let mut actions = HashMap::new();
    for (row_index, row) in view.kb.inline_keyboard.iter().enumerate() {
        if view.text_page.is_none() && view.next_state.is_none() {
            if let Some(section) = view
                .html_sections
                .iter()
                .find(|section| section.start_row == row_index)
            {
                parts.push(current);
                empty = continuation(&root, &section.title)?;
                current = empty.clone();
            }
        }
        // Preserve every button, including source rows wider than the widget.
        // Keep the classic chart layout: time navigation, four range presets,
        // then custom range. The client wraps only on unusually narrow cards.
        let columns = if view.html_chart.is_some() { 4 } else { 2 };
        for buttons in row.chunks(columns) {
            let mut rendered = Vec::new();
            for button in buttons {
                let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                    unreachable!()
                };
                let payload = Payload::from_string(data)?;
                let key = format!("a{}", actions.len());
                let pinned = pager.as_ref().is_some_and(|pager| pager.row == row_index)
                    || empty.back.as_deref() == Some(&key)
                    || (button.text == t(view.lang, "control.refresh")
                        && payload == view.payload.for_refresh());
                if !pinned {
                    let pending_kind = match &payload {
                        Payload::Control(ControlPayload::QuickAction { device, cmd, .. })
                            if super::super::router::action_requires_control(cmd) =>
                        {
                            pending.get(device)
                        }
                        _ => None,
                    };
                    rendered.push(Button {
                        text: {
                            let label = view.html_button_labels.get(data).unwrap_or(&button.text);
                            if bounded {
                                bounded_label(label)
                            } else {
                                label.clone()
                            }
                        },
                        key: key.clone(),
                        kind: if let Some(status) = pending_kind {
                            status
                        } else if button.text.starts_with("✓ ") || button.text.starts_with("☑ ")
                        {
                            "selected"
                        } else {
                            kind(&payload)
                        }
                        .into(),
                    });
                }
                actions.insert(key, payload);
            }
            if rendered.is_empty() {
                continue;
            }
            let mut candidate = current.clone();
            candidate.rows.push(rendered.clone());
            let text_chars = candidate.body.chars().count()
                + candidate.notice.chars().count()
                + candidate.alert.chars().count()
                + candidate.hint.chars().count();
            // Ordinary cards are partitioned by the wire budget here and by
            // measured height in the client. A fixed row count wastes space.
            let graph_overflow = view.html_chart.is_some()
                && (candidate.rows.len() > if current.graph.is_some() { 3 } else { 4 }
                    || (candidate.rows.len() > 2 && text_chars > 80));
            if graph_overflow || candidate.json().is_err() {
                let mut fresh = empty.clone();
                fresh.rows.push(rendered.clone());
                if !graph_overflow && fresh.json().is_err() && rendered.len() > 1 {
                    // A wide row may exceed the wire budget even on an empty
                    // page. Keep each labelled action intact rather than fall back.
                    for button in rendered {
                        let mut candidate = current.clone();
                        candidate.rows.push(vec![button.clone()]);
                        if candidate.json().is_err() {
                            parts.push(current);
                            current = empty.clone();
                            current.rows.push(vec![button]);
                            current.json()?;
                        } else {
                            current = candidate;
                        }
                    }
                } else {
                    parts.push(current);
                    current = fresh;
                    current.json()?;
                }
            } else {
                current = candidate;
            }
        }
    }
    // Attach status after reserving the home sections, so notifications cannot
    // evict Settings/Admin/etc. A short status needs no separate summary page.
    let home_status =
        matches!(view.payload, Payload::Home) && summary.is_some() && !body.is_empty();
    let inline_status = home_status && {
        let first = parts.first_mut().unwrap_or(&mut current);
        let mut candidate = first.clone();
        candidate.status = view
            .notifications
            .iter()
            .map(|item| home_status_tile(item, false))
            .collect();
        let fits = view.notifications.len() <= 2
            && view.text.is_empty()
            && body.chars().count() <= 100
            && body.lines().count() <= 5
            && candidate.json().is_ok();
        if fits {
            *first = candidate;
        }
        fits
    };
    let mut home_details = None;
    if summary.is_some()
        && view.html_metrics.is_empty()
        && view.html_temperature.is_none()
        && !inline_status
        && (!body.is_empty()
            || (room_notice && view.notice.as_ref().is_some_and(|s| !s.is_empty())))
    {
        parts.push(current);
        if home_status {
            home_details = Some(parts.len());
        }
        empty = match &view.html_details_title {
            Some(title) => continuation(&root, title)?,
            None => root,
        };
        current = empty.clone();
        if room_notice {
            current = append_text(
                current,
                &empty,
                &mut parts,
                "notice",
                view.notice.as_deref().unwrap_or(""),
            )?;
        }
        current = append_text(current, &empty, &mut parts, "body", &body)?;
    }
    parts.push(current);
    if let Some(index) = home_details {
        // A read-only link to the complete, unabridged statuses. It is a page
        // key, not a device action, and works like the existing pinned p0 Back.
        parts[0].rows.insert(
            0,
            vec![Button {
                text: if english {
                    format!("All statuses: {}", view.notifications.len())
                } else {
                    format!("Все статусы: {}", view.notifications.len())
                },
                key: format!("p{index}"),
                kind: "nav".into(),
            }],
        );
        if let Some(item) = view.notifications.first() {
            let mut candidate = parts[0].clone();
            candidate.status.push(home_status_tile(item, true));
            candidate.rows[0][0].kind = "status".into();
            candidate.rows[0][0].text.push_str(" ›");
            if candidate.json().is_ok() {
                parts[0] = candidate;
            }
        }
        parts[0].json()?;
    }
    ensure!(parts.len() <= 9999, "Too many HTML pages");
    // The thermostat owns a single card. Fall back as a whole rather than
    // deliver continuation pages without the slider specification/Apply pair.
    ensure!(
        view.html_temperature.is_none() || parts.len() == 1,
        "Temperature card exceeds the single-page budget"
    );
    let pages = parts.len();
    for (index, page) in parts.iter_mut().enumerate() {
        if index == 0 && matches!(view.payload, Payload::Home) {
            page.kind = "home".into();
        }
        page.page = index + 1;
        page.pages = pages;
        page.prev = (index > 0).then(|| format!("p{}", index - 1));
        page.next = (index + 1 < pages).then(|| format!("p{}", index + 1));
        page.json()?;
    }
    if let Some(parent) = parent {
        if !actions.values().any(|action| action == &parent) {
            actions.insert("back".into(), parent);
        }
    }
    Ok(Prepared {
        pages: parts,
        actions,
    })
}

fn continuation(root: &Page, title: &str) -> Result<Page> {
    let page = Page {
        title: title.into(),
        context: root.title.clone(),
        back: Some("p0".into()),
        ..root.clone()
    };
    ensure!(page.json()?.len() <= 600, "Header is too large for HTML");
    Ok(page)
}
